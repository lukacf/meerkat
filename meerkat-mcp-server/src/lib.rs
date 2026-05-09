//! meerkat-mcp-server - MCP server exposing Meerkat as a tool
//!
//! This crate provides an MCP server that exposes Meerkat agent capabilities
//! as MCP tools: meerkat_run and meerkat_resume.

#![allow(dead_code, unused_imports, clippy::expect_used, clippy::large_futures)]

mod runtime_ingress;
mod schedule_host;

#[cfg(test)]
use meerkat::SessionStore;
use meerkat::surface::{
    RequestContext, install_prepared_runtime_interrupt_handle, prepare_surface_session,
    request_action, run_runtime_backed_initial_turn_with_machine,
    split_runtime_backed_eager_create_request,
};
use meerkat::{
    AgentFactory, FactoryAgentBuilder, MachineServiceTurnCommitProtocol,
    MachineSessionArchiveProtocol, OutputSchema, PersistenceBundle, PersistentSessionService,
    ScheduleService, ScheduleToolDispatcher, ToolError, ToolResult,
};
use meerkat_contracts::{RealtimeOpenRequest, SkillsParams};
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, ResumeOverrideMask,
    SessionBuildOptions, SessionError, SessionService, SessionServiceHistoryExt, StartTurnRequest,
};
use meerkat_core::{
    AgentEvent, BlobId, Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy,
    ConfigRuntimeError, ConfigStore, EventEnvelope, FileConfigStore, HookRunOverrides, Provider,
    RealmSelection, RuntimeBootstrap, ToolCallView, ToolCategoryOverride, format_verbose_event,
};
use meerkat_mcp::{McpReloadTarget, McpRouter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use futures::StreamExt;

fn runtime_driver_error_to_session_error(err: meerkat_runtime::RuntimeDriverError) -> SessionError {
    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
        err.to_string(),
    ))
}

fn surface_materialize_error_to_session_error(
    err: meerkat::surface::SurfaceRuntimeMaterializeError,
) -> SessionError {
    match err {
        meerkat::surface::SurfaceRuntimeMaterializeError::Session(err) => err,
        meerkat::surface::SurfaceRuntimeMaterializeError::RuntimeDriver(err) => {
            runtime_driver_error_to_session_error(err)
        }
        other => SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            other.to_string(),
        )),
    }
}

async fn create_runtime_backed_session_and_run_initial_turn(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &meerkat::SessionId,
    req: CreateSessionRequest,
    admission: Option<meerkat::RuntimeContextAdmissionGuard>,
) -> Result<meerkat::RunResult, SessionError> {
    let (req, initial_turn) = split_runtime_backed_eager_create_request(req);
    let create_result = match admission {
        Some(admission) => {
            service
                .create_session_with_reserved_admission(req, admission)
                .await
        }
        None => service.create_session(req).await,
    }?;

    match initial_turn {
        Some(initial_turn) => run_runtime_backed_initial_turn_with_machine(
            service,
            runtime_adapter,
            session_id,
            initial_turn,
        )
        .await
        .map_err(surface_materialize_error_to_session_error),
        None => Ok(create_result),
    }
}

fn skill_source_provenance(
    source_uuid: meerkat_core::skills::SourceUuid,
    display_name: impl Into<String>,
) -> meerkat_contracts::SkillSourceProvenance {
    let display_name = display_name.into();
    meerkat_contracts::SkillSourceProvenance {
        identity: meerkat_core::skills::SourceIdentityRecord {
            source_uuid,
            display_name: display_name.clone(),
            transport_kind: meerkat_core::skills::SourceTransportKind::Embedded,
            fingerprint: format!("mcp:{display_name}"),
            status: meerkat_core::skills::SourceIdentityStatus::Active,
        },
    }
}
use meerkat_client::TestClient;
use tokio::sync::Mutex;

/// Tool definition provided by the MCP client
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpToolDef {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// JSON Schema for tool input
    pub input_schema: Value,
    /// Handler type: "callback" means the tool result will be provided via meerkat_resume
    #[serde(default)]
    pub handler: Option<String>,
}

impl McpToolDef {
    pub fn handler_kind(&self) -> &str {
        self.handler.as_deref().unwrap_or("callback")
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderInput {
    Anthropic,
    Openai,
    Gemini,
    Other,
}

impl ProviderInput {
    pub fn to_provider(self) -> Provider {
        match self {
            ProviderInput::Anthropic => Provider::Anthropic,
            ProviderInput::Openai => Provider::OpenAI,
            ProviderInput::Gemini => Provider::Gemini,
            ProviderInput::Other => Provider::Other,
        }
    }
}

/// Input schema for meerkat_run tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatRunInput {
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub provider: Option<ProviderInput>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation.
    /// Omit to use the product default.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Stream agent events to the MCP client via notifications.
    #[serde(default)]
    pub stream: bool,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Tool definitions for the agent to use
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Enable built-in tools (task management, shell, etc.).
    /// Omit to use the product default.
    #[serde(default)]
    pub enable_builtins: Option<bool>,
    /// Configuration for built-in tools (only used when enable_builtins is true)
    #[serde(default)]
    pub builtin_config: Option<BuiltinConfigInput>,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    /// Requires comms_name when enabled.
    #[serde(default)]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (name, description, labels).
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable semantic memory.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
    /// Explicit budget limits for this run.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimitsInput>,
    /// Skills to preload into the system prompt — typed `SkillKey`s
    /// (source_uuid + skill_name).
    #[serde(default)]
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillKey>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Legacy compatibility skill refs for per-turn injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
    /// Key-value labels attached at session creation (e.g. workflow tagging).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Additional system-level instructions prepended to the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Opaque application context forwarded to the agent build pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Route this session's LLM calls through a realm-scoped provider
    /// binding. Typed `WireAuthBindingRef` referencing a
    /// `[realm.<realm>.binding.<binding>]` entry in the active Config.
    /// Pre-wave-c this was `Option<String>` parsed as `"realm:binding"`
    /// — the string form is now rejected at the deserialization
    /// boundary (dogma #5: no untyped joins on the ingress seam).
    /// When set, the provider runtime registry resolves the binding's
    /// auth profile and backend profile through the standard
    /// `ProviderRuntime::resolve` pipeline (Phase 4d.mcp.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<meerkat_contracts::WireAuthBindingRef>,
}

fn default_structured_output_retries() -> u32 {
    2
}

fn mcp_resume_requires_rebuild(input: &MeerkatResumeInput) -> bool {
    !input.tool_results.is_empty()
        || !input.tools.is_empty()
        || input.model.is_some()
        || input.provider.is_some()
        || input.max_tokens.is_some()
        || input.system_prompt.is_some()
        || input.output_schema.is_some()
        || input.structured_output_retries.is_some()
        || input.provider_params.is_some()
        || input.hooks_override.is_some()
        || input.enable_builtins.is_some()
        || input
            .builtin_config
            .as_ref()
            .is_some_and(|cfg| cfg.enable_shell.is_some())
        || input.enable_memory.is_some()
        || input.enable_mob.is_some()
        || input.budget_limits.is_some()
        || input.preload_skills.is_some()
        || input.comms_name.is_some()
        || input.peer_meta.is_some()
}

/// Configuration options for built-in tools
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct BuiltinConfigInput {
    /// Enable shell tools. Omit to inherit/default.
    #[serde(default)]
    pub enable_shell: Option<bool>,
    /// Default timeout for shell commands in seconds (default: 30)
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,
}

/// Actions supported by the config tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigAction {
    Get,
    Set,
    Patch,
}

/// Input schema for meerkat_config tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatConfigInput {
    pub action: ConfigAction,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub patch: Option<Value>,
    #[serde(default)]
    pub expected_generation: Option<u64>,
}

/// Structured MCP tool-call error payload.
#[derive(Debug, Clone)]
pub struct ToolCallError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl ToolCallError {
    fn new(code: i32, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message, None)
    }

    fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(-32601, message, None)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message, None)
    }

    fn internal_with_data(message: impl Into<String>, data: Value) -> Self {
        Self::new(-32603, message, Some(data))
    }
}

fn map_config_runtime_error(err: ConfigRuntimeError) -> ToolCallError {
    match err {
        ConfigRuntimeError::GenerationConflict { expected, current } => ToolCallError::new(
            -32602,
            format!("generation conflict: expected {expected}, current {current}"),
            Some(json!({
                "type": "generation_conflict",
                "expected_generation": expected,
                "current_generation": current
            })),
        ),
        other => ToolCallError::new(
            -32603,
            other.to_string(),
            Some(json!({ "type": "config_runtime_error" })),
        ),
    }
}

async fn load_config_async(
    realm_id: &meerkat_core::connection::RealmId,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
) -> Config {
    let store = match realm_config_store(
        realm_id,
        realms_root,
        backend_hint,
        origin_hint,
        instance_id,
    )
    .await
    {
        Ok(store) => store,
        Err(_) => return Config::default(),
    };
    let mut config = store.get().await.unwrap_or_else(|_| Config::default());
    if let Err(err) = config.apply_env_overrides() {
        tracing::warn!("Failed to apply env overrides: {}", err);
    }
    if let Err(err) = config.validate() {
        tracing::warn!("Invalid realm config; using defaults: {}", err);
        return Config::default();
    }
    config
}

/// Resolve an explicit keep_alive override. Returns None when input is None (inherit).
fn resolve_keep_alive(requested: Option<bool>) -> Result<Option<bool>, String> {
    match requested {
        Some(true) => {
            #[cfg(feature = "comms")]
            {
                meerkat::surface::resolve_keep_alive(true).map(Some)
            }
            #[cfg(not(feature = "comms"))]
            {
                Err("keep_alive requires comms support (build with --features comms)".to_string())
            }
        }
        other => Ok(other), // None (inherit) or Some(false) (disable) pass through
    }
}

fn validate_public_peer_meta(peer_meta: Option<&meerkat_core::PeerMeta>) -> Result<(), String> {
    meerkat::surface::validate_public_peer_meta(peer_meta)
}

fn tagged_realm_config_store(
    realms_root: &std::path::Path,
    realm_id: &meerkat_core::connection::RealmId,
    backend: meerkat_store::RealmBackend,
    instance_id: Option<&str>,
) -> Arc<dyn ConfigStore> {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id.as_str());
    let base: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(paths.config_path.clone()));
    let tagged = meerkat_core::TaggedConfigStore::new(
        base,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(realm_id.to_string()),
            instance_id: instance_id.map(ToOwned::to_owned),
            backend: Some(backend.as_str().to_string()),
            resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                root: paths.root.display().to_string(),
                manifest_path: paths.manifest_path.display().to_string(),
                config_path: paths.config_path.display().to_string(),
                sessions_sqlite_path: Some(paths.sessions_sqlite_path.display().to_string()),
                sessions_jsonl_dir: paths.sessions_jsonl_dir.display().to_string(),
            }),
        },
    );
    Arc::new(tagged)
}

async fn realm_config_store(
    realm_id: &meerkat_core::connection::RealmId,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
) -> Result<Arc<dyn ConfigStore>, String> {
    let manifest = meerkat_store::ensure_realm_manifest_in(
        realms_root,
        realm_id.as_str(),
        backend_hint,
        origin_hint,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(tagged_realm_config_store(
        realms_root,
        realm_id,
        manifest.backend,
        instance_id,
    ))
}

fn realm_store_path(
    realms_root: &std::path::Path,
    realm_id: &meerkat_core::connection::RealmId,
    backend: meerkat_store::RealmBackend,
) -> PathBuf {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id.as_str());
    match backend {
        meerkat_store::RealmBackend::Jsonl => paths.sessions_jsonl_dir,
        meerkat_store::RealmBackend::Sqlite => paths.root,
    }
}

/// Shared state for the MCP server, holding the session service.
///
/// The service is configured once with max-permissive factory flags
/// (`builtins: true`, `shell: true`). Per-request tool configuration is
/// controlled via `override_builtins` / `override_shell` in `SessionBuildOptions`.
pub struct MeerkatMcpState {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    schedule_service: ScheduleService,
    realm_id: meerkat_core::connection::RealmId,
    backend: String,
    instance_id: Option<String>,
    expose_paths: bool,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    #[cfg(feature = "mob")]
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    mcp_adapters: Arc<Mutex<HashMap<String, Arc<meerkat_mcp::McpRouterAdapter>>>>,
    runtime_sessions: runtime_ingress::SharedMcpRuntimeSessions,
    runtime_pre_admissions: runtime_ingress::SharedMcpRuntimePreAdmissions,
    runtime_registration_locks: runtime_ingress::SharedMcpRuntimeRegistrationLocks,
    session_event_streams: Arc<Mutex<HashMap<String, Arc<SessionEventStreamHandle>>>>,
    #[cfg(feature = "mob")]
    mob_event_streams: Arc<Mutex<HashMap<String, Arc<MobEventStreamInner>>>>,
    schedule_host: StdMutex<Option<meerkat::surface::ScheduleHostHandle>>,
    /// Runtime adapter for comms drain lifecycle and runtime operations.
    #[allow(dead_code)] // Only used with `comms` feature
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    #[cfg(feature = "comms")]
    _realm_lease: Option<meerkat_store::RealmLeaseGuard>,
}

struct SessionEventStreamHandle {
    stream: Mutex<meerkat_core::EventStream>,
}

#[cfg(feature = "mob")]
enum MobEventStreamInner {
    /// Per-member agent event stream.
    Member(Mutex<meerkat_core::comms::EventStream>),
    /// Mob-wide attributed event stream.
    MobWide(Mutex<meerkat_mob::MobEventRouterHandle>),
}

impl MeerkatMcpState {
    pub(crate) fn runtime_ingress_context(&self) -> runtime_ingress::McpRuntimeIngressContext {
        runtime_ingress::McpRuntimeIngressContext::new(
            runtime_ingress::McpRuntimeIngressResources {
                service: Arc::clone(&self.service),
                runtime_adapter: Arc::clone(&self.runtime_adapter),
                config_runtime: Arc::clone(&self.config_runtime),
                realm_id: self.realm_id.clone(),
                instance_id: self.instance_id.clone(),
                backend: self.backend.clone(),
                mcp_adapters: Arc::clone(&self.mcp_adapters),
                runtime_sessions: Arc::clone(&self.runtime_sessions),
                runtime_pre_admissions: Arc::clone(&self.runtime_pre_admissions),
                runtime_registration_locks: Arc::clone(&self.runtime_registration_locks),
            },
        )
    }

    pub(crate) async fn clear_surface_bindings(&self, session_id: &meerkat::SessionId) {
        self.runtime_ingress_context()
            .clear_session(session_id)
            .await;
    }

