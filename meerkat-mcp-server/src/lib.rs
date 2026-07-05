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
use meerkat_contracts::{RequestLifecycle, SkillsParams};
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

/// Wrap the REAL source-identity record (projected from the skill runtime's
/// `SourceIdentityRegistry`) as wire provenance. The MCP surface must never
/// synthesize identity from display data; the registry record is the single
/// source of truth (mirrors `meerkat-rpc`'s `skills` handler).
fn skill_source_provenance(
    identity: meerkat_core::skills::SourceIdentityRecord,
) -> meerkat_contracts::SkillSourceProvenance {
    meerkat_contracts::SkillSourceProvenance { identity }
}

/// Project a registry-backed introspection entry into the wire `SkillEntry`,
/// failing closed when the typed source identity is absent rather than
/// fabricating a synthetic provenance record.
fn skill_entry_from_introspection(
    entry: &meerkat_core::skills::SkillIntrospectionEntry,
) -> Result<meerkat_contracts::SkillEntry, String> {
    let source_identity = entry.source_identity.clone().ok_or_else(|| {
        format!(
            "skill {} missing typed source identity",
            entry.descriptor.key
        )
    })?;
    Ok(meerkat_contracts::SkillEntry {
        key: entry.descriptor.key.clone(),
        name: entry.descriptor.name.clone(),
        description: entry.descriptor.description.clone(),
        scope: entry.descriptor.scope,
        source: skill_source_provenance(source_identity),
        is_active: entry.is_active,
        shadowed_by: entry
            .shadowed_by_identity
            .clone()
            .map(skill_source_provenance),
    })
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
    #[schemars(with = "Option<String>")]
    pub model: Option<meerkat_core::lifecycle::run_primitive::ModelId>,
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
    /// Enable schedule tools.
    #[serde(default)]
    pub enable_schedule: Option<bool>,
    /// Enable WorkGraph tools.
    #[serde(default)]
    pub enable_workgraph: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Enable Meerkat-owned fallback web search. Omit to keep hidden.
    #[serde(default)]
    pub enable_web_search: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
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
        || input.enable_schedule.is_some()
        || input.enable_workgraph.is_some()
        || input.enable_mob.is_some()
        || input.enable_web_search.is_some()
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

/// Shared filesystem realm-config source for the MCP server.
///
/// Mirrors REST: maps the reserved `global` realm to the home-rooted doc (or a
/// never-existent path when no HOME-rooted path exists, so `global` yields None
/// and behaves like a leaf realm) and every other realm to its per-realm config.
/// Surfaces inject this rather than re-deriving the projection.
fn mcp_realm_config_source(
    realms_root: &std::path::Path,
) -> Arc<dyn meerkat_core::RealmConfigSource> {
    let global_doc = meerkat_core::Config::global_config_path()
        .unwrap_or_else(|| realms_root.join("__no_global__").join("config.toml"));
    Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        realms_root.to_path_buf(),
        global_doc,
        meerkat_models::canonical(),
    ))
}