    async fn cleanup_archived_session_runtime(&self, session_id: &meerkat::SessionId) {
        self.clear_surface_bindings(session_id).await;
        #[cfg(feature = "mob")]
        if let Err(error) = self
            .mob_state
            .destroy_bridge_session_mobs(&session_id.to_string())
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "archived-session mob cleanup incomplete"
            );
        }
    }

    async fn clear_surface_bindings_if_new(
        &self,
        session_id: &meerkat::SessionId,
        runtime_was_registered: bool,
        runtime_state_existed: bool,
    ) {
        self.runtime_ingress_context()
            .clear_session_if_new(session_id, runtime_was_registered, runtime_state_existed)
            .await;
    }

    /// Create a new MCP state with a session service backed by `AgentFactory`.
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_and_options(RuntimeBootstrap::default(), false).await
    }

    pub async fn new_with_bootstrap(
        bootstrap: RuntimeBootstrap,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_and_options(bootstrap, false).await
    }

    pub async fn new_with_bootstrap_and_options(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_and_llm(bootstrap, expose_paths, None).await
    }

    #[doc(hidden)]
    pub async fn new_with_bootstrap_and_test_client(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_and_llm(
            bootstrap,
            expose_paths,
            Some(Arc::new(TestClient::default())),
        )
        .await
    }

    async fn new_with_bootstrap_options_and_llm(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
        default_llm_client: Option<Arc<dyn meerkat::LlmClient>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let locator = bootstrap.realm.resolve_locator()?;
        // Locator now carries the typed `RealmId` directly; no need to
        // reparse at the mcp-server boundary. Downstream `meerkat_store::*_in`
        // call sites obtain `&str` via `RealmId::as_str`.
        let realm_id = locator.realm;
        let realms_root = locator.state_root;
        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint);
        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));
        let config = load_config_async(
            &realm_id,
            &realms_root,
            backend_hint,
            origin_hint,
            bootstrap.realm.instance_id.as_deref(),
        )
        .await;
        let (manifest, persistence) = meerkat::open_realm_persistence_in(
            &realms_root,
            realm_id.as_str(),
            backend_hint,
            origin_hint,
        )
        .await?;
        let store_path = persistence
            .store_path()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| realm_store_path(&realms_root, &realm_id, manifest.backend));
        let runtime_store = persistence.runtime_store();
        let session_store = persistence.session_store();
        let blob_store = persistence.blob_store();
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, realm_id.as_str());
        let conventions_context_root = bootstrap.context.context_root.clone();
        let project_root = conventions_context_root
            .clone()
            .unwrap_or_else(|| realm_paths.root.clone());
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            manifest.backend,
            bootstrap.realm.instance_id.as_deref(),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&config_store),
            realm_paths.root.join("config_state.json"),
        ));
        let _lease = meerkat_store::start_realm_lease_in(
            &realms_root,
            realm_id.as_str(),
            bootstrap.realm.instance_id.as_deref(),
            "rkat-mcp",
        )
        .await?;

        // Create factory with max-permissive flags; per-request overrides
        // in SessionBuildOptions control what tools are actually enabled.
        let mut factory = AgentFactory::new(store_path)
            .session_store(session_store.clone())
            .runtime_root(realm_paths.root.clone())
            .project_root(project_root)
            .builtins(true)
            .shell(true)
            .schedule(true);
        if let Some(context_root) = conventions_context_root {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let skill_runtime = factory.build_skill_runtime(&config).await?;

        let max_sessions = config.max_sessions();
        let mut builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store);
        builder.default_llm_client = default_llm_client;
        #[cfg(feature = "mob")]
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        let (service, runtime_adapter) = meerkat::surface::build_runtime_backed_service(
            builder,
            max_sessions,
            PersistenceBundle::new(session_store, runtime_store, blob_store),
        );
        let service = Arc::new(service);
        #[cfg(feature = "mob")]
        let mob_state = {
            let persistent_mob_root = realm_paths.root.clone();
            let state = Arc::new(
                meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
                    service.clone(),
                    Some(runtime_adapter.clone()),
                )
                .with_persistent_storage_root(Some(persistent_mob_root)),
            );
            *mob_tools_slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
                meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&state)),
            ));
            state
        };

        let state = Self {
            service,
            schedule_service,
            realm_id,
            backend: manifest.backend.as_str().to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths,
            config_runtime,
            skill_runtime,
            #[cfg(feature = "mob")]
            mob_state,
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_pre_admissions: Arc::new(Mutex::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
            session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "mob")]
            mob_event_streams: Arc::new(Mutex::new(HashMap::new())),
            schedule_host: StdMutex::new(None),
            runtime_adapter,
            #[cfg(feature = "comms")]
            _realm_lease: Some(_lease),
        };
        if let Err(error) = state.start_schedule_host() {
            tracing::warn!("schedule host failed to start: {error}");
        }
        Ok(state)
    }

    /// Test constructor that accepts an injected store (avoids opening redb at platform data dir).
    #[cfg(test)]
    pub(crate) async fn new_with_store(store: Arc<dyn SessionStore>) -> Self {
        Self::new_with_store_options(store, None, None).await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_store_and_runtime_store(
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    ) -> Self {
        Self::new_with_store_options(store, runtime_store, None).await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_store_and_max_sessions(
        store: Arc<dyn SessionStore>,
        max_sessions_override: Option<usize>,
    ) -> Self {
        Self::new_with_store_options(store, None, max_sessions_override).await
    }

    #[cfg(test)]
    async fn new_with_store_options(
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
        max_sessions_override: Option<usize>,
    ) -> Self {
        let bootstrap = RuntimeBootstrap::default();
        let locator = match bootstrap.realm.resolve_locator() {
            Ok(locator) => locator,
            Err(_) => meerkat_core::RealmLocator {
                state_root: meerkat_core::default_state_root(),
                realm: meerkat_core::connection::RealmId::parse(meerkat_core::generate_realm_id())
                    .expect("generate_realm_id emits a valid slug by construction"),
            },
        };
        let realm_id = locator.realm.clone();
        let realms_root = locator.state_root;
        let mut config = load_config_async(
            &realm_id,
            &realms_root,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
            bootstrap.realm.instance_id.as_deref(),
        )
        .await;
        if let Some(max_sessions) = max_sessions_override {
            config.limits.max_sessions = Some(max_sessions);
        }
        let store_path =
            realm_store_path(&realms_root, &realm_id, meerkat_store::RealmBackend::Sqlite);
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, realm_id.as_str());
        let project_root = realm_paths.root.clone();
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            meerkat_store::RealmBackend::Sqlite,
            bootstrap.realm.instance_id.as_deref(),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&config_store),
            realm_paths.root.join("config_state.json"),
        ));

        let runtime_root = realm_paths.root.clone();
        let mut factory = AgentFactory::new(store_path)
            .runtime_root(runtime_root)
            .project_root(project_root)
            .builtins(true)
            .shell(true)
            .schedule(true);
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let max_sessions = config.max_sessions();
        let builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store);
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(meerkat::MemoryScheduleStore::default()),
            )))),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(
            meerkat_store::FsBlobStore::new(realm_paths.root.join("blobs")),
        );
        let (service, runtime_adapter) = meerkat::surface::build_runtime_backed_service(
            builder,
            max_sessions,
            PersistenceBundle::new(store, runtime_store, blob_store),
        );
        let service = Arc::new(service);

        let state = Self {
            service,
            schedule_service: ScheduleService::new(Arc::new(
                meerkat::MemoryScheduleStore::default(),
            )),
            realm_id,
            backend: "sqlite".to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths: false,
            config_runtime,
            skill_runtime: None,
            #[cfg(feature = "mob")]
            mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_pre_admissions: Arc::new(Mutex::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
            session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "mob")]
            mob_event_streams: Arc::new(Mutex::new(HashMap::new())),
            schedule_host: StdMutex::new(None),
            runtime_adapter,
            #[cfg(feature = "comms")]
            _realm_lease: None,
        };
        if let Err(error) = state.start_schedule_host() {
            tracing::warn!("schedule host failed to start: {error}");
        }
        state
    }

    pub fn realm_id(&self) -> &meerkat_core::connection::RealmId {
        &self.realm_id
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn expose_paths(&self) -> bool {
        self.expose_paths
    }

    #[cfg(feature = "mob")]
    pub fn set_realtime_rpc_tcp_addr(&mut self, addr: Option<String>) {
        self.mob_state.set_realtime_rpc_tcp_addr(addr);
    }

    async fn upsert_mcp_adapter(
        &self,
        session_id: &meerkat::SessionId,
        adapter: Arc<meerkat_mcp::McpRouterAdapter>,
    ) {
        self.mcp_adapters
            .lock()
            .await
            .insert(session_id.to_string(), adapter);
    }

    async fn mcp_adapter_for_session(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<Arc<meerkat_mcp::McpRouterAdapter>, String> {
        if self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(|error| format!("Failed to load session: {error}"))?
            .is_none()
        {
            return Err(format!("Session not found: {session_id}"));
        }
        self.mcp_adapters
            .lock()
            .await
            .get(&session_id.to_string())
            .cloned()
            .ok_or_else(|| {
                "Live MCP unavailable for this session. Resume the session with this MCP server before staging live MCP changes.".to_string()
            })
    }
}

fn parse_backend_hint(raw: &str) -> Option<meerkat_store::RealmBackend> {
    match raw {
        "jsonl" => Some(meerkat_store::RealmBackend::Jsonl),
        "sqlite" => Some(meerkat_store::RealmBackend::Sqlite),
        _ => None,
    }
}

fn realm_origin_from_selection(selection: &RealmSelection) -> meerkat_store::RealmOrigin {
    match selection {
        RealmSelection::Explicit { .. } => meerkat_store::RealmOrigin::Explicit,
        RealmSelection::Isolated => meerkat_store::RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => meerkat_store::RealmOrigin::Workspace,
    }
}

/// Input schema for meerkat_resume tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatResumeInput {
    pub session_id: String,
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Stream agent events to the MCP client via notifications.
    #[serde(default)]
    pub stream: bool,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Tool definitions for the agent to use (should match the original run)
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Tool results to provide for pending tool calls
    #[serde(default)]
    pub tool_results: Vec<ToolResultInput>,
    /// Enable built-in tools (task management, shell, etc.).
    /// Omit to inherit the resumed session's setting.
    #[serde(default)]
    pub enable_builtins: Option<bool>,
    /// Configuration for built-in tools (only used when enable_builtins is true)
    #[serde(default)]
    pub builtin_config: Option<BuiltinConfigInput>,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    #[serde(default)]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Optional model override for resume.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional max_tokens override for resume.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Optional provider override for resume.
    #[serde(default)]
    pub provider: Option<ProviderInput>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation.
    /// Omit to inherit the resumed session's setting.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable semantic memory.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
    /// Explicit budget limits for this resumed run.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimitsInput>,
    /// Skills to preload into the system prompt — typed `SkillKey`s
    /// (source_uuid + skill_name).
    #[serde(default)]
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillKey>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Legacy compatibility skill refs for per-turn injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
    /// Optional per-turn tool overlay.
    #[serde(default)]
    pub flow_tool_overlay: Option<TurnToolOverlayInput>,
    /// Additional system-level instructions prepended to the prompt for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionIdInput {
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionListInput {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// Filter sessions by labels (all specified k/v pairs must match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionHistoryInput {
    pub session_id: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatBlobGetInput {
    pub blob_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeerkatMcpAddInput {
    pub session_id: String,
    pub server_config: meerkat_core::McpServerConfig,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMcpRemoveInput {
    pub session_id: String,
    pub server_name: String,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMcpReloadInput {
    pub session_id: String,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeerkatSkillsAction {
    List,
    Inspect,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSkillsInput {
    pub action: MeerkatSkillsAction,
    /// Typed skill identity for the `inspect` action.
    #[serde(default)]
    pub skill_key: Option<meerkat_core::skills::SkillKey>,
    /// Optional source selector for inspect action.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamOpenInput {
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamReadInput {
    pub stream_id: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Disable timeout and wait indefinitely for the next event.
    #[serde(default)]
    pub no_timeout: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamCloseInput {
    pub stream_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMobEventStreamOpenInput {
    /// The mob to subscribe to.
    pub mob_id: String,
    /// Optional member ID. If provided, subscribes to that member's agent events only.
    /// If absent, subscribes to all mob-wide attributed events.
    #[serde(default)]
    pub member_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMobEventStreamReadInput {
    pub stream_id: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Disable timeout and wait indefinitely for the next event.
    #[serde(default)]
    pub no_timeout: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMobEventStreamCloseInput {
    pub stream_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatCommsPeersInput {
    pub session_id: String,
}

pub type MeerkatCommsSendInput = meerkat_contracts::CommsSendParams;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BudgetLimitsInput {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
    #[serde(default)]
    pub max_tool_calls: Option<usize>,
}

impl From<BudgetLimitsInput> for meerkat_core::BudgetLimits {
    fn from(value: BudgetLimitsInput) -> Self {
        meerkat_core::BudgetLimits {
            max_tokens: value.max_tokens,
            max_duration: value.max_duration_secs.map(std::time::Duration::from_secs),
            max_tool_calls: value.max_tool_calls,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TurnToolOverlayInput {
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
}

impl From<TurnToolOverlayInput> for meerkat_core::service::TurnToolOverlay {
    fn from(value: TurnToolOverlayInput) -> Self {
        Self {
            allowed_tools: value.allowed_tools,
            blocked_tools: value.blocked_tools,
        }
    }
}

/// Tool result provided by the MCP client
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ToolResultInput {
    /// ID of the tool call this is a result for
    pub tool_use_id: String,
    /// Result content (or error message)
    pub content: String,
    /// Whether this is an error result
    #[serde(default)]
    pub is_error: bool,
}

pub type EventNotifier = Arc<dyn Fn(&str, &EventEnvelope<AgentEvent>) + Send + Sync>;
type EventEnvelopeSender = mpsc::Sender<EventEnvelope<AgentEvent>>;
type EventEnvelopeReceiver = mpsc::Receiver<EventEnvelope<AgentEvent>>;

fn spawn_event_forwarder(
    mut rx: EventEnvelopeReceiver,
    session_id: String,
    verbose: bool,
    notifier: Option<EventNotifier>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(ref notify) = notifier {
                notify(&session_id, &event);
            }

            if !verbose {
                continue;
            }

            if let Some(line) = format_verbose_event(&event.payload) {
                tracing::info!("{}", line);
            }
        }
    })
}

fn maybe_event_channel(
    verbose: bool,
    stream: bool,
) -> (Option<EventEnvelopeSender>, Option<EventEnvelopeReceiver>) {
    if !verbose && !stream {
        return (None, None);
    }
    let (tx, rx) = mpsc::channel(100);
    (Some(tx), Some(rx))
}

/// Format an agent run result (success, callback pending, or error) into an MCP response.
///
/// The `session_id` parameter is the pre-claimed ID, used as fallback when the
/// result is a `CallbackPending` error (which doesn't carry a session ID).
fn format_agent_result(
    result: Result<meerkat_core::types::RunResult, SessionError>,
    session_id: &meerkat::SessionId,
) -> Result<Value, String> {
    match result {
        Ok(result) => {
            let payload = json!({
                "content": [{
                    "type": "text",
                    "text": result.text
                }],
                "session_id": result.session_id.to_string(),
                "turns": result.turns,
                "tool_calls": result.tool_calls,
                "structured_output": result.structured_output,
                "extraction_error": result.extraction_error,
                "schema_warnings": result.schema_warnings,
                "skill_diagnostics": result.skill_diagnostics
            });
            Ok(wrap_tool_payload(payload))
        }
        Err(SessionError::Agent(meerkat::AgentError::CallbackPending { tool_name, args })) => {
            let payload = json!({
                "content": [{
                    "type": "text",
                    "text": "Agent is waiting for tool results"
                }],
                "session_id": session_id.to_string(),
                "status": "pending_tool_call",
                "pending_tool_calls": [{
                    "tool_name": tool_name,
                    "args": args
                }]
            });
            Ok(wrap_tool_payload(payload))
        }
        Err(e) => Err(format!("Agent error: {e}")),
    }
}

fn format_agent_result_tool(
    result: Result<meerkat_core::types::RunResult, SessionError>,
    session_id: &meerkat::SessionId,
) -> Result<Value, ToolCallError> {
    match result {
        Err(SessionError::Agent(meerkat::AgentError::Cancelled)) => {
            Err(request_cancelled_tool_error())
        }
        result => format_agent_result(result, session_id).map_err(ToolCallError::internal),
    }
}

fn build_callback_dispatcher(tools: &[McpToolDef]) -> Option<Arc<dyn AgentToolDispatcher>> {
    if tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(tools)) as Arc<dyn AgentToolDispatcher>)
    }
}

async fn compose_run_external_tool_dispatchers(
    state: &MeerkatMcpState,
    session_id: &meerkat::SessionId,
    primary: Option<Arc<dyn AgentToolDispatcher>>,
    secondary: Option<Arc<dyn AgentToolDispatcher>>,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, ToolCallError> {
    match compose_external_tool_dispatchers(primary, secondary) {
        Ok(tools) => Ok(tools),
        Err(error) => {
            state.clear_surface_bindings(session_id).await;
            Err(ToolCallError::internal(error))
        }
    }
}

fn recoverable_callback_tool_defs(tools: &[McpToolDef]) -> Vec<ToolDef> {
    tools
        .iter()
        .filter(|tool| tool.handler_kind() == "callback")
        .map(|tool| ToolDef {
            name: tool.name.clone().into(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            provenance: Some(meerkat_core::types::ToolProvenance {
                kind: meerkat_core::types::ToolSourceKind::Builtin,
                source_id: "mcp-server".into(),
            }),
        })
        .collect()
}

fn post_commit_session_created_error(
    session_id: &meerkat::SessionId,
    err: &SessionError,
) -> ToolCallError {
    if matches!(err, SessionError::Agent(meerkat::AgentError::Cancelled)) {
        return ToolCallError::new(
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code(),
            "request cancelled",
            Some(json!({
                "session_id": session_id.to_string(),
                "session_ref": session_id.to_string(),
                "session_created": true,
                "resumable": true,
            })),
        );
    }

    ToolCallError::internal_with_data(
        format!("Agent error: {err}"),
        json!({
            "session_id": session_id.to_string(),
            "session_ref": session_id.to_string(),
            "session_created": true,
            "resumable": true,
        }),
    )
}

fn request_cancelled_tool_error() -> ToolCallError {
    ToolCallError::new(
        meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code(),
        "request cancelled",
        None,
    )
}

async fn reject_if_cancelled_before_mcp_service_admission<F>(
    request_context: Option<&RequestContext>,
    cleanup: F,
) -> Result<(), ToolCallError>
where
    F: Future<Output = Result<(), ToolCallError>>,
{
    if request_context.is_some_and(RequestContext::cancel_already_requested) {
        cleanup.await?;
        Err(request_cancelled_tool_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
const DEFAULT_STREAM_READ_TIMEOUT_MS: u64 = 5;
#[cfg(not(test))]
const DEFAULT_STREAM_READ_TIMEOUT_MS: u64 = 5_000;

fn base_tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "meerkat_run",
            "description": "Run a new Meerkat agent with the given prompt. Returns the agent's response. If tools are provided and the agent requests a tool call, the response will include pending_tool_calls that must be fulfilled via meerkat_resume.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatRunInput>()
        }),
        json!({
            "name": "meerkat_resume",
            "description": "Resume an existing Meerkat session. Use this to continue a conversation or provide tool results for pending tool calls.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatResumeInput>()
        }),
        json!({
            "name": "meerkat_help",
            "description": "Ask how to use Meerkat. Runs a help session with the embedded meerkat-platform skill and returns the answer.",
            "inputSchema": meerkat_tools::schema_for::<meerkat_contracts::HelpRequest>()
        }),
        json!({
            "name": "meerkat_config",
            "description": "Get or update Meerkat config for this MCP server instance.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatConfigInput>()
        }),
        json!({
            "name": "meerkat_capabilities",
            "description": "Get the list of capabilities available in this Meerkat runtime.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "meerkat_models_catalog",
            "description": "Get the catalog of supported LLM models with provider grouping, tiers, and parameter schemas.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "meerkat_skills",
            "description": "List or inspect available skills. Use action 'list' to see all skills, or 'inspect' with a typed skill_key and optional source UUID selector to see full content.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSkillsInput>()
        }),
        json!({
            "name": "meerkat_read",
            "description": "Read current session state.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_sessions",
            "description": "List sessions in the active realm.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionListInput>()
        }),
        json!({
            "name": "meerkat_history",
            "description": "Read a session's full history.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionHistoryInput>()
        }),
        json!({
            "name": "meerkat_blob_get",
            "description": "Fetch raw blob bytes and metadata by blob id.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatBlobGetInput>()
        }),
        json!({
            "name": "meerkat_interrupt",
            "description": "Interrupt an in-flight turn for a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_archive",
            "description": "Archive (remove) a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_mcp_add",
            "description": "Stage a live MCP server add operation on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpAddInput>()
        }),
        json!({
            "name": "meerkat_mcp_remove",
            "description": "Stage a live MCP server removal on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpRemoveInput>()
        }),
        json!({
            "name": "meerkat_mcp_reload",
            "description": "Stage a live MCP server reload on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpReloadInput>()
        }),
        json!({
            "name": "meerkat_event_stream_open",
            "description": "Open a session-level agent event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamOpenInput>()
        }),
        json!({
            "name": "meerkat_realtime_open_info",
            "description": "Issue realtime websocket bootstrap information through the configured RPC realtime host.",
            "inputSchema": meerkat_tools::schema_for::<RealtimeOpenRequest>()
        }),
        json!({
            "name": "meerkat_event_stream_read",
            "description": "Read the next item from an open session-level event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamReadInput>()
        }),
        json!({
            "name": "meerkat_event_stream_close",
            "description": "Close a previously opened session-level event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamCloseInput>()
        }),
    ]
}

#[cfg(feature = "mob")]
fn mob_event_stream_tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "meerkat_mob_event_stream_open",
            "description": "Open a mob-level event stream. If member_id is provided, streams that member's agent events. Otherwise streams all mob-wide attributed events.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMobEventStreamOpenInput>()
        }),
        json!({
            "name": "meerkat_mob_event_stream_read",
            "description": "Read the next event from an open mob event stream (optional timeout).",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMobEventStreamReadInput>()
        }),
        json!({
            "name": "meerkat_mob_event_stream_close",
            "description": "Close a previously opened mob event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMobEventStreamCloseInput>()
        }),
    ]
}

#[cfg(feature = "mob")]
fn mob_host_tools_list() -> Vec<Value> {
    meerkat_mob_mcp::public_tools_list()
}

#[cfg(feature = "comms")]
fn comms_tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "meerkat_comms_send",
            "description": "Send a canonical comms command to a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsSendInput>()
        }),
        json!({
            "name": "meerkat_comms_peers",
            "description": "List peers visible to a session's comms runtime.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsPeersInput>()
        }),
    ]
}

/// Returns the list of tools exposed by this MCP server
pub fn tools_list() -> Vec<Value> {
    let mut tools = base_tools_list();
    tools.extend(meerkat::schedule_tools_list());

    #[cfg(feature = "mob")]
    tools.extend(mob_host_tools_list());

    #[cfg(feature = "mob")]
    tools.extend(mob_event_stream_tools_list());

    #[cfg(feature = "comms")]
    tools.extend(comms_tools_list());

    tools
}

/// Handle a tools/call request
pub async fn handle_tools_call(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, ToolCallError> {
    Box::pin(handle_tools_call_with_notifier(
        state, tool_name, arguments, None, None,
    ))
    .await
}

/// Handle a tools/call request with optional event notifications.
pub async fn handle_tools_call_with_notifier(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    #[cfg(feature = "mob")]
    if meerkat_mob_mcp::public_tool_names().contains(&tool_name) {
        let payload =
            meerkat_mob_mcp::handle_public_tools_call(&state.mob_state, tool_name, arguments)
                .await
                .map_err(|err| ToolCallError {
                    code: err.code,
                    message: err.message,
                    data: err.data,
                })?;
        return Ok(wrap_tool_payload(payload));
    }

    match tool_name {
        "meerkat_help" => {
            let input: meerkat_contracts::HelpRequest = serde_json::from_value(arguments.clone())
                .map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
            })?;
            Box::pin(handle_meerkat_help(state, input, notifier, request_context)).await
        }
        "meerkat_run" => {
            let input: MeerkatRunInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            Box::pin(handle_meerkat_run(state, input, notifier, request_context)).await
        }
        "meerkat_resume" => {
            let input: MeerkatResumeInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            Box::pin(handle_meerkat_resume(
                state,
                input,
                notifier,
                request_context,
            ))
            .await
        }
        "meerkat_config" => {
            let input: MeerkatConfigInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_config(state, input)
                .await
                .map(wrap_tool_payload)
        }
        "meerkat_capabilities" => handle_meerkat_capabilities(state)
            .await
            .map(wrap_tool_payload)
            .map_err(ToolCallError::internal),
        "meerkat_models_catalog" => handle_meerkat_models_catalog(state)
            .await
            .map(wrap_tool_payload)
            .map_err(ToolCallError::internal),
        "meerkat_skills" => {
            let input: MeerkatSkillsInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_skills(state, input)
                .await
                .map(wrap_tool_payload)
                .map_err(ToolCallError::internal)
        }
        "meerkat_read" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_sessions" => {
            let input: MeerkatSessionListInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_sessions(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_history" => {
            let input: MeerkatSessionHistoryInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_history(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_blob_get" => {
            let input: MeerkatBlobGetInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_blob_get(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_interrupt" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_interrupt(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_archive" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_archive(state, input).await
        }
        "meerkat_mcp_add" => {
            let input: MeerkatMcpAddInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_add(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_mcp_remove" => {
            let input: MeerkatMcpRemoveInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_remove(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_mcp_reload" => {
            let input: MeerkatMcpReloadInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_reload(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_open" => {
            let input: MeerkatSessionEventStreamOpenInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_open(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_realtime_open_info" => {
            let input: RealtimeOpenRequest = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_realtime_open_info(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_read" => {
            let input: MeerkatSessionEventStreamReadInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_close" => {
            let input: MeerkatSessionEventStreamCloseInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_close(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        name if name.starts_with("meerkat_schedule_") => {
            state
                .start_schedule_host()
                .map_err(|error| ToolCallError::internal(error.to_string()))?;
            meerkat::handle_schedule_tools_call(&state.schedule_service, name, arguments)
                .await
                .map(wrap_tool_payload)
                .map_err(map_schedule_tool_error)
        }
        #[cfg(feature = "mob")]
        "meerkat_mob_event_stream_open" => {
            let input: MeerkatMobEventStreamOpenInput = serde_json::from_value(arguments.clone())
                .map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
            })?;
            handle_meerkat_mob_event_stream_open(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "mob")]
        "meerkat_mob_event_stream_read" => {
            let input: MeerkatMobEventStreamReadInput = serde_json::from_value(arguments.clone())
                .map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
            })?;
            handle_meerkat_mob_event_stream_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "mob")]
        "meerkat_mob_event_stream_close" => {
            let input: MeerkatMobEventStreamCloseInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mob_event_stream_close(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_send" => {
            let input: MeerkatCommsSendInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_send(state, input).await
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_peers" => {
            let input: MeerkatCommsPeersInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_peers(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {tool_name}"
        ))),
    }
}

async fn handle_meerkat_skills(
    state: &MeerkatMcpState,
    input: MeerkatSkillsInput,
) -> Result<Value, String> {
    let runtime = state
        .skill_runtime
        .as_ref()
        .ok_or_else(|| "skills not enabled".to_string())?;

    match input.action {
        MeerkatSkillsAction::List => {
            let entries = runtime
                .list_all_with_provenance(&meerkat_core::skills::SkillFilter::default())
                .await
                .map_err(|e| format!("skill list failed: {e}"))?;
            let wire: Vec<meerkat_contracts::SkillEntry> = entries
                .iter()
                .map(|e| meerkat_contracts::SkillEntry {
                    key: e.descriptor.key.clone(),
                    name: e.descriptor.name.clone(),
                    description: e.descriptor.description.clone(),
                    scope: e.descriptor.scope.to_string(),
                    source: skill_source_provenance(
                        e.descriptor.key.source_uuid.clone(),
                        e.descriptor.source_name.clone(),
                    ),
                    is_active: e.is_active,
                    shadowed_by: e.shadowed_by_source_uuid.clone().map(|source_uuid| {
                        skill_source_provenance(
                            source_uuid,
                            e.shadowed_by.clone().unwrap_or_default(),
                        )
                    }),
                })
                .collect();
            serde_json::to_value(meerkat_contracts::SkillListResponse { skills: wire })
                .map_err(|e| format!("serialization failed: {e}"))
        }
        MeerkatSkillsAction::Inspect => {
            let skill_key = input
                .skill_key
                .as_ref()
                .ok_or_else(|| "missing 'skill_key' for inspect action".to_string())?;
            // Apply the identity registry remap chain before dispatch
            // so legacy source_uuids still resolve to the canonical
            // backing skill (C-4 invariant — registry-backed engines
            // apply remaps; others identity-project).
            let canonical = runtime
                .canonical_skill_key(skill_key)
                .await
                .map_err(|e| format!("skill canonicalization failed: {e}"))?;
            let doc = runtime
                .load_from_source(&canonical, input.source.as_deref())
                .await
                .map_err(|e| format!("skill inspect failed: {e}"))?;
            serde_json::to_value(meerkat_contracts::SkillInspectResponse {
                key: doc.descriptor.key.clone(),
                name: doc.descriptor.name.clone(),
                description: doc.descriptor.description.clone(),
                scope: doc.descriptor.scope.to_string(),
                source: skill_source_provenance(
                    doc.descriptor.key.source_uuid.clone(),
                    doc.descriptor.source_name.clone(),
                ),
                body: doc.body,
            })
            .map_err(|e| format!("serialization failed: {e}"))
        }
    }
}

async fn handle_meerkat_capabilities(state: &MeerkatMcpState) -> Result<Value, String> {
    let config = state
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();
    let response = meerkat::surface::build_capabilities_response(&config);
    serde_json::to_value(&response).map_err(|e| format!("Serialization failed: {e}"))
}

async fn handle_meerkat_models_catalog(state: &MeerkatMcpState) -> Result<Value, String> {
    let config = state
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();
    let response =
        meerkat::surface::build_models_catalog_response(&config).map_err(|e| e.to_string())?;
    serde_json::to_value(&response).map_err(|e| format!("Serialization failed: {e}"))
}

async fn handle_meerkat_config(
    state: &MeerkatMcpState,
    input: MeerkatConfigInput,
) -> Result<Value, ToolCallError> {
    match input.action {
        ConfigAction::Get => {
            let snapshot = state
                .config_runtime
                .get()
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
        ConfigAction::Set => {
            let config_value = input.config.ok_or_else(|| {
                ToolCallError::invalid_params("config is required for action=set")
            })?;
            let config: Config = serde_json::from_value(config_value.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid config: {e}")))?;
            reject_config_payload_defaulting(&config_value, &config)?;
            validate_config_for_commit(&config)?;
            let snapshot = state
                .config_runtime
                .set(config, input.expected_generation)
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
        ConfigAction::Patch => {
            let patch = input.patch.ok_or_else(|| {
                ToolCallError::invalid_params("patch is required for action=patch")
            })?;
            let current = state
                .config_runtime
                .get()
                .await
                .map_err(map_config_runtime_error)?;
            let preview = apply_patch_preview(&current.config, patch.clone())?;
            validate_config_for_commit(&preview)?;
            let snapshot = state
                .config_runtime
                .patch(ConfigDelta(patch), input.expected_generation)
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
    }
}

fn config_envelope_value(
    snapshot: meerkat_core::ConfigSnapshot,
    expose_paths: bool,
) -> Result<Value, ToolCallError> {
    let policy = if expose_paths {
        ConfigEnvelopePolicy::Diagnostic
    } else {
        ConfigEnvelopePolicy::Public
    };
    serde_json::to_value(ConfigEnvelope::from_snapshot(snapshot, policy))
        .map_err(|e| ToolCallError::internal(e.to_string()))
}

fn validate_config_for_commit(config: &Config) -> Result<(), ToolCallError> {
    config
        .validate()
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid config: {e}")))?;
    config
        .skills
        .build_source_identity_registry()
        .map_err(|e| {
            ToolCallError::invalid_params(format!("Invalid skills source-identity config: {e}"))
        })?;
    Ok(())
}

fn reject_config_payload_defaulting(raw: &Value, config: &Config) -> Result<(), ToolCallError> {
    let typed = serde_json::to_value(config).map_err(|e| {
        ToolCallError::internal(format!(
            "Failed to serialize typed config for validation: {e}"
        ))
    })?;
    compare_config_payload_shape(raw, &typed, "config")
}

fn compare_config_payload_shape(
    raw: &Value,
    typed: &Value,
    path: &str,
) -> Result<(), ToolCallError> {
    match (raw, typed) {
        (Value::Object(raw_obj), Value::Object(typed_obj)) => {
            for key in raw_obj.keys() {
                if !typed_obj.contains_key(key) {
                    return Err(ToolCallError::invalid_params(format!(
                        "Invalid config: unknown field `{path}.{key}`"
                    )));
                }
            }
            for (key, typed_value) in typed_obj {
                let Some(raw_value) = raw_obj.get(key) else {
                    return Err(ToolCallError::invalid_params(format!(
                        "Invalid config: missing field `{path}.{key}`; action=set requires a complete typed config payload"
                    )));
                };
                compare_config_payload_shape(raw_value, typed_value, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        (Value::Array(raw_items), Value::Array(typed_items)) => {
            for (index, (raw_item, typed_item)) in
                raw_items.iter().zip(typed_items.iter()).enumerate()
            {
                compare_config_payload_shape(raw_item, typed_item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn merge_patch(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                if v.is_null() {
                    base_map.remove(&k);
                } else {
                    merge_patch(base_map.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (base_val, patch_val) => {
            *base_val = patch_val;
        }
    }
}

fn map_schedule_tool_error(error: meerkat::ScheduleToolError) -> ToolCallError {
    ToolCallError::new(error.code, error.message, error.data)
}

fn apply_patch_preview(config: &Config, patch: Value) -> Result<Config, ToolCallError> {
    let mut value = serde_json::to_value(config)
        .map_err(|e| ToolCallError::internal(format!("Failed to serialize config: {e}")))?;
    merge_patch(&mut value, patch);
    serde_json::from_value(value).map_err(|e| ToolCallError::invalid_params(format!("{e}")))
}

fn canonical_skill_keys(
    _config: &Config,
    skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    skill_references: Option<Vec<String>>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, String> {
    // Wave-b retypes removed the legacy stringly `skill_references` path:
    // `SkillsParams` now flattens only typed `skill_refs` into `SkillKey`
    // via `canonical_skill_keys()` (no registry lookup at wire boundary).
    // Reject any caller still supplying the legacy string form.
    if skill_references
        .as_ref()
        .is_some_and(|refs| !refs.is_empty())
    {
        return Err(
            "legacy `skill_references` string form is no longer supported; \
             pass typed `skill_refs` (source_uuid + skill_name) instead"
                .to_string(),
        );
    }
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
    };
    Ok(params.canonical_skill_keys())
}

async fn handle_meerkat_read(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let view = state
        .service
        .read(&session_id)
        .await
        .map_err(|e| format!("Failed to read session: {e}"))?;
    let payload = json!({
        "session_id": session_id.to_string(),
        "session_ref": meerkat_contracts::format_session_ref(&state.realm_id, &session_id),
        "state": view.state,
        "billing": view.billing
    });
    Ok(wrap_tool_payload(payload))
}

async fn handle_meerkat_interrupt(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    state
        .service
        .read(&session_id)
        .await
        .map_err(|e| format!("Failed to interrupt session: {e}"))?;
    match state
        .runtime_adapter
        .hard_cancel_current_run(&session_id, "MCP session interrupt")
        .await
    {
        Ok(()) => {}
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state })
            if interrupt_not_ready_is_noop(state) => {}
        Err(
            meerkat_runtime::RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Destroyed,
            }
            | meerkat_runtime::RuntimeDriverError::Destroyed,
        ) => {}
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => {
            return Err(format!(
                "Failed to interrupt session: runtime is not interruptible while {state}"
            ));
        }
        Err(e) => return Err(format!("Failed to interrupt session: {e}")),
    }
    Ok(wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "interrupted": true
    })))
}

fn interrupt_not_ready_is_noop(state: meerkat_runtime::RuntimeState) -> bool {
    matches!(
        state,
        meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached
    )
}

async fn handle_meerkat_sessions(
    state: &MeerkatMcpState,
    input: MeerkatSessionListInput,
) -> Result<Value, String> {
    let query = meerkat_core::service::SessionQuery {
        limit: input.limit,
        offset: input.offset,
        labels: input.labels,
    };
    let sessions = state
        .service
        .list(query)
        .await
        .map_err(|e| format!("Failed to list sessions: {e}"))?;
    let wire_sessions: Vec<meerkat_contracts::WireSessionSummary> = sessions
        .into_iter()
        .map(|s| {
            let session_ref = meerkat_contracts::format_session_ref(&state.realm_id, &s.session_id);
            let mut wire = meerkat_contracts::WireSessionSummary::from(s);
            wire.session_ref = Some(session_ref);
            wire
        })
        .collect();
    let payload = json!({ "sessions": wire_sessions });
    Ok(wrap_tool_payload(payload))
}

async fn handle_meerkat_history(
    state: &MeerkatMcpState,
    input: MeerkatSessionHistoryInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let history = state
        .service
        .read_history(
            &session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: input.offset.unwrap_or(0),
                limit: input.limit,
            },
        )
        .await
        .map_err(|e| format!("Failed to read session history: {e}"))?;
    let mut payload: meerkat_contracts::WireSessionHistory = history.into();
    payload.session_ref = Some(meerkat_contracts::format_session_ref(
        &state.realm_id,
        &session_id,
    ));
    Ok(wrap_tool_payload(
        serde_json::to_value(payload).unwrap_or(Value::Null),
    ))
}

async fn handle_meerkat_blob_get(
    state: &MeerkatMcpState,
    input: MeerkatBlobGetInput,
) -> Result<Value, String> {
    let blob_id = BlobId::new(input.blob_id);
    let payload = state
        .service
        .blob_store()
        .get(&blob_id)
        .await
        .map_err(|err| err.to_string())?;
    Ok(wrap_tool_payload(
        serde_json::to_value(payload).unwrap_or(Value::Null),
    ))
}

#[derive(Clone)]
struct McpArchiveCleanup {
    ingress: runtime_ingress::McpRuntimeIngressContext,
    #[cfg(feature = "mob")]
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
}

impl McpArchiveCleanup {
    async fn clear_surface_session(&self, session_id: &meerkat::SessionId) {
        self.ingress.clear_session(session_id).await;
    }

    async fn run(&self, session_id: &meerkat::SessionId) -> Result<(), SessionError> {
        self.clear_surface_session(session_id).await;
        #[cfg(feature = "mob")]
        if let Err(error) = self
            .mob_state
            .destroy_bridge_session_mobs(&session_id.to_string())
            .await
        {
            return Err(error.into_session_error("mob cleanup during archive incomplete"));
        }
        Ok(())
    }
}

async fn archive_with_mcp_machine_authority(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
    session_id: &meerkat::SessionId,
) -> Result<(), SessionError> {
    service
        .archive_with_machine_protocol(
            session_id,
            MachineSessionArchiveProtocol::from_machine(runtime_adapter),
        )
        .await
}

async fn cleanup_unpublished_prepared_session(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ingress: &runtime_ingress::McpRuntimeIngressContext,
    session_id: &meerkat::SessionId,
) -> Result<(), SessionError> {
    let runtime_was_registered = runtime_adapter.contains_session(session_id).await;
    match archive_with_mcp_machine_authority(service, runtime_adapter, session_id).await {
        Ok(()) => {}
        Err(SessionError::NotFound { .. }) if runtime_was_registered => {}
        Err(error) => return Err(error),
    }
    ingress.clear_session(session_id).await;
    Ok(())
}

async fn archive_session_with_runtime_cleanup(
    state: &MeerkatMcpState,
    session_id: meerkat::SessionId,
) -> Result<(), SessionError> {
    let service = Arc::clone(&state.service);
    let runtime_adapter = Arc::clone(&state.runtime_adapter);
    let cleanup = McpArchiveCleanup {
        ingress: state.runtime_ingress_context(),
        #[cfg(feature = "mob")]
        mob_state: Arc::clone(&state.mob_state),
    };
    let result_session_id = session_id.clone();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        #[cfg(feature = "mob")]
        match cleanup
            .mob_state
            .archive_mob_owned_bridge_session_with_cleanup(
                &session_id,
                "mob cleanup during mob-owned archive incomplete",
            )
            .await
        {
            Ok(true) => {
                cleanup.clear_surface_session(&session_id).await;
                let _ = result_tx.send(Ok(()));
                return;
            }
            Ok(false) => {}
            Err(error) => {
                let _ = result_tx.send(Err(error));
                return;
            }
        }

        #[cfg(feature = "mob")]
        let had_cleanup_anchor = cleanup
            .mob_state
            .has_bridge_session_scoped_mobs(&session_id.to_string())
            .await;
        let result = archive_with_mcp_machine_authority(
            service.as_ref(),
            runtime_adapter.as_ref(),
            &session_id,
        )
        .await;
        if matches!(result, Ok(()) | Err(SessionError::NotFound { .. }))
            && let Err(error) = cleanup.run(&session_id).await
        {
            let _ = result_tx.send(Err(error));
            return;
        }
        #[cfg(feature = "mob")]
        let result = if had_cleanup_anchor && matches!(result, Err(SessionError::NotFound { .. })) {
            Ok(())
        } else {
            result
        };
        let _ = result_tx.send(result);
    });
    result_rx.await.map_err(|_| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "MCP archive task ended before reporting a result for {result_session_id}"
        )))
    })?
}

async fn handle_meerkat_archive(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, ToolCallError> {
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|err| ToolCallError::invalid_params(invalid_session_id_message(err)))?;
    archive_session_with_runtime_cleanup(state, session_id.clone())
        .await
        .map_err(archive_session_error_to_tool_error)?;
    Ok(wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "archived": true
    })))
}

fn archive_session_error_to_tool_error(error: SessionError) -> ToolCallError {
    match error {
        SessionError::FailedWithData { message, data } => {
            ToolCallError::internal_with_data(format!("Failed to archive session: {message}"), data)
        }
        other => ToolCallError::internal(format!("Failed to archive session: {other}")),
    }
}

async fn handle_meerkat_mcp_add(
    state: &MeerkatMcpState,
    input: MeerkatMcpAddInput,
) -> Result<Value, String> {
    let server_name = input.server_config.name.clone();
    if server_name.trim().is_empty() {
        return Err("server_name cannot be empty".to_string());
    }
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let adapter = state.mcp_adapter_for_session(&session_id).await?;

    adapter
        .stage_add(input.server_config)
        .await
        .map_err(|e| format!("failed to stage add: {e}"))?;

    mcp_live_response_value(
        input.session_id,
        meerkat_contracts::McpLiveOperation::Add,
        Some(server_name),
        false,
    )
    .map(wrap_tool_payload)
}

async fn handle_meerkat_mcp_remove(
    state: &MeerkatMcpState,
    input: MeerkatMcpRemoveInput,
) -> Result<Value, String> {
    if input.server_name.trim().is_empty() {
        return Err("server_name cannot be empty".to_string());
    }
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let adapter = state.mcp_adapter_for_session(&session_id).await?;
    adapter
        .stage_remove(input.server_name.clone())
        .await
        .map_err(|e| format!("failed to stage remove: {e}"))?;

    mcp_live_response_value(
        input.session_id,
        meerkat_contracts::McpLiveOperation::Remove,
        Some(input.server_name),
        false,
    )
    .map(wrap_tool_payload)
}

async fn handle_meerkat_mcp_reload(
    state: &MeerkatMcpState,
    input: MeerkatMcpReloadInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let adapter = state.mcp_adapter_for_session(&session_id).await?;

    match input.server_name.clone() {
        Some(name) => {
            if name.trim().is_empty() {
                return Err("server_name cannot be empty".to_string());
            }
            adapter
                .stage_reload(McpReloadTarget::ServerName(name))
                .await
                .map_err(|e| format!("failed to stage reload: {e}"))?;
        }
        None => {
            for name in adapter.active_server_names().await {
                adapter
                    .stage_reload(McpReloadTarget::ServerName(name))
                    .await
                    .map_err(|e| format!("failed to stage reload: {e}"))?;
            }
        }
    }

    mcp_live_response_value(
        input.session_id,
        meerkat_contracts::McpLiveOperation::Reload,
        input.server_name,
        false,
    )
    .map(wrap_tool_payload)
}

fn mcp_live_response_value(
    session_id: String,
    operation: meerkat_contracts::McpLiveOperation,
    server_name: Option<String>,
    persisted: bool,
) -> Result<Value, String> {
    serde_json::to_value(meerkat_contracts::McpLiveOpResponse {
        session_id,
        operation,
        server_name,
        status: meerkat_contracts::McpLiveOpStatus::Staged,
        persisted,
        applied_at_turn: None,
    })
    .map_err(|error| format!("failed to serialize MCP live response: {error}"))
}

async fn handle_meerkat_realtime_open_info(
    state: &MeerkatMcpState,
    input: RealtimeOpenRequest,
) -> Result<Value, String> {
    #[cfg(not(feature = "mob"))]
    {
        let _ = (state, input);
        Err("realtime/open_info delegation requires the mob-enabled MCP server build".to_string())
    }
    #[cfg(feature = "mob")]
    {
        let addr = state.mob_state.realtime_rpc_tcp_addr().ok_or_else(|| {
            "realtime/open_info delegation requires --realtime-rpc-tcp".to_string()
        })?;
        let result = rpc_tcp_call(&addr, "realtime/open_info", json!(input)).await?;
        Ok(wrap_tool_payload(result))
    }
}

async fn rpc_tcp_call(addr: &str, method: &str, params: Value) -> Result<Value, String> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|err| format!("failed to connect to RPC host {addr}: {err}"))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half).lines();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    write_half
        .write_all(request.to_string().as_bytes())
        .await
        .map_err(|err| format!("failed to write RPC request: {err}"))?;
    write_half
        .write_all(b"\n")
        .await
        .map_err(|err| format!("failed to terminate RPC request: {err}"))?;
    write_half
        .flush()
        .await
        .map_err(|err| format!("failed to flush RPC request: {err}"))?;
    let line = reader
        .next_line()
        .await
        .map_err(|err| format!("failed to read RPC response: {err}"))?
        .ok_or_else(|| "RPC host closed without a response".to_string())?;
    let response: Value =
        serde_json::from_str(&line).map_err(|err| format!("invalid RPC response JSON: {err}"))?;
    if let Some(error) = response.get("error") {
        return Err(format!("RPC {method} failed: {error}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| format!("RPC {method} response missing result"))
}

async fn handle_meerkat_event_stream_open(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamOpenInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let stream = state
        .service
        .subscribe_session_events(&session_id)
        .await
        .map_err(|e| format!("Failed to open session event stream: {e}"))?;
    let stream_id = meerkat::SessionId::new().to_string();
    state.session_event_streams.lock().await.insert(
        stream_id.clone(),
        Arc::new(SessionEventStreamHandle {
            stream: Mutex::new(stream),
        }),
    );

    Ok(wrap_tool_payload(json!({
        "stream_id": stream_id,
        "session_id": session_id.to_string()
    })))
}

async fn handle_meerkat_event_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamReadInput,
) -> Result<Value, String> {
    let handle = state
        .session_event_streams
        .lock()
        .await
        .get(&input.stream_id)
        .cloned()
        .ok_or_else(|| format!("Stream not found: {}", input.stream_id))?;

    let timeout_ms = if input.no_timeout {
        None
    } else {
        Some(input.timeout_ms.unwrap_or(DEFAULT_STREAM_READ_TIMEOUT_MS))
    };
    let next_event = {
        let mut stream = handle.stream.lock().await;
        match timeout_ms {
            None => stream.next().await,
            Some(timeout_ms) => {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    stream.next(),
                )
                .await
                {
                    Ok(item) => item,
                    Err(_) => {
                        return Ok(wrap_tool_payload(json!({
                            "stream_id": input.stream_id,
                            "status": "timeout"
                        })));
                    }
                }
            }
        }
    };

    match next_event {
        Some(envelope) => {
            let envelope_json = serde_json::to_value(&envelope)
                .map_err(|e| format!("Failed to serialize stream event: {e}"))?;
            Ok(wrap_tool_payload(json!({
                "stream_id": input.stream_id,
                "status": "event",
                "event": envelope_json
            })))
        }
        None => {
            state
                .session_event_streams
                .lock()
                .await
                .remove(&input.stream_id);
            Ok(wrap_tool_payload(json!({
                "stream_id": input.stream_id,
                "status": "closed"
            })))
        }
    }
}

async fn handle_meerkat_event_stream_close(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamCloseInput,
) -> Result<Value, String> {
    let removed = state
        .session_event_streams
        .lock()
        .await
        .remove(&input.stream_id);
    Ok(wrap_tool_payload(json!({
        "stream_id": input.stream_id,
        "closed": removed.is_some()
    })))
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_open(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamOpenInput,
) -> Result<Value, String> {
    let mob_id = meerkat_mob::MobId::from(input.mob_id.as_str());
    let stream_id = meerkat::SessionId::new().to_string();

    let inner = if let Some(member_id) = &input.member_id {
        let identity = meerkat_mob::AgentIdentity::from(member_id.as_str());
        let stream = state
            .mob_state
            .subscribe_agent_events(&mob_id, &identity)
            .await
            .map_err(|e| format!("Failed to subscribe to member events: {e}"))?;
        MobEventStreamInner::Member(Mutex::new(stream))
    } else {
        let router_handle = state
            .mob_state
            .subscribe_mob_events(&mob_id)
            .await
            .map_err(|e| format!("Failed to subscribe to mob events: {e}"))?;
        MobEventStreamInner::MobWide(Mutex::new(router_handle))
    };

    state
        .mob_event_streams
        .lock()
        .await
        .insert(stream_id.clone(), Arc::new(inner));

    Ok(wrap_tool_payload(json!({
        "stream_id": stream_id,
        "mob_id": input.mob_id,
        "member_id": input.member_id,
    })))
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamReadInput,
) -> Result<Value, String> {
    let handle = state
        .mob_event_streams
        .lock()
        .await
        .get(&input.stream_id)
        .cloned()
        .ok_or_else(|| format!("Mob event stream not found: {}", input.stream_id))?;

    let timeout_ms = if input.no_timeout {
        None
    } else {
        Some(input.timeout_ms.unwrap_or(DEFAULT_STREAM_READ_TIMEOUT_MS))
    };

    match handle.as_ref() {
        MobEventStreamInner::Member(stream_mutex) => {
            let next_event = {
                let mut stream = stream_mutex.lock().await;
                match timeout_ms {
                    None => stream.next().await,
                    Some(ms) => {
                        match tokio::time::timeout(
                            std::time::Duration::from_millis(ms),
                            stream.next(),
                        )
                        .await
                        {
                            Ok(item) => item,
                            Err(_) => {
                                return Ok(wrap_tool_payload(json!({
                                    "stream_id": input.stream_id,
                                    "status": "timeout"
                                })));
                            }
                        }
                    }
                }
            };
            match next_event {
                Some(envelope) => {
                    let event_json = serde_json::to_value(&envelope)
                        .map_err(|e| format!("Failed to serialize event: {e}"))?;
                    Ok(wrap_tool_payload(json!({
                        "stream_id": input.stream_id,
                        "status": "event",
                        "event": event_json
                    })))
                }
                None => {
                    state
                        .mob_event_streams
                        .lock()
                        .await
                        .remove(&input.stream_id);
                    Ok(wrap_tool_payload(json!({
                        "stream_id": input.stream_id,
                        "status": "closed"
                    })))
                }
            }
        }
        MobEventStreamInner::MobWide(router_mutex) => {
            let next_event = {
                let mut router_handle = router_mutex.lock().await;
                match timeout_ms {
                    None => router_handle.event_rx.recv().await,
                    Some(ms) => {
                        match tokio::time::timeout(
                            std::time::Duration::from_millis(ms),
                            router_handle.event_rx.recv(),
                        )
                        .await
                        {
                            Ok(item) => item,
                            Err(_) => {
                                return Ok(wrap_tool_payload(json!({
                                    "stream_id": input.stream_id,
                                    "status": "timeout"
                                })));
                            }
                        }
                    }
                }
            };
            match next_event {
                Some(attributed) => {
                    let event_json = serde_json::to_value(&attributed)
                        .map_err(|e| format!("Failed to serialize attributed event: {e}"))?;
                    Ok(wrap_tool_payload(json!({
                        "stream_id": input.stream_id,
                        "status": "event",
                        "event": event_json
                    })))
                }
                None => {
                    state
                        .mob_event_streams
                        .lock()
                        .await
                        .remove(&input.stream_id);
                    Ok(wrap_tool_payload(json!({
                        "stream_id": input.stream_id,
                        "status": "closed"
                    })))
                }
            }
        }
    }
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_close(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamCloseInput,
) -> Result<Value, String> {
    let removed = state
        .mob_event_streams
        .lock()
        .await
        .remove(&input.stream_id);
    Ok(wrap_tool_payload(json!({
        "stream_id": input.stream_id,
        "closed": removed.is_some()
    })))
}

#[cfg(feature = "comms")]
fn comms_send_tool_payload(receipt: meerkat_core::comms::SendReceipt) -> Value {
    wrap_tool_payload(json!({
        "receipt": meerkat_contracts::CommsSendResult::from(receipt),
    }))
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_send(
    state: &MeerkatMcpState,
    input: MeerkatCommsSendInput,
) -> Result<Value, ToolCallError> {
    let session_id = meerkat::SessionId::parse(input.session_id())
        .map_err(|err| ToolCallError::invalid_params(invalid_session_id_message(err)))?;
    let comms = state
        .service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| {
            ToolCallError::new(
                -32603,
                format!("Session not found or comms not enabled: {session_id}"),
                Some(json!({
                    "code": "session_not_found_or_comms_disabled",
                    "session_id": session_id.to_string(),
                })),
            )
        })?;
    let peer_name = input.peer_label();
    let cmd = input
        .into_command()
        .into_command(&session_id)
        .map_err(|err| {
            ToolCallError::new(
                -32602,
                "Invalid comms command",
                Some(json!({
                    "code": "invalid_comms_command",
                    "message": err.to_string(),
                })),
            )
        })?;
    let receipt = comms
        .send(cmd)
        .await
        .map_err(|e| normalize_mcp_comms_send_error(peer_name.as_deref(), &e))?;
    Ok(comms_send_tool_payload(receipt))
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_peers(
    state: &MeerkatMcpState,
    input: MeerkatCommsPeersInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let comms = state
        .service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| format!("Session not found or comms not enabled: {session_id}"))?;
    Ok(comms_peers_tool_payload(comms.peers().await))
}

#[cfg(feature = "comms")]
fn comms_peers_tool_payload(peers: Vec<meerkat_core::comms::PeerDirectoryEntry>) -> Value {
    wrap_tool_payload(json!(meerkat_contracts::CommsPeersResult::from_entries(
        &peers
    )))
}

#[cfg(feature = "comms")]
fn normalize_mcp_comms_send_error(
    peer_name: Option<&str>,
    error: &meerkat_core::comms::SendError,
) -> ToolCallError {
    match error {
        meerkat_core::comms::SendError::PeerNotFound(peer) => ToolCallError::new(
            -32603,
            format!("peer_not_found_or_not_trusted: peer '{peer}' is not found or not trusted"),
            Some(json!({
                "code": "peer_not_found_or_not_trusted",
                "peer": peer,
            })),
        ),
        meerkat_core::comms::SendError::PeerOffline => ToolCallError::new(
            -32603,
            format!(
                "peer_unreachable: peer '{}' is unreachable: offline_or_no_ack",
                peer_name.unwrap_or("<unknown>")
            ),
            Some(json!({
                "code": "peer_unreachable",
                "peer": peer_name.unwrap_or("<unknown>"),
                "reason": "offline_or_no_ack",
            })),
        ),
        meerkat_core::comms::SendError::Internal(details)
            if peer_name.is_some() && is_transport_internal(details) =>
        {
            ToolCallError::new(
                -32603,
                format!(
                    "peer_unreachable: peer '{}' is unreachable: transport_error ({details})",
                    peer_name.unwrap_or("<unknown>")
                ),
                Some(json!({
                    "code": "peer_unreachable",
                    "peer": peer_name.unwrap_or("<unknown>"),
                    "reason": "transport_error",
                    "details": details,
                })),
            )
        }
        other => ToolCallError::new(
            -32603,
            other.to_string(),
            Some(json!({
                "code": "send_failed",
                "message": other.to_string(),
            })),
        ),
    }
}

#[cfg(feature = "comms")]
fn is_transport_internal(message: &str) -> bool {
    message.starts_with("Transport error:") || message.starts_with("IO error:")
}

fn help_provider_to_mcp_provider(
    raw: Option<String>,
) -> Result<Option<ProviderInput>, ToolCallError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match meerkat_core::Provider::parse_strict(raw.as_str()) {
        Some(meerkat_core::Provider::Anthropic) => Ok(Some(ProviderInput::Anthropic)),
        Some(meerkat_core::Provider::OpenAI) => Ok(Some(ProviderInput::Openai)),
        Some(meerkat_core::Provider::Gemini) => Ok(Some(ProviderInput::Gemini)),
        Some(meerkat_core::Provider::SelfHosted) => Ok(Some(ProviderInput::Other)),
        Some(meerkat_core::Provider::Other) | None => Err(ToolCallError::invalid_params(format!(
            "invalid help provider `{raw}`"
        ))),
    }
}

async fn handle_meerkat_help(
    state: &MeerkatMcpState,
    input: meerkat_contracts::HelpRequest,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    let prompt = meerkat::help::render_help_prompt(&input)
        .map_err(|err| ToolCallError::invalid_params(err.to_string()))?;
    let provider = help_provider_to_mcp_provider(input.provider.clone())?;
    let run_input = MeerkatRunInput {
        prompt,
        system_prompt: Some(meerkat::help::help_system_prompt().to_string()),
        model: input.model,
        max_tokens: input.max_tokens,
        provider,
        output_schema: None,
        structured_output_retries: None,
        stream: false,
        verbose: false,
        tools: Vec::new(),
        enable_builtins: Some(false),
        builtin_config: Some(BuiltinConfigInput {
            enable_shell: Some(false),
            shell_timeout_secs: None,
        }),
        keep_alive: Some(false),
        comms_name: None,
        peer_meta: None,
        hooks_override: None,
        enable_memory: Some(false),
        enable_mob: Some(false),
        provider_params: None,
        budget_limits: None,
        preload_skills: Some(meerkat::help::platform_preload_skills()),
        skill_refs: None,
        skill_references: None,
        labels: None,
        additional_instructions: None,
        app_context: None,
        shell_env: None,
        auth_binding: None,
    };

    Box::pin(handle_meerkat_run(
        state,
        run_input,
        notifier,
        request_context,
    ))
    .await
}

async fn handle_meerkat_run(
    state: &MeerkatMcpState,
    input: MeerkatRunInput,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    validate_public_peer_meta(input.peer_meta.as_ref()).map_err(ToolCallError::invalid_params)?;
    let ingress = state.runtime_ingress_context();
    let keep_alive_override =
        resolve_keep_alive(input.keep_alive).map_err(ToolCallError::invalid_params)?;
    // Create: no persisted session to inherit from, so None → false.
    let keep_alive = keep_alive_override.unwrap_or(false);
    if keep_alive
        && input
            .comms_name
            .as_ref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err(ToolCallError::invalid_params(
            "keep_alive requires comms_name",
        ));
    }
    let config = state
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| config.agent.model.clone());

    // Build callback tools supplied by the MCP client. The per-session live
    // MCP router is created after runtime bindings are prepared so its surface
    // owner is session-canonical from construction.
    let callback_tools = build_callback_dispatcher(&input.tools);

    let enable_shell_override = input.builtin_config.as_ref().and_then(|c| c.enable_shell);
    let preload_skills = input.preload_skills.clone();
    let skill_references = canonical_skill_keys(
        &config,
        input.skill_refs.clone(),
        input.skill_references.clone(),
    )
    .map_err(ToolCallError::invalid_params)?;

    // Parse output schema if provided
    let output_schema =
        match input.output_schema.clone() {
            Some(schema) => Some(OutputSchema::from_json_value(schema).map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid output_schema: {e}"))
            })?),
            None => None,
        };

    // Pre-create a session to claim a stable session_id.
    let create_admission = state
        .service
        .reserve_create_session_admission()
        .await
        .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?;
    let prepared_session = prepare_surface_session(&state.runtime_adapter)
        .await
        .map_err(ToolCallError::internal)?;
    let session = prepared_session.session;
    let session_id = prepared_session.session_id;
    let bindings = prepared_session.bindings;
    if let Err(e) = install_prepared_runtime_interrupt_handle(
        &state.service,
        &state.runtime_adapter,
        &session_id,
    )
    .await
    {
        state.runtime_adapter.unregister_session(&session_id).await;
        ingress.clear_session(&session_id).await;
        return Err(ToolCallError::internal(format!(
            "failed to install prepared interrupt handle for session {session_id}: {e}"
        )));
    }
    let mcp_adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(
        McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
    ));
    let mcp_tools: Arc<dyn AgentToolDispatcher> = mcp_adapter.clone();
    let external_tools = compose_run_external_tool_dispatchers(
        state,
        &session_id,
        callback_tools.clone(),
        Some(mcp_tools),
    )
    .await?;

    if let Some(context) = request_context.as_ref() {
        let runtime_adapter = state.runtime_adapter.clone();
        let session_id_for_cancel = session_id.clone();
        let install = context
            .install_cancel_action_or_cancelled(request_action(move || {
                let runtime_adapter = runtime_adapter.clone();
                let session_id = session_id_for_cancel.clone();
                async move {
                    let _ = runtime_adapter
                        .hard_cancel_current_run(&session_id, "MCP request cancelled")
                        .await;
                }
            }))
            .await;
        let service = state.service.clone();
        let archive_runtime_adapter = state.runtime_adapter.clone();
        let session_id_for_cleanup = session_id.clone();
        let ingress_for_cleanup = ingress.clone();
        context.set_unpublished_cleanup(request_action(move || {
            let service = service.clone();
            let archive_runtime_adapter = archive_runtime_adapter.clone();
            let ingress = ingress_for_cleanup.clone();
            let session_id = session_id_for_cleanup.clone();
            async move {
                if let Err(error) = cleanup_unpublished_prepared_session(
                    service.as_ref(),
                    archive_runtime_adapter.as_ref(),
                    &ingress,
                    &session_id,
                )
                .await
                {
                    tracing::error!(
                        session_id = %session_id,
                        error = %error,
                        "MCP unpublished create archive failed; leaving runtime ingress registered"
                    );
                }
            }
        }));
        if install == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
            cleanup_unpublished_prepared_session(
                state.service.as_ref(),
                state.runtime_adapter.as_ref(),
                &ingress,
                &session_id,
            )
            .await
            .map_err(archive_session_error_to_tool_error)?;
            ingress.clear_session(&session_id).await;
            return Err(request_cancelled_tool_error());
        }
    }

    // Set up event forwarding
    let (event_tx, event_rx) = maybe_event_channel(input.verbose, input.stream);
    let event_task = event_rx.map(|rx| {
        let stream_notifier = if input.stream { notifier.clone() } else { None };
        spawn_event_forwarder(rx, session_id.to_string(), input.verbose, stream_notifier)
    });

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let llm_binding = meerkat_core::session_recovery::resolve_resume_llm_binding(
        session
            .session_metadata()
            .map(|meta| meta.provider)
            .unwrap_or(meerkat_core::Provider::Other),
        session
            .session_metadata()
            .and_then(|meta| meta.self_hosted_server_id),
        input.model.as_deref(),
        input.provider.map(ProviderInput::to_provider),
    );
    let mut build = SessionBuildOptions {
        provider: llm_binding.provider,
        self_hosted_server_id: llm_binding.self_hosted_server_id,
        output_schema,
        structured_output_retries: input
            .structured_output_retries
            .unwrap_or(default_structured_output_retries()),
        hooks_override: input.hooks_override.clone().unwrap_or_default(),
        comms_name: input.comms_name.clone(),
        peer_meta: input.peer_meta.clone(),
        resume_session: Some(session),
        budget_limits: input.budget_limits.clone().map(Into::into),
        provider_params: input.provider_params.clone(),
        call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
        external_tools,
        recoverable_tool_defs: (!input.tools.is_empty())
            .then(|| recoverable_callback_tool_defs(&input.tools)),
        llm_client_override: None,
        agent_llm_client_decorator: None,
        runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
        initial_turn_metadata: Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(None)),
        override_builtins: ToolCategoryOverride::from_override(input.enable_builtins),
        override_shell: ToolCategoryOverride::from_override(enable_shell_override),
        override_memory: ToolCategoryOverride::from_override(input.enable_memory),
        override_schedule: ToolCategoryOverride::Inherit,
        override_mob: ToolCategoryOverride::Inherit,
        override_image_generation: ToolCategoryOverride::Inherit,
        schedule_tools: None,
        mob_tool_authority_context: None,
        preload_skills,
        realm_id: Some(state.realm_id.to_string()),
        instance_id: state.instance_id.clone(),
        backend: Some(state.backend.clone()),
        config_generation: current_generation,
        auth_binding: input
            .auth_binding
            .clone()
            .map(meerkat_core::AuthBindingRef::from),
        keep_alive,
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
        app_context: input.app_context.clone(),
        additional_instructions: input.additional_instructions.clone(),
        shell_env: input.shell_env.clone(),
        resume_override_mask: ResumeOverrideMask {
            provider: llm_binding.provider_overridden,
            max_tokens: input.max_tokens.is_some(),
            structured_output_retries: input.structured_output_retries.is_some(),
            provider_params: input.provider_params.is_some(),
            preload_skills: input.preload_skills.is_some(),
            keep_alive: keep_alive_override.is_some(),
            comms_name: input.comms_name.is_some(),
            peer_meta: input.peer_meta.is_some(),
            ..Default::default()
        },
        blob_store_override: None,
        mob_tools: None,
    };
    build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::from_override(
        input.enable_mob,
    ));

    reject_if_cancelled_before_mcp_service_admission(request_context.as_ref(), async {
        archive_with_mcp_machine_authority(
            state.service.as_ref(),
            state.runtime_adapter.as_ref(),
            &session_id,
        )
        .await
        .map_err(archive_session_error_to_tool_error)?;
        state.runtime_adapter.unregister_session(&session_id).await;
        ingress.clear_session(&session_id).await;
        Ok(())
    })
    .await?;

    let req = CreateSessionRequest {
        model,
        prompt: input.prompt.into(),
        render_metadata: None,
        system_prompt: input.system_prompt,
        max_tokens: input.max_tokens,
        event_tx: event_tx.clone(),

        skill_references,
        initial_turn: InitialTurnPolicy::RunImmediately,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(build),
        labels: input.labels,
    };

    let result = create_runtime_backed_session_and_run_initial_turn(
        &state.service,
        &state.runtime_adapter,
        &session_id,
        req,
        Some(create_admission),
    )
    .await;
    drop(event_tx);
    if let Some(task) = event_task
        && let Err(e) = task.await
    {
        tracing::warn!("event task panicked: {e}");
    }

    let session_exists = state.service.read(&session_id).await.is_ok();
    if session_exists {
        state.upsert_mcp_adapter(&session_id, mcp_adapter).await;
        ingress
            .configure_session(&session_id, callback_tools, false)
            .await;
    }

    // Manage comms drain lifecycle for the new session.
    // keep_alive is a session-level mutation that may commit independently of
    // first-turn success, so once the session exists we align the drain state
    // even when the initial turn fails.
    #[cfg(feature = "comms")]
    if session_exists {
        let comms_rt = state.service.comms_runtime(&session_id).await;
        state
            .runtime_adapter
            .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
            .await;
    }
    match result {
        Ok(run_result) => format_agent_result_tool(Ok(run_result), &session_id),
        Err(err) => {
            if session_exists {
                Err(post_commit_session_created_error(&session_id, &err))
            } else {
                ingress.clear_session(&session_id).await;
                format_agent_result_tool(Err(err), &session_id)
            }
        }
    }
}