async fn load_config_async(
    realm_id: &meerkat_core::connection::RealmId,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
    source: &Arc<dyn meerkat_core::RealmConfigSource>,
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
    let head_config = store.get().await.unwrap_or_else(|_| Config::default());
    // Fold the head realm's parent chain (head ⊕ ancestors ⊕ `global` tail) into
    // the effective config so top-level fields inherit before env overrides win.
    // A compose failure (e.g. malformed parent edge) falls back to the head
    // realm's own config rather than nuking everything to defaults.
    let mut config = match meerkat_core::EffectiveConfigReader::new(Arc::clone(source))
        .effective_config_over_head(realm_id, head_config.clone())
        .await
    {
        Ok(effective) => effective,
        Err(err) => {
            tracing::warn!("Failed to compose realm inheritance; using head config: {err}");
            head_config
        }
    };
    if let Err(err) = config.apply_env_overrides() {
        tracing::warn!("Failed to apply env overrides: {}", err);
    }
    if let Err(err) = config.validate(meerkat_models::canonical()) {
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
    let base: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(
        paths.config_path.clone(),
        meerkat_models::canonical(),
    ));
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
        meerkat_store::RealmBackend::Memory => paths.root,
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
    workgraph_service: meerkat::WorkGraphService,
    realm_id: meerkat_core::connection::RealmId,
    backend: String,
    instance_id: Option<String>,
    expose_paths: bool,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    /// Realm config source for composing the effective config on model-resolution
    /// read paths (create default-model), so an inherited (ancestor-realm) default
    /// model / self-hosted alias resolves like the agent build path.
    realm_config_source: Arc<dyn meerkat_core::RealmConfigSource>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    #[cfg(feature = "mob")]
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    mcp_adapters: Arc<Mutex<HashMap<String, Arc<meerkat_mcp::McpRouterAdapter>>>>,
    runtime_sessions: runtime_ingress::SharedMcpRuntimeSessions,
    runtime_pre_admissions: runtime_ingress::SharedMcpRuntimePreAdmissions,
    runtime_registration_locks: runtime_ingress::SharedMcpRuntimeRegistrationLocks,
    session_event_streams: Arc<Mutex<HashMap<McpStreamId, Arc<SessionEventStreamHandle>>>>,
    #[cfg(feature = "mob")]
    mob_event_streams: Arc<Mutex<HashMap<McpStreamId, Arc<MobEventStreamInner>>>>,
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

/// Typed owner of an MCP event-stream identity.
///
/// Stream identity is its own domain fact — it is NOT a session id (the
/// previous code minted stream ids through `SessionId::new()`, conflating two
/// identity spaces). A stream id is minted as a fresh UUID at stream open and
/// parsed fail-closed from the untrusted wire `stream_id` on read/close,
/// mirroring the RPC `StreamRef` ownership shape. The stream registries are
/// keyed by this type — there is no bare-string stream key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct McpStreamId(uuid::Uuid);

impl McpStreamId {
    fn mint() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Parse an untrusted wire `stream_id`, failing closed on malformed input.
    fn parse(raw: &str) -> Result<Self, String> {
        uuid::Uuid::parse_str(raw)
            .map(Self)
            .map_err(|e| format!("invalid stream_id '{raw}': {e}"))
    }
}

impl std::fmt::Display for McpStreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
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
        let realm_config_source = mcp_realm_config_source(&realms_root);
        let config = load_config_async(
            &realm_id,
            &realms_root,
            backend_hint,
            origin_hint,
            bootstrap.realm.instance_id.as_deref(),
            &realm_config_source,
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
        let workgraph_service = meerkat::WorkGraphService::with_scope(
            persistence.workgraph_store(),
            realm_id.as_str().to_owned(),
            meerkat::WorkNamespace::default(),
        );
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
            .workgraph(true)
            .schedule(true);
        if let Some(context_root) = conventions_context_root {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let skill_runtime = factory.build_skill_runtime(&config).await?;

        let max_sessions = config.max_sessions();
        let mut builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store)
            .with_realm_inheritance(Arc::clone(&realm_config_source), realm_id.clone());
        builder.default_llm_client = default_llm_client;
        #[cfg(feature = "mob")]
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        meerkat::surface::set_default_workgraph_tools(
            &builder,
            Some(Arc::new(meerkat::WorkGraphToolSurface::new(
                workgraph_service.clone(),
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
            workgraph_service,
            realm_id,
            backend: manifest.backend.as_str().to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths,
            config_runtime,
            realm_config_source,
            skill_runtime,
            #[cfg(feature = "mob")]
            mob_state,
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_pre_admissions: Arc::new(StdMutex::new(HashMap::new())),
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
        Self::new_with_store_options(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_store_and_runtime_store(
        store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    ) -> Self {
        Self::new_with_store_options(store, runtime_store, None).await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_store_and_max_sessions(
        store: Arc<dyn SessionStore>,
        max_sessions_override: Option<usize>,
    ) -> Self {
        Self::new_with_store_options(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            max_sessions_override,
        )
        .await
    }

    #[cfg(test)]
    async fn new_with_store_options(
        store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
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
        let realm_config_source = mcp_realm_config_source(&realms_root);
        let mut config = load_config_async(
            &realm_id,
            &realms_root,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
            bootstrap.realm.instance_id.as_deref(),
            &realm_config_source,
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
            .workgraph(true)
            .schedule(true);
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let max_sessions = config.max_sessions();
        let builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store)
            .with_realm_inheritance(Arc::clone(&realm_config_source), realm_id.clone());
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(meerkat::MemoryScheduleStore::default()),
            )))),
        );
        let workgraph_service = meerkat::WorkGraphService::with_scope(
            Arc::new(meerkat::MemoryWorkGraphStore::new()),
            realm_id.as_str().to_owned(),
            meerkat::WorkNamespace::default(),
        );
        meerkat::surface::set_default_workgraph_tools(
            &builder,
            Some(Arc::new(meerkat::WorkGraphToolSurface::new(
                workgraph_service.clone(),
            ))),
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
            workgraph_service,
            realm_id,
            backend: "sqlite".to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths: false,
            config_runtime,
            realm_config_source,
            skill_runtime: None,
            #[cfg(feature = "mob")]
            mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_pre_admissions: Arc::new(StdMutex::new(HashMap::new())),
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
        "memory" => Some(meerkat_store::RealmBackend::Memory),
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
    #[schemars(with = "Option<String>")]
    pub model: Option<meerkat_core::lifecycle::run_primitive::ModelId>,
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
    /// Enable schedule tools.
    #[serde(default)]
    pub enable_schedule: Option<bool>,
    /// Enable WorkGraph tools.
    #[serde(default)]
    pub enable_workgraph: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Enable Meerkat-owned fallback web search. Omit to keep hidden.
    #[serde(default)]
    pub enable_web_search: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
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

/// Tri-state read-timeout policy for an event-stream read.
///
/// Owns the single semantic distinction that drives which timeout branch runs:
/// a finite caller timeout, an unbounded wait, or the surface default. The
/// legacy two-field wire form (`timeout_ms` + `no_timeout`) is collapsed into
/// this enum once at the deserialization boundary
/// ([`StreamReadTimeoutWire`]); consumers read [`StreamReadTimeout::duration`]
/// rather than re-deriving the policy by field precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamReadTimeout {
    /// No `timeout_ms` / `no_timeout` supplied: apply the surface default.
    #[default]
    Default,
    /// `no_timeout` requested: wait indefinitely for the next event.
    Infinite,
    /// `timeout_ms` supplied: apply this finite per-read timeout.
    Fixed { ms: u64 },
}

impl StreamReadTimeout {
    /// Resolve the policy to an optional wait duration. `None` means
    /// "wait indefinitely"; `Some` is the finite timeout for this read.
    #[must_use]
    pub fn duration(self) -> Option<std::time::Duration> {
        match self {
            Self::Infinite => None,
            Self::Fixed { ms } => Some(std::time::Duration::from_millis(ms)),
            Self::Default => Some(std::time::Duration::from_millis(
                DEFAULT_STREAM_READ_TIMEOUT_MS,
            )),
        }
    }
}

/// Legacy two-field wire form for [`StreamReadTimeout`]. Kept so the MCP tool
/// input schema and existing clients (which send `timeout_ms`/`no_timeout`)
/// continue to work unchanged; the precedence collapse happens here, once.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct StreamReadTimeoutWire {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Disable timeout and wait indefinitely for the next event.
    #[serde(default)]
    pub no_timeout: bool,
}

impl From<StreamReadTimeoutWire> for StreamReadTimeout {
    fn from(wire: StreamReadTimeoutWire) -> Self {
        // Precedence (preserved from the prior consumer logic): an explicit
        // `no_timeout` wins over any `timeout_ms`; otherwise a supplied
        // `timeout_ms` is finite; otherwise the surface default applies.
        if wire.no_timeout {
            Self::Infinite
        } else {
            match wire.timeout_ms {
                Some(ms) => Self::Fixed { ms },
                None => Self::Default,
            }
        }
    }
}

/// `#[serde(flatten)]` carrier that deserializes the legacy
/// `timeout_ms`/`no_timeout` fields into the typed [`StreamReadTimeout`] policy
/// without changing the wire form. Consumers read `.policy.duration()`.
#[derive(Debug, Default)]
pub struct StreamReadTimeoutFlat {
    pub policy: StreamReadTimeout,
}

impl<'de> Deserialize<'de> for StreamReadTimeoutFlat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StreamReadTimeoutWire::deserialize(deserializer).map(|wire| Self {
            policy: wire.into(),
        })
    }
}

impl JsonSchema for StreamReadTimeoutFlat {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        StreamReadTimeoutWire::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        StreamReadTimeoutWire::json_schema(generator)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamReadInput {
    pub stream_id: String,
    /// Read-timeout policy. Deserialized from the legacy `timeout_ms` /
    /// `no_timeout` fields and flattened so the wire form is unchanged.
    #[serde(flatten)]
    #[schemars(flatten)]
    pub timeout: StreamReadTimeoutFlat,
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
    /// Read-timeout policy. Deserialized from the legacy `timeout_ms` /
    /// `no_timeout` fields and flattened so the wire form is unchanged.
    #[serde(flatten)]
    #[schemars(flatten)]
    pub timeout: StreamReadTimeoutFlat,
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
            allowed_tools: value.allowed_tools.map(|names| {
                names
                    .into_iter()
                    .map(meerkat_core::ToolName::from)
                    .collect()
            }),
            blocked_tools: value.blocked_tools.map(|names| {
                names
                    .into_iter()
                    .map(meerkat_core::ToolName::from)
                    .collect()
            }),
            ..Default::default()
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
            wrap_tool_payload(payload)
        }
        Err(SessionError::Agent(meerkat::AgentError::CallbackPending { tool_name, args })) => {
            // Pending state is a typed terminal-control fact, not a success
            // envelope. Serialize the canonical `WireCallbackPending` contract
            // rather than hand-building a success-looking JSON object so the
            // `status` discriminant + pending-tool list stay schema-aligned
            // across surfaces. The MCP human-readable `content` rides alongside
            // the contract fields.
            let pending = meerkat_contracts::WireCallbackPending::single(
                session_id.clone(),
                None,
                false,
                true,
                tool_name,
                args,
            );
            let mut payload = serde_json::to_value(&pending)
                .map_err(|err| format!("Failed to serialize callback pending contract: {err}"))?;
            if let Value::Object(map) = &mut payload {
                map.insert(
                    "content".to_string(),
                    json!([{
                        "type": "text",
                        "text": "Agent is waiting for tool results"
                    }]),
                );
            }
            wrap_tool_payload(payload)
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

/// Typed descriptor for a Meerkat MCP tool.
///
/// The descriptor is the single owner of every fact about a base tool:
/// its wire `name`, the human-readable `description`, the `input_schema`
/// builder, and — co-located here rather than in a separate string-keyed
/// catalog — its [`RequestLifecycle`] classification. The rendered
/// `tools/list` JSON (see [`base_tools_list`]) and the lifecycle lookup (see
/// [`mcp_tool_request_lifecycle`]) are both read-only projections of this
/// single typed table, so a tool's lifecycle can never drift from where its
/// name/description/schema are declared.
pub struct BaseToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: fn() -> Value,
    pub request_lifecycle: RequestLifecycle,
}

/// Empty input schema for tools that take no arguments.
fn empty_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "required": []
    })
}