async fn handle_meerkat_resume(
    state: &MeerkatMcpState,
    input: MeerkatResumeInput,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    validate_public_peer_meta(input.peer_meta.as_ref()).map_err(ToolCallError::invalid_params)?;
    let ingress = state.runtime_ingress_context();
    let config = state
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();

    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|err| ToolCallError::invalid_params(invalid_session_id_message(err)))?;

    if let Some(context) = request_context.as_ref() {
        let runtime_adapter = state.runtime_adapter.clone();
        let session_id_for_cancel = session_id.clone();
        let install = context
            .install_cancel_action_or_cancelled(request_action(move || {
                let runtime_adapter = runtime_adapter.clone();
                let session_id = session_id_for_cancel.clone();
                async move {
                    let _ = runtime_adapter
                        .hard_cancel_current_run(&session_id, "MCP request cancelled")
                        .await;
                }
            }))
            .await;
        if install == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
            return Err(request_cancelled_tool_error());
        }
    }

    let mut session = state
        .service
        .load_authoritative_session(&session_id)
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to load session: {e}")))?
        .ok_or_else(|| {
            ToolCallError::new(
                -32004,
                format!("Session not found: {}", input.session_id),
                None,
            )
        })?;
    if state
        .service
        .session_archived_by_authority(&session_id, &session)
        .await
        .map_err(|e| {
            ToolCallError::internal(format!("Failed to load session archive state: {e}"))
        })?
    {
        state.cleanup_archived_session_runtime(&session_id).await;
        return Err(ToolCallError::new(
            -32004,
            format!("Session not found: {}", input.session_id),
            None,
        ));
    }

    // Inject tool results into the session before resuming
    if !input.tool_results.is_empty() {
        let results: Vec<ToolResult> = input
            .tool_results
            .iter()
            .map(|r| ToolResult::new(r.tool_use_id.clone(), r.content.clone(), r.is_error))
            .collect();
        session.push(Message::tool_results(results));
    }

    // Resolve settings from stored metadata, falling back to input overrides
    let stored_metadata = session.session_metadata();

    let enable_builtins_override = input.enable_builtins;
    let enable_shell_override = input
        .builtin_config
        .as_ref()
        .and_then(|cfg| cfg.enable_shell);
    // §10: inherit, disable, and set are different facts.
    // None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    let keep_alive_override =
        resolve_keep_alive(input.keep_alive).map_err(ToolCallError::invalid_params)?;
    let keep_alive = match keep_alive_override {
        Some(val) => val,
        None => stored_metadata.as_ref().is_some_and(|meta| meta.keep_alive),
    };
    let comms_name = input.comms_name.clone().or_else(|| {
        stored_metadata
            .as_ref()
            .and_then(|meta| meta.comms_name.clone())
    });
    if keep_alive
        && comms_name
            .as_ref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err(ToolCallError::invalid_params(
            "keep_alive requires comms_name",
        ));
    }
    let model = input
        .model
        .clone()
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.model.clone()))
        .unwrap_or_else(|| config.agent.model.clone());
    let max_tokens = input
        .max_tokens
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.max_tokens));
    let provider = input
        .provider
        .map(ProviderInput::to_provider)
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.provider));
    if keep_alive_override.is_some()
        && keep_alive
        && state.service.comms_runtime(&session_id).await.is_none()
    {
        return Err(ToolCallError::invalid_params(
            "keep_alive requires a session created with comms_name",
        ));
    }

    let runtime_registration_lock = ingress.runtime_registration_lock(&session_id);
    let _runtime_registration_guard = runtime_registration_lock.mutex().lock().await;
    let runtime_was_registered = state.runtime_adapter.contains_session(&session_id).await;
    let runtime_state_existed = state
        .runtime_sessions
        .read()
        .await
        .contains_key(&session_id);
    let callback_tools = build_callback_dispatcher(&input.tools);
    let existing_adapter = state
        .mcp_adapters
        .lock()
        .await
        .get(&session_id.to_string())
        .cloned();

    // Decide the branch before moving any owned request fields.
    let needs_rebuild = existing_adapter.is_none() || mcp_resume_requires_rebuild(&input);
    let live_session_existed = state
        .service
        .has_live_session(&session_id)
        .await
        .unwrap_or(false);
    let create_requires_materialization = needs_rebuild || !live_session_existed;
    let mut resume_admission = if create_requires_materialization {
        Some(
            state
                .service
                .reserve_runtime_turn_admission(&session_id)
                .await
                .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
        )
    } else {
        None
    };
    let mut mcp_adapter = existing_adapter.clone();

    // Use empty prompt when only providing tool results
    let prompt = if input.prompt.is_empty() && !input.tool_results.is_empty() {
        String::new()
    } else {
        input.prompt
    };
    let preload_skills = input.preload_skills.clone();
    let skill_references = match canonical_skill_keys(
        &config,
        input.skill_refs.clone(),
        input.skill_references.clone(),
    ) {
        Ok(keys) => keys,
        Err(error) => {
            ingress
                .clear_session_if_new_locked(
                    &session_id,
                    runtime_was_registered,
                    runtime_state_existed,
                )
                .await;
            return Err(ToolCallError::invalid_params(error));
        }
    };

    // Set up event forwarding
    let (event_tx, event_rx) = maybe_event_channel(input.verbose, input.stream);
    let event_task = event_rx.map(|rx| {
        let stream_notifier = if input.stream { notifier.clone() } else { None };
        spawn_event_forwarder(rx, session_id.to_string(), input.verbose, stream_notifier)
    });

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let output_schema = match input.output_schema.clone() {
        Some(schema) => match OutputSchema::from_json_value(schema) {
            Ok(schema) => Some(schema),
            Err(error) => {
                ingress
                    .clear_session_if_new_locked(
                        &session_id,
                        runtime_was_registered,
                        runtime_state_existed,
                    )
                    .await;
                return Err(ToolCallError::invalid_params(format!(
                    "Invalid output_schema: {error}"
                )));
            }
        },
        None => None,
    };
    let llm_binding = meerkat_core::session_recovery::resolve_resume_llm_binding(
        stored_metadata
            .as_ref()
            .map(|m| m.provider)
            .unwrap_or(meerkat_core::Provider::Other),
        stored_metadata
            .as_ref()
            .and_then(|m| m.self_hosted_server_id.clone()),
        input.model.as_deref(),
        provider,
    );
    let build_session_options = |runtime_bindings, external_tools| {
        let mut build = SessionBuildOptions {
            provider: llm_binding.provider,
            self_hosted_server_id: llm_binding.self_hosted_server_id.clone(),
            output_schema: output_schema.clone(),
            structured_output_retries: input
                .structured_output_retries
                .unwrap_or(default_structured_output_retries()),
            hooks_override: input.hooks_override.clone().unwrap_or_default(),
            comms_name: input.comms_name.clone(),
            resume_session: Some(session.clone()),
            budget_limits: input.budget_limits.clone().map(Into::into),
            provider_params: input.provider_params.clone(),
            call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
            external_tools,
            recoverable_tool_defs: (!input.tools.is_empty())
                .then(|| recoverable_callback_tool_defs(&input.tools)),
            llm_client_override: None,
            agent_llm_client_decorator: None,
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(runtime_bindings),
            initial_turn_metadata: Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
                None,
            )),
            override_builtins: ToolCategoryOverride::from_override(enable_builtins_override),
            override_shell: ToolCategoryOverride::from_override(enable_shell_override),
            override_memory: ToolCategoryOverride::from_override(input.enable_memory),
            override_schedule: ToolCategoryOverride::Inherit,
            override_mob: ToolCategoryOverride::Inherit,
            override_image_generation: ToolCategoryOverride::Inherit,
            schedule_tools: None,
            mob_tool_authority_context: None,
            preload_skills: preload_skills.clone(),
            peer_meta: input.peer_meta.clone(),
            realm_id: stored_metadata
                .as_ref()
                .and_then(|m| m.realm_id.clone())
                .or_else(|| Some(state.realm_id.to_string())),
            instance_id: stored_metadata
                .as_ref()
                .and_then(|m| m.instance_id.clone())
                .or_else(|| state.instance_id.clone()),
            backend: stored_metadata
                .as_ref()
                .and_then(|m| m.backend.clone())
                .or_else(|| Some(state.backend.clone())),
            config_generation: current_generation,
            auth_binding: None,
            keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            additional_instructions: input.additional_instructions.clone(),
            shell_env: None,
            resume_override_mask: ResumeOverrideMask {
                model: input.model.is_some(),
                provider: llm_binding.provider_overridden,
                max_tokens: input.max_tokens.is_some(),
                structured_output_retries: input.structured_output_retries.is_some(),
                provider_params: input.provider_params.is_some(),
                preload_skills: input.preload_skills.is_some(),
                keep_alive: keep_alive_override.is_some(),
                comms_name: input.comms_name.is_some(),
                peer_meta: input.peer_meta.is_some(),
                ..Default::default()
            },
            blob_store_override: None,
            mob_tools: None,
        };
        build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::from_override(
            input.enable_mob,
        ));
        build
    };

    let result = if create_requires_materialization {
        let resume_bindings = match state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
        {
            Ok(bindings) => bindings,
            Err(e) => {
                ingress
                    .clear_session_if_new_locked(
                        &session_id,
                        runtime_was_registered,
                        runtime_state_existed,
                    )
                    .await;
                return Err(ToolCallError::internal(format!(
                    "failed to prepare bindings for session {session_id}: {e}"
                )));
            }
        };
        if let Err(e) = install_prepared_runtime_interrupt_handle(
            &state.service,
            &state.runtime_adapter,
            &session_id,
        )
        .await
        {
            ingress
                .clear_session_if_new_locked(
                    &session_id,
                    runtime_was_registered,
                    runtime_state_existed,
                )
                .await;
            return Err(ToolCallError::internal(format!(
                "failed to install prepared interrupt handle for session {session_id}: {e}"
            )));
        }
        let adapter = mcp_adapter.clone().unwrap_or_else(|| {
            Arc::new(meerkat_mcp::McpRouterAdapter::new(
                McpRouter::new_with_surface_handle(Arc::clone(
                    resume_bindings.external_tool_surface(),
                )),
            ))
        });
        let mcp_tools: Arc<dyn AgentToolDispatcher> = adapter.clone();
        let external_tools =
            match compose_external_tool_dispatchers(callback_tools.clone(), Some(mcp_tools)) {
                Ok(tools) => tools,
                Err(error) => {
                    ingress
                        .clear_session_if_new_locked(
                            &session_id,
                            runtime_was_registered,
                            runtime_state_existed,
                        )
                        .await;
                    return Err(ToolCallError::internal(error));
                }
            };
        mcp_adapter = Some(adapter);
        let build = build_session_options(resume_bindings, external_tools);
        reject_if_cancelled_before_mcp_service_admission(request_context.as_ref(), async {
            ingress
                .clear_session_if_new_locked(
                    &session_id,
                    runtime_was_registered,
                    runtime_state_existed,
                )
                .await;
            Ok(())
        })
        .await?;
        let req = CreateSessionRequest {
            model,
            prompt: prompt.clone().into(),
            render_metadata: None,
            system_prompt: input.system_prompt.clone(),
            max_tokens,
            event_tx: event_tx.clone(),

            skill_references: skill_references.clone(),
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            req,
            resume_admission.take(),
        )
        .await
    } else {
        let mut live_turn_admission = None;
        if keep_alive_override.is_some() {
            let comms_rt = state.service.comms_runtime(&session_id).await;
            if keep_alive && comms_rt.is_none() {
                ingress
                    .clear_session_if_new_locked(
                        &session_id,
                        runtime_was_registered,
                        runtime_state_existed,
                    )
                    .await;
                return Err(ToolCallError::invalid_params(
                    "keep_alive requires a session created with comms_name",
                ));
            }
            live_turn_admission = Some(
                state
                    .service
                    .reserve_runtime_turn_admission(&session_id)
                    .await
                    .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
            );
            if let Err(e) = state
                .service
                .apply_runtime_session_keep_alive(&session_id, keep_alive)
                .await
            {
                ingress
                    .clear_session_if_new_locked(
                        &session_id,
                        runtime_was_registered,
                        runtime_state_existed,
                    )
                    .await;
                return Err(ToolCallError::internal(format!(
                    "failed to persist keep_alive: {e}"
                )));
            }
            state
                .runtime_adapter
                .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                .await;
        }
        // Live MCP resumes still use the runtime/machine service-turn receipt
        // path; the persistent service only owns the live mutation and post-
        // receipt projection, not lifecycle truth.
        let turn_req = StartTurnRequest {
            prompt: prompt.clone().into(),
            system_prompt: None,
            event_tx: event_tx.clone(),
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                None,
                meerkat_core::types::HandlingMode::Queue,
                skill_references.clone(),
                input.flow_tool_overlay.clone().map(Into::into),
                Vec::new(),
                Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(None)),
            ),
        };
        let admission = match live_turn_admission.take() {
            Some(admission) => admission,
            None => state
                .service
                .reserve_runtime_turn_admission(&session_id)
                .await
                .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
        };
        let turn_result = state
            .service
            .run_machine_committed_live_turn(
                MachineServiceTurnCommitProtocol::from_machine(state.runtime_adapter.as_ref()),
                &session_id,
                turn_req,
                admission,
            )
            .await;
        match turn_result {
            Ok(run_result) => Ok(run_result),
            Err((SessionError::NotFound { .. }, recovered_admission)) => {
                let admission = match recovered_admission.or_else(|| resume_admission.take()) {
                    Some(admission) => admission,
                    None => state
                        .service
                        .reserve_runtime_turn_admission(&session_id)
                        .await
                        .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
                };
                let resume_bindings = match state
                    .runtime_adapter
                    .prepare_bindings(session_id.clone())
                    .await
                {
                    Ok(bindings) => bindings,
                    Err(e) => {
                        ingress
                            .clear_session_if_new_locked(
                                &session_id,
                                runtime_was_registered,
                                runtime_state_existed,
                            )
                            .await;
                        return Err(ToolCallError::internal(format!(
                            "failed to prepare bindings for session {session_id}: {e}"
                        )));
                    }
                };
                if let Err(e) = install_prepared_runtime_interrupt_handle(
                    &state.service,
                    &state.runtime_adapter,
                    &session_id,
                )
                .await
                {
                    ingress
                        .clear_session_if_new_locked(
                            &session_id,
                            runtime_was_registered,
                            runtime_state_existed,
                        )
                        .await;
                    return Err(ToolCallError::internal(format!(
                        "failed to install prepared interrupt handle for session {session_id}: {e}"
                    )));
                }
                let adapter = mcp_adapter.clone().unwrap_or_else(|| {
                    Arc::new(meerkat_mcp::McpRouterAdapter::new(
                        McpRouter::new_with_surface_handle(Arc::clone(
                            resume_bindings.external_tool_surface(),
                        )),
                    ))
                });
                let mcp_tools: Arc<dyn AgentToolDispatcher> = adapter.clone();
                let external_tools = match compose_external_tool_dispatchers(
                    callback_tools.clone(),
                    Some(mcp_tools),
                ) {
                    Ok(tools) => tools,
                    Err(error) => {
                        ingress
                            .clear_session_if_new_locked(
                                &session_id,
                                runtime_was_registered,
                                runtime_state_existed,
                            )
                            .await;
                        return Err(ToolCallError::internal(error));
                    }
                };
                mcp_adapter = Some(adapter);
                let build = build_session_options(resume_bindings, external_tools);
                reject_if_cancelled_before_mcp_service_admission(request_context.as_ref(), async {
                    ingress
                        .clear_session_if_new_locked(
                            &session_id,
                            runtime_was_registered,
                            runtime_state_existed,
                        )
                        .await;
                    Ok(())
                })
                .await?;
                let req = CreateSessionRequest {
                    model,
                    prompt: prompt.into(),
                    render_metadata: None,
                    system_prompt: input.system_prompt.clone(),
                    max_tokens,
                    event_tx: event_tx.clone(),

                    skill_references,
                    initial_turn: InitialTurnPolicy::RunImmediately,
                    deferred_prompt_policy: DeferredPromptPolicy::Discard,
                    build: Some(build),
                    labels: None,
                };
                create_runtime_backed_session_and_run_initial_turn(
                    &state.service,
                    &state.runtime_adapter,
                    &session_id,
                    req,
                    Some(admission),
                )
                .await
            }
            Err((other, _admission)) => Err(other),
        }
    };
    let session_exists = match state.service.load_authoritative_session(&session_id).await {
        Ok(Some(session)) => match state
            .service
            .session_archived_by_authority(&session_id, &session)
            .await
        {
            Ok(true) => {
                state.cleanup_archived_session_runtime(&session_id).await;
                false
            }
            Ok(false) => true,
            Err(err) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to load machine archive state during MCP session existence check"
                );
                false
            }
        },
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to load authoritative session after MCP resume attempt"
            );
            state.service.read(&session_id).await.is_ok()
        }
    };
    let live_session_exists = session_exists
        && state
            .service
            .has_live_session(&session_id)
            .await
            .unwrap_or(false);
    if result.is_err() {
        if !session_exists {
            ingress.clear_session(&session_id).await;
        } else if !live_session_exists {
            ingress
                .clear_session_if_new_locked(
                    &session_id,
                    runtime_was_registered,
                    runtime_state_existed,
                )
                .await;
        }
    }

    drop(event_tx);
    if let Some(task) = event_task
        && let Err(e) = task.await
    {
        tracing::warn!("event task panicked: {e}");
    }

    let should_configure_runtime_ingress = session_exists
        && (result.is_ok()
            || (live_session_exists
                && (create_requires_materialization
                    || runtime_was_registered
                    || runtime_state_existed)));
    if should_configure_runtime_ingress {
        if let Some(mcp_adapter) = mcp_adapter {
            state.upsert_mcp_adapter(&session_id, mcp_adapter).await;
        }
        if input.tools.is_empty() {
            ingress.ensure_session(&session_id).await;
        } else {
            ingress
                .configure_session(&session_id, callback_tools, false)
                .await;
        }
    }

    // Manage comms drain lifecycle for rebuilt sessions after the session
    // commit boundary. keep_alive may commit independently of turn success.
    #[cfg(feature = "comms")]
    if session_exists && create_requires_materialization {
        let comms_rt = state.service.comms_runtime(&session_id).await;
        state
            .runtime_adapter
            .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
            .await;
    }

    format_agent_result_tool(result, &session_id)
}

fn wrap_tool_payload(payload: Value) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_default();
    json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    })
}

fn compose_external_tool_dispatchers(
    primary: Option<Arc<dyn AgentToolDispatcher>>,
    secondary: Option<Arc<dyn AgentToolDispatcher>>,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, String> {
    match (primary, secondary) {
        (None, None) => Ok(None),
        (Some(dispatcher), None) | (None, Some(dispatcher)) => Ok(Some(dispatcher)),
        (Some(a), Some(b)) => {
            let primary_names: HashSet<String> =
                a.tools().iter().map(|t| t.name.to_string()).collect();
            let secondary_tools = b.tools();
            let secondary_unique: Vec<String> = secondary_tools
                .iter()
                .map(|t| t.name.to_string())
                .filter(|name| !primary_names.contains(name.as_str()))
                .collect();

            if secondary_unique.is_empty() {
                return Ok(Some(a));
            }

            let secondary: Arc<dyn AgentToolDispatcher> =
                if secondary_unique.len() == secondary_tools.len() {
                    b
                } else {
                    Arc::new(meerkat_core::FilteredToolDispatcher::new(
                        b,
                        secondary_unique,
                    ))
                };

            let gateway = meerkat_core::ToolGatewayBuilder::new()
                .add_dispatcher(a)
                .add_dispatcher(secondary)
                .build()
                .map_err(|e| format!("failed to compose external tools: {e}"))?;
            Ok(Some(Arc::new(gateway)))
        }
    }
}

// Adapter types needed for the MCP server

use async_trait::async_trait;
use meerkat::{AgentToolDispatcher, Message, ToolDef};

/// MCP tool dispatcher - exposes tools to the LLM and handles callback tools
/// by returning a special error that signals the MCP client needs to handle the tool call
pub struct MpcToolDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    callback_tools: HashSet<String>,
}