/// The typed descriptor table for the base (always-present) MCP tools.
///
/// This is the authority for both the advertised descriptor JSON and the
/// per-tool request lifecycle. Tools contributed by other crates
/// (`schedule_tools_list`, `workgraph_tools_list`, the mob/comms surfaces)
/// declare their lifecycle in [`contributed_tool_lifecycles`], keyed by the
/// surface's own advertised list.
fn base_tool_descriptors() -> Vec<BaseToolDescriptor> {
    vec![
        BaseToolDescriptor {
            name: "meerkat_run",
            description: "Run a new Meerkat agent with the given prompt. Returns the agent's response. If tools are provided and the agent requests a tool call, the response will include pending_tool_calls that must be fulfilled via meerkat_resume.",
            input_schema: || meerkat_tools::schema_for::<MeerkatRunInput>(),
            request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
        },
        BaseToolDescriptor {
            name: "meerkat_resume",
            description: "Resume an existing Meerkat session. Use this to continue a conversation or provide tool results for pending tool calls.",
            input_schema: || meerkat_tools::schema_for::<MeerkatResumeInput>(),
            request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
        },
        BaseToolDescriptor {
            name: "meerkat_help",
            description: "Ask how to use Meerkat. Runs a help session with the embedded meerkat-platform skill and returns the answer.",
            input_schema: || meerkat_tools::schema_for::<meerkat_contracts::HelpRequest>(),
            request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
        },
        BaseToolDescriptor {
            name: "meerkat_config",
            description: "Get or update Meerkat config for this MCP server instance.",
            input_schema: || meerkat_tools::schema_for::<MeerkatConfigInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_capabilities",
            description: "Get the list of capabilities available in this Meerkat runtime.",
            input_schema: empty_input_schema,
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_models_catalog",
            description: "Get the catalog of supported LLM models with provider grouping, tiers, and parameter schemas.",
            input_schema: empty_input_schema,
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_skills",
            description: "List or inspect available skills. Use action 'list' to see all skills, or 'inspect' with a typed skill_key and optional source UUID selector to see full content.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSkillsInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_read",
            description: "Read current session state.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionIdInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_sessions",
            description: "List sessions in the active realm.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionListInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_history",
            description: "Read a session's full history.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionHistoryInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_blob_get",
            description: "Fetch raw blob bytes and metadata by blob id.",
            input_schema: || meerkat_tools::schema_for::<MeerkatBlobGetInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_interrupt",
            description: "Interrupt an in-flight turn for a session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionIdInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_archive",
            description: "Archive (remove) a session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionIdInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_mcp_add",
            description: "Stage a live MCP server add operation on an active session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatMcpAddInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_mcp_remove",
            description: "Stage a live MCP server removal on an active session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatMcpRemoveInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_mcp_reload",
            description: "Stage a live MCP server reload on an active session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatMcpReloadInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_event_stream_open",
            description: "Open a session-level agent event stream.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionEventStreamOpenInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_event_stream_read",
            description: "Read the next item from an open session-level event stream.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionEventStreamReadInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_event_stream_close",
            description: "Close a previously opened session-level event stream.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionEventStreamCloseInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
    ]
}

fn base_tools_list() -> Vec<Value> {
    base_tool_descriptors()
        .into_iter()
        .map(|descriptor| {
            json!({
                "name": descriptor.name,
                "description": descriptor.description,
                "inputSchema": (descriptor.input_schema)(),
            })
        })
        .collect()
}

/// Feature-owned lifecycle declarations for contributed tool surfaces.
///
/// Each contributed surface composed into [`tools_list`] (schedule,
/// workgraph, mob host tools, mob event streams, comms) declares exactly one
/// [`RequestLifecycle`] for the tools it advertises. The name set is read
/// from the surface's own advertised list — the same projection
/// [`tools_list`] extends — so the lifecycle key set can never drift from
/// advertisement, and no name-string fall-through exists.
fn contributed_tool_lifecycles() -> Vec<(Vec<String>, RequestLifecycle)> {
    fn advertised_names(tools: Vec<Value>) -> Vec<String> {
        tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
            .collect()
    }
    #[allow(unused_mut)]
    let mut surfaces = vec![
        (
            advertised_names(meerkat::schedule_tools_list()),
            RequestLifecycle::LongRunningObservation,
        ),
        (
            advertised_names(meerkat::workgraph_tools_list()),
            RequestLifecycle::LongRunningObservation,
        ),
    ];
    #[cfg(feature = "mob")]
    surfaces.push((
        advertised_names(mob_host_tools_list()),
        RequestLifecycle::LongRunningObservation,
    ));
    #[cfg(feature = "mob")]
    surfaces.push((
        advertised_names(mob_event_stream_tools_list()),
        RequestLifecycle::LongRunningObservation,
    ));
    #[cfg(feature = "comms")]
    surfaces.push((
        advertised_names(comms_tools_list()),
        RequestLifecycle::LongRunningObservation,
    ));
    surfaces
}

/// Resolve the [`RequestLifecycle`] for a tools/call by name.
///
/// Owner-of-record for MCP tool lifecycle: base tools read the
/// classification straight off the typed [`BaseToolDescriptor`] co-located
/// with the tool's name/description/schema; contributed tools read the
/// lifecycle their owning surface declares in
/// [`contributed_tool_lifecycles`]. Unknown names resolve to `None` — the
/// caller must fail closed (reject the call as an unknown tool) instead of
/// classifying an unadvertised name under a default lifecycle.
pub fn mcp_tool_request_lifecycle(tool_name: &str) -> Option<RequestLifecycle> {
    if let Some(descriptor) = base_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == tool_name)
    {
        return Some(descriptor.request_lifecycle);
    }
    contributed_tool_lifecycles()
        .into_iter()
        .find(|(names, _)| names.iter().any(|name| name == tool_name))
        .map(|(_, lifecycle)| lifecycle)
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

impl MeerkatMcpState {
    /// Advertised tools for THIS runtime instance.
    ///
    /// The static composition ([`tools_list`]) is filtered by the runtime
    /// capability seam: `meerkat_skills` advertises only when the resolved
    /// skill runtime exists, so the advertised availability can never split
    /// from the handler that would otherwise reject the call with
    /// "skills not enabled".
    pub fn advertised_tools_list(&self) -> Vec<Value> {
        let mut tools = tools_list();
        if self.skill_runtime.is_none() {
            tools.retain(|tool| tool.get("name").and_then(Value::as_str) != Some("meerkat_skills"));
        }
        tools
    }
}

/// Returns the full static list of tools composable by this MCP server.
///
/// Runtime-conditional availability (e.g. skills) is applied by
/// [`MeerkatMcpState::advertised_tools_list`]; serving this static catalog
/// directly would advertise tools the runtime cannot dispatch.
pub fn tools_list() -> Vec<Value> {
    let mut tools = base_tools_list();
    tools.extend(meerkat::schedule_tools_list());
    tools.extend(meerkat::workgraph_tools_list());

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
        return wrap_tool_payload(payload).map_err(ToolCallError::internal);
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
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
        }
        "meerkat_capabilities" => handle_meerkat_capabilities(state)
            .await
            .map_err(ToolCallError::internal)
            .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal)),
        "meerkat_models_catalog" => handle_meerkat_models_catalog(state)
            .await
            .map_err(ToolCallError::internal)
            .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal)),
        "meerkat_skills" => {
            let input: MeerkatSkillsInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_skills(state, input)
                .await
                .map_err(ToolCallError::internal)
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
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
                .map_err(map_schedule_tool_error)
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
        }
        name if name.starts_with("workgraph_") => {
            meerkat::handle_workgraph_tools_call(&state.workgraph_service, name, arguments)
                .await
                .map_err(map_workgraph_tool_error)
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
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
                .map(skill_entry_from_introspection)
                .collect::<Result<_, _>>()?;
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
            // Project the REAL provenance from the registry-backed introspection
            // listing (matching the canonical key) rather than fabricating one
            // from display data. Fail closed if the registry does not surface a
            // typed identity for this key.
            let provenance_entries = runtime
                .list_all_with_provenance(&meerkat_core::skills::SkillFilter::default())
                .await
                .map_err(|e| format!("skill provenance lookup failed: {e}"))?;
            let source_identity = provenance_entries
                .into_iter()
                .find(|entry| entry.descriptor.key == doc.descriptor.key)
                .and_then(|entry| entry.source_identity)
                .ok_or_else(|| {
                    format!("skill {} missing typed source identity", doc.descriptor.key)
                })?;
            serde_json::to_value(meerkat_contracts::SkillInspectResponse {
                key: doc.descriptor.key.clone(),
                name: doc.descriptor.name.clone(),
                description: doc.descriptor.description.clone(),
                scope: doc.descriptor.scope,
                source: skill_source_provenance(source_identity),
                body: doc.body,
            })
            .map_err(|e| format!("serialization failed: {e}"))
        }
    }
}

async fn handle_meerkat_capabilities(state: &MeerkatMcpState) -> Result<Value, String> {
    // A ConfigError is a TRUE fault (benign absence is already Ok(default) inside the
    // store), so it must surface as an error, never as a phantom default-config payload.
    // Compose the realm chain so a capability gated on an inherited config field
    // reports the same status as the other surfaces.
    let head_config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| format!("Failed to read config: {e}"))?
        .config;
    let config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
        .effective_config_over_head(&state.realm_id, head_config)
        .await
        .map_err(|e| format!("Failed to compose realm config chain: {e}"))?;
    let response = meerkat::surface::build_capabilities_response(&config);
    serde_json::to_value(&response).map_err(|e| format!("Serialization failed: {e}"))
}