impl MpcToolDispatcher {
    /// Create a new tool dispatcher from MCP tool definitions
    pub fn new(mcp_tools: &[McpToolDef]) -> Self {
        let tools: Arc<[Arc<ToolDef>]> = mcp_tools
            .iter()
            .map(|t| {
                Arc::new(ToolDef {
                    name: t.name.clone().into(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                    provenance: Some(meerkat_core::types::ToolProvenance {
                        kind: meerkat_core::types::ToolSourceKind::Builtin,
                        source_id: "mcp-server".into(),
                    }),
                })
            })
            .collect::<Vec<_>>()
            .into();

        let callback_tools = mcp_tools
            .iter()
            .filter(|t| t.handler_kind() == "callback")
            .map(|t| t.name.clone())
            .collect();

        Self {
            tools,
            callback_tools,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for MpcToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        // Check if this is a callback tool
        if self.callback_tools.contains(call.name) {
            // Return a special error that signals the agent loop should pause
            Err(ToolError::callback_pending(call.name, args))
        } else {
            Err(ToolError::not_found(call.name))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use futures::stream;
    use meerkat::Session;
    use meerkat::surface::{
        CancelActionInstallOutcome, SurfaceRequestExecutor, noop_request_action,
    };
    use meerkat::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::time::{Duration, timeout};

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn unwrap_payload(value: Value) -> Value {
        if value.get("content").is_none() {
            return value;
        }
        let raw = value["content"][0]["text"]
            .as_str()
            .expect("wrapped MCP payload text");
        serde_json::from_str(raw).expect("wrapped payload JSON")
    }

    async fn state_with_persisted_session() -> (MeerkatMcpState, String) {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store.clone()).await;
        let session = Session::new();
        let session_id = session.id().to_string();
        store.save(&session).await.expect("persisted session");
        (state, session_id)
    }

    #[cfg(feature = "mob")]
    struct McpFailClearEventStore {
        inner: meerkat_mob::store::InMemoryMobEventStore,
        fail_clear: AtomicBool,
    }

    #[cfg(feature = "mob")]
    impl McpFailClearEventStore {
        fn new() -> Self {
            Self {
                inner: meerkat_mob::store::InMemoryMobEventStore::new(),
                fail_clear: AtomicBool::new(true),
            }
        }

        fn allow_clear(&self) {
            self.fail_clear.store(false, Ordering::SeqCst);
        }
    }

    #[cfg(feature = "mob")]
    #[async_trait]
    impl meerkat_mob::store::MobEventStore for McpFailClearEventStore {
        async fn append(
            &self,
            event: meerkat_mob::NewMobEvent,
        ) -> Result<meerkat_mob::MobEvent, meerkat_mob::store::MobStoreError> {
            self.inner.append(event).await
        }

        async fn append_terminal_event_if_absent(
            &self,
            event: meerkat_mob::NewMobEvent,
        ) -> Result<Option<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.append_terminal_event_if_absent(event).await
        }

        async fn append_batch(
            &self,
            events: Vec<meerkat_mob::NewMobEvent>,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.append_batch(events).await
        }

        async fn poll(
            &self,
            after_cursor: u64,
            limit: usize,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.poll(after_cursor, limit).await
        }

        async fn replay_all(
            &self,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.replay_all().await
        }

        async fn latest_cursor(&self) -> Result<u64, meerkat_mob::store::MobStoreError> {
            self.inner.latest_cursor().await
        }

        fn subscribe(
            &self,
        ) -> Result<meerkat_mob::store::MobEventReceiver, meerkat_mob::store::MobStoreError>
        {
            self.inner.subscribe()
        }

        async fn clear(&self) -> Result<(), meerkat_mob::store::MobStoreError> {
            if !self.fail_clear.load(Ordering::SeqCst) {
                return self.inner.clear().await;
            }
            Err(meerkat_mob::store::MobStoreError::Internal(
                "forced MCP archive mob destroy clear failure".to_string(),
            ))
        }
    }

    #[cfg(feature = "mob")]
    async fn insert_mcp_archive_partial_destroy_mob(
        state: &MeerkatMcpState,
        owner_session_id: &str,
    ) -> (meerkat_mob::MobId, Arc<McpFailClearEventStore>) {
        let mob_id = meerkat_mob::MobId::from("mcp-session-archive-partial-destroy");
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        definition.mark_owner_bridge_session_indexed(owner_session_id);
        let events = Arc::new(McpFailClearEventStore::new());
        let storage = meerkat_mob::MobStorage::with_events(events.clone());
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_session_service(state.mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create archive-owned mob with failing event clear");
        state
            .mob_state
            .mob_insert_handle(mob_id.clone(), handle)
            .await;
        (mob_id, events)
    }

    #[cfg(feature = "mob")]
    async fn insert_mcp_archive_live_member(
        mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
    ) -> (meerkat_mob::MobId, meerkat::SessionId) {
        let mob_id = meerkat_mob::MobId::from("mcp-session-archive-live-member");
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig {
                    comms: true,
                    ..meerkat_mob::ToolConfig::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        let handle = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create live MCP archive mob");
        let identity = meerkat_mob::AgentIdentity::from("worker-1");
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                identity.clone(),
            ))
            .await
            .expect("spawn live MCP mob member");
        let bridge_session_id = handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("turn-driven worker should have a bridge session");
        mob_state.mob_insert_handle(mob_id.clone(), handle).await;
        (mob_id, bridge_session_id)
    }

    fn seed_active_external_surface(
        handle: &dyn meerkat_core::handles::ExternalToolSurfaceHandle,
        surface_id: &str,
    ) {
        handle
            .stage_add(surface_id.to_string(), 0)
            .expect("seed stage add");
        let staged_sequence = handle
            .surface_snapshot(surface_id)
            .and_then(|snapshot| snapshot.staged_intent_sequence)
            .expect("seed staged sequence");
        handle
            .apply_boundary(surface_id.to_string(), 0, staged_sequence, staged_sequence)
            .expect("seed apply boundary");
        let pending_sequence = handle
            .surface_snapshot(surface_id)
            .and_then(|snapshot| snapshot.pending_task_sequence)
            .expect("seed pending sequence");
        handle
            .mark_pending_succeeded(surface_id.to_string(), pending_sequence, staged_sequence)
            .expect("seed active surface");
    }

    async fn cancelled_request_context(key: &str) -> RequestContext {
        use meerkat::surface::CancelOutcome;
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let context = executor.begin_request(key.to_string(), noop_request_action());
        let outcome = executor.cancel_request(key).await;
        assert_eq!(
            outcome,
            CancelOutcome::Cancelled,
            "pre-cancel should transition Pending → Cancelled"
        );
        context
    }

    fn hooks_override_fixture() -> HookRunOverrides {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../test-fixtures/hooks/run_override.json");
        let payload = std::fs::read_to_string(path).expect("hook override fixture must exist");
        serde_json::from_str::<HookRunOverrides>(&payload)
            .expect("hook override fixture must deserialize")
    }

    #[test]
    fn test_config_envelope_value_redacts_paths_by_default() {
        let snapshot = meerkat_core::ConfigSnapshot {
            config: Config::default(),
            generation: 1,
            metadata: Some(meerkat_core::ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("mcp-1".to_string()),
                backend: Some("sqlite".to_string()),
                resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_sqlite_path: Some("/tmp/root/sessions.sqlite3".to_string()),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let value = config_envelope_value(snapshot, false).expect("envelope value");
        assert!(value.get("resolved_paths").is_none());
    }

    #[test]
    fn test_config_envelope_value_includes_paths_when_enabled() {
        let snapshot = meerkat_core::ConfigSnapshot {
            config: Config::default(),
            generation: 1,
            metadata: Some(meerkat_core::ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("mcp-1".to_string()),
                backend: Some("sqlite".to_string()),
                resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_sqlite_path: Some("/tmp/root/sessions.sqlite3".to_string()),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let value = config_envelope_value(snapshot, true).expect("envelope value");
        assert!(value.get("resolved_paths").is_some());
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_tool_payload_uses_typed_comms_result_contract() {
        let envelope_id = "550e8400-e29b-41d4-a716-446655440000"
            .parse()
            .expect("valid envelope uuid");
        let interaction_id = meerkat_core::interaction::InteractionId(
            "550e8400-e29b-41d4-a716-446655440001"
                .parse()
                .expect("valid interaction uuid"),
        );

        let wrapped = comms_send_tool_payload(meerkat_core::comms::SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved: true,
        });
        let payload = unwrap_payload(wrapped);
        let receipt = &payload["receipt"];

        assert_eq!(receipt["kind"], serde_json::json!("peer_request_sent"));
        assert_eq!(
            receipt["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
        assert_eq!(
            receipt["interaction_id"],
            serde_json::json!(interaction_id.0.to_string())
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_peers_tool_payload_uses_typed_core_wire_contract() {
        let wrapped = comms_peers_tool_payload(vec![sample_peer_directory_entry()]);
        let payload = unwrap_payload(wrapped);

        assert_peer_directory_wire(&payload);
    }

    #[cfg(feature = "comms")]
    fn sample_peer_directory_entry() -> meerkat_core::comms::PeerDirectoryEntry {
        meerkat_core::comms::PeerDirectoryEntry {
            peer_id: meerkat_core::comms::PeerId::new(),
            name: meerkat_core::comms::PeerName::new("agent").unwrap(),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Inproc,
                "agent",
            ),
            source: meerkat_core::comms::PeerDirectorySource::Inproc,
            sendable_kinds: vec![
                meerkat_core::comms::PeerSendability::PeerMessage,
                meerkat_core::comms::PeerSendability::PeerRequest,
            ],
            capabilities: meerkat_core::comms::PeerCapabilitySet::default()
                .with_extension("vendor.echo", serde_json::json!({ "enabled": true })),
            reachability: meerkat_core::comms::PeerReachability::Reachable,
            last_unreachable_reason: None,
            meta: meerkat_core::PeerMeta::default(),
        }
    }

    #[cfg(feature = "comms")]
    fn assert_peer_directory_wire(result: &Value) {
        let peer = &result["peers"][0];

        assert_eq!(peer["source"], "inproc");
        assert_eq!(
            peer["sendable_kinds"],
            serde_json::json!(["peer_message", "peer_request"])
        );
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(
            peer["capabilities"]["extensions"]["vendor.echo"]["enabled"],
            true
        );
    }

    #[test]
    fn test_config_runtime_error_generation_conflict_is_typed() {
        let err = map_config_runtime_error(ConfigRuntimeError::GenerationConflict {
            expected: 2,
            current: 5,
        });
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("generation conflict"));
        let data = err.data.expect("generation conflict should include data");
        assert_eq!(data["type"], "generation_conflict");
        assert_eq!(data["expected_generation"], 2);
        assert_eq!(data["current_generation"], 5);
    }

    #[test]
    fn test_validate_config_for_commit_rejects_invalid_skills_identity() {
        let mut config = Config::default();
        let source_uuid =
            meerkat_core::skills::SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("uuid");
        config.skills.repositories = vec![
            meerkat_core::skills_config::SkillRepositoryConfig {
                name: "a".into(),
                source_uuid: source_uuid.clone(),
                transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                    path: "/tmp/a".to_string(),
                },
            },
            meerkat_core::skills_config::SkillRepositoryConfig {
                name: "b".into(),
                source_uuid,
                transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                    path: "/tmp/b".to_string(),
                },
            },
        ];

        let err = validate_config_for_commit(&config).expect_err("duplicate source uuid");
        assert_eq!(err.code, -32602);
        assert!(
            err.message
                .contains("Invalid skills source-identity config")
        );
    }

    #[test]
    fn test_config_set_rejects_payload_that_would_default_missing_fields() {
        let raw = json!({
            "max_tokens": 448
        });
        let parsed: Config = serde_json::from_value(raw.clone())
            .expect("serde would otherwise fabricate config defaults");
        let err = reject_config_payload_defaulting(&raw, &parsed)
            .expect_err("partial config set must be rejected");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("missing field `config.agent`"));
    }

    #[test]
    fn test_config_set_accepts_complete_typed_payload() {
        let config = Config::default();
        let raw = serde_json::to_value(&config).expect("config serializes");
        reject_config_payload_defaulting(&raw, &config).expect("complete config is accepted");
    }

    #[tokio::test]
    async fn test_config_rejects_unknown_action_string_before_dispatch() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({
                "action": "default_if_unknown",
                "config": {"max_tokens": 448}
            }),
        ))
        .await
        .expect_err("unknown config action string must be rejected before dispatch");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("unknown variant") || err.message.contains("expected one of"));
    }

    #[cfg(feature = "mob")]
    fn unwrap_tool_payload_json(value: Value) -> Value {
        let text = value["content"][0]["text"]
            .as_str()
            .expect("tool payload text");
        serde_json::from_str(text).expect("json payload")
    }

    #[test]
    fn test_tools_list_schema() {
        let tools = tools_list();
        let schedule_tool_count = meerkat::schedule_tools_list().len();
        #[cfg(all(feature = "comms", feature = "mob"))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + mob_host_tools_list().len()
                + mob_event_stream_tools_list().len()
                + comms_tools_list().len()
        );
        #[cfg(all(not(feature = "comms"), feature = "mob"))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + mob_host_tools_list().len()
                + mob_event_stream_tools_list().len()
        );
        #[cfg(all(feature = "comms", not(feature = "mob")))]
        assert_eq!(
            tools.len(),
            base_tools_list().len() + schedule_tool_count + comms_tools_list().len()
        );
        #[cfg(all(not(feature = "comms"), not(feature = "mob")))]
        assert_eq!(tools.len(), base_tools_list().len() + schedule_tool_count);