async fn handle_meerkat_models_catalog(state: &MeerkatMcpState) -> Result<Value, String> {
    // A ConfigError is a TRUE fault: an empty/default catalog must not masquerade
    // as success. Compose the realm chain so an inherited (ancestor-realm)
    // self-hosted alias / per-provider default is listed identically to the other
    // surfaces and matches what a session build resolves.
    let head_config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| format!("Failed to read config: {e}"))?
        .config;
    let config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
        .effective_config_over_head(&state.realm_id, head_config)
        .await
        .map_err(|e| format!("Failed to compose realm config chain: {e}"))?;
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
        .validate(meerkat_models::canonical())
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

fn map_schedule_tool_error(error: meerkat::ScheduleToolError) -> ToolCallError {
    ToolCallError::new(error.code, error.message, error.data)
}

fn map_workgraph_tool_error(error: meerkat::WorkGraphToolError) -> ToolCallError {
    use meerkat::WorkGraphToolErrorCode;
    let code = match error.code {
        WorkGraphToolErrorCode::InvalidArguments
        | WorkGraphToolErrorCode::Conflict
        | WorkGraphToolErrorCode::InvalidTransition => {
            meerkat_contracts::ErrorCode::InvalidParams.jsonrpc_code()
        }
        WorkGraphToolErrorCode::NotFound => -32601,
        WorkGraphToolErrorCode::CapabilityUnavailable => {
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
        }
        WorkGraphToolErrorCode::StoreError | WorkGraphToolErrorCode::InternalError => {
            meerkat_contracts::ErrorCode::InternalError.jsonrpc_code()
        }
    };
    ToolCallError::new(code, error.message, None)
}

fn apply_patch_preview(config: &Config, patch: Value) -> Result<Config, ToolCallError> {
    // Single owner of RFC-7386 patch semantics: meerkat-core.
    meerkat_core::apply_config_patch_preview(config, patch)
        .map_err(|e| ToolCallError::invalid_params(format!("{e}")))
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
    wrap_tool_payload(payload)
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
    wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "interrupted": true
    }))
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
    wrap_tool_payload(payload)
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
    let payload_value = serde_json::to_value(payload)
        .map_err(|e| format!("Failed to serialize session history: {e}"))?;
    wrap_tool_payload(payload_value)
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
    let payload_value = serde_json::to_value(payload)
        .map_err(|e| format!("Failed to serialize blob payload: {e}"))?;
    wrap_tool_payload(payload_value)
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
    if service
        .load_authoritative_session(session_id)
        .await?
        .is_some()
    {
        match archive_with_mcp_machine_authority(service, runtime_adapter, session_id).await {
            Ok(()) => {}
            Err(SessionError::NotFound { .. }) if runtime_was_registered => {}
            Err(error) => return Err(error),
        }
    } else if runtime_was_registered {
        runtime_adapter.unregister_session(session_id).await;
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
    wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "archived": true
    }))
    .map_err(ToolCallError::internal)
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
    .and_then(wrap_tool_payload)
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
    .and_then(wrap_tool_payload)
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
            // K14: the lifecycle owner's reload-all primitive stages the whole
            // roster under one router lock and returns the typed per-server
            // report; a non-clean report is an explicit fault naming both the
            // staged servers (whose reloads WILL still apply at the next
            // boundary) and the rejected ones.
            let report = adapter
                .stage_reload_all()
                .await
                .map_err(|e| format!("failed to stage reload-all: {e}"))?;
            if !report.is_clean() {
                let failed = report
                    .failed
                    .iter()
                    .map(|failure| format!("{}: {}", failure.server, failure.error))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(format!(
                    "failed to stage reload for [{failed}]; reload already staged for [{}] and will still apply at the next boundary",
                    report.staged.join(", ")
                ));
            }
        }
    }

    mcp_live_response_value(
        input.session_id,
        meerkat_contracts::McpLiveOperation::Reload,
        input.server_name,
        false,
    )
    .and_then(wrap_tool_payload)
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
    let stream_id = McpStreamId::mint();
    state.session_event_streams.lock().await.insert(
        stream_id,
        Arc::new(SessionEventStreamHandle {
            stream: Mutex::new(stream),
        }),
    );

    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "session_id": session_id.to_string()
    }))
}

async fn handle_meerkat_event_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamReadInput,
) -> Result<Value, String> {
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let stream_id_str = stream_id.to_string();
    let handle = state
        .session_event_streams
        .lock()
        .await
        .get(&stream_id)
        .cloned()
        .ok_or_else(|| format!("Stream not found: {stream_id}"))?;

    let read_timeout = input.timeout.policy.duration();
    let next_event = {
        let mut stream = handle.stream.lock().await;
        match read_timeout {
            None => stream.next().await,
            Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
                Ok(item) => item,
                Err(_) => {
                    return stream_read_payload(
                        &stream_id_str,
                        meerkat_contracts::StreamReadStatus::Timeout,
                    );
                }
            },
        }
    };

    match next_event {
        Some(envelope) => {
            let status = stream_read_event_status(&envelope)?;
            stream_read_payload(&stream_id_str, status)
        }
        None => {
            state.session_event_streams.lock().await.remove(&stream_id);
            stream_read_payload(&stream_id_str, meerkat_contracts::StreamReadStatus::Closed)
        }
    }
}

async fn handle_meerkat_event_stream_close(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamCloseInput,
) -> Result<Value, String> {
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let removed = state.session_event_streams.lock().await.remove(&stream_id);
    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "closed": removed.is_some()
    }))
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_open(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamOpenInput,
) -> Result<Value, String> {
    let mob_id = meerkat_mob::MobId::from(input.mob_id.as_str());
    let stream_id = McpStreamId::mint();

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
        .insert(stream_id, Arc::new(inner));

    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "mob_id": input.mob_id,
        "member_id": input.member_id,
    }))
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamReadInput,
) -> Result<Value, String> {
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let stream_id_str = stream_id.to_string();
    let handle = state
        .mob_event_streams
        .lock()
        .await
        .get(&stream_id)
        .cloned()
        .ok_or_else(|| format!("Mob event stream not found: {stream_id}"))?;

    let read_timeout = input.timeout.policy.duration();

    match handle.as_ref() {
        MobEventStreamInner::Member(stream_mutex) => {
            let next_event = {
                let mut stream = stream_mutex.lock().await;
                match read_timeout {
                    None => stream.next().await,
                    Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
                        Ok(item) => item,
                        Err(_) => {
                            return stream_read_payload(
                                &stream_id_str,
                                meerkat_contracts::StreamReadStatus::Timeout,
                            );
                        }
                    },
                }
            };
            match next_event {
                Some(envelope) => {
                    let status = stream_read_event_status(&envelope)?;
                    stream_read_payload(&stream_id_str, status)
                }
                None => {
                    state.mob_event_streams.lock().await.remove(&stream_id);
                    stream_read_payload(&stream_id_str, meerkat_contracts::StreamReadStatus::Closed)
                }
            }
        }
        MobEventStreamInner::MobWide(router_mutex) => {
            let next_event = {
                let mut router_handle = router_mutex.lock().await;
                match read_timeout {
                    None => router_handle.event_rx.recv().await,
                    Some(timeout) => {
                        match tokio::time::timeout(timeout, router_handle.event_rx.recv()).await {
                            Ok(item) => item,
                            Err(_) => {
                                return stream_read_payload(
                                    &stream_id_str,
                                    meerkat_contracts::StreamReadStatus::Timeout,
                                );
                            }
                        }
                    }
                }
            };
            match next_event {
                Some(attributed) => {
                    let status = stream_read_event_status(&attributed)?;
                    stream_read_payload(&stream_id_str, status)
                }
                None => {
                    state.mob_event_streams.lock().await.remove(&stream_id);
                    stream_read_payload(&stream_id_str, meerkat_contracts::StreamReadStatus::Closed)
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
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let removed = state.mob_event_streams.lock().await.remove(&stream_id);
    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "closed": removed.is_some()
    }))
}

#[cfg(feature = "comms")]
fn comms_send_tool_payload(receipt: meerkat_core::comms::SendReceipt) -> Result<Value, String> {
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
    comms_send_tool_payload(receipt).map_err(ToolCallError::internal)
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
    comms_peers_tool_payload(comms.peers().await)
}

#[cfg(feature = "comms")]
fn comms_peers_tool_payload(
    peers: Vec<meerkat_core::comms::PeerDirectoryEntry>,
) -> Result<Value, String> {
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
        meerkat_core::comms::SendError::Transport(details) if peer_name.is_some() => {
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
        meerkat_core::comms::SendError::AdmissionDropped { reason } => {
            let peer = peer_name.unwrap_or("<unknown>");
            ToolCallError::new(
                -32603,
                format!(
                    "peer_admission_dropped: peer '{peer}' rejected envelope at ingress: {}",
                    reason.as_code()
                ),
                Some(json!({
                    "code": "peer_admission_dropped",
                    "peer": peer,
                    "reason": reason,
                    "message": format!(
                        "peer '{peer}' rejected envelope at ingress: {}",
                        reason.as_code()
                    ),
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
        model: input
            .model
            .map(meerkat_core::lifecycle::run_primitive::ModelId::new),
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
        enable_schedule: Some(false),
        enable_workgraph: Some(false),
        enable_mob: Some(false),
        enable_web_search: Some(false),
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
    // A ConfigError is a TRUE fault and must not be laundered into a default config
    // that silently picks a phantom default model. Compose the realm chain so an
    // inherited (ancestor-realm) default model / self-hosted alias resolves like
    // the agent build path.
    let head_config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to read config: {e}")))?
        .config;
    let config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
        .effective_config_over_head(&state.realm_id, head_config)
        .await
        .map_err(|e| {
            ToolCallError::internal(format!("Failed to compose realm config chain: {e}"))
        })?;
    let model = input
        .model
        .as_ref()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| meerkat::resolve_create_session_default_model(&config));

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
    let create_provider = input.provider.map(ProviderInput::to_provider);
    let mut build = SessionBuildOptions {
        tool_access_policy: None,
        custom_models: std::collections::BTreeMap::new(),
        image_generation_provider: None,
        auto_compact_threshold_override: None,
        provider: create_provider,
        override_comms: Default::default(),
        self_hosted_server_id: None,
        output_schema,
        structured_output_retries: input.structured_output_retries,
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
        override_schedule: ToolCategoryOverride::from_override(input.enable_schedule),
        override_workgraph: ToolCategoryOverride::from_override(input.enable_workgraph),
        override_mob: ToolCategoryOverride::Inherit,
        override_image_generation: ToolCategoryOverride::Inherit,
        override_web_search: ToolCategoryOverride::from_override(input.enable_web_search),
        schedule_tools: None,
        workgraph_tools: None,
        mob_tool_authority_context: None,
        preload_skills,
        realm_id: Some(state.realm_id.clone()),
        instance_id: state.instance_id.clone(),
        backend: meerkat_core::RecoveryBackendKind::parse(&state.backend),
        config_generation: current_generation,
        auth_binding: input
            .auth_binding
            .clone()
            .map(meerkat_core::AuthBindingRef::from),
        mob_member_binding: None,
        keep_alive,
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
        app_context: input.app_context.clone(),
        additional_instructions: input.additional_instructions.clone(),
        initial_metadata_entries: std::collections::BTreeMap::new(),
        initial_tool_filter: None,
        shell_env: input.shell_env.clone(),
        resume_override_mask: ResumeOverrideMask {
            provider: input.provider.is_some() || input.model.is_some(),
            max_tokens: input.max_tokens.is_some(),
            structured_output_retries: input.structured_output_retries.is_some(),
            provider_params: input.provider_params.is_some(),
            preload_skills: input.preload_skills.is_some(),
            keep_alive: keep_alive_override.is_some(),
            comms_name: input.comms_name.is_some(),
            peer_meta: input.peer_meta.is_some(),
            override_schedule: input.enable_schedule.is_some(),
            override_workgraph: input.enable_workgraph.is_some(),
            override_web_search: input.enable_web_search.is_some(),
            ..Default::default()
        },
        blob_store_override: None,
        mob_tools: None,
    };
    build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::from_override(
        input.enable_mob,
    ));
    // Dogma K10: initial-turn skill references ride the ONE typed carrier
    // (`build.initial_turn_metadata`), not a request-level duplicate.
    if let Some(refs) = skill_references.clone() {
        build
            .initial_turn_metadata
            .get_or_insert_with(Default::default)
            .skill_references
            .get_or_insert_with(Vec::new)
            .extend(refs);
    }

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
        injected_context: Vec::new(),
        model,
        prompt: input.prompt.into(),
        system_prompt: match input.system_prompt {
            Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
            None => meerkat::SystemPromptOverride::Inherit,
        },
        max_tokens: input.max_tokens,
        event_tx: event_tx.clone(),

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

    let session_exists = match state.service.load_authoritative_session(&session_id).await {
        Ok(Some(session)) => {
            match state
                .service
                .session_archived_by_authority(&session_id, &session)
                .await
            {
                Ok(true) => {
                    state.cleanup_archived_session_runtime(&session_id).await;
                    false
                }
                Ok(false) => true,
                Err(error) => {
                    return Err(ToolCallError::internal(format!(
                        "failed to load session archive state for {session_id}: {error}"
                    )));
                }
            }
        }
        Ok(None) => false,
        Err(error) => {
            return Err(ToolCallError::internal(format!(
                "failed to load authoritative session {session_id}: {error}"
            )));
        }
    };
    if session_exists {
        state.upsert_mcp_adapter(&session_id, mcp_adapter).await;
        ingress
            .configure_session(&session_id, callback_tools, false)
            .await
            .map_err(|error| {
                ToolCallError::internal(format!(
                    "failed to attach MCP runtime executor for {session_id}: {error}"
                ))
            })?;
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
            .await
            .map_err(|error| {
                ToolCallError::internal(format!(
                    "failed to update peer ingress context for {session_id}: {error}"
                ))
            })?;
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
    // A ConfigError is a TRUE fault and must not be laundered into a default config.
    let config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to read config: {e}")))?
        .config;

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

    // Resolve settings from stored metadata, failing closed when the durable
    // session identity is absent.
    let stored_metadata = session.session_metadata().ok_or_else(|| {
        ToolCallError::internal(format!(
            "persisted session {session_id} is missing session metadata"
        ))
    })?;

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
        None => stored_metadata.keep_alive,
    };
    let comms_name = input
        .comms_name
        .clone()
        .or_else(|| stored_metadata.comms_name.clone());
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
        .as_ref()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| stored_metadata.model.clone());
    let max_tokens = input.max_tokens.or(Some(stored_metadata.max_tokens));
    let provider = input.provider.map(ProviderInput::to_provider);
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
    let live_session_existed =
        state
            .service
            .has_live_session(&session_id)
            .await
            .map_err(|err| {
                ToolCallError::internal(format!(
                    "failed to read live-session authority for {session_id}: {err}"
                ))
            })?;
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
        stored_metadata.provider,
        stored_metadata.self_hosted_server_id.clone(),
        input.model.as_ref().map(|m| m.as_str()),
        provider,
    )
    .map_err(|error| ToolCallError::invalid_params(error.to_string()))?;
    let build_session_options = |runtime_bindings, external_tools| {
        let mut build = SessionBuildOptions {
            tool_access_policy: None,
            custom_models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            auto_compact_threshold_override: None,
            provider: llm_binding.provider,
            override_comms: Default::default(),
            self_hosted_server_id: llm_binding.self_hosted_server_id.clone(),
            output_schema: output_schema.clone(),
            structured_output_retries: input.structured_output_retries,
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
            override_schedule: ToolCategoryOverride::from_override(input.enable_schedule),
            override_workgraph: ToolCategoryOverride::from_override(input.enable_workgraph),
            override_mob: ToolCategoryOverride::Inherit,
            override_image_generation: ToolCategoryOverride::Inherit,
            override_web_search: ToolCategoryOverride::from_override(input.enable_web_search),
            schedule_tools: None,
            workgraph_tools: None,
            mob_tool_authority_context: None,
            preload_skills: preload_skills.clone(),
            peer_meta: input.peer_meta.clone(),
            realm_id: stored_metadata
                .realm_id
                .clone()
                .or_else(|| Some(state.realm_id.clone())),
            instance_id: stored_metadata
                .instance_id
                .clone()
                .or_else(|| state.instance_id.clone()),
            backend: stored_metadata
                .backend
                .as_deref()
                .and_then(meerkat_core::RecoveryBackendKind::parse)
                .or_else(|| meerkat_core::RecoveryBackendKind::parse(&state.backend)),
            config_generation: current_generation,
            auth_binding: None,
            mob_member_binding: stored_metadata.mob_member_binding.clone(),
            keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            additional_instructions: input.additional_instructions.clone(),
            initial_metadata_entries: std::collections::BTreeMap::new(),
            initial_tool_filter: None,
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
                override_schedule: input.enable_schedule.is_some(),
                override_workgraph: input.enable_workgraph.is_some(),
                override_web_search: input.enable_web_search.is_some(),
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
        let mut build = build_session_options(resume_bindings, external_tools);
        // Dogma K10: initial-turn skill references ride the ONE typed carrier
        // (`build.initial_turn_metadata`), not a request-level duplicate.
        if let Some(refs) = skill_references.clone() {
            build
                .initial_turn_metadata
                .get_or_insert_with(Default::default)
                .skill_references
                .get_or_insert_with(Vec::new)
                .extend(refs);
        }
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
            injected_context: Vec::new(),
            model,
            prompt: prompt.clone().into(),
            system_prompt: match input.system_prompt.clone() {
                Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
                None => meerkat::SystemPromptOverride::Inherit,
            },
            max_tokens,
            event_tx: event_tx.clone(),
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
            // RuntimeTurnMetadata carries the keep_alive mutation; the
            // session service applies it only after generated turn-admission
            // feedback. The surface may refresh machine-owned peer ingress.
            state
                .runtime_adapter
                .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                .await
                .map_err(|error| {
                    ToolCallError::internal(format!(
                        "failed to update peer ingress context for {session_id}: {error}"
                    ))
                })?;
        }
        // Live MCP resumes still use the runtime/machine service-turn receipt
        // path; the persistent service only owns the live mutation and post-
        // receipt projection, not lifecycle truth.
        // Dogma K13: forward the full typed keep-alive tri-state to the
        // machine. `Some(true)` -> Enable(policy), `Some(false)` -> Disable
        // (explicit operator intent, never dropped into preserve), `None` ->
        // preserve the persisted session intent.
        let turn_seed_metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            skill_references: skill_references.clone(),
            flow_tool_overlay: input.flow_tool_overlay.clone().map(Into::into),
            keep_alive: keep_alive_override.map(|keep_alive| {
                if keep_alive {
                    meerkat_core::lifecycle::run_primitive::KeepAliveDirective::Enable(
                        meerkat_core::lifecycle::run_primitive::KeepAlivePolicy {
                            ttl: std::time::Duration::from_secs(30),
                            policy: meerkat_core::lifecycle::run_primitive::KeepAliveMode::Pinned,
                        },
                    )
                } else {
                    meerkat_core::lifecycle::run_primitive::KeepAliveDirective::Disable
                }
            }),
            ..Default::default()
        };
        let turn_req = StartTurnRequest {
            injected_context: Vec::new(),
            prompt: prompt.clone().into(),
            system_prompt: None,
            event_tx: event_tx.clone(),
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                meerkat_core::types::HandlingMode::Queue,
                input.flow_tool_overlay.clone().map(Into::into),
                Vec::new(),
                Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(Some(
                    turn_seed_metadata,
                ))),
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
                let mut build = build_session_options(resume_bindings, external_tools);
                // Dogma K10: initial-turn skill references ride the ONE typed
                // carrier (`build.initial_turn_metadata`), not a request-level
                // duplicate.
                if let Some(refs) = skill_references.clone() {
                    build
                        .initial_turn_metadata
                        .get_or_insert_with(Default::default)
                        .skill_references
                        .get_or_insert_with(Vec::new)
                        .extend(refs);
                }
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
                    injected_context: Vec::new(),
                    model,
                    prompt: prompt.into(),
                    system_prompt: match input.system_prompt.clone() {
                        Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
                        None => meerkat::SystemPromptOverride::Inherit,
                    },
                    max_tokens,
                    event_tx: event_tx.clone(),

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
                return Err(ToolCallError::internal(format!(
                    "failed to load machine archive state during MCP session existence check for {session_id}: {err}"
                )));
            }
        },
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to load authoritative session after MCP resume attempt"
            );
            return Err(ToolCallError::internal(format!(
                "failed to load authoritative session after MCP resume attempt for {session_id}: {err}"
            )));
        }
    };
    let live_session_exists = if session_exists {
        state
            .service
            .has_live_session(&session_id)
            .await
            .map_err(|err| {
                ToolCallError::internal(format!(
                    "failed to read live-session authority for {session_id}: {err}"
                ))
            })?
    } else {
        false
    };
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
            ingress.ensure_session(&session_id).await.map_err(|error| {
                ToolCallError::internal(format!(
                    "failed to attach MCP runtime executor for {session_id}: {error}"
                ))
            })?;
        } else {
            ingress
                .configure_session(&session_id, callback_tools, false)
                .await
                .map_err(|error| {
                    ToolCallError::internal(format!(
                        "failed to attach MCP runtime executor for {session_id}: {error}"
                    ))
                })?;
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
            .await
            .map_err(|error| {
                ToolCallError::internal(format!(
                    "failed to update peer ingress context for {session_id}: {error}"
                ))
            })?;
    }

    format_agent_result_tool(result, &session_id)
}