        let tool_names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        let find_tool = |name: &str| {
            tools
                .iter()
                .find(|tool| tool["name"] == name)
                .expect("tool should be present")
        };

        let run_tool = find_tool("meerkat_run");
        let help_tool = find_tool("meerkat_help");
        assert!(help_tool["inputSchema"]["properties"]["question"].is_object());
        assert!(help_tool["inputSchema"]["properties"]["prompt"].is_object());
        assert!(help_tool["inputSchema"]["properties"]["execution_mode"].is_object());
        assert!(run_tool["inputSchema"]["properties"]["prompt"].is_object());
        assert!(run_tool["inputSchema"]["properties"]["verbose"].is_object());
        assert!(
            run_tool["inputSchema"]["properties"]
                .get("output_schema")
                .is_some()
        );
        assert_eq!(
            find_tool("meerkat_schedule_create")["name"],
            "meerkat_schedule_create"
        );
        assert_eq!(
            find_tool("meerkat_schedule_update")["name"],
            "meerkat_schedule_update"
        );
        assert_eq!(
            find_tool("meerkat_schedule_occurrences")["name"],
            "meerkat_schedule_occurrences"
        );
        assert!(
            run_tool["inputSchema"]["properties"]
                .get("structured_output_retries")
                .is_some()
        );

        let resume_tool = find_tool("meerkat_resume");
        assert!(resume_tool["inputSchema"]["properties"]["session_id"].is_object());
        assert!(resume_tool["inputSchema"]["properties"]["verbose"].is_object());

        let config_tool = find_tool("meerkat_config");
        assert!(config_tool["inputSchema"]["properties"]["action"].is_object());

        let capabilities_tool = find_tool("meerkat_capabilities");
        assert_eq!(capabilities_tool["name"], "meerkat_capabilities");
        let models_catalog_tool = find_tool("meerkat_models_catalog");
        assert_eq!(models_catalog_tool["name"], "meerkat_models_catalog");
        let read_tool = find_tool("meerkat_read");
        assert_eq!(read_tool["name"], "meerkat_read");
        let history_tool = find_tool("meerkat_history");
        assert_eq!(history_tool["name"], "meerkat_history");
        let sessions_tool = find_tool("meerkat_sessions");
        assert_eq!(sessions_tool["name"], "meerkat_sessions");
        let interrupt_tool = find_tool("meerkat_interrupt");
        assert_eq!(interrupt_tool["name"], "meerkat_interrupt");
        let archive_tool = find_tool("meerkat_archive");
        assert_eq!(archive_tool["name"], "meerkat_archive");
        let mcp_add_tool = find_tool("meerkat_mcp_add");
        assert_eq!(mcp_add_tool["name"], "meerkat_mcp_add");
        assert!(
            mcp_add_tool["inputSchema"]["properties"]
                .get("server_config")
                .is_some()
        );
        assert!(
            mcp_add_tool["inputSchema"]["properties"]
                .get("server_name")
                .is_none()
        );
        let mcp_remove_tool = find_tool("meerkat_mcp_remove");
        assert_eq!(mcp_remove_tool["name"], "meerkat_mcp_remove");
        let mcp_reload_tool = find_tool("meerkat_mcp_reload");
        assert_eq!(mcp_reload_tool["name"], "meerkat_mcp_reload");
        let event_stream_open_tool = find_tool("meerkat_event_stream_open");
        assert_eq!(event_stream_open_tool["name"], "meerkat_event_stream_open");
        let realtime_open_info_tool = find_tool("meerkat_realtime_open_info");
        assert_eq!(
            realtime_open_info_tool["name"],
            "meerkat_realtime_open_info"
        );
        let event_stream_read_tool = find_tool("meerkat_event_stream_read");
        assert_eq!(event_stream_read_tool["name"], "meerkat_event_stream_read");
        let event_stream_close_tool = find_tool("meerkat_event_stream_close");
        assert_eq!(
            event_stream_close_tool["name"],
            "meerkat_event_stream_close"
        );

        #[cfg(feature = "mob")]
        {
            assert!(!tool_names.contains(&"meerkat_mob_prefabs"));
            assert!(!tool_names.contains(&"mob_create"));
            assert!(!tool_names.contains(&"mob_list"));
            assert!(!tool_names.contains(&"mob_lifecycle"));
            assert!(tool_names.contains(&"meerkat_mob_create"));
            assert!(tool_names.contains(&"meerkat_mob_list"));
            assert!(tool_names.contains(&"meerkat_mob_status"));
            assert!(tool_names.contains(&"meerkat_mob_lifecycle"));
            assert!(tool_names.contains(&"meerkat_mob_spawn"));
            assert!(tool_names.contains(&"meerkat_mob_spawn_many"));
            assert!(tool_names.contains(&"meerkat_mob_member_send"));
            assert!(tool_names.contains(&"meerkat_mob_event_stream_open"));
            assert!(tool_names.contains(&"meerkat_mob_event_stream_read"));
            assert!(tool_names.contains(&"meerkat_mob_event_stream_close"));
        }
        #[cfg(not(feature = "mob"))]
        {
            assert!(!tool_names.contains(&"mob_create"));
            assert!(!tool_names.contains(&"mob_list"));
            assert!(!tool_names.contains(&"mob_lifecycle"));
            assert!(!tool_names.contains(&"meerkat_mob_create"));
            assert!(!tool_names.contains(&"meerkat_mob_list"));
            assert!(!tool_names.contains(&"meerkat_mob_status"));
            assert!(!tool_names.contains(&"meerkat_mob_lifecycle"));
            assert!(!tool_names.contains(&"meerkat_mob_event_stream_open"));
            assert!(!tool_names.contains(&"meerkat_mob_event_stream_read"));
            assert!(!tool_names.contains(&"meerkat_mob_event_stream_close"));
        }

        #[cfg(feature = "comms")]
        {
            assert!(tool_names.contains(&"meerkat_comms_send"));
            assert!(tool_names.contains(&"meerkat_comms_peers"));
        }
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_tools_list_omits_comms_tools_when_feature_disabled() {
        let tools = tools_list();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.contains(&"meerkat_comms_send"));
        assert!(!names.contains(&"meerkat_comms_peers"));
        assert!(names.contains(&"meerkat_event_stream_open"));
        assert!(names.contains(&"meerkat_event_stream_read"));
        assert!(names.contains(&"meerkat_event_stream_close"));
    }

    #[test]
    fn test_meerkat_run_input_parsing() {
        let input_json = json!({
            "prompt": "Hello",
            "model": "claude-sonnet-4"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.model, Some("claude-sonnet-4".to_string()));
        assert_eq!(input.max_tokens, None);
        assert_eq!(input.structured_output_retries, None);
        assert!(input.output_schema.is_none());
        assert!(!input.verbose);
    }

    #[test]
    fn test_meerkat_resume_input_parsing() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Continue"
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.session_id, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(input.prompt, "Continue");
        assert!(input.system_prompt.is_none());
        assert!(input.output_schema.is_none());
        assert_eq!(input.structured_output_retries, None);
        assert!(!input.verbose);
    }

    #[test]
    fn test_meerkat_run_input_with_tools() {
        let input_json = json!({
            "prompt": "Hello",
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Get weather for a city",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"}
                        },
                        "required": ["city"]
                    }
                }
            ]
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.tools.len(), 1);
        assert_eq!(input.tools[0].name, "get_weather");
        assert_eq!(input.tools[0].handler_kind(), "callback"); // default
    }

    #[test]
    fn test_meerkat_resume_input_with_tool_results() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "",
            "tool_results": [
                {
                    "tool_use_id": "tc_123",
                    "content": "Sunny, 72°F"
                }
            ]
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.session_id, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(input.tool_results.len(), 1);
        assert_eq!(input.tool_results[0].tool_use_id, "tc_123");
        assert_eq!(input.tool_results[0].content, "Sunny, 72\u{b0}F");
        assert!(!input.tool_results[0].is_error);
    }

    #[test]
    fn test_meerkat_run_input_accepts_hooks_override_fixture() {
        let input_json = json!({
            "prompt": "Hello",
            "hooks_override": hooks_override_fixture(),
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert!(input.hooks_override.is_some());
        let overrides = input
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[0].point,
            meerkat_core::HookPoint::PreToolExecution
        );
    }

    #[test]
    fn test_meerkat_resume_input_accepts_hooks_override_fixture() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Resume",
            "hooks_override": hooks_override_fixture(),
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert!(input.hooks_override.is_some());
        let overrides = input
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[1].mode,
            meerkat_core::HookExecutionMode::Background
        );
    }

    #[test]
    fn test_format_agent_result_includes_skill_diagnostics() {
        let session_id = meerkat::SessionId::new();
        let result = meerkat_core::types::RunResult {
            text: "ok".to_string(),
            session_id: session_id.clone(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: Some(meerkat_core::skills::SkillRuntimeDiagnostics {
                source_health: meerkat_core::skills::SourceHealthSnapshot {
                    state: meerkat_core::skills::SourceHealthState::Unhealthy,
                    invalid_ratio: 0.9,
                    invalid_count: 9,
                    total_count: 10,
                    failure_streak: 10,
                    handshake_failed: true,
                },
                quarantined: vec![],
            }),
        };

        let payload = format_agent_result(Ok(result), &session_id).expect("formatted payload");
        let raw = payload["content"][0]["text"]
            .as_str()
            .expect("wrapped MCP payload text");
        let decoded: serde_json::Value = serde_json::from_str(raw).expect("valid wrapped JSON");
        assert_eq!(decoded["content"][0]["text"], "ok");
        assert_eq!(
            decoded["skill_diagnostics"]["source_health"]["state"],
            "unhealthy"
        );
    }

    #[test]
    fn test_mpc_tool_dispatcher_creates_tool_defs() {
        let mcp_tools = vec![
            McpToolDef {
                name: "get_weather".into(),
                description: "Get weather".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                handler: Some("callback".to_string()),
            },
            McpToolDef {
                name: "search".into(),
                description: "Search".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                handler: Some("callback".to_string()),
            },
        ];

        let dispatcher = MpcToolDispatcher::new(&mcp_tools);
        let tool_defs = dispatcher.tools();

        assert_eq!(tool_defs.len(), 2);
        assert_eq!(tool_defs[0].name, "get_weather");
        assert_eq!(tool_defs[1].name, "search");
    }

    #[tokio::test]
    async fn test_mpc_tool_dispatcher_returns_callback_error() {
        let mcp_tools = vec![McpToolDef {
            name: "get_weather".into(),
            description: "Get weather".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        }];

        let dispatcher = MpcToolDispatcher::new(&mcp_tools);
        let args_raw =
            serde_json::value::RawValue::from_string(json!({"city": "Tokyo"}).to_string()).unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: "get_weather",
            args: &args_raw,
        };
        let result = dispatcher.dispatch(call).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_callback_pending(), "Expected CallbackPending error");
        if let Some((tool_name, args)) = err.as_callback_pending() {
            assert_eq!(tool_name, "get_weather");
            assert_eq!(args["city"], "Tokyo");
        }
    }

    #[test]
    fn test_tools_list_has_tools_parameter() {
        let tools = tools_list();
        let run_tool = &tools[0];

        // Verify tools parameter exists in the schema
        assert!(run_tool["inputSchema"]["properties"]["tools"].is_object());
        assert_eq!(
            run_tool["inputSchema"]["properties"]["tools"]["type"],
            "array"
        );
    }

    #[test]
    fn test_tools_list_has_tool_results_parameter() {
        let tools = tools_list();
        let resume_tool = &tools[1];

        // Verify tool_results parameter exists in the schema
        let tool_results = &resume_tool["inputSchema"]["properties"]["tool_results"];
        assert!(tool_results.is_object(), "tool_results should be an object");
        assert_eq!(
            tool_results["type"], "array",
            "tool_results should be an array"
        );
        // Items may use $ref for the ToolResultInput schema
        assert!(
            tool_results["items"].is_object(),
            "tool_results items should be defined"
        );
    }

    #[test]
    fn test_tools_list_has_blob_get_tool() {
        let blob_tool = tools_list()
            .into_iter()
            .find(|tool| tool["name"] == "meerkat_blob_get")
            .expect("meerkat_blob_get tool must exist");
        assert_eq!(blob_tool["name"], "meerkat_blob_get");
        assert_eq!(
            blob_tool["inputSchema"]["properties"]["blob_id"]["type"],
            "string"
        );
    }

    #[test]
    fn test_tools_list_skills_schema_has_source_selector() {
        let tools = tools_list();
        let skills_tool = tools.iter().find(|t| t["name"] == "meerkat_skills");
        let skills_tool = skills_tool.expect("meerkat_skills tool must exist");
        assert!(
            skills_tool["inputSchema"]["properties"]["source"].is_object(),
            "skills inspect should expose optional source selector"
        );
    }

    #[test]
    fn test_tools_list_skills_schema_has_typed_action_enum() {
        let tools = tools_list();
        let skills_tool = tools
            .iter()
            .find(|t| t["name"] == "meerkat_skills")
            .expect("meerkat_skills tool must exist");
        let action_schema = &skills_tool["inputSchema"]["properties"]["action"];
        let action_enum = action_schema.get("enum").or_else(|| {
            let action_ref = action_schema["$ref"].as_str()?;
            let definition = action_ref
                .strip_prefix("#/$defs/")
                .or_else(|| action_ref.strip_prefix("#/definitions/"))?;
            skills_tool["inputSchema"]["$defs"][definition]
                .get("enum")
                .or_else(|| skills_tool["inputSchema"]["definitions"][definition].get("enum"))
        });
        assert_eq!(
            action_enum,
            Some(&serde_json::json!(["list", "inspect"])),
            "skills action should be a closed typed enum, got {action_schema}"
        );

        let input: MeerkatSkillsInput =
            serde_json::from_value(serde_json::json!({ "action": "list" })).unwrap();
        assert!(matches!(input.action, MeerkatSkillsAction::List));
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_resolve_keep_alive_rejects_when_comms_disabled() {
        let err = resolve_keep_alive(Some(true)).expect_err("keep_alive should be rejected");
        assert!(err.contains("keep_alive requires comms support"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_resolve_keep_alive_allows_when_comms_enabled() {
        assert_eq!(resolve_keep_alive(Some(true)).unwrap(), Some(true));
        assert_eq!(resolve_keep_alive(Some(false)).unwrap(), Some(false));
        assert_eq!(resolve_keep_alive(None).unwrap(), None);
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_meerkat_run_input_with_keep_alive() {
        let input_json = json!({
            "prompt": "Hello",
            "keep_alive": true,
            "comms_name": "test-agent"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.keep_alive, Some(true));
        assert_eq!(input.comms_name, Some("test-agent".to_string()));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_meerkat_run_input_keep_alive_defaults_to_none() {
        let input_json = json!({
            "prompt": "Hello"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.keep_alive, None);
        assert!(input.comms_name.is_none());
    }

    #[test]
    fn test_meerkat_resume_input_preserves_optional_override_presence() {
        let omitted: MeerkatResumeInput = serde_json::from_value(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Resume"
        }))
        .unwrap();
        assert_eq!(omitted.enable_builtins, None);
        assert_eq!(omitted.structured_output_retries, None);
        assert!(
            omitted
                .builtin_config
                .as_ref()
                .is_none_or(|cfg| cfg.enable_shell.is_none())
        );

        let explicit: MeerkatResumeInput = serde_json::from_value(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Resume",
            "enable_builtins": false,
            "structured_output_retries": 4,
            "builtin_config": {
                "enable_shell": false
            }
        }))
        .unwrap();
        assert_eq!(explicit.enable_builtins, Some(false));
        assert_eq!(explicit.structured_output_retries, Some(4));
        assert_eq!(
            explicit
                .builtin_config
                .as_ref()
                .and_then(|cfg| cfg.enable_shell),
            Some(false)
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_mcp_comms_send_error_includes_structured_details() {
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::PeerOffline,
        );
        assert_eq!(err.code, -32603);
        assert!(err.message.starts_with("peer_unreachable:"));
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("peer_unreachable")
        );
        assert_eq!(data.get("peer").and_then(Value::as_str), Some("peer-a"));
        assert_eq!(
            data.get("reason").and_then(Value::as_str),
            Some("offline_or_no_ack")
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_run_keep_alive_requires_comms_name() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let result = Box::pin(handle_meerkat_run(
            &state,
            MeerkatRunInput {
                prompt: "test".to_string(),
                system_prompt: None,
                model: Some("claude-opus-4-6".to_string()),
                max_tokens: Some(4096),
                provider: None,
                output_schema: None,
                structured_output_retries: Some(2),
                stream: false,
                verbose: false,
                tools: vec![],
                enable_builtins: Some(false),
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None, // Missing!
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                labels: None,
                additional_instructions: None,
                app_context: None,
                shell_env: None,
                auth_binding: None,
            },
            None,
            None,
        ))
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("comms_name"));
    }

    #[tokio::test]
    async fn test_handle_meerkat_run_returns_request_cancelled_when_pre_cancelled() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let context = cancelled_request_context("req-run-cancelled").await;

        let result = Box::pin(handle_meerkat_run(
            &state,
            MeerkatRunInput {
                prompt: "test".to_string(),
                system_prompt: None,
                model: Some("claude-opus-4-6".to_string()),
                max_tokens: Some(4096),
                provider: None,
                output_schema: None,
                structured_output_retries: Some(2),
                stream: false,
                verbose: false,
                tools: vec![],
                enable_builtins: Some(false),
                builtin_config: None,
                keep_alive: Some(false),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                labels: None,
                additional_instructions: None,
                app_context: None,
                shell_env: None,
                auth_binding: None,
            },
            None,
            Some(context),
        ))
        .await;

        let err = result.expect_err("pre-cancelled run should be rejected");
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
    }

    #[tokio::test]
    async fn test_meerkat_run_composition_failure_unregisters_prepared_runtime() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_and_max_sessions(store, Some(1)).await;
        let prepared = prepare_surface_session(&state.runtime_adapter)
            .await
            .expect("initial runtime slot should prepare");
        let primary_tool = McpToolDef {
            name: "primary_callback".to_string(),
            description: "Primary callback".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        };
        let duplicate_secondary = McpToolDef {
            name: "duplicate_secondary".to_string(),
            description: "Duplicate secondary callback".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        };
        let primary =
            Arc::new(MpcToolDispatcher::new(&[primary_tool])) as Arc<dyn AgentToolDispatcher>;
        let secondary = Arc::new(MpcToolDispatcher::new(&[
            duplicate_secondary.clone(),
            duplicate_secondary,
        ])) as Arc<dyn AgentToolDispatcher>;

        let result = compose_run_external_tool_dispatchers(
            &state,
            &prepared.session_id,
            Some(primary),
            Some(secondary),
        )
        .await;

        let err = match result {
            Ok(_) => panic!("duplicate composed tools should fail composition"),
            Err(err) => err,
        };
        assert!(
            err.message.contains("failed to compose external tools"),
            "expected composition failure, got {err:?}"
        );

        let next_prepared = prepare_surface_session(&state.runtime_adapter)
            .await
            .expect("composition failure should release the single runtime slot");
        state
            .runtime_adapter
            .unregister_session(&next_prepared.session_id)
            .await;
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_returns_request_cancelled_when_pre_cancelled() {
        let (state, session_id) = state_with_persisted_session().await;
        let context = cancelled_request_context("req-resume-cancelled").await;

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id,
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            Some(context),
        ))
        .await;

        let err = result.expect_err("pre-cancelled resume should be rejected");
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
    }

    #[tokio::test]
    async fn mcp_request_cancel_after_action_install_rejects_before_service_admission() {
        use meerkat::surface::CancelOutcome;

        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let context = executor.begin_request("req-cancel-before-admission", noop_request_action());
        let install = context
            .install_cancel_action_or_cancelled(noop_request_action())
            .await;
        assert_eq!(install, CancelActionInstallOutcome::Installed);

        let outcome = executor.cancel_request(context.key()).await;
        assert_eq!(outcome, CancelOutcome::Cancelled);

        let cleaned = Arc::new(AtomicBool::new(false));
        let cleaned_for_gate = Arc::clone(&cleaned);
        let err = reject_if_cancelled_before_mcp_service_admission(Some(&context), async move {
            cleaned_for_gate.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect_err("cancel after action install must reject before service admission");

        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
        assert!(
            cleaned.load(Ordering::SeqCst),
            "pre-admission cancel gate must run cleanup"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_post_prepare_validation_failure_unregisters_runtime() {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id,
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: Some(json!("not-a-schema")),
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("invalid output schema should reject resume");
        assert!(
            err.message.contains("Invalid output_schema"),
            "expected output schema validation error: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&parsed).await,
            "post-prepare resume validation failure should unregister runtime state"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_post_prepare_validation_failure_preserves_existing_runtime()
    {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        state
            .runtime_ingress_context()
            .ensure_session(&parsed)
            .await;
        assert!(
            state.runtime_adapter.contains_session(&parsed).await,
            "test requires a pre-existing runtime registration"
        );

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id,
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: Some(json!("not-a-schema")),
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("invalid output schema should reject resume");
        assert!(
            err.message.contains("Invalid output_schema"),
            "expected output schema validation error: {err:?}"
        );
        assert!(
            state.runtime_adapter.contains_session(&parsed).await,
            "post-prepare resume validation failure should preserve existing runtime state"
        );
    }

    #[tokio::test]
    async fn test_mcp_resume_new_binding_cleanup_preserves_foreign_active_input() {
        use meerkat_runtime::SessionServiceRuntimeExt;
        use meerkat_runtime::input::{
            ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
        };

        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        let runtime_was_registered = state.runtime_adapter.contains_session(&parsed).await;
        let runtime_state_existed = state.runtime_sessions.read().await.contains_key(&parsed);
        state
            .runtime_adapter
            .prepare_bindings(parsed.clone())
            .await
            .expect("prepare runtime bindings");

        let input = Input::ExternalEvent(ExternalEventInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: meerkat_core::types::message_timestamp_now(),
                source: InputOrigin::External {
                    source_name: "mcp-review".to_string(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "mcp-review".to_string(),
            payload: json!({"status": "queued"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let accepted = state
            .runtime_adapter
            .accept_input_without_wake(&parsed, input)
            .await
            .expect("foreign active input should queue");
        assert!(
            matches!(accepted, meerkat_runtime::AcceptOutcome::Accepted { .. }),
            "foreign input should be accepted: {accepted:?}"
        );
        assert!(
            !state
                .runtime_adapter
                .list_active_inputs(&parsed)
                .await
                .expect("list active inputs")
                .is_empty(),
            "test requires an active runtime input"
        );

        state
            .clear_surface_bindings_if_new(&parsed, runtime_was_registered, runtime_state_existed)
            .await;
        assert!(
            state.runtime_adapter.contains_session(&parsed).await,
            "new-binding cleanup must preserve a foreign active runtime input"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_capacity_failure_unregisters_prepared_runtime() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state =
            MeerkatMcpState::new_with_store_and_max_sessions(Arc::clone(&store), Some(1)).await;
        let session = Session::new();
        let session_id = session.id().clone();
        store.save(&session).await.expect("persisted session");

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        let _blocker = state
            .service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("blocker should fill active capacity");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("capacity-full resume should reject");
        assert!(
            err.message.contains("Max sessions"),
            "expected capacity error: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "failed persisted-only resume must not leave the prepared runtime registered"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_live_no_rebuild_capacity_failure_does_not_prepare_runtime()
    {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state =
            MeerkatMcpState::new_with_store_and_max_sessions(Arc::clone(&store), Some(1)).await;
        let pre_session = Session::new();
        let session_id = pre_session.id().clone();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should prepare");
        let mcp_adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(
            McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
        ));
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            CreateSessionRequest {
                model: "claude-opus-4-6".to_string(),
                prompt: "Initial live turn".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(4096),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    resume_session: Some(pre_session),
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                    )),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..Default::default()
                }),
                labels: None,
            },
            None,
        )
        .await
        .expect("live session create should succeed");
        state.upsert_mcp_adapter(&session_id, mcp_adapter).await;
        assert!(
            state
                .service
                .has_live_session(&session_id)
                .await
                .expect("live session lookup"),
            "test requires an existing live session"
        );
        state.runtime_adapter.unregister_session(&session_id).await;
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "test starts with no runtime adapter registration"
        );

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        let _blocker = state
            .service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("blocker should fill active capacity");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume live no rebuild".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("capacity-full live no-rebuild resume should reject");
        assert!(
            err.message.contains("Max sessions"),
            "expected capacity error: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "capacity failure in live no-rebuild resume must not prepare runtime bindings"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_resume_keep_alive_capacity_failure_does_not_configure_peer_ingress()
     {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state =
            MeerkatMcpState::new_with_store_and_max_sessions(Arc::clone(&store), Some(1)).await;
        let pre_session = Session::new();
        let session_id = pre_session.id().clone();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should prepare");
        let mcp_adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(
            McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
        ));
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            CreateSessionRequest {
                model: "claude-opus-4-6".to_string(),
                prompt: "Initial live turn".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(4096),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    resume_session: Some(pre_session),
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                    )),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    comms_name: Some("mcp-agent".to_string()),
                    keep_alive: false,
                    ..Default::default()
                }),
                labels: None,
            },
            None,
        )
        .await
        .expect("live session create should succeed");
        state.upsert_mcp_adapter(&session_id, mcp_adapter).await;
        assert!(
            state.service.comms_runtime(&session_id).await.is_some(),
            "test requires a live session with comms runtime"
        );
        assert!(
            !state.runtime_adapter.session_has_comms(&session_id).await,
            "test starts before peer ingress has been configured"
        );

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        let _blocker = state
            .service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("blocker should fill active capacity");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume live keep alive".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("capacity-full keep_alive resume should reject");
        assert!(
            err.message.contains("Max sessions"),
            "expected capacity error: {err:?}"
        );
        assert!(
            !state.runtime_adapter.session_has_comms(&session_id).await,
            "capacity failure must not configure peer ingress before active admission"
        );
        let stored = state
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load authoritative session")
            .expect("session should remain persisted");
        assert!(
            !stored
                .session_metadata()
                .expect("session metadata")
                .keep_alive,
            "capacity failure must not persist keep_alive before active admission"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_resume_keep_alive_live_missing_failure_unregisters_runtime() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(Arc::clone(&store)).await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-6".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: Some("stale-mcp-agent".to_string()),
                peer_meta: None,
                realm_id: Some(state.realm_id.to_string()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
            })
            .expect("session metadata should serialize");
        store.save(&session).await.expect("persisted session");
        state
            .upsert_mcp_adapter(
                &session_id,
                Arc::new(meerkat_mcp::McpRouterAdapter::new(McpRouter::new())),
            )
            .await;

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("live-missing keep_alive resume should reject");
        assert_eq!(err.code, -32602);
        assert!(
            err.message.contains("session created with comms_name"),
            "expected live-missing keep_alive rejection: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "failed keep_alive resume must not leave the prepared runtime registered"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_resume_keep_alive_live_missing_failure_preserves_existing_runtime()
    {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(Arc::clone(&store)).await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-6".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: Some("existing-mcp-agent".to_string()),
                peer_meta: None,
                realm_id: Some(state.realm_id.to_string()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
            })
            .expect("session metadata should serialize");
        store.save(&session).await.expect("persisted session");
        state
            .runtime_ingress_context()
            .ensure_session(&session_id)
            .await;
        state
            .upsert_mcp_adapter(
                &session_id,
                Arc::new(meerkat_mcp::McpRouterAdapter::new(McpRouter::new())),
            )
            .await;
        assert!(
            state.runtime_adapter.contains_session(&session_id).await,
            "test requires a pre-existing runtime registration"
        );

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("live-missing keep_alive resume should reject");
        assert_eq!(err.code, -32602);
        assert!(
            err.message.contains("session created with comms_name"),
            "expected live-missing keep_alive rejection: {err:?}"
        );
        assert!(
            state.runtime_adapter.contains_session(&session_id).await,
            "failed keep_alive resume should preserve existing runtime state"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_blob_get_returns_payload() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let blob_ref = state
            .service
            .blob_store()
            .put_image("image/png", "aGVsbG8=")
            .await
            .expect("blob stored");

        let result = Box::pin(handle_tools_call(
            &state,
            "meerkat_blob_get",
            &json!({ "blob_id": blob_ref.blob_id.as_str() }),
        ))
        .await
        .expect("blob get succeeds");

        let text = result["content"][0]["text"].as_str().expect("text payload");
        let payload: Value = serde_json::from_str(text).expect("valid json payload");
        assert_eq!(payload["blob_id"], blob_ref.blob_id.as_str());
        assert_eq!(payload["media_type"], "image/png");
        assert_eq!(payload["data"], "aGVsbG8=");
    }

    #[test]
    fn test_mcp_resume_requires_rebuild_for_tool_results_and_config_changes() {
        let mut input: MeerkatResumeInput = serde_json::from_value(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Continue"
        }))
        .unwrap();
        assert!(!mcp_resume_requires_rebuild(&input));

        input.tool_results = vec![ToolResultInput {
            tool_use_id: "tool-1".to_string(),
            content: "done".to_string(),
            is_error: false,
        }];
        assert!(mcp_resume_requires_rebuild(&input));
        input.tool_results.clear();

        input.enable_builtins = Some(false);
        assert!(mcp_resume_requires_rebuild(&input));
        input.enable_builtins = None;

        input.builtin_config = Some(BuiltinConfigInput {
            enable_shell: Some(false),
            shell_timeout_secs: None,
        });
        assert!(mcp_resume_requires_rebuild(&input));
        input.builtin_config = None;

        input.comms_name = Some("agent-a".to_string());
        assert!(mcp_resume_requires_rebuild(&input));
    }

    #[test]
    fn test_post_commit_session_created_error_includes_identity() {
        let session_id = meerkat::SessionId::new();
        let session_id_string = session_id.to_string();
        let err = post_commit_session_created_error(
            &session_id,
            &SessionError::Agent(meerkat::AgentError::InternalError("boom".to_string())),
        );
        let data = err.data.as_ref().expect("structured error data");
        assert_eq!(err.code, -32603);
        assert_eq!(
            data.get("session_created").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(data.get("resumable").and_then(Value::as_bool), Some(true));
        assert_eq!(
            data.get("session_id").and_then(Value::as_str),
            Some(session_id_string.as_str())
        );
    }

    #[test]
    fn test_format_agent_result_tool_preserves_cancelled_error_code() {
        let session_id = meerkat::SessionId::new();
        let err = format_agent_result_tool(
            Err(SessionError::Agent(meerkat::AgentError::Cancelled)),
            &session_id,
        )
        .expect_err("cancelled terminal should be a tool error");

        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
    }

    #[test]
    fn test_post_commit_session_created_error_preserves_cancelled_error_code() {
        let session_id = meerkat::SessionId::new();
        let err = post_commit_session_created_error(
            &session_id,
            &SessionError::Agent(meerkat::AgentError::Cancelled),
        );

        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
        let data = err.data.as_ref().expect("structured cancellation data");
        assert_eq!(
            data.get("session_created").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_tools_list_has_keep_alive_parameter() {
        let tools = tools_list();
        let run_tool = &tools[0];

        // Verify keep_alive parameter exists in the schema (nullable boolean)
        assert!(run_tool["inputSchema"]["properties"]["keep_alive"].is_object());

        // Verify comms_name parameter exists in the schema
        assert!(run_tool["inputSchema"]["properties"]["comms_name"].is_object());
        let comms_name_type = &run_tool["inputSchema"]["properties"]["comms_name"]["type"];
        assert!(
            comms_name_type == "string"
                || comms_name_type
                    .as_array()
                    .is_some_and(|types| types.contains(&json!("string"))),
            "unexpected comms_name type: {comms_name_type}"
        );
    }

    #[tokio::test]
    async fn test_handle_tools_call_unknown_tool_still_errors() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_prefabz_typo",
            &json!({}),
        ))
        .await
        .expect_err("unknown tool must error");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Unknown tool"));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_public_mcp_server_rejects_raw_mob_dispatcher_tool_names() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let err = Box::pin(handle_tools_call(&state, "mob_create", &json!({})))
            .await
            .expect_err("raw internal mob tool name must not be exposed");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Unknown tool"));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_public_mcp_server_exposes_typed_mob_create_and_rejects_internal_fields() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let created = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "mob-1",
                    "profiles": {
                        "worker": { "model": "claude-sonnet-4-6" }
                    }
                }
            }),
        ))
        .await
        .expect("typed mob create should succeed");
        let created = unwrap_tool_payload_json(created);
        assert_eq!(created["mob_id"], "mob-1");

        let listed = Box::pin(handle_tools_call(&state, "meerkat_mob_list", &json!({})))
            .await
            .expect("typed mob list should succeed");
        let listed = unwrap_tool_payload_json(listed);
        assert_eq!(listed["mobs"][0]["mob_id"], "mob-1");

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "mob-2",
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
        ))
        .await
        .expect_err("nested internal tool bundle fields must be rejected");
        assert_eq!(err.code, -32602);
        assert!(
            !err.message.is_empty(),
            "validation errors should return a non-empty message"
        );
    }

    #[tokio::test]
    async fn test_mcp_handlers_sessions_read_list_interrupt_archive() {
        let (state, session_id) = state_with_persisted_session().await;

        let listed = Box::pin(handle_tools_call(&state, "meerkat_sessions", &json!({})))
            .await
            .expect("sessions list call should succeed");
        let listed_payload = unwrap_payload(listed);
        let sessions = listed_payload["sessions"]
            .as_array()
            .expect("sessions should be an array");
        assert!(
            sessions
                .iter()
                .any(|entry| entry["session_id"] == json!(session_id)),
            "persisted session should appear in list"
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_read",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("read call should succeed");
        let read_payload = unwrap_payload(read);
        assert_eq!(
            read_payload["session_id"],
            read_payload["state"]["session_id"]
        );

        let interrupted = Box::pin(handle_tools_call(
            &state,
            "meerkat_interrupt",
            &json!({ "session_id": read_payload["session_id"] }),
        ))
        .await
        .expect("interrupt should no-op for non-running persisted session");
        let interrupted_payload = unwrap_payload(interrupted);
        assert_eq!(interrupted_payload["interrupted"], true);

        // Archive requires runtime authority; store-only sessions are
        // rejected. Verify archive produces a typed error rather than
        // silently succeeding on a store-only projection.
        let archive_result = Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": read_payload["session_id"] }),
        ))
        .await;
        assert!(
            archive_result.is_err(),
            "store-only session archive should require runtime authority"
        );
    }

    // test_mcp_archive_surfaces_incomplete_mob_cleanup_data deleted:
    // its retry premise (retain mob anchor, retry after partial cleanup)
    // is obsolete under runtime authority — the first archive retires
    // the session regardless of mob cleanup outcome.

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mcp_archive_does_not_mask_mob_member_archive_failure_with_child_cleanup() {
        let (mut state, _session_id) = state_with_persisted_session().await;
        let (mob_state, archive_failures) =
            meerkat_mob_mcp::MobMcpState::new_in_memory_with_archive_failure_control();
        state.mob_state = mob_state.clone();
        let (_parent_mob_id, member_session_id) = insert_mcp_archive_live_member(&mob_state).await;
        archive_failures
            .fail_archive(
                member_session_id.clone(),
                "forced MCP mob archive failure after retire event",
            )
            .await;
        let member_session_key = member_session_id.to_string();
        let (child_mob_id, child_events) =
            insert_mcp_archive_partial_destroy_mob(&state, &member_session_key).await;
        child_events.allow_clear();

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": member_session_key }),
        ))
        .await
        .expect_err("failed parent mob member archive must fail meerkat_archive");

        assert_eq!(err.code, -32603);
        assert!(
            err.message
                .contains("forced MCP mob archive failure after retire event"),
            "MCP archive should surface the parent bridge-session archive failure: {err:?}"
        );
        assert!(
            state
                .mob_state
                .session_service()
                .has_live_session(&member_session_id)
                .await
                .expect("check failed parent bridge session"),
            "failed ArchiveSession must retain the parent bridge session retry anchor"
        );
        assert!(
            state.mob_state.handle_for(&child_mob_id).await.is_ok(),
            "child cleanup must not be run as a success fallback while parent archive failed"
        );

        archive_failures
            .clear_archive_failure(&member_session_id)
            .await;
        let retry_success = Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": member_session_key }),
        ))
        .await
        .expect("retry should complete after parent archive failure clears");
        let retry_payload = unwrap_payload(retry_success);
        assert_eq!(
            retry_payload["archived"], true,
            "MCP archive retry should report success after parent archive and child cleanup complete"
        );
        assert!(
            !state
                .mob_state
                .session_service()
                .has_live_session(&member_session_id)
                .await
                .expect("check retried parent bridge session"),
            "successful retry must archive the parent bridge session"
        );
        assert!(
            state.mob_state.handle_for(&child_mob_id).await.is_err(),
            "successful retry must remove the child cleanup retry anchor"
        );
    }

    #[test]
    fn test_mcp_interrupt_not_ready_noop_is_only_idle_or_attached() {
        assert!(interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Idle
        ));
        assert!(interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Attached
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Destroyed
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Retired
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Stopped
        ));
    }

    #[tokio::test]
    async fn test_mcp_interrupt_ignores_cold_persisted_stopped_projection_when_session_exists() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_and_runtime_store(
            store,
            Some(Arc::clone(&runtime_store)),
        )
        .await;
        let created = state
            .service
            .create_session(CreateSessionRequest {
                model: "gpt-5.4".to_string(),
                prompt: "seed".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(32),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(TestClient::default()),
                    )),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("runtime-backed MCP service should create deferred session");
        state
            .runtime_adapter
            .register_session(created.session_id.clone())
            .await;
        state
            .runtime_adapter
            .stop_runtime_executor(&created.session_id, "seed stopped projection")
            .await
            .expect("runtime state should persist");
        state
            .runtime_adapter
            .unregister_session(&created.session_id)
            .await;

        let payload = Box::pin(handle_tools_call(
            &state,
            "meerkat_interrupt",
            &json!({ "session_id": created.session_id }),
        ))
        .await
        .expect("persisted stopped projection must not reject cold interrupt no-op");

        let payload = unwrap_payload(payload);
        assert_eq!(payload["interrupted"], true);
    }

    #[tokio::test]
    async fn test_mcp_history_returns_messages_for_live_and_archived_sessions() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(Arc::clone(&store)).await;
        let mut session = meerkat::Session::new();
        let session_id = session.id().to_string();

        session.push(meerkat_core::types::Message::System(
            meerkat_core::types::SystemMessage::new("system rules"),
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Hello".to_string()),
        ));
        session.push(meerkat_core::types::Message::Assistant(
            meerkat_core::types::AssistantMessage {
                content: "Hi there".to_string(),
                tool_calls: vec![],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Follow up".to_string()),
        ));
        session.push(meerkat_core::types::Message::Assistant(
            meerkat_core::types::AssistantMessage {
                content: "Second answer".to_string(),
                tool_calls: vec![],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        store
            .save(&session)
            .await
            .expect("persisted history session");

        let history = Box::pin(handle_tools_call(
            &state,
            "meerkat_history",
            &json!({ "session_id": session_id, "offset": 1, "limit": 2 }),
        ))
        .await
        .expect("history should succeed");
        let history_payload = unwrap_payload(history);
        assert!(
            history_payload["message_count"].as_u64().unwrap_or(0) >= 4,
            "history should expose the full transcript: {history_payload}"
        );
        assert_eq!(history_payload["offset"], 1);
        assert_eq!(history_payload["limit"], 2);
        assert_eq!(history_payload["has_more"], true);
        assert_eq!(history_payload["messages"].as_array().unwrap().len(), 2);

        // Archive requires runtime authority; store-only sessions are
        // rejected. Skip archived-history assertions when archive is
        // unavailable on the store-only test path.
        let archive_result = Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": session_id }),
        ))
        .await;

        if archive_result.is_err() {
            return;
        }

        let archived_history = Box::pin(handle_tools_call(
            &state,
            "meerkat_history",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("archived history should succeed");
        let archived_payload = unwrap_payload(archived_history);
        assert!(
            archived_payload["message_count"].as_u64().unwrap_or(0) >= 4,
            "archived history should preserve the transcript: {archived_payload}"
        );
        assert!(
            archived_payload["messages"].as_array().unwrap().len() >= 4,
            "archived history should return the full transcript"
        );

        let resume_err = Box::pin(handle_tools_call(
            &state,
            "meerkat_resume",
            &json!({
                "session_id": session_id,
                "prompt": "should not resume archived session"
            }),
        ))
        .await
        .expect_err("archived resume should be rejected");
        assert!(
            resume_err.message.contains("Session not found"),
            "archived resume should surface not found: {resume_err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(session.id()).await,
            "archived MCP resume must not register runtime state"
        );
    }

    #[tokio::test]
    async fn test_mcp_history_serializes_mixed_message_kinds() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(Arc::clone(&store)).await;
        let mut session = meerkat::Session::new();
        let session_id = session.id().to_string();

        session.push(meerkat_core::types::Message::System(
            meerkat_core::types::SystemMessage::new("system rules"),
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));
        session.push(meerkat_core::types::Message::Assistant(
            meerkat_core::types::AssistantMessage {
                content: "legacy ok".to_string(),
                tool_calls: vec![meerkat_core::types::ToolCall {
                    id: "tool-1".to_string(),
                    name: "search".into(),
                    args: serde_json::json!({ "query": "history" }),
                }],
                stop_reason: meerkat_core::types::StopReason::ToolUse,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        session.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage::new(
                vec![meerkat_core::types::AssistantBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "lookup".into(),
                    args: serde_json::value::RawValue::from_string(
                        serde_json::json!({ "item": "transcript" }).to_string(),
                    )
                    .expect("raw tool args"),
                    meta: None,
                }],
                meerkat_core::types::StopReason::ToolUse,
            ),
        ));
        session.push(meerkat_core::types::Message::tool_results(vec![
            meerkat_core::types::ToolResult::new("tool-2".to_string(), "done".to_string(), false),
        ]));
        store.save(&session).await.expect("persisted mixed session");

        let history = Box::pin(handle_tools_call(
            &state,
            "meerkat_history",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("mixed history should succeed");
        let payload = unwrap_payload(history);
        let messages = payload["messages"]
            .as_array()
            .expect("history messages should be an array");

        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["tool_calls"][0]["name"], "search");
        assert_eq!(messages[3]["role"], "block_assistant");
        assert_eq!(messages[3]["blocks"][0]["block_type"], "tool_use");
        assert_eq!(
            messages[3]["blocks"][0]["data"]["args"]["item"],
            "transcript"
        );
        assert_eq!(messages[4]["role"], "tool_results");
        assert_eq!(messages[4]["results"][0]["tool_use_id"], "tool-2");
    }

    #[tokio::test]
    async fn test_mcp_handlers_add_remove_reload_require_registered_adapter() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": {"name": "demo", "command": "cat", "args": [], "env": {}}
            }),
        ))
        .await
        .expect_err("mcp add should fail without adapter registration");
        assert!(err.message.contains("Live MCP unavailable"));
    }

    #[tokio::test]
    async fn test_mcp_add_rejects_legacy_server_name_mirror_before_dispatch() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_name": "demo",
                "server_config": {"name": "demo", "command": "cat", "args": [], "env": {}}
            }),
        ))
        .await
        .expect_err("legacy server_name mirror must be rejected at input boundary");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("unknown field"));
    }

    #[tokio::test]
    async fn test_mcp_add_rejects_malformed_server_config_before_dispatch() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": {}
            }),
        ))
        .await
        .expect_err("malformed server_config must be rejected at input boundary");
        assert_eq!(err.code, -32602);
        assert!(
            err.message.contains("missing field")
                || err.message.contains("data did not match any variant")
        );
    }

    #[tokio::test]
    async fn test_mcp_add_rejects_legacy_config_string_before_dispatch() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": "demo"
            }),
        ))
        .await
        .expect_err("legacy config string must be rejected at input boundary");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("invalid type"));
    }

    #[tokio::test]
    async fn test_mcp_handlers_add_remove_reload_after_adapter_registration() {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        let bindings = state
            .runtime_adapter
            .prepare_bindings(parsed.clone())
            .await
            .expect("session runtime bindings");
        seed_active_external_surface(bindings.external_tool_surface().as_ref(), "demo");
        state
            .upsert_mcp_adapter(
                &parsed,
                Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle(Arc::clone(
                        bindings.external_tool_surface(),
                    )),
                )),
            )
            .await;

        let add = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": {"name": "demo", "command": "cat", "args": [], "env": {}}
            }),
        ))
        .await
        .expect("mcp add should succeed");
        let add_payload = unwrap_payload(add);
        assert_eq!(add_payload["operation"], "add");
        assert_eq!(add_payload["status"], "staged");

        let remove = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_remove",
            &json!({
                "session_id": add_payload["session_id"],
                "server_name": "demo"
            }),
        ))
        .await
        .expect("mcp remove should succeed");
        let remove_payload = unwrap_payload(remove);
        assert_eq!(remove_payload["operation"], "remove");
        assert_eq!(remove_payload["status"], "staged");

        let reload = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_reload",
            &json!({
                "session_id": remove_payload["session_id"],
                "server_name": "demo"
            }),
        ))
        .await
        .expect("mcp reload should succeed");
        let reload_payload = unwrap_payload(reload);
        assert_eq!(reload_payload["operation"], "reload");
        assert_eq!(reload_payload["status"], "staged");
    }

    #[tokio::test]
    async fn test_event_stream_open_missing_session_errors() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let missing_id = meerkat::SessionId::new().to_string();
        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_open",
            &json!({ "session_id": missing_id }),
        ))
        .await
        .expect_err("open should fail for missing session");
        assert!(err.message.contains("Failed to open session event stream"));
    }

    #[tokio::test]
    async fn test_event_stream_read_default_timeout_and_close_behavior() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let stream_id = "stream-timeout-default".to_string();
        let pending_stream: meerkat_core::EventStream = Box::pin(stream::pending());
        state.session_event_streams.lock().await.insert(
            stream_id.clone(),
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(pending_stream),
            }),
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": stream_id }),
        ))
        .await
        .expect("read should complete with timeout");
        let read_payload = unwrap_payload(read);
        assert_eq!(read_payload["status"], "timeout");

        let closed = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_close",
            &json!({ "stream_id": "stream-timeout-default" }),
        ))
        .await
        .expect("close should succeed");
        let close_payload = unwrap_payload(closed);
        assert_eq!(close_payload["closed"], true);
    }

    #[tokio::test]
    async fn test_event_stream_read_no_timeout_opt_in_blocks() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let stream_id = "stream-no-timeout".to_string();
        let pending_stream: meerkat_core::EventStream = Box::pin(stream::pending());
        state.session_event_streams.lock().await.insert(
            stream_id.clone(),
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(pending_stream),
            }),
        );

        let result = Box::pin(timeout(
            Duration::from_millis(50),
            handle_tools_call(
                &state,
                "meerkat_event_stream_read",
                &json!({ "stream_id": stream_id, "no_timeout": true }),
            ),
        ))
        .await;
        assert!(result.is_err(), "no_timeout should allow blocking reads");
    }

    #[tokio::test]
    async fn test_event_stream_read_empty_stream_reports_closed_and_removes_entry() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let stream_id = "stream-closed".to_string();
        let empty_stream: meerkat_core::EventStream = Box::pin(stream::empty());
        state.session_event_streams.lock().await.insert(
            stream_id.clone(),
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(empty_stream),
            }),
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": stream_id }),
        ))
        .await
        .expect("read should succeed");
        let read_payload = unwrap_payload(read);
        assert_eq!(read_payload["status"], "closed");

        let close = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_close",
            &json!({ "stream_id": "stream-closed" }),
        ))
        .await
        .expect("close should succeed");
        let close_payload = unwrap_payload(close);
        assert_eq!(close_payload["closed"], false);
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_input_invalid_handling_mode_rejected_at_serde_boundary() {
        let err = serde_json::from_value::<MeerkatCommsSendInput>(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "kind": "peer_message",
            "to": "550e8400-e29b-41d4-a716-446655440000",
            "body": "hi",
            "handling_mode": "invalid"
        }))
        .expect_err("invalid handling_mode must fail deserialization");
        assert!(
            err.to_string().contains("handling_mode") || err.to_string().contains("invalid"),
            "expected serde error mentioning handling_mode, got: {err}"
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_input_unknown_intent_rejected_at_serde_boundary() {
        let err = serde_json::from_value::<MeerkatCommsSendInput>(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "kind": "peer_request",
            "to": "550e8400-e29b-41d4-a716-446655440000",
            "intent": "not.generated",
            "params": {}
        }))
        .expect_err("unknown comms intent must fail deserialization");
        assert!(
            err.to_string().contains("not.generated") || err.to_string().contains("variant"),
            "expected serde error mentioning the unknown intent, got: {err}"
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_input_malformed_result_rejected_at_serde_boundary() {
        let err = serde_json::from_value::<MeerkatCommsSendInput>(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "kind": "peer_response",
            "to": "550e8400-e29b-41d4-a716-446655440000",
            "in_reply_to": "550e8400-e29b-41d4-a716-446655440001",
            "status": "completed",
            "result": {
                "result": "ack",
                "ok": "yes"
            }
        }))
        .expect_err("malformed typed comms result must fail deserialization");
        let message = err.to_string();
        assert!(
            message.contains("ok")
                || message.contains("invalid type")
                || message.contains("did not match any variant"),
            "expected serde error mentioning malformed result, got: {message}"
        );
    }
}