/// Wrap a structured payload in the MCP text-content envelope.
///
/// Serialization is a TRUE fault: a payload that fails to serialize must surface
/// as an error, never as an empty/`null` content envelope masquerading as success.
fn wrap_tool_payload(payload: Value) -> Result<Value, String> {
    let text = serde_json::to_string(&payload)
        .map_err(|err| format!("Failed to serialize tool payload: {err}"))?;
    Ok(json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    }))
}

/// Build a stream-read tool payload from the typed
/// [`meerkat_contracts::StreamReadStatus`] contract.
///
/// A stream read resolves into exactly one terminal shape — a delivered event,
/// an expired timeout, or a closed stream — and that shape is the single typed
/// authority for read status. The transport-level `stream_id` is carried by the
/// enclosing response (one fact, one owner), so surfaces never hand-roll
/// `status: "event" | "timeout" | "closed"` string conventions.
fn stream_read_payload(
    stream_id: &str,
    status: meerkat_contracts::StreamReadStatus,
) -> Result<Value, String> {
    let mut payload = serde_json::to_value(&status)
        .map_err(|err| format!("Failed to serialize stream read status: {err}"))?;
    match &mut payload {
        Value::Object(map) => {
            map.insert(
                "stream_id".to_string(),
                Value::String(stream_id.to_string()),
            );
        }
        _ => {
            return Err("stream read status did not serialize to a JSON object".to_string());
        }
    }
    wrap_tool_payload(payload)
}

/// Build a [`meerkat_contracts::StreamReadStatus::Event`] from a serializable
/// event envelope, riding the body as opaque `Box<RawValue>` (never matched at
/// this layer).
fn stream_read_event_status(
    envelope: &impl serde::Serialize,
) -> Result<meerkat_contracts::StreamReadStatus, String> {
    let body = serde_json::to_string(envelope)
        .map_err(|err| format!("Failed to serialize stream event: {err}"))?;
    let event = serde_json::value::RawValue::from_string(body)
        .map_err(|err| format!("Failed to encode stream event: {err}"))?;
    Ok(meerkat_contracts::StreamReadStatus::Event { event })
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
        // K1: callback ingress goes through the typed tool-argument contract.
        // Malformed / non-object args fail closed with a typed
        // `InvalidArguments` error — never a `Value::String` wrap that would
        // silently reach the callback consumer.
        let args = meerkat_core::ToolCallArguments::from_raw_json(call.args)
            .map_err(|err| ToolError::invalid_arguments(call.name, err.to_string()))?;
        // Check if this is a callback tool
        if self.callback_tools.contains(call.name) {
            // Return a special error that signals the agent loop should pause
            Err(ToolError::callback_pending(call.name, args.into_value()))
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
    use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::time::{Duration, timeout};

    struct RuntimeTerminationFixtureExecutor;

    #[async_trait]
    impl meerkat_core::lifecycle::CoreExecutor for RuntimeTerminationFixtureExecutor {
        async fn apply(
            &mut self,
            run_id: meerkat_core::RunId,
            primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
        ) -> Result<CoreApplyOutput, meerkat_core::lifecycle::CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                    run_id,
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            Ok(())
        }
    }

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

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[test]
    fn test_wrap_tool_payload_returns_typed_result_envelope() {
        // wrap_tool_payload must surface serialization as a typed Result, never a
        // silent empty-text/null content envelope. The happy path yields the
        // text-content envelope carrying the serialized payload.
        let wrapped = wrap_tool_payload(json!({ "key": "value" }))
            .expect("valid payload serializes into the content envelope");
        let text = wrapped["content"][0]["text"]
            .as_str()
            .expect("content envelope carries serialized text");
        assert_eq!(serde_json::from_str::<Value>(text).unwrap()["key"], "value");
        // The envelope is never the fail-open empty-text shape.
        assert_ne!(text, "");
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
        let mut session = Session::new();
        let session_id = session.id().to_string();
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
                comms_name: None,
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata should serialize");
        store.save(&session).await.expect("persisted session");
        (state, session_id)
    }

    #[cfg(feature = "mob")]
    async fn insert_mcp_archive_partial_destroy_mob(
        state: &MeerkatMcpState,
        owner_session_id: &str,
    ) -> (
        meerkat_mob::MobId,
        Arc<meerkat_mob::store::InMemoryMobEventStore>,
    ) {
        let mob_id = meerkat_mob::MobId::from("mcp-session-archive-partial-destroy");
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );
        let owner_session_id =
            meerkat::SessionId::parse(owner_session_id).expect("valid owner bridge session id");
        let events = Arc::new(meerkat_mob::store::InMemoryMobEventStore::new());
        events.fail_clear_until_allowed();
        let storage = meerkat_mob::MobStorage::with_events(events.clone());
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_owner_bridge_session_create_authority(owner_session_id, true, false)
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
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
            })),
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
        })
        .expect("comms send payload serializes");
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
        let wrapped = comms_peers_tool_payload(vec![sample_peer_directory_entry()])
            .expect("comms peers payload serializes");
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
    fn test_workgraph_tool_error_mapping_preserves_generated_classes() {
        use meerkat::WorkGraphToolErrorCode;
        let cases = [
            (WorkGraphToolErrorCode::InvalidArguments, -32602),
            (WorkGraphToolErrorCode::NotFound, -32601),
            (WorkGraphToolErrorCode::Conflict, -32602),
            (WorkGraphToolErrorCode::InvalidTransition, -32602),
            (
                WorkGraphToolErrorCode::CapabilityUnavailable,
                meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            ),
            (
                WorkGraphToolErrorCode::StoreError,
                meerkat_contracts::ErrorCode::InternalError.jsonrpc_code(),
            ),
            (
                WorkGraphToolErrorCode::InternalError,
                meerkat_contracts::ErrorCode::InternalError.jsonrpc_code(),
            ),
        ];

        for (tool_code, rpc_code) in cases {
            let err = map_workgraph_tool_error(meerkat::WorkGraphToolError {
                code: tool_code,
                message: format!("workgraph {tool_code:?}"),
            });
            assert_eq!(err.code, rpc_code, "tool code {tool_code:?}");
        }
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
        let workgraph_tool_count = meerkat::workgraph_tools_list().len();
        #[cfg(all(feature = "comms", feature = "mob"))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + workgraph_tool_count
                + mob_host_tools_list().len()
                + mob_event_stream_tools_list().len()
                + comms_tools_list().len()
        );
        #[cfg(all(not(feature = "comms"), feature = "mob"))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + workgraph_tool_count
                + mob_host_tools_list().len()
                + mob_event_stream_tools_list().len()
        );
        #[cfg(all(feature = "comms", not(feature = "mob")))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + workgraph_tool_count
                + comms_tools_list().len()
        );
        #[cfg(all(not(feature = "comms"), not(feature = "mob")))]
        assert_eq!(
            tools.len(),
            base_tools_list().len() + schedule_tool_count + workgraph_tool_count
        );

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
        assert_eq!(find_tool("workgraph_create")["name"], "workgraph_create");
        assert_eq!(find_tool("workgraph_ready")["name"], "workgraph_ready");
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

    #[test]
    fn base_tool_descriptor_owns_request_lifecycle() {
        // The typed descriptor table is the single owner of base MCP tool
        // lifecycle: the resolver reads the classification straight off the
        // descriptor co-located with name/description/schema.
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_help"),
            Some(RequestLifecycle::LongRunningPublishOnSuccess)
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_run"),
            Some(RequestLifecycle::LongRunningPublishOnSuccess)
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_resume"),
            Some(RequestLifecycle::LongRunningPublishOnSuccess)
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_sessions"),
            Some(RequestLifecycle::LongRunningObservation)
        );
        // A tool contributed by another surface resolves through that
        // surface's feature-owned declaration, not a default fall-through.
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_schedule_create"),
            Some(RequestLifecycle::LongRunningObservation)
        );
        // An unadvertised name has no lifecycle: the resolver fails closed
        // with `None` rather than classifying it under a default.
        assert_eq!(mcp_tool_request_lifecycle("definitely_not_a_tool"), None);
    }

    #[test]
    fn every_advertised_tool_has_an_owned_lifecycle() {
        // Completeness gate: every tool `tools_list` advertises — base AND
        // contributed (schedule/workgraph/mob/comms) — must resolve to a
        // feature-owned lifecycle. A `None` here means a contributed surface
        // was composed into the advertisement without declaring its
        // lifecycle.
        for tool in tools_list() {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .expect("advertised tool carries a name");
            assert!(
                mcp_tool_request_lifecycle(name).is_some(),
                "advertised tool '{name}' has no feature-owned request lifecycle"
            );
        }
    }

    #[test]
    fn base_tools_list_projects_every_descriptor() {
        // The advertised JSON is a faithful projection of the typed descriptor
        // table — same count, same names, and each carries its schema.
        let descriptors = base_tool_descriptors();
        let tools = base_tools_list();
        assert_eq!(tools.len(), descriptors.len());
        for (descriptor, tool) in descriptors.iter().zip(tools.iter()) {
            assert_eq!(tool["name"], descriptor.name);
            assert_eq!(tool["description"], descriptor.description);
            assert!(tool["inputSchema"].is_object());
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
        assert_eq!(
            input.model,
            Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                "claude-sonnet-4"
            ))
        );
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
                collection_fault: None,
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

    /// Gate for dogma row #133: MCP skills provenance must PROJECT the real
    /// `SourceIdentityRecord` from the runtime's identity registry, never
    /// fabricate a synthetic `mcp:{display}` / Embedded record from display
    /// data. The wire entry's transport and fingerprint must equal the registry
    /// record exactly.
    #[test]
    fn test_skill_entry_projects_real_registry_provenance_not_synthetic() {
        use meerkat_core::skills::{
            SkillDescriptor, SkillKey, SkillName, SkillScope, SourceIdentityRecord,
            SourceIdentityStatus, SourceTransportKind, SourceUuid,
        };

        let source_uuid = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").expect("uuid");
        // The authoritative record the registry would surface: a Filesystem
        // source with a real content fingerprint — NOT the synthetic
        // `mcp:{display_name}` / Embedded shape.
        let registry_identity = SourceIdentityRecord {
            source_uuid: source_uuid.clone(),
            display_name: "company-skills".to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: "sha256:abc123".to_string(),
            status: SourceIdentityStatus::Active,
        };

        let key = SkillKey::new(
            source_uuid,
            SkillName::parse("email-extractor").expect("name"),
        );
        let mut descriptor = SkillDescriptor::new(key, "Email Extractor", "Extracts email");
        descriptor.scope = SkillScope::Project;
        descriptor.source_name = "company-skills".to_string();

        let entry = meerkat_core::skills::SkillIntrospectionEntry {
            descriptor,
            source_identity: Some(registry_identity.clone()),
            shadowed_by: None,
            shadowed_by_identity: None,
            shadowed_by_source_uuid: None,
            is_active: true,
        };

        let wire = skill_entry_from_introspection(&entry).expect("real provenance projects");

        // Provenance must equal the registry record verbatim.
        assert_eq!(wire.source.identity, registry_identity);
        assert_eq!(
            wire.source.identity.transport_kind,
            SourceTransportKind::Filesystem
        );
        assert_eq!(wire.source.identity.fingerprint, "sha256:abc123");
        // The synthetic forms must be gone.
        assert_ne!(
            wire.source.identity.transport_kind,
            SourceTransportKind::Embedded
        );
        assert!(
            !wire.source.identity.fingerprint.starts_with("mcp:"),
            "fingerprint must not be the synthetic mcp: form, got {}",
            wire.source.identity.fingerprint
        );

        // Fail closed: an entry without a typed identity must error rather than
        // fabricate one.
        let mut orphan = entry;
        orphan.source_identity = None;
        let err = skill_entry_from_introspection(&orphan)
            .expect_err("missing typed identity must fail closed");
        assert!(
            err.contains("missing typed source identity"),
            "unexpected error: {err}"
        );
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
    #[test]
    fn test_normalize_mcp_comms_send_error_transport_maps_to_peer_unreachable() {
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::Transport(
                "Transport error: connection refused".into(),
            ),
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
            Some("transport_error")
        );
        assert_eq!(
            data.get("details").and_then(Value::as_str),
            Some("Transport error: connection refused")
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_mcp_comms_send_error_preserves_admission_drop_reason() {
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::AdmissionDropped {
                reason: meerkat_core::comms::AdmissionDropReason::ClassificationRejected,
            },
        );
        assert_eq!(err.code, -32603);
        assert!(err.message.starts_with("peer_admission_dropped:"));
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("peer_admission_dropped")
        );
        assert_eq!(data.get("peer").and_then(Value::as_str), Some("peer-a"));
        assert_eq!(
            data.get("reason").and_then(Value::as_str),
            Some("classification_rejected")
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
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "claude-opus-4-8",
                )),
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "claude-opus-4-8",
                )),
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
            .await
            .expect("MCP runtime executor should attach");
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
            .ensure_session_with_executor(
                parsed.clone(),
                Box::new(RuntimeTerminationFixtureExecutor),
            )
            .await
            .expect("attach runtime executor");

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

    /// Seed a session into runtime authority the way a prior host lifetime
    /// would have: a committed runtime-store snapshot (store rows alone are
    /// projection, not authority).
    async fn seed_runtime_authority_session(
        runtime_store: &Arc<dyn meerkat_runtime::RuntimeStore>,
        session: &Session,
    ) {
        use meerkat_runtime::RuntimeStore as _;
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::identifiers::LogicalRuntimeId::for_session(session.id()),
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(session)
                        .expect("session snapshot should serialize"),
                },
            )
            .await
            .expect("session snapshot should commit to runtime authority");
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_capacity_failure_unregisters_prepared_runtime() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Some(1),
        )
        .await;
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
                comms_name: None,
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata should serialize");
        store.save(&session).await.expect("persisted session");

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        seed_runtime_authority_session(&runtime_store, &blocker_session).await;
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Some(1),
        )
        .await;
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
                injected_context: Vec::new(),
                model: "claude-opus-4-8".to_string(),
                prompt: "Initial live turn".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(4096),
                event_tx: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
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
        seed_runtime_authority_session(&runtime_store, &blocker_session).await;
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Some(1),
        )
        .await;
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
                injected_context: Vec::new(),
                model: "claude-opus-4-8".to_string(),
                prompt: "Initial live turn".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(4096),
                event_tx: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
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
            !state
                .runtime_adapter
                .session_has_comms(&session_id)
                .await
                .expect("session_has_comms should resolve"),
            "test starts before peer ingress has been configured"
        );

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        seed_runtime_authority_session(&runtime_store, &blocker_session).await;
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
            !state
                .runtime_adapter
                .session_has_comms(&session_id)
                .await
                .expect("session_has_comms should resolve"),
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
                model: "claude-opus-4-8".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: Some("stale-mcp-agent".to_string()),
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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
                model: "claude-opus-4-8".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: Some("existing-mcp-agent".to_string()),
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata should serialize");
        store.save(&session).await.expect("persisted session");
        state
            .runtime_ingress_context()
            .ensure_session(&session_id)
            .await
            .expect("MCP runtime executor should attach");
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
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
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

        input.enable_schedule = Some(true);
        assert!(mcp_resume_requires_rebuild(&input));
        input.enable_schedule = None;

        input.enable_workgraph = Some(true);
        assert!(mcp_resume_requires_rebuild(&input));
        input.enable_workgraph = None;

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

    #[tokio::test]
    async fn test_handle_tools_call_dispatches_workgraph_tools() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let created = Box::pin(handle_tools_call(
            &state,
            "workgraph_create",
            &json!({
                "title": "mcp visible work",
                "labels": ["mcp-workgraph"]
            }),
        ))
        .await
        .expect("workgraph create should dispatch");
        let text = created["content"][0]["text"]
            .as_str()
            .expect("tool payload text");
        let payload: Value = serde_json::from_str(text).expect("json payload");
        assert_eq!(payload["item"]["title"], "mcp visible work");

        let ready = Box::pin(handle_tools_call(
            &state,
            "workgraph_ready",
            &json!({ "labels": ["mcp-workgraph"] }),
        ))
        .await
        .expect("workgraph ready should dispatch");
        let text = ready["content"][0]["text"]
            .as_str()
            .expect("tool payload text");
        let payload: Value = serde_json::from_str(text).expect("json payload");
        assert_eq!(payload["items"].as_array().map(Vec::len), Some(1));
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
        let state =
            MeerkatMcpState::new_with_store_and_runtime_store(store, Arc::clone(&runtime_store))
                .await;
        let created = state
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: "seed".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(32),
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
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
            .await
            .expect("register session");
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
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            None,
        )
        .await;
        let mut session = meerkat::Session::new();
        let session_id = session.id().to_string();

        session.push(meerkat_core::types::Message::System(
            meerkat_core::types::SystemMessage::new("system rules"),
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Hello".to_string()),
        ));
        session.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Hi there".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Follow up".to_string()),
        ));
        session.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Second answer".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        store
            .save(&session)
            .await
            .expect("persisted history session");
        seed_runtime_authority_session(&runtime_store, &session).await;

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

        Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("archive through runtime authority should succeed");

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
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            None,
        )
        .await;
        let mut session = meerkat::Session::new();
        let session_id = session.id().to_string();

        session.push(meerkat_core::types::Message::System(
            meerkat_core::types::SystemMessage::new("system rules"),
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
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
        seed_runtime_authority_session(&runtime_store, &session).await;

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

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "block_assistant");
        assert_eq!(messages[2]["blocks"][0]["block_type"], "tool_use");
        assert_eq!(
            messages[2]["blocks"][0]["data"]["args"]["item"],
            "transcript"
        );
        assert_eq!(messages[3]["role"], "tool_results");
        assert_eq!(messages[3]["results"][0]["tool_use_id"], "tool-2");
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
        let stream_id = McpStreamId::mint();
        let pending_stream: meerkat_core::EventStream = Box::pin(stream::pending());
        state.session_event_streams.lock().await.insert(
            stream_id,
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(pending_stream),
            }),
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": stream_id.to_string() }),
        ))
        .await
        .expect("read should complete with timeout");
        let read_payload = unwrap_payload(read);
        assert_eq!(read_payload["status"], "timeout");

        let closed = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_close",
            &json!({ "stream_id": stream_id.to_string() }),
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
        let stream_id = McpStreamId::mint();
        let pending_stream: meerkat_core::EventStream = Box::pin(stream::pending());
        state.session_event_streams.lock().await.insert(
            stream_id,
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(pending_stream),
            }),
        );

        let result = Box::pin(timeout(
            Duration::from_millis(50),
            handle_tools_call(
                &state,
                "meerkat_event_stream_read",
                &json!({ "stream_id": stream_id.to_string(), "no_timeout": true }),
            ),
        ))
        .await;
        assert!(result.is_err(), "no_timeout should allow blocking reads");
    }

    #[tokio::test]
    async fn test_event_stream_read_empty_stream_reports_closed_and_removes_entry() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let stream_id = McpStreamId::mint();
        let empty_stream: meerkat_core::EventStream = Box::pin(stream::empty());
        state.session_event_streams.lock().await.insert(
            stream_id,
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(empty_stream),
            }),
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": stream_id.to_string() }),
        ))
        .await
        .expect("read should succeed");
        let read_payload = unwrap_payload(read);
        assert_eq!(read_payload["status"], "closed");

        let close = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_close",
            &json!({ "stream_id": stream_id.to_string() }),
        ))
        .await
        .expect("close should succeed");
        let close_payload = unwrap_payload(close);
        assert_eq!(close_payload["closed"], false);
    }

    #[tokio::test]
    async fn test_event_stream_read_rejects_malformed_stream_id() {
        // Stream identity is a typed fact parsed fail-closed at ingress
        // (`McpStreamId::parse`): a malformed wire stream_id is rejected
        // outright, never treated as a probe into the string-keyed registry.
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": "not-a-stream-id" }),
        ))
        .await
        .expect_err("malformed stream_id must fail closed");
        assert!(
            err.message.contains("invalid stream_id"),
            "expected fail-closed parse error, got: {}",
            err.message
        );
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
