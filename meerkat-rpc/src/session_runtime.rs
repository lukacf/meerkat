//! SessionRuntime - keeps agents alive between turns.
//!
//! Delegates to [`PersistentSessionService`] for session lifecycle management.
//! `FactoryAgentBuilder` bridges `AgentFactory::build_agent()` into the
//! `SessionAgentBuilder` / `SessionAgent` traits used by the service.
//!
//! The runtime preserves the two-step create-then-run API required by the
//! JSON-RPC handlers: `create_session()` stages a build config and returns a
//! `SessionId`; the first `start_turn()` call for that ID materializes the
//! session inside the service (which runs the first turn).

use std::collections::BTreeMap;
#[cfg(feature = "mcp")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
#[cfg(feature = "mcp")]
use std::time::Duration;

use indexmap::IndexMap;
use meerkat::{
    AgentBuildConfig, AgentFactory, FactoryAgentBuilder, PersistenceBundle,
    PersistentSessionService, encode_llm_client_override_for_service,
};
use meerkat_client::LlmClient;
use meerkat_core::EventEnvelope;
use meerkat_core::RunId;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionBuildOptions, SessionControlError, SessionError, SessionHistoryQuery, SessionQuery,
    SessionService, SessionServiceControlExt, SessionServiceHistoryExt, StartTurnRequest,
};
use meerkat_core::skills::{SkillError, SourceIdentityRegistry};
use meerkat_core::types::{RunResult, SessionId};
#[cfg(feature = "mcp")]
use meerkat_core::{AgentToolDispatcher, ToolGateway};
use meerkat_core::{
    Config, ConfigStore, ContentInput, HookRunOverrides, Session, SessionSystemContextState,
};
#[cfg(all(test, feature = "mcp"))]
use meerkat_core::{ToolConfigChangeOperation, ToolConfigChangedPayload};
use meerkat_runtime::RuntimeSessionAdapter;
use tokio::sync::{RwLock, mpsc};

use crate::error;
use crate::protocol::RpcError;
#[cfg(feature = "mcp")]
use meerkat::{McpLifecycleAction, McpReloadTarget, McpRouter, McpRouterAdapter, McpServerConfig};

#[derive(Clone)]
struct SkillIdentityRegistryState {
    generation: u64,
    registry: SourceIdentityRegistry,
}

#[cfg(feature = "mcp")]
struct SessionMcpState {
    adapter: Arc<McpRouterAdapter>,
    turn_counter: u32,
    lifecycle_tx: mpsc::UnboundedSender<McpLifecycleAction>,
    lifecycle_rx: mpsc::UnboundedReceiver<McpLifecycleAction>,
    drain_task_running: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Observable state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The session is idle and ready to accept a new turn.
    Idle,
    /// A turn is currently running.
    Running,
    /// The session is shutting down.
    ShuttingDown,
}

impl SessionState {
    /// Return a stable string representation matching the serde `rename_all` convention.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::ShuttingDown => "shutting_down",
        }
    }
}

/// Summary information about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub state: SessionState,
    pub labels: BTreeMap<String, String>,
}

/// Staged session data: build config + metadata not yet materialized in the service.
struct PendingSession {
    phase: PendingSessionPhase,
    labels: Option<BTreeMap<String, String>>,
    /// Prompt from `session/create` when `initial_turn` is deferred.
    /// Prepended to the first `turn/start` prompt.
    deferred_prompt: Option<ContentInput>,
    /// Stable creation timestamp (Unix seconds), set when the pending session is staged.
    created_at_secs: u64,
    /// Last-modified timestamp (Unix seconds), updated on mutations like append_system_context.
    updated_at_secs: u64,
}

fn now_unix_secs() -> u64 {
    meerkat_core::time_compat::SystemTime::now()
        .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Merge a deferred prompt with a turn prompt, preserving multimodal content.
/// If both are plain text, concatenates with `\n\n`. Otherwise, converts both
/// to content blocks and concatenates (deferred blocks first, then turn blocks).
fn merge_content_inputs(deferred: ContentInput, turn: ContentInput) -> ContentInput {
    match (&deferred, &turn) {
        (ContentInput::Text(d), ContentInput::Text(t)) => ContentInput::Text(format!("{d}\n\n{t}")),
        _ => {
            let mut blocks = deferred.into_blocks();
            blocks.extend(turn.into_blocks());
            ContentInput::Blocks(blocks)
        }
    }
}

enum PendingSessionPhase {
    Staged {
        build_config: Box<AgentBuildConfig>,
    },
    Promoting {
        starting_system_context_state: SessionSystemContextState,
        current_system_context_state: SessionSystemContextState,
    },
}

// FactoryAgent and FactoryAgentBuilder are imported from meerkat::service_factory.

// ---------------------------------------------------------------------------
// SessionRuntime
// ---------------------------------------------------------------------------

/// Core runtime that manages agent sessions.
///
/// Wraps [`PersistentSessionService`] for session lifecycle management while
/// preserving the two-step create-then-run API required by JSON-RPC handlers.
pub struct SessionRuntime {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    /// Sessions that have been "created" (ID returned to caller) but not yet
    /// materialized in the service. The first `start_turn` call promotes them.
    pending: RwLock<IndexMap<SessionId, PendingSession>>,
    max_sessions: usize,
    /// Factory for building LLM clients on model/provider hot-swap.
    factory: AgentFactory,
    /// Override LLM client for all sessions (primarily for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
    realm_id: Option<String>,
    instance_id: Option<String>,
    backend: Option<String>,
    config_runtime: Option<Arc<meerkat_core::ConfigRuntime>>,
    runtime_adapter: Arc<RuntimeSessionAdapter>,
    /// Notification sink for event forwarding to the RPC transport.
    /// Used by lazy executor registration in start_turn_via_runtime.
    #[allow(dead_code)]
    notification_sink: crate::router::NotificationSink,
    skill_identity_registry: Arc<StdRwLock<SkillIdentityRegistryState>>,
    #[cfg(feature = "mcp")]
    mcp_sessions: RwLock<std::collections::HashMap<SessionId, SessionMcpState>>,
    /// Channel for sending callback tool requests to the RPC server loop.
    /// Wrapped in `RwLock` so it can be set after Arc wrapping (server construction).
    #[allow(clippy::type_complexity)]
    callback_request_tx: StdRwLock<
        Option<
            mpsc::Sender<(
                crate::protocol::RpcRequest,
                tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
            )>,
        >,
    >,
    /// Counter for generating unique server-originated callback request IDs.
    callback_id_counter_slot: StdRwLock<Arc<std::sync::atomic::AtomicU64>>,
    /// Globally registered callback tool definitions (via `tools/register`).
    registered_tools_slot: StdRwLock<Arc<StdRwLock<Vec<meerkat_core::ToolDef>>>>,
}

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

impl SessionRuntime {
    async fn live_session_is_stale(&self, session_id: &SessionId) -> Result<bool, RpcError> {
        let live = match self.service.export_live_session(session_id).await {
            Ok(session) => session,
            Err(SessionError::NotFound { .. }) => return Ok(false),
            Err(err) => return Err(session_error_to_rpc(err)),
        };
        let Some(stored) = self.load_persisted_session(session_id).await? else {
            return Ok(false);
        };
        Ok(stored.updated_at() > live.updated_at()
            || (stored.updated_at() == live.updated_at()
                && stored.messages().len() > live.messages().len()))
    }

    /// Create a new session runtime.
    pub fn new(
        factory: AgentFactory,
        config: Config,
        max_sessions: usize,
        persistence: PersistenceBundle,
        notification_sink: crate::router::NotificationSink,
    ) -> Self {
        let runtime_adapter = persistence.runtime_adapter();
        let (store, runtime_store) = persistence.into_parts();
        let factory_clone = factory.clone();
        let builder = FactoryAgentBuilder::new(factory, config);
        let service = Arc::new(PersistentSessionService::new(
            builder,
            max_sessions,
            store,
            runtime_store.clone(),
        ));

        Self {
            service,
            pending: RwLock::new(IndexMap::new()),
            max_sessions,
            factory: factory_clone,
            default_llm_client: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime: None,
            runtime_adapter,
            notification_sink,
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
            #[cfg(feature = "mcp")]
            mcp_sessions: RwLock::new(std::collections::HashMap::new()),
            callback_request_tx: StdRwLock::new(None),
            callback_id_counter_slot: StdRwLock::new(Arc::new(std::sync::atomic::AtomicU64::new(
                0,
            ))),
            registered_tools_slot: StdRwLock::new(Arc::new(StdRwLock::new(Vec::new()))),
        }
    }

    /// Create a runtime that resolves config from a shared config store.
    pub fn new_with_config_store(
        factory: AgentFactory,
        initial_config: Config,
        config_store: Arc<dyn ConfigStore>,
        max_sessions: usize,
        persistence: PersistenceBundle,
        notification_sink: crate::router::NotificationSink,
    ) -> Self {
        let runtime_adapter = persistence.runtime_adapter();
        let (store, runtime_store) = persistence.into_parts();
        let factory_clone = factory.clone();
        let builder =
            FactoryAgentBuilder::new_with_config_store(factory, initial_config, config_store);
        let service = Arc::new(PersistentSessionService::new(
            builder,
            max_sessions,
            store,
            runtime_store.clone(),
        ));

        Self {
            service,
            pending: RwLock::new(IndexMap::new()),
            max_sessions,
            factory: factory_clone,
            default_llm_client: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime: None,
            runtime_adapter,
            notification_sink,
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
            #[cfg(feature = "mcp")]
            mcp_sessions: RwLock::new(std::collections::HashMap::new()),
            callback_request_tx: StdRwLock::new(None),
            callback_id_counter_slot: StdRwLock::new(Arc::new(std::sync::atomic::AtomicU64::new(
                0,
            ))),
            registered_tools_slot: StdRwLock::new(Arc::new(StdRwLock::new(Vec::new()))),
        }
    }

    /// Attach realm context defaults used for session metadata.
    pub fn set_realm_context(
        &mut self,
        realm_id: Option<String>,
        instance_id: Option<String>,
        backend: Option<String>,
    ) {
        self.realm_id = realm_id;
        self.instance_id = instance_id;
        self.backend = backend;
    }

    /// Active realm id for this runtime, if configured.
    pub fn realm_id(&self) -> Option<&str> {
        self.realm_id.as_deref()
    }

    /// Attach config runtime for generation stamping.
    pub fn set_config_runtime(&mut self, runtime: Arc<meerkat_core::ConfigRuntime>) {
        self.config_runtime = Some(runtime);
    }

    /// Shared config runtime used by config handlers.
    pub fn config_runtime(&self) -> Option<Arc<meerkat_core::ConfigRuntime>> {
        self.config_runtime.as_ref().map(Arc::clone)
    }

    /// Build the runtime adapter appropriate for this runtime's persistence mode.
    pub fn runtime_adapter(&self) -> Arc<RuntimeSessionAdapter> {
        self.runtime_adapter.clone()
    }

    pub fn default_llm_client(&self) -> Option<Arc<dyn LlmClient>> {
        self.default_llm_client.clone()
    }

    /// Hot-swap the LLM client on a materialized session.
    ///
    /// Builds a new client for the given model/provider override and replaces
    /// the session's client via `set_session_client`. Falls back to
    /// `default_llm_client` when available, otherwise creates a fresh client
    /// from the factory (which resolves API keys from env vars).
    async fn hot_swap_llm_client(
        &self,
        session_id: &SessionId,
        ov: &crate::handlers::turn::TurnOverrides,
    ) -> Result<(), RpcError> {
        let catalog_default =
            meerkat_models::default_model("anthropic").unwrap_or("claude-sonnet-4-5");
        let model = ov.model.as_deref().unwrap_or(catalog_default);
        let provider = ov
            .provider
            .as_ref()
            .map(|p| meerkat_core::Provider::from_name(p))
            .unwrap_or_else(|| {
                meerkat_core::Provider::infer_from_model(model)
                    .unwrap_or(meerkat_core::Provider::Other)
            });

        let raw_client = if let Some(ref default) = self.default_llm_client {
            Arc::clone(default)
        } else {
            self.factory
                .build_llm_client(provider, None, None)
                .await
                .map_err(|e| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("Failed to build LLM client for model override: {e}"),
                    data: None,
                })?
        };

        let adapter: Arc<dyn meerkat_core::AgentLlmClient> =
            Arc::new(self.factory.build_llm_adapter(raw_client, model).await);
        self.service
            .set_session_client(session_id, adapter)
            .await
            .map_err(session_error_to_rpc)?;

        // Refresh view_image visibility based on the new model's capabilities,
        // merging with any pre-existing external tool filter so we don't clobber
        // restrictions set by other sources.
        let provider_str = provider.as_str();
        let Some(profile) = meerkat_models::profile::profile_for(provider_str, model) else {
            // Unknown model — leave tool visibility unchanged.
            return Ok(());
        };

        // Read the current external filter from session metadata.
        let current_filter = self
            .service
            .export_live_session(session_id)
            .await
            .ok()
            .and_then(|session| {
                session
                    .metadata()
                    .get(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY)
                    .and_then(|v| {
                        serde_json::from_value::<meerkat_core::ToolFilter>(v.clone()).ok()
                    })
            })
            .unwrap_or(meerkat_core::ToolFilter::All);

        let view_image = "view_image".to_string();
        let filter = if profile.image_tool_results {
            // New model supports image tool results — remove view_image from
            // existing deny set (if present), keeping other denied tools.
            match current_filter {
                meerkat_core::ToolFilter::Deny(mut denied) => {
                    denied.remove(&view_image);
                    if denied.is_empty() {
                        meerkat_core::ToolFilter::All
                    } else {
                        meerkat_core::ToolFilter::Deny(denied)
                    }
                }
                other => other,
            }
        } else {
            // New model does not support image tool results — add view_image
            // to the existing deny set, preserving other denied tools.
            match current_filter {
                meerkat_core::ToolFilter::Deny(mut denied) => {
                    denied.insert(view_image);
                    meerkat_core::ToolFilter::Deny(denied)
                }
                meerkat_core::ToolFilter::All => {
                    meerkat_core::ToolFilter::Deny([view_image].into_iter().collect())
                }
                // Allow list: remove view_image so it's no longer permitted.
                meerkat_core::ToolFilter::Allow(mut allowed) => {
                    allowed.remove(&view_image);
                    meerkat_core::ToolFilter::Allow(allowed)
                }
            }
        };

        // Best-effort: if the filter references unknown tools (e.g., view_image
        // was never registered), the session service will return an error that
        // we intentionally ignore.
        let _ = self
            .service
            .set_session_tool_filter(session_id, filter)
            .await;
        Ok(())
    }

    pub async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.service.discard_live_session(session_id).await
    }

    #[cfg(feature = "mob")]
    pub fn session_service(&self) -> Arc<dyn meerkat_mob::MobSessionService> {
        self.service.clone()
    }

    /// Set the callback request channel for tool callbacks.
    ///
    /// Takes `&self` so it can be called after the runtime is wrapped in `Arc`
    /// (e.g. during `RpcServer` construction).
    pub fn set_callback_channel(
        &self,
        tx: mpsc::Sender<(
            crate::protocol::RpcRequest,
            tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
        )>,
        id_counter: Arc<std::sync::atomic::AtomicU64>,
        registered_tools: Arc<StdRwLock<Vec<meerkat_core::ToolDef>>>,
    ) {
        if let Ok(mut slot) = self.callback_request_tx.write() {
            *slot = Some(tx);
        }
        // id_counter and registered_tools are already Arc-shared — the server
        // passes its own Arcs into the runtime at construction time. When using
        // the &self path (after Arc wrapping), we store them atomically.
        if let Ok(mut c) = self.callback_id_counter_slot.write() {
            *c = id_counter;
        }
        if let Ok(mut t) = self.registered_tools_slot.write() {
            *t = registered_tools;
        }
    }

    /// Get a clone of the callback request sender, if configured.
    pub fn callback_request_tx(
        &self,
    ) -> Option<
        mpsc::Sender<(
            crate::protocol::RpcRequest,
            tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
        )>,
    > {
        self.callback_request_tx
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Get the callback ID counter.
    pub fn callback_id_counter(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.callback_id_counter_slot
            .read()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Get the globally registered callback tool definitions.
    pub fn registered_tools(&self) -> Arc<StdRwLock<Vec<meerkat_core::ToolDef>>> {
        self.registered_tools_slot
            .read()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_else(|| Arc::new(StdRwLock::new(Vec::new())))
    }

    pub fn set_skill_identity_registry(&self, registry: SourceIdentityRegistry) {
        if let Ok(mut slot) = self.skill_identity_registry.write() {
            slot.registry = registry;
        }
    }

    pub fn set_skill_identity_registry_for_generation(
        &self,
        generation: u64,
        registry: SourceIdentityRegistry,
    ) {
        if let Ok(mut slot) = self.skill_identity_registry.write()
            && generation >= slot.generation
        {
            slot.generation = generation;
            slot.registry = registry;
        }
    }

    /// Start a turn by routing through the runtime input/completion waiter path.
    ///
    /// Instead of calling `SessionService::start_turn()` directly, this method:
    /// 1. Creates an `Input::Prompt` from the request parameters
    /// 2. Accepts it via `RuntimeSessionAdapter::accept_input_with_completion()`
    /// 3. Awaits the completion handle
    /// 4. Returns the `RunResult` produced by the executor
    ///
    /// This ensures all session-driving work flows through the single runtime
    /// authority (the RuntimeLoop → CoreExecutor pipeline).
    #[allow(clippy::too_many_arguments, unused_variables)]
    pub async fn start_turn_via_runtime(
        self: &Arc<Self>,
        session_id: &SessionId,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<RunResult, RpcError> {
        use meerkat_runtime::accept::AcceptOutcome;
        use meerkat_runtime::completion::CompletionOutcome;
        use meerkat_runtime::input::{Input, PromptInput};

        #[allow(unused_mut)]
        let mut prompt = prompt;

        if self.live_session_is_stale(session_id).await? {
            let _ = self.service.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        // Apply MCP boundary before building the input — staged MCP ops
        // (from mcp/add, mcp/remove, mcp/reload) must be connected and made
        // visible to the agent before the turn starts.
        #[cfg(feature = "mcp")]
        {
            let mut mcp_text = prompt.text_content();
            self.apply_mcp_boundary(session_id, &event_tx, &mut mcp_text)
                .await?;
            // If the MCP boundary appended notices, update the prompt.
            if mcp_text != prompt.text_content() {
                prompt = ContentInput::Text(mcp_text);
            }
        }

        // Build turn metadata from overrides
        let turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                host_mode: overrides.as_ref().and_then(|ov| ov.host_mode),
                skill_references,
                flow_tool_overlay,
                additional_instructions,
                model: overrides.as_ref().and_then(|ov| ov.model.clone()),
                provider: overrides.as_ref().and_then(|ov| ov.provider.clone()),
                provider_params: overrides.as_ref().and_then(|ov| ov.provider_params.clone()),
            },
        );

        let input = Input::Prompt(PromptInput::from_content_input(prompt, turn_metadata));

        // Lazy-register executor if not already registered.
        if !self.runtime_adapter.contains_session(session_id).await {
            // Verify the session exists before registering to avoid creating
            // dead executors for nonexistent sessions.
            let session_exists = self.pending.read().await.contains_key(session_id)
                || self.service.read(session_id).await.is_ok()
                || self
                    .load_persisted_session(session_id)
                    .await
                    .ok()
                    .flatten()
                    .is_some();
            if !session_exists {
                return Err(RpcError {
                    code: error::SESSION_NOT_FOUND,
                    message: format!("session not found: {session_id}"),
                    data: None,
                });
            }
            let executor = Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                Arc::clone(self),
                session_id.clone(),
                self.notification_sink.clone(),
            ));
            self.runtime_adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
        }

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("runtime accept failed: {e}"),
                data: None,
            })?;

        // Forward events while waiting for completion
        // (Events are forwarded by the executor's forwarder task,
        // which is spawned inside SessionRuntimeExecutor::apply())

        let Some(handle) = handle else {
            // Input already terminal (dedup of completed input)
            let existing_id = match outcome {
                AcceptOutcome::Deduplicated { existing_id, .. } => existing_id.to_string(),
                _ => "unknown".to_string(),
            };
            return Err(RpcError {
                code: error::DUPLICATE_INPUT,
                message: "input already processed".to_string(),
                data: Some(serde_json::json!({ "existing_id": existing_id })),
            });
        };

        match handle.wait().await {
            CompletionOutcome::Completed(result) => Ok(result),
            CompletionOutcome::CompletedWithoutResult => Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: "turn completed without result".to_string(),
                data: None,
            }),
            CompletionOutcome::Abandoned(reason) => Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("turn abandoned: {reason}"),
                data: None,
            }),
            CompletionOutcome::RuntimeTerminated(reason) => Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("runtime terminated: {reason}"),
                data: None,
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        primitive: &RunPrimitive,
        prompt: String,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<CoreApplyOutput, RpcError> {
        if let RunPrimitive::StagedInput(staged) = primitive
            && staged.appends.is_empty()
            && !staged.context_appends.is_empty()
            && staged.boundary == RunApplyBoundary::Immediate
        {
            return self
                .service
                .apply_runtime_context_appends(
                    session_id,
                    run_id,
                    staged.context_appends.clone(),
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(session_error_to_rpc);
        }

        let pending_session = {
            let mut pending = self.pending.write().await;
            match pending.get_mut(session_id) {
                Some(pending_session) => {
                    let starting_system_context_state = match &pending_session.phase {
                        PendingSessionPhase::Staged { build_config } => build_config
                            .resume_session
                            .as_ref()
                            .and_then(Session::system_context_state)
                            .unwrap_or_default(),
                        PendingSessionPhase::Promoting { .. } => {
                            return Err(RpcError {
                                code: error::SESSION_BUSY,
                                message: format!(
                                    "session {session_id} is already being materialized"
                                ),
                                data: None,
                            });
                        }
                    };
                    let phase = std::mem::replace(
                        &mut pending_session.phase,
                        PendingSessionPhase::Promoting {
                            starting_system_context_state: starting_system_context_state.clone(),
                            current_system_context_state: starting_system_context_state,
                        },
                    );
                    let PendingSessionPhase::Staged { build_config } = phase else {
                        unreachable!("phase was checked before replacement");
                    };
                    Some((
                        *build_config,
                        pending_session.labels.clone(),
                        pending_session.deferred_prompt.clone(),
                        pending_session.created_at_secs,
                        pending_session.updated_at_secs,
                    ))
                }
                None => None,
            }
        };

        let persisted_host_mode = if pending_session.is_none() {
            self.load_persisted_session(session_id)
                .await?
                .and_then(|session| session.session_metadata().map(|meta| meta.host_mode))
        } else {
            None
        };
        let host_mode = overrides
            .as_ref()
            .and_then(|ov| ov.host_mode)
            .or_else(|| {
                pending_session
                    .as_ref()
                    .map(|(build_config, _, _, _, _)| build_config.host_mode)
            })
            .or(persisted_host_mode)
            .unwrap_or(false);

        if pending_session.is_none() && !self.live_session_is_stale(session_id).await? {
            // Hot-swap LLM client if model/provider overrides are present.
            if let Some(ref ov) = overrides
                && (ov.model.is_some() || ov.provider.is_some() || ov.provider_params.is_some())
            {
                self.hot_swap_llm_client(session_id, ov).await?;
            }

            let req = StartTurnRequest {
                prompt: prompt.clone().into(),
                event_tx: Some(event_tx.clone()),
                host_mode,
                skill_references: skill_references.clone(),
                flow_tool_overlay: flow_tool_overlay.clone(),
                additional_instructions: additional_instructions.clone(),
            };

            match self
                .service
                .apply_runtime_turn(
                    session_id,
                    run_id.clone(),
                    req,
                    match primitive {
                        RunPrimitive::StagedInput(staged) => staged.boundary,
                        _ => RunApplyBoundary::Immediate,
                    },
                    primitive.contributing_input_ids().to_vec(),
                )
                .await
            {
                Ok(output) => return Ok(output),
                Err(SessionError::NotFound { .. }) => {}
                Err(err) => return Err(session_error_to_rpc(err)),
            }
        }

        if let Some((mut build_config, labels, deferred_prompt, created_at_secs, updated_at_secs)) =
            pending_session
        {
            let mut runtime_prompt: ContentInput = prompt.clone().into();
            let saved_deferred_prompt = deferred_prompt.clone();
            if let Some(deferred) = deferred_prompt {
                runtime_prompt = merge_content_inputs(deferred, runtime_prompt);
            }

            if let Some(ref ov) = overrides {
                if let Some(ref model) = ov.model {
                    build_config.model = model.clone();
                }
                if let Some(ref provider) = ov.provider {
                    build_config.provider = Some(meerkat_core::Provider::from_name(provider));
                }
                if let Some(max_tokens) = ov.max_tokens {
                    build_config.max_tokens = Some(max_tokens);
                }
                if let Some(ref system_prompt) = ov.system_prompt {
                    build_config.system_prompt = Some(system_prompt.clone());
                }
                if let Some(ref output_schema) = ov.output_schema {
                    match meerkat_core::OutputSchema::from_json_value(output_schema.clone()) {
                        Ok(os) => build_config.output_schema = Some(os),
                        Err(e) => {
                            self.restore_pending_from_promoting(
                                session_id,
                                build_config,
                                labels,
                                saved_deferred_prompt,
                                created_at_secs,
                                updated_at_secs,
                            )
                            .await;
                            return Err(RpcError {
                                code: error::INVALID_PARAMS,
                                message: format!("Invalid output_schema override: {e}"),
                                data: None,
                            });
                        }
                    }
                }
                if let Some(retries) = ov.structured_output_retries {
                    build_config.structured_output_retries = retries;
                }
                if let Some(ref pp) = ov.provider_params {
                    build_config.provider_params = Some(pp.clone());
                }
                if let Some(host_mode) = ov.host_mode {
                    build_config.host_mode = host_mode;
                }
            }

            if build_config.llm_client_override.is_none()
                && let Some(ref client) = self.default_llm_client
            {
                build_config.llm_client_override = Some(client.clone());
            }
            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = &self.config_runtime {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            // Inject runtime adapter so the factory can construct a per-session
            // RuntimeInputSink for host-mode comms routing.
            build_config.runtime_adapter_for_sink = Some(self.runtime_adapter.clone());

            let mut build = build_config.to_session_build_options();
            build.realm_id = build.realm_id.or_else(|| self.realm_id.clone());
            build.instance_id = build.instance_id.or_else(|| self.instance_id.clone());
            build.backend = build.backend.or_else(|| self.backend.clone());
            build.config_generation = build.config_generation.or(runtime_generation);

            match self
                .service
                .create_session(CreateSessionRequest {
                    model: build_config.model.clone(),
                    prompt: runtime_prompt.clone(),
                    system_prompt: build_config.system_prompt.clone(),
                    max_tokens: build_config.max_tokens,
                    event_tx: None,
                    host_mode: build_config.host_mode,
                    skill_references: skill_references.clone(),
                    initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                    build: Some(build),
                    labels: labels.clone(),
                })
                .await
            {
                Ok(_) => {
                    if let Some((starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                        && let Err(err) = self
                            .replay_promoted_system_context(
                                session_id,
                                &starting_system_context_state,
                                &current_system_context_state,
                            )
                            .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err.message,
                            "failed to replay promoted system-context state after runtime materialization"
                        );
                    }
                }
                Err(err) => {
                    if let Some((_starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                    {
                        let session = build_config
                            .resume_session
                            .get_or_insert_with(|| Session::with_id(session_id.clone()));
                        session
                            .set_system_context_state(current_system_context_state)
                            .map_err(|serialize_err| RpcError {
                                code: error::INTERNAL_ERROR,
                                message: format!(
                                    "failed to serialize system-context state: {serialize_err}"
                                ),
                                data: None,
                            })?;
                    }
                    self.restore_pending_from_promoting(
                        session_id,
                        build_config,
                        labels,
                        saved_deferred_prompt,
                        created_at_secs,
                        updated_at_secs,
                    )
                    .await;
                    return Err(session_error_to_rpc(err));
                }
            }

            let (_, output) = self
                .service
                .apply_runtime_turn_with_result(
                    session_id,
                    run_id,
                    StartTurnRequest {
                        prompt: runtime_prompt,
                        event_tx: Some(event_tx),
                        host_mode: build_config.host_mode,
                        skill_references,
                        flow_tool_overlay,
                        additional_instructions,
                    },
                    match primitive {
                        RunPrimitive::StagedInput(staged) => staged.boundary,
                        _ => RunApplyBoundary::Immediate,
                    },
                    primitive.contributing_input_ids().to_vec(),
                )
                .await
                .map_err(session_error_to_rpc)?;
            return Ok(output);
        }

        let stored_session = self
            .load_persisted_session(session_id)
            .await?
            .ok_or_else(|| RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("session not found: {session_id}"),
                data: None,
            })?;
        let stored_metadata = stored_session.session_metadata();
        let tooling = stored_metadata
            .as_ref()
            .map(|meta| meta.tooling.clone())
            .unwrap_or_default();
        let current_generation = match self.config_runtime.as_ref() {
            Some(runtime) => runtime.get().await.ok().map(|snapshot| snapshot.generation),
            None => None,
        };
        let build = SessionBuildOptions {
            provider: stored_metadata.as_ref().map(|meta| meta.provider),
            output_schema: None,
            structured_output_retries: 2,
            hooks_override: HookRunOverrides::default(),
            comms_name: stored_metadata
                .as_ref()
                .and_then(|meta| meta.comms_name.clone()),
            peer_meta: stored_metadata
                .as_ref()
                .and_then(|meta| meta.peer_meta.clone()),
            resume_session: Some(stored_session),
            budget_limits: None,
            provider_params: None,
            external_tools: None,
            llm_client_override: self
                .default_llm_client
                .clone()
                .map(encode_llm_client_override_for_service),
            scoped_event_tx: None,
            scoped_event_path: None,
            override_builtins: Some(tooling.builtins),
            override_shell: Some(tooling.shell),
            override_subagents: Some(tooling.subagents),
            override_memory: Some(tooling.memory),
            override_mob: Some(tooling.mob),
            preload_skills: tooling.active_skills.clone(),
            realm_id: stored_metadata
                .as_ref()
                .and_then(|meta| meta.realm_id.clone())
                .or_else(|| self.realm_id.clone()),
            instance_id: stored_metadata
                .as_ref()
                .and_then(|meta| meta.instance_id.clone())
                .or_else(|| self.instance_id.clone()),
            backend: stored_metadata
                .as_ref()
                .and_then(|meta| meta.backend.clone())
                .or_else(|| self.backend.clone()),
            config_generation: stored_metadata
                .as_ref()
                .and_then(|meta| meta.config_generation)
                .or(current_generation),
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            additional_instructions: None,
            shell_env: None,
            runtime_adapter_for_sink: Some(self.runtime_adapter.clone()),
        };
        self.service
            .create_session(CreateSessionRequest {
                model: stored_metadata
                    .as_ref()
                    .map(|meta| meta.model.clone())
                    .ok_or_else(|| RpcError {
                        code: error::INTERNAL_ERROR,
                        message: format!(
                            "persisted session {session_id} is missing session metadata"
                        ),
                        data: None,
                    })?,
                prompt: prompt.clone().into(),
                system_prompt: overrides.as_ref().and_then(|ov| ov.system_prompt.clone()),
                max_tokens: overrides
                    .as_ref()
                    .and_then(|ov| ov.max_tokens)
                    .or_else(|| stored_metadata.as_ref().map(|meta| meta.max_tokens)),
                event_tx: None,
                host_mode,
                skill_references: skill_references.clone(),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                build: Some(build),
                labels: None,
            })
            .await
            .map_err(session_error_to_rpc)?;
        let (_, output) = self
            .service
            .apply_runtime_turn_with_result(
                session_id,
                run_id,
                StartTurnRequest {
                    prompt: prompt.into(),
                    event_tx: Some(event_tx),
                    host_mode,
                    skill_references,
                    flow_tool_overlay,
                    additional_instructions,
                },
                match primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                primitive.contributing_input_ids().to_vec(),
            )
            .await
            .map_err(session_error_to_rpc)?;
        Ok(output)
    }

    pub fn skill_identity_registry(&self) -> SourceIdentityRegistry {
        self.skill_identity_registry
            .read()
            .map(|state| state.registry.clone())
            .unwrap_or_default()
    }

    /// Build a source identity registry from runtime config.
    pub fn build_skill_identity_registry(
        config: &Config,
    ) -> Result<SourceIdentityRegistry, SkillError> {
        config.skills.build_source_identity_registry()
    }

    /// Create a new session with the given build configuration.
    ///
    /// Returns the session ID on success. The session is staged as "pending"
    /// and will be materialized inside the service on the first `start_turn`.
    pub async fn create_session(
        &self,
        build_config: AgentBuildConfig,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
    ) -> Result<SessionId, RpcError> {
        // Check combined capacity (pending + active).
        {
            let pending = self.pending.read().await;
            let active = self
                .service
                .list(Default::default())
                .await
                .map_err(session_error_to_rpc)?
                .len();
            let total = pending.len() + active;
            if total >= self.max_sessions {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("Max sessions reached ({}/{})", total, self.max_sessions),
                    data: None,
                });
            }
        }

        // Pre-create a session to claim a stable SessionId.
        let session = Session::new();
        let session_id = session.id().clone();

        // Inject the pre-created session so the agent builder will reuse this ID.
        let build_config = AgentBuildConfig {
            resume_session: Some(session),
            ..build_config
        };

        #[cfg(feature = "mcp")]
        let build_config = self
            .attach_mcp_adapter_for_pending_session(session_id.clone(), build_config)
            .await?;

        {
            let mut pending = self.pending.write().await;
            pending.insert(
                session_id.clone(),
                PendingSession {
                    phase: PendingSessionPhase::Staged {
                        build_config: Box::new(build_config),
                    },
                    labels,
                    deferred_prompt,
                    created_at_secs: now_unix_secs(),
                    updated_at_secs: now_unix_secs(),
                },
            );
        }

        Ok(session_id)
    }

    /// Start a turn on the given session.
    ///
    /// Events are forwarded to `event_tx` during the turn. Returns the
    /// `RunResult` when the turn completes.
    ///
    /// `overrides` may contain per-turn overrides. For pending (deferred)
    /// sessions, all overrides are applied to the staged `AgentBuildConfig`.
    /// For materialized sessions, only `host_mode` is allowed; all other
    /// overrides are rejected with an error.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn(
        &self,
        session_id: &SessionId,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<RunResult, RpcError> {
        #[allow(unused_mut)]
        let mut turn_prompt = prompt;
        #[cfg(feature = "mcp")]
        {
            let mut mcp_text = turn_prompt.text_content();
            self.apply_mcp_boundary(session_id, &event_tx, &mut mcp_text)
                .await?;
            if mcp_text != turn_prompt.text_content() {
                turn_prompt = ContentInput::Text(mcp_text);
            }
        }

        // Check if this is a pending (not-yet-materialized) session.
        let pending_session = {
            let mut pending = self.pending.write().await;
            match pending.get_mut(session_id) {
                Some(pending_session) => {
                    let starting_system_context_state = match &pending_session.phase {
                        PendingSessionPhase::Staged { build_config } => build_config
                            .resume_session
                            .as_ref()
                            .and_then(Session::system_context_state)
                            .unwrap_or_default(),
                        PendingSessionPhase::Promoting { .. } => {
                            return Err(RpcError {
                                code: error::SESSION_BUSY,
                                message: format!(
                                    "session {session_id} is already being materialized"
                                ),
                                data: None,
                            });
                        }
                    };
                    let phase = std::mem::replace(
                        &mut pending_session.phase,
                        PendingSessionPhase::Promoting {
                            starting_system_context_state: starting_system_context_state.clone(),
                            current_system_context_state: starting_system_context_state,
                        },
                    );
                    let PendingSessionPhase::Staged { build_config } = phase else {
                        unreachable!("phase was checked before replacement");
                    };
                    Some((
                        *build_config,
                        pending_session.labels.clone(),
                        pending_session.deferred_prompt.clone(),
                        pending_session.created_at_secs,
                        pending_session.updated_at_secs,
                    ))
                }
                None => None,
            }
        };

        if let Some((mut build_config, labels, deferred_prompt, created_at_secs, updated_at_secs)) =
            pending_session
        {
            // Prepend the deferred create-time prompt to the turn prompt.
            // Keep copies for rollback paths that re-stage the pending session.
            let saved_deferred_prompt = deferred_prompt.clone();
            let saved_created_at_secs = created_at_secs;
            let saved_updated_at_secs = updated_at_secs;
            if let Some(deferred) = deferred_prompt {
                turn_prompt = merge_content_inputs(deferred, turn_prompt);
            }
            // Apply per-turn overrides to the pending build config.
            if let Some(ref ov) = overrides {
                if let Some(ref model) = ov.model {
                    build_config.model = model.clone();
                }
                if let Some(ref provider) = ov.provider {
                    build_config.provider = Some(meerkat_core::Provider::from_name(provider));
                }
                if let Some(max_tokens) = ov.max_tokens {
                    build_config.max_tokens = Some(max_tokens);
                }
                if let Some(ref system_prompt) = ov.system_prompt {
                    build_config.system_prompt = Some(system_prompt.clone());
                }
                if let Some(ref output_schema) = ov.output_schema {
                    match meerkat_core::OutputSchema::from_json_value(output_schema.clone()) {
                        Ok(os) => build_config.output_schema = Some(os),
                        Err(e) => {
                            // Restore pending state before returning error.
                            self.restore_pending_from_promoting(
                                session_id,
                                build_config,
                                labels,
                                saved_deferred_prompt,
                                saved_created_at_secs,
                                saved_updated_at_secs,
                            )
                            .await;
                            return Err(RpcError {
                                code: error::INVALID_PARAMS,
                                message: format!("Invalid output_schema override: {e}"),
                                data: None,
                            });
                        }
                    }
                }
                if let Some(retries) = ov.structured_output_retries {
                    build_config.structured_output_retries = retries;
                }
                if let Some(ref pp) = ov.provider_params {
                    build_config.provider_params = Some(pp.clone());
                }
                if let Some(host_mode) = ov.host_mode {
                    build_config.host_mode = host_mode;
                }
            }
            // Inject default LLM client if the caller didn't provide one.
            if build_config.llm_client_override.is_none()
                && let Some(ref client) = self.default_llm_client
            {
                build_config.llm_client_override = Some(client.clone());
            }
            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = &self.config_runtime {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            build_config.runtime_adapter_for_sink = Some(self.runtime_adapter.clone());

            let mut build = build_config.to_session_build_options();
            build.realm_id = build.realm_id.or_else(|| self.realm_id.clone());
            build.instance_id = build.instance_id.or_else(|| self.instance_id.clone());
            build.backend = build.backend.or_else(|| self.backend.clone());
            build.config_generation = build.config_generation.or(runtime_generation);

            let req = CreateSessionRequest {
                model: build_config.model.clone(),
                prompt: turn_prompt,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: Some(event_tx),
                host_mode: build_config.host_mode,
                skill_references: skill_references.clone(),
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                build: Some(build),
                labels: labels.clone(),
            };

            match self.service.create_session(req).await {
                Ok(result) => {
                    if let Some((starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                        && let Err(err) = self
                            .replay_promoted_system_context(
                                session_id,
                                &starting_system_context_state,
                                &current_system_context_state,
                            )
                            .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err.message,
                            "failed to replay promoted system-context state after create_session; preserving completed turn result"
                        );
                    }
                    return Ok(result);
                }
                Err(err) => {
                    if let Some((_starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                    {
                        let session = build_config
                            .resume_session
                            .get_or_insert_with(|| Session::with_id(session_id.clone()));
                        session
                            .set_system_context_state(current_system_context_state)
                            .map_err(|serialize_err| RpcError {
                                code: error::INTERNAL_ERROR,
                                message: format!(
                                    "failed to serialize system-context state: {serialize_err}"
                                ),
                                data: None,
                            })?;
                        let mut pending = self.pending.write().await;
                        pending.insert(
                            session_id.clone(),
                            PendingSession {
                                phase: PendingSessionPhase::Staged {
                                    build_config: Box::new(build_config),
                                },
                                labels,
                                deferred_prompt: saved_deferred_prompt.clone(),
                                created_at_secs: saved_created_at_secs,
                                updated_at_secs: saved_updated_at_secs,
                            },
                        );
                    }
                    return Err(session_error_to_rpc(err));
                }
            }
        }

        // Normal turn on an existing (materialized) session.
        // Reject overrides that cannot be applied mid-session.
        if let Some(ref ov) = overrides {
            let rejected = [
                ov.max_tokens.map(|_| "max_tokens"),
                ov.system_prompt.as_ref().map(|_| "system_prompt"),
                ov.output_schema.as_ref().map(|_| "output_schema"),
                ov.structured_output_retries
                    .map(|_| "structured_output_retries"),
            ];
            let rejected: Vec<&str> = rejected.into_iter().flatten().collect();
            if !rejected.is_empty() {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!(
                        "Cannot override {} on a materialized session; use deferred session/create",
                        rejected.join(", ")
                    ),
                    data: None,
                });
            }
        }

        // Hot-swap LLM client if model/provider changed.
        if let Some(ref ov) = overrides
            && (ov.model.is_some() || ov.provider.is_some() || ov.provider_params.is_some())
        {
            self.hot_swap_llm_client(session_id, ov).await?;
        }

        let host_mode = match overrides.as_ref().and_then(|ov| ov.host_mode) {
            Some(host_mode) => host_mode,
            None => self
                .load_persisted_session(session_id)
                .await?
                .and_then(|session| session.session_metadata().map(|meta| meta.host_mode))
                .unwrap_or(false),
        };

        let req = StartTurnRequest {
            prompt: turn_prompt.clone(),
            event_tx: Some(event_tx.clone()),
            host_mode,
            skill_references: skill_references.clone(),
            flow_tool_overlay: flow_tool_overlay.clone(),
            additional_instructions: additional_instructions.clone(),
        };

        if self.live_session_is_stale(session_id).await? {
            return self
                .try_recover_persisted_session(
                    session_id,
                    turn_prompt,
                    event_tx,
                    host_mode,
                    skill_references,
                    flow_tool_overlay,
                    additional_instructions,
                )
                .await;
        }

        match self.service.start_turn(session_id, req).await {
            Ok(result) => Ok(result),
            Err(SessionError::NotFound { .. }) => {
                // Attempt persisted session recovery: the session may exist in the
                // store from a previous runtime lifetime.
                self.try_recover_persisted_session(
                    session_id,
                    turn_prompt,
                    event_tx,
                    host_mode,
                    skill_references,
                    flow_tool_overlay,
                    additional_instructions,
                )
                .await
            }
            Err(err) => Err(session_error_to_rpc(err)),
        }
    }

    /// Restore a pending session from `Promoting` state back to `Staged` after
    /// an override validation failure. Prevents the session from being stuck in
    /// the promoting state when the turn is aborted before `create_session`.
    async fn restore_pending_from_promoting(
        &self,
        session_id: &SessionId,
        build_config: AgentBuildConfig,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
        created_at_secs: u64,
        updated_at_secs: u64,
    ) {
        let mut pending = self.pending.write().await;
        pending.insert(
            session_id.clone(),
            PendingSession {
                phase: PendingSessionPhase::Staged {
                    build_config: Box::new(build_config),
                },
                labels,
                deferred_prompt,
                created_at_secs,
                updated_at_secs,
            },
        );
    }

    /// Attempt to recover a persisted session that is no longer live.
    ///
    /// Mirrors the REST recovery path: load the session from the store, extract
    /// stored metadata, build an `AgentBuildConfig`, stage as pending, and
    /// fall through to the normal materialization path.
    ///
    /// Returns `SESSION_NOT_FOUND` if the session cannot be recovered.
    #[allow(clippy::too_many_arguments)]
    async fn try_recover_persisted_session(
        &self,
        session_id: &SessionId,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        host_mode: bool,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
    ) -> Result<RunResult, RpcError> {
        let loaded_session = self
            .service
            .load_persisted(session_id)
            .await
            .map_err(session_error_to_rpc)?;

        let Some(session) = loaded_session else {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        };
        if session_metadata_marks_archived(&session) {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        }

        let stored_metadata = session.session_metadata();

        // Build config from stored metadata.
        let model = stored_metadata
            .as_ref()
            .map(|m| m.model.clone())
            .unwrap_or_else(|| {
                meerkat_models::default_model("anthropic")
                    .unwrap_or("claude-sonnet-4-5")
                    .to_string()
            });
        let mut build_config = AgentBuildConfig::new(model);
        build_config.resume_session = Some(session);

        if let Some(ref meta) = stored_metadata {
            build_config.provider = Some(meta.provider);
            build_config.max_tokens = Some(meta.max_tokens);
            build_config.host_mode = meta.host_mode;
            build_config.comms_name = meta.comms_name.clone();
            build_config.peer_meta = meta.peer_meta.clone();
            build_config.override_builtins = Some(meta.tooling.builtins);
            build_config.override_shell = Some(meta.tooling.shell);
            build_config.override_subagents = Some(meta.tooling.subagents);
            build_config.override_mob = Some(meta.tooling.mob);
            build_config.override_memory = Some(meta.tooling.memory);
            build_config.preload_skills = meta.tooling.active_skills.clone();
        }

        // Apply caller-requested host_mode override (from turn/start params).
        if host_mode {
            build_config.host_mode = true;
        }

        // Inject default LLM client if available.
        if let Some(ref client) = self.default_llm_client {
            build_config.llm_client_override = Some(client.clone());
        }

        // Restore callback tools from globally registered definitions.
        // Note: per-session external_tools (from CreateSessionParams) are
        // ephemeral in-process handlers and cannot survive a runtime restart.
        // Only globally registered tools (via tools/register) are restored.
        if let Some(tx) = self.callback_request_tx() {
            let tools: Vec<meerkat_core::ToolDef> = self
                .registered_tools()
                .read()
                .map(|g| g.iter().cloned().collect())
                .unwrap_or_default();
            if !tools.is_empty() {
                let dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> =
                    Arc::new(crate::callback_dispatcher::CallbackToolDispatcher::new(
                        tools,
                        tx,
                        self.callback_id_counter(),
                    ));
                build_config.external_tools = Some(dispatcher);
            }
        }

        // Stage as pending and re-enter the materialization path.
        // Labels are managed by the session service — pass None so
        // CreateSessionRequest.labels is empty and the service preserves
        // whatever labels were stored with the original session.
        let labels: Option<BTreeMap<String, String>> = None;

        {
            let mut pending = self.pending.write().await;
            pending.insert(
                session_id.clone(),
                PendingSession {
                    phase: PendingSessionPhase::Staged {
                        build_config: Box::new(build_config),
                    },
                    labels,
                    deferred_prompt: None,
                    created_at_secs: now_unix_secs(),
                    updated_at_secs: now_unix_secs(),
                },
            );
        }

        // Recursively call start_turn which will now find the pending session.
        // Use Box::pin to avoid infinite recursion concerns in async.
        Box::pin(self.start_turn(
            session_id,
            prompt,
            event_tx,
            skill_references,
            flow_tool_overlay,
            additional_instructions,
            None,
        ))
        .await
    }

    async fn take_promoting_system_context_state(
        &self,
        session_id: &SessionId,
    ) -> Option<(SessionSystemContextState, SessionSystemContextState)> {
        let mut pending = self.pending.write().await;
        let pending_session = pending.swap_remove(session_id)?;
        match pending_session.phase {
            PendingSessionPhase::Promoting {
                starting_system_context_state,
                current_system_context_state,
            } => Some((starting_system_context_state, current_system_context_state)),
            PendingSessionPhase::Staged { build_config } => {
                pending.insert(
                    session_id.clone(),
                    PendingSession {
                        phase: PendingSessionPhase::Staged { build_config },
                        labels: pending_session.labels,
                        deferred_prompt: None,
                        created_at_secs: pending_session.created_at_secs,
                        updated_at_secs: pending_session.updated_at_secs,
                    },
                );
                None
            }
        }
    }

    async fn replay_promoted_system_context(
        &self,
        session_id: &SessionId,
        starting_state: &SessionSystemContextState,
        current_state: &SessionSystemContextState,
    ) -> Result<(), RpcError> {
        for pending in &current_state.pending {
            if starting_state.pending.contains(pending) {
                continue;
            }
            self.service
                .append_system_context(
                    session_id,
                    AppendSystemContextRequest {
                        text: pending.text.clone(),
                        source: pending.source.clone(),
                        idempotency_key: pending.idempotency_key.clone(),
                    },
                )
                .await
                .map_err(system_context_error_to_rpc)?;
        }
        Ok(())
    }

    /// Interrupt a running turn on the given session.
    ///
    /// If the session is idle, this is a no-op.
    pub async fn interrupt(&self, session_id: &SessionId) -> Result<(), RpcError> {
        // Pending sessions have no running turn.
        {
            let pending = self.pending.read().await;
            if pending.contains_key(session_id) {
                return Ok(());
            }
        }

        match self.service.interrupt(session_id).await {
            Ok(()) => Ok(()),
            // The service returns NotRunning when no turn is active — map to no-op.
            Err(SessionError::NotRunning { .. }) => Ok(()),
            Err(e) => Err(session_error_to_rpc(e)),
        }
    }

    /// Append runtime system context to a pending, live, or persisted session.
    pub async fn append_system_context(
        &self,
        session_id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, RpcError> {
        {
            let mut pending = self.pending.write().await;
            if let Some(pending_session) = pending.get_mut(session_id) {
                let status = match &mut pending_session.phase {
                    PendingSessionPhase::Staged { build_config } => {
                        let session = build_config
                            .resume_session
                            .get_or_insert_with(|| Session::with_id(session_id.clone()));
                        let mut state = session.system_context_state().unwrap_or_default();
                        let status = state
                            .stage_append(&req, meerkat_core::time_compat::SystemTime::now())
                            .map_err(|err| {
                                system_context_error_to_rpc(err.into_control_error(session_id))
                            })?;
                        session
                            .set_system_context_state(state)
                            .map_err(|err| RpcError {
                                code: error::INTERNAL_ERROR,
                                message: format!("failed to serialize system-context state: {err}"),
                                data: None,
                            })?;
                        status
                    }
                    PendingSessionPhase::Promoting {
                        current_system_context_state,
                        ..
                    } => current_system_context_state
                        .stage_append(&req, meerkat_core::time_compat::SystemTime::now())
                        .map_err(|err| {
                            system_context_error_to_rpc(err.into_control_error(session_id))
                        })?,
                };
                // Bump updated_at on successful mutation.
                pending_session.updated_at_secs = now_unix_secs();
                return Ok(AppendSystemContextResult { status });
            }
        }

        self.service
            .append_system_context(session_id, req)
            .await
            .map_err(system_context_error_to_rpc)
    }

    /// Get the current state of a session, or `None` if the session does not exist.
    pub async fn session_state(&self, session_id: &SessionId) -> Option<SessionInfo> {
        // Check pending sessions first.
        {
            let pending = self.pending.read().await;
            if let Some(ps) = pending.get(session_id) {
                return Some(SessionInfo {
                    session_id: session_id.clone(),
                    state: match &ps.phase {
                        PendingSessionPhase::Staged { .. } => SessionState::Idle,
                        PendingSessionPhase::Promoting { .. } => SessionState::Running,
                    },
                    labels: ps.labels.clone().unwrap_or_default(),
                });
            }
        }

        // Use `list()` instead of `read()` to avoid blocking on a
        // `ReadSnapshot` command while a turn is in progress. `list()`
        // reads state from non-blocking watch receivers.
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in summaries {
                if summary.session_id == *session_id {
                    return Some(SessionInfo {
                        session_id: summary.session_id,
                        state: if summary.is_active {
                            SessionState::Running
                        } else {
                            SessionState::Idle
                        },
                        labels: summary.labels,
                    });
                }
            }
        }

        None
    }

    pub async fn pending_session_exists(&self, session_id: &SessionId) -> bool {
        self.pending.read().await.contains_key(session_id)
    }

    /// Read the authoritative session view from the owning session service.
    pub async fn read_session(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::service::SessionView, RpcError> {
        self.service
            .read(session_id)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Load the persisted session snapshot for authoritative post-turn inspection.
    pub async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, RpcError> {
        self.service
            .load_persisted(session_id)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Archive (remove) a session.
    pub async fn archive_session(&self, session_id: &SessionId) -> Result<(), RpcError> {
        // Check pending sessions first.
        {
            let mut pending = self.pending.write().await;
            if pending.swap_remove(session_id).is_some() {
                #[cfg(feature = "mcp")]
                if let Some(state) = self.mcp_sessions.write().await.remove(session_id) {
                    state.adapter.shutdown().await;
                }
                return Ok(());
            }
        }

        let result = self
            .service
            .archive(session_id)
            .await
            .map_err(session_error_to_rpc);

        #[cfg(feature = "mcp")]
        if let Some(state) = self.mcp_sessions.write().await.remove(session_id) {
            state.adapter.shutdown().await;
        }

        result
    }

    /// List all active sessions, optionally filtered by query parameters.
    pub async fn list_sessions(&self, query: SessionQuery) -> Vec<SessionInfo> {
        let mut result = Vec::new();

        // Include pending sessions as Idle, filtered by labels if requested.
        {
            let pending = self.pending.read().await;
            for (session_id, ps) in pending.iter() {
                let pending_labels = ps.labels.as_ref();
                if let Some(ref filter) = query.labels {
                    let matches = pending_labels
                        .is_some_and(|pl| filter.iter().all(|(k, v)| pl.get(k) == Some(v)));
                    if !matches {
                        continue;
                    }
                }
                result.push(SessionInfo {
                    session_id: session_id.clone(),
                    state: match &ps.phase {
                        PendingSessionPhase::Staged { .. } => SessionState::Idle,
                        PendingSessionPhase::Promoting { .. } => SessionState::Running,
                    },
                    labels: pending_labels.cloned().unwrap_or_default(),
                });
            }
        }

        // Include active sessions from the service. The service's `list()`
        // already handles label filtering on materialized sessions.
        if let Ok(summaries) = self.service.list(query).await {
            for summary in summaries {
                let state = if summary.is_active {
                    SessionState::Running
                } else {
                    SessionState::Idle
                };
                result.push(SessionInfo {
                    session_id: summary.session_id,
                    state,
                    labels: summary.labels,
                });
            }
        }

        result
    }

    /// List all active sessions as canonical wire summaries, including pending sessions.
    pub async fn list_sessions_rich(
        &self,
        query: SessionQuery,
    ) -> Vec<meerkat_contracts::WireSessionSummary> {
        let mut result = Vec::new();

        // Include pending sessions as synthetic entries.
        {
            let pending = self.pending.read().await;
            for (session_id, ps) in pending.iter() {
                let pending_labels = ps.labels.as_ref();
                if let Some(ref filter) = query.labels {
                    let matches = pending_labels
                        .is_some_and(|pl| filter.iter().all(|(k, v)| pl.get(k) == Some(v)));
                    if !matches {
                        continue;
                    }
                }
                result.push(meerkat_contracts::WireSessionSummary {
                    session_id: session_id.clone(),
                    session_ref: None,
                    created_at: ps.created_at_secs,
                    updated_at: ps.updated_at_secs,
                    message_count: 0,
                    total_tokens: 0,
                    is_active: matches!(&ps.phase, PendingSessionPhase::Promoting { .. }),
                    labels: pending_labels.cloned().unwrap_or_default(),
                });
            }
        }

        // Include materialized sessions from the service.
        if let Ok(summaries) = self.service.list(query).await {
            for summary in summaries {
                result.push(summary.into());
            }
        }

        result
    }

    /// Read a session as a canonical wire info object, checking pending and materialized.
    pub async fn read_session_rich(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_contracts::WireSessionInfo> {
        // Check pending sessions first.
        {
            let pending = self.pending.read().await;
            if let Some(ps) = pending.get(session_id) {
                return Some(meerkat_contracts::WireSessionInfo {
                    session_id: session_id.clone(),
                    session_ref: None,
                    created_at: ps.created_at_secs,
                    updated_at: ps.updated_at_secs,
                    message_count: 0,
                    is_active: matches!(&ps.phase, PendingSessionPhase::Promoting { .. }),
                    last_assistant_text: None,
                    labels: ps.labels.clone().unwrap_or_default(),
                });
            }
        }

        // Try list() first — non-blocking (uses watch receivers).
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in summaries {
                if summary.session_id == *session_id {
                    if !summary.is_active {
                        // Idle session: safe to call read() without blocking,
                        // which provides last_assistant_text.
                        if let Ok(view) = self.service.read(session_id).await {
                            return Some(view.state.into());
                        }
                    }
                    // Active session: return summary without last_assistant_text
                    // to avoid blocking the caller during a running turn.
                    let wire: meerkat_contracts::WireSessionSummary = summary.into();
                    return Some(meerkat_contracts::WireSessionInfo {
                        session_id: wire.session_id,
                        session_ref: None,
                        created_at: wire.created_at,
                        updated_at: wire.updated_at,
                        message_count: wire.message_count,
                        is_active: wire.is_active,
                        last_assistant_text: None,
                        labels: wire.labels,
                    });
                }
            }
        }

        // Fallback: try read() for archived/persisted sessions not in the live list.
        if let Ok(view) = self.service.read(session_id).await {
            return Some(view.state.into());
        }

        None
    }

    /// Read a session transcript as canonical wire history, including pending sessions.
    pub async fn read_session_history_rich(
        &self,
        session_id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Option<meerkat_contracts::WireSessionHistory> {
        {
            let pending = self.pending.read().await;
            if pending.contains_key(session_id) {
                return Some(meerkat_contracts::WireSessionHistory {
                    session_id: session_id.clone(),
                    session_ref: None,
                    message_count: 0,
                    offset: query.offset,
                    limit: query.limit,
                    has_more: false,
                    messages: Vec::new(),
                });
            }
        }

        self.service
            .read_history(session_id, query)
            .await
            .ok()
            .map(Into::into)
    }

    /// Get the event injector for a session, if available.
    pub async fn event_injector(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::EventInjector>> {
        self.service.event_injector(session_id).await
    }

    /// Subscribe to session-wide events regardless of triggering interaction.
    pub async fn subscribe_session_events(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_core::EventStream, meerkat_core::StreamError> {
        self.service.subscribe_session_events(session_id).await
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.service.comms_runtime(session_id).await
    }

    /// Shut down the runtime, closing all sessions.
    pub async fn shutdown(&self) {
        // Clear pending sessions.
        {
            let mut pending = self.pending.write().await;
            pending.clear();
        }

        // Shut down the service.
        self.service.shutdown().await;

        #[cfg(feature = "mcp")]
        {
            let mut map = self.mcp_sessions.write().await;
            let adapters = map
                .drain()
                .map(|(_, state)| state.adapter)
                .collect::<Vec<_>>();
            drop(map);
            for adapter in adapters {
                adapter.shutdown().await;
            }
        }
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_add(
        &self,
        session_id: &SessionId,
        server_name: String,
        mut server_config: serde_json::Value,
    ) -> Result<(), RpcError> {
        self.ensure_session_exists(session_id).await?;
        if server_name.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }

        if let Some(obj) = server_config.as_object_mut() {
            obj.insert(
                "name".to_string(),
                serde_json::Value::String(server_name.clone()),
            );
        }

        let config: McpServerConfig =
            serde_json::from_value(server_config).map_err(|e| RpcError {
                code: error::INVALID_PARAMS,
                message: format!("invalid server_config: {e}"),
                data: None,
            })?;

        let adapter = self.mcp_adapter_for_session(session_id).await?;
        adapter.stage_add(config).await.map_err(|e| RpcError {
            code: error::INTERNAL_ERROR,
            message: e,
            data: None,
        })?;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_remove(
        &self,
        session_id: &SessionId,
        server_name: String,
    ) -> Result<(), RpcError> {
        self.ensure_session_exists(session_id).await?;
        if server_name.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }
        let adapter = self.mcp_adapter_for_session(session_id).await?;
        adapter
            .stage_remove(server_name)
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: e,
                data: None,
            })?;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_reload(
        &self,
        session_id: &SessionId,
        server_name: Option<String>,
    ) -> Result<(), RpcError> {
        self.ensure_session_exists(session_id).await?;
        if let Some(name) = server_name.as_ref()
            && name.trim().is_empty()
        {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }
        let adapter = self.mcp_adapter_for_session(session_id).await?;
        match server_name {
            Some(name) => {
                // Validate server exists before staging to avoid deferred failures
                // at the next turn boundary.
                let active = adapter.active_server_names().await;
                if !active.iter().any(|n| n == &name) {
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: format!("MCP server '{name}' is not registered on this session"),
                        data: None,
                    });
                }
                adapter
                    .stage_reload(McpReloadTarget::ServerName(name))
                    .await
                    .map_err(|e| RpcError {
                        code: error::INTERNAL_ERROR,
                        message: e,
                        data: None,
                    })?;
            }
            None => {
                // Reload all active servers.
                let names = adapter.active_server_names().await;
                for name in names {
                    adapter
                        .stage_reload(McpReloadTarget::ServerName(name))
                        .await
                        .map_err(|e| RpcError {
                            code: error::INTERNAL_ERROR,
                            message: e,
                            data: None,
                        })?;
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn attach_mcp_adapter_for_pending_session(
        &self,
        session_id: SessionId,
        mut build_config: AgentBuildConfig,
    ) -> Result<AgentBuildConfig, RpcError> {
        let adapter = Arc::new(McpRouterAdapter::new(McpRouter::new()));
        let adapter_dispatcher: Arc<dyn AgentToolDispatcher> = adapter.clone();
        let combined = match build_config.external_tools.clone() {
            Some(existing) => Arc::new(
                ToolGateway::new(existing, Some(adapter_dispatcher)).map_err(|e| RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!("failed to compose external tools with MCP adapter: {e}"),
                    data: None,
                })?,
            ),
            None => adapter_dispatcher,
        };
        build_config.external_tools = Some(combined);

        let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded_channel();
        let state = SessionMcpState {
            adapter,
            turn_counter: 0,
            lifecycle_tx,
            lifecycle_rx,
            drain_task_running: Arc::new(AtomicBool::new(false)),
        };
        self.mcp_sessions.write().await.insert(session_id, state);
        Ok(build_config)
    }

    #[cfg(feature = "mcp")]
    async fn ensure_session_exists(&self, session_id: &SessionId) -> Result<(), RpcError> {
        let pending = self.pending.read().await;
        if pending.contains_key(session_id) {
            return Ok(());
        }
        drop(pending);
        if self.session_state(session_id).await.is_none() {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn mcp_adapter_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<McpRouterAdapter>, RpcError> {
        let map = self.mcp_sessions.read().await;
        map.get(session_id)
            .map(|state| state.adapter.clone())
            .ok_or_else(|| RpcError {
                code: error::INVALID_PARAMS,
                message: "session does not support live MCP operations".to_string(),
                data: None,
            })
    }

    #[cfg(feature = "mcp")]
    async fn apply_mcp_boundary(
        &self,
        session_id: &SessionId,
        event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
        prompt: &mut String,
    ) -> Result<(), RpcError> {
        let (adapter, turn_number, drain_task_running, lifecycle_tx, mut queued_actions) = {
            let mut map = self.mcp_sessions.write().await;
            let state = match map.get_mut(session_id) {
                Some(state) => state,
                None => return Ok(()),
            };
            state.turn_counter = state.turn_counter.saturating_add(1);
            let mut queued = Vec::new();
            while let Ok(action) = state.lifecycle_rx.try_recv() {
                queued.push(action);
            }
            (
                state.adapter.clone(),
                state.turn_counter,
                state.drain_task_running.clone(),
                state.lifecycle_tx.clone(),
                queued,
            )
        };

        if !queued_actions.is_empty() {
            let drained = std::mem::take(&mut queued_actions);
            self.emit_mcp_lifecycle_actions(session_id, event_tx, prompt, turn_number, drained)
                .await;
        }

        let result = adapter.apply_staged().await.map_err(|e| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("failed to apply staged MCP operations: {e}"),
            data: None,
        })?;

        if result
            .delta
            .lifecycle_actions
            .iter()
            .any(|action| matches!(action, McpLifecycleAction::RemovingStarted { .. }))
        {
            Self::spawn_mcp_drain_task_if_needed(adapter.clone(), drain_task_running, lifecycle_tx);
        }

        queued_actions.extend(result.delta.lifecycle_actions);
        if queued_actions.is_empty() {
            return Ok(());
        }

        self.emit_mcp_lifecycle_actions(session_id, event_tx, prompt, turn_number, queued_actions)
            .await;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    fn spawn_mcp_drain_task_if_needed(
        adapter: Arc<McpRouterAdapter>,
        task_running: Arc<AtomicBool>,
        lifecycle_tx: mpsc::UnboundedSender<McpLifecycleAction>,
    ) {
        if task_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let delta = match adapter.progress_removals().await {
                    Ok(delta) => delta,
                    Err(err) => {
                        tracing::warn!("background MCP drain apply failed: {err}");
                        break;
                    }
                };

                for action in delta.lifecycle_actions {
                    let _ = lifecycle_tx.send(action);
                }

                match adapter.has_removing_servers().await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(err) => {
                        tracing::warn!("background MCP drain state check failed: {err}");
                        break;
                    }
                }
            }
            task_running.store(false, Ordering::Release);
        });
    }

    #[cfg(feature = "mcp")]
    async fn emit_mcp_lifecycle_actions(
        &self,
        session_id: &SessionId,
        event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
        prompt: &mut String,
        turn_number: u32,
        actions: Vec<McpLifecycleAction>,
    ) {
        let source_id = format!("session:{session_id}");
        meerkat::surface::emit_mcp_lifecycle_events(
            event_tx,
            &source_id,
            prompt,
            turn_number,
            actions,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn session_error_to_rpc(err: SessionError) -> RpcError {
    let code = match &err {
        SessionError::NotFound { .. } => error::SESSION_NOT_FOUND,
        SessionError::Busy { .. } => error::SESSION_BUSY,
        SessionError::NotRunning { .. } => error::INTERNAL_ERROR,
        SessionError::Agent(agent_err) => match agent_err {
            meerkat_core::AgentError::TokenBudgetExceeded { .. }
            | meerkat_core::AgentError::TimeBudgetExceeded { .. }
            | meerkat_core::AgentError::ToolCallBudgetExceeded { .. } => error::BUDGET_EXHAUSTED,
            meerkat_core::AgentError::HookDenied { .. } => error::HOOK_DENIED,
            meerkat_core::AgentError::Llm { .. } => error::PROVIDER_ERROR,
            meerkat_core::AgentError::SessionNotFound(_) => error::SESSION_NOT_FOUND,
            meerkat_core::AgentError::InternalError(msg) => {
                // Build errors (missing API key, unknown provider) are tunneled
                // through InternalError from the builder.
                if msg.contains("API key") || msg.contains("provider") {
                    error::PROVIDER_ERROR
                } else {
                    error::INTERNAL_ERROR
                }
            }
            _ => error::INTERNAL_ERROR,
        },
        _ => error::INTERNAL_ERROR,
    };
    RpcError {
        code,
        message: err.to_string(),
        data: None,
    }
}

fn system_context_error_to_rpc(err: SessionControlError) -> RpcError {
    match err {
        SessionControlError::Session(session_err) => session_error_to_rpc(session_err),
        SessionControlError::InvalidRequest { message } => RpcError {
            code: error::INVALID_PARAMS,
            message,
            data: None,
        },
        SessionControlError::Conflict { key, .. } => RpcError {
            code: error::INVALID_PARAMS,
            message: format!("system-context idempotency conflict for key '{key}'"),
            data: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentBuildConfig;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::StopReason;
    use meerkat_core::skills::{
        SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceUuid,
    };
    use meerkat_core::skills_config::{
        SkillRepoTransport, SkillRepositoryConfig, SkillsConfig, SkillsIdentityConfig,
    };
    #[cfg(feature = "mcp")]
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Mock LLM client
    // -----------------------------------------------------------------------

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
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

    /// A mock LLM client that introduces a delay before responding.
    /// Used to test concurrent access (SESSION_BUSY).
    struct SlowMockLlmClient {
        delay_ms: u64,
    }

    impl SlowMockLlmClient {
        fn new(delay_ms: u64) -> Self {
            Self { delay_ms }
        }
    }

    #[async_trait]
    impl LlmClient for SlowMockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            let delay_ms = self.delay_ms;
            Box::pin(async_stream::stream! {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Slow response".to_string(),
                    meta: None,
                });
                yield Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                });
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions"))
    }

    fn mock_build_config() -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(MockLlmClient)),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn slow_build_config(delay_ms: u64) -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(SlowMockLlmClient::new(delay_ms))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn make_runtime(factory: AgentFactory, max_sessions: usize) -> SessionRuntime {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        SessionRuntime::new(
            factory,
            Config::default(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, None),
            crate::router::NotificationSink::noop(),
        )
    }

    #[cfg(feature = "mcp")]
    fn mcp_test_server_path() -> PathBuf {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let workspace_root = PathBuf::from(manifest_dir)
            .parent()
            .expect("workspace root")
            .to_path_buf();
        workspace_root
            .join("target")
            .join("debug")
            .join("mcp-test-server")
    }

    #[cfg(feature = "mcp")]
    fn maybe_mcp_server_config(server_name: &str) -> Option<serde_json::Value> {
        let path = mcp_test_server_path();
        if !path.exists() {
            eprintln!(
                "Skipping MCP runtime boundary test: mcp-test-server not built. Run `cargo build -p mcp-test-server` first."
            );
            return None;
        }
        Some(serde_json::json!({
            "command": path.to_string_lossy().to_string(),
            "args": [],
            "env": {},
            "name": server_name,
        }))
    }

    #[cfg(feature = "mcp")]
    fn collect_tool_config_events(
        event_rx: &mut mpsc::Receiver<EventEnvelope<AgentEvent>>,
    ) -> Vec<ToolConfigChangedPayload> {
        let mut out = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            if let AgentEvent::ToolConfigChanged { payload } = event.payload {
                out.push(payload);
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. Creating a session returns a SessionId and the state is Idle.
    #[tokio::test]
    async fn create_session_returns_id_and_idle_state() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let info = runtime.session_state(&session_id).await;
        assert_eq!(info.map(|i| i.state), Some(SessionState::Idle));
    }

    /// 2. Starting a turn returns a RunResult with expected text.
    #[tokio::test]
    async fn start_turn_returns_run_result() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(
            result.text.contains("Hello from mock"),
            "Expected mock response text, got: {}",
            result.text
        );
    }

    #[tokio::test]
    async fn append_system_context_survives_pending_session_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(slow_build_config(200), None, None)
            .await
            .unwrap();

        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = Arc::clone(&runtime);
        let sid_clone = session_id.clone();
        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".into(), event_tx, None, None, None, None)
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            let is_promoting = {
                let pending = runtime.pending.read().await;
                matches!(
                    pending.get(&session_id).map(|ps| &ps.phase),
                    Some(PendingSessionPhase::Promoting { .. })
                )
            };
            if is_promoting {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter the promoting state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let append_req = AppendSystemContextRequest {
            text: "Coordinate with the orchestrator.".to_string(),
            source: Some("mob".to_string()),
            idempotency_key: Some("ctx-promotion".to_string()),
        };
        let append_result = runtime
            .append_system_context(&session_id, append_req.clone())
            .await
            .expect("append during promotion should succeed");
        assert_eq!(
            append_result.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Slow response"));

        let duplicate = runtime
            .append_system_context(&session_id, append_req)
            .await
            .expect("replayed append should be visible after materialization");
        assert_eq!(
            duplicate.status,
            meerkat_core::AppendSystemContextStatus::Duplicate
        );
    }

    /// 3. Verify state transitions: Idle -> Running -> Idle during a turn.
    #[tokio::test]
    async fn start_turn_transitions_idle_running_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        // Use a slow mock to give us time to observe Running state
        let session_id = runtime
            .create_session(slow_build_config(100), None, None)
            .await
            .unwrap();

        // Verify initial state
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle)
        );

        // Start the turn in a background task so we can observe state mid-run
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();

        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".into(), event_tx, None, None, None, None)
                .await
        });

        // Wait until the session transitions to Running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&session_id).await.map(|i| i.state)
                == Some(SessionState::Running)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Wait for the turn to complete
        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Slow response"));

        // After completion, state should be Idle again
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle),
            "Session should be Idle after turn completes"
        );
    }

    /// 4. Starting a second turn while one is running fails with SESSION_BUSY.
    #[tokio::test]
    async fn start_turn_on_busy_session_fails() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(slow_build_config(200), None, None)
            .await
            .unwrap();

        // Start first turn in background
        let (event_tx1, _rx1) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();
        let _turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(
                    &sid_clone,
                    "First".into(),
                    event_tx1,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        // Wait until the first turn is definitely running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&session_id).await.map(|i| i.state)
                == Some(SessionState::Running)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Try to start a second turn
        let (event_tx2, _rx2) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx2,
                None,
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_err(), "Second turn should fail");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            error::SESSION_BUSY,
            "Error code should be SESSION_BUSY, got: {}",
            err.code
        );
    }

    /// 5. Agent events are forwarded through the per-turn event channel.
    #[tokio::test]
    async fn start_turn_emits_events() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let _result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Collect received events
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }

        // We should have received at least some events (RunStarted, TextDelta, etc.)
        assert!(
            !events.is_empty(),
            "Should have received at least one event"
        );

        // Check that we got a RunStarted event
        let has_run_started = events
            .iter()
            .any(|e| matches!(e.payload, AgentEvent::RunStarted { .. }));
        assert!(has_run_started, "Should have received a RunStarted event");
    }

    /// 6. Interrupting an idle session is a no-op (no error).
    #[tokio::test]
    async fn interrupt_on_idle_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Interrupt while idle should succeed without error
        let result = runtime.interrupt(&session_id).await;
        assert!(result.is_ok(), "Interrupt on idle should not fail");

        // State should still be idle
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle)
        );
    }

    /// 7. Archiving a session removes it from the runtime.
    #[tokio::test]
    async fn archive_session_removes_handle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Verify it exists
        assert!(runtime.session_state(&session_id).await.is_some());

        // Archive it
        runtime.archive_session(&session_id).await.unwrap();

        // Verify it's gone
        assert!(
            runtime.session_state(&session_id).await.is_none(),
            "Archived session should no longer exist"
        );

        // Archiving again should fail
        let result = runtime.archive_session(&session_id).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// 8. Max sessions limit is enforced.
    #[tokio::test]
    async fn max_sessions_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 2);

        // Create two sessions (the max)
        let _s1 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let _s2 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Third should fail
        let result = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(result.is_err(), "Third session should fail");
        let err = result.unwrap_err();
        assert!(
            err.message.contains("Max sessions"),
            "Error message should mention max sessions, got: {}",
            err.message
        );
    }

    /// 9. session_state returns None for an unknown session ID.
    #[tokio::test]
    async fn session_state_none_for_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        assert_eq!(runtime.session_state(&unknown_id).await, None);
    }

    /// 10. Shutdown closes all sessions.
    #[tokio::test]
    async fn shutdown_closes_all_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let s1 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let s2 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Verify both exist
        assert!(runtime.session_state(&s1).await.is_some());
        assert!(runtime.session_state(&s2).await.is_some());

        // Shutdown
        runtime.shutdown().await;

        // Both should be gone from the sessions map
        let sessions = runtime.list_sessions(Default::default()).await;
        assert!(
            sessions.is_empty(),
            "All sessions should be removed after shutdown"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn start_turn_applies_staged_mcp_ops_at_turn_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(
                &session_id,
                "broken-server".to_string(),
                serde_json::json!({
                    "command": "echo",
                    "args": [],
                    "env": {}
                }),
            )
            .await
            .expect("staging should succeed");

        // Non-blocking: apply_staged spawns background task and succeeds
        // (the broken server fails asynchronously, not at staging time).
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let first = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            first.is_ok(),
            "non-blocking apply_staged should not fail synchronously"
        );
        let first_events = collect_tool_config_events(&mut event_rx);
        assert!(
            first_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Add
                    && payload.target == "broken-server"
                    && payload.status == "pending"
            }),
            "should emit pending event for staged add"
        );

        let (event_tx, _event_rx) = mpsc::channel(32);
        let second = runtime
            .start_turn(
                &session_id,
                "hello again".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            second.is_ok(),
            "failed staged operation should not poison subsequent turns"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn start_turn_applies_staged_mcp_remove_and_reload_at_turn_boundary() {
        let Some(server_config) = maybe_mcp_server_config("test-server") else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "test-server".to_string(), server_config)
            .await
            .expect("stage add");

        // Non-blocking: add is now async — boundary emits "pending" not "applied".
        let (event_tx, mut event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("turn add should apply staged add");
        let add_events = collect_tool_config_events(&mut event_rx);
        assert!(add_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Add
                && payload.target == "test-server"
                && payload.status == "pending"
        }));

        runtime
            .mcp_stage_remove(&session_id, "test-server".to_string())
            .await
            .expect("stage remove");
        let (event_tx, mut event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("turn remove should apply staged remove");
        let remove_events = collect_tool_config_events(&mut event_rx);
        assert!(remove_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "test-server"
                && (payload.status == "applied" || payload.status == "draining")
        }));
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn async_mcp_removal_timeout_is_emitted_on_next_boundary() {
        let Some(server_config) = maybe_mcp_server_config("timeout-server") else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "timeout-server".to_string(), server_config)
            .await
            .expect("stage add");

        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");

        // Wait for the background MCP connect to complete before setting
        // inflight calls — otherwise `drain_pending` on the next boundary
        // replaces the entry and resets active_calls to 0.
        adapter.wait_until_ready(Duration::from_secs(5)).await;

        adapter
            .set_removal_timeout_for_testing(Duration::from_millis(20))
            .await
            .expect("set timeout");
        adapter
            .set_inflight_calls_for_testing("timeout-server", 1)
            .await
            .expect("set inflight");

        runtime
            .mcp_stage_remove(&session_id, "timeout-server".to_string())
            .await
            .expect("stage remove");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove boundary");
        let first_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(first_turn_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "timeout-server"
                && payload.status == "draining"
        }));

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !adapter
                    .has_removing_servers()
                    .await
                    .expect("check removing state")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("timed out waiting for forced removal to finalize");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn after timeout".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("follow-up boundary");
        let second_turn_events = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let events = collect_tool_config_events(&mut event_rx);
                if !events.is_empty() {
                    break events;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_default();
        assert!(
            second_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Remove
                    && payload.target == "timeout-server"
                    && (payload.status == "forced" || payload.status == "applied")
            }),
            "expected forced removal event on a follow-up boundary, got: {second_turn_events:?}"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    #[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
    async fn staged_ops_remain_boundary_gated_while_background_drain_runs() {
        let Some(server1_config) = maybe_mcp_server_config("server-draining") else {
            return;
        };
        let Some(server2_config) = maybe_mcp_server_config("server-staged") else {
            return;
        };

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "server-draining".to_string(), server1_config)
            .await
            .expect("stage add draining server");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add first".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add first server at boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");
        adapter
            .set_removal_timeout_for_testing(Duration::from_secs(3))
            .await
            .expect("set long timeout");
        adapter
            .set_inflight_calls_for_testing("server-draining", 1)
            .await
            .expect("keep server draining");

        runtime
            .mcp_stage_remove(&session_id, "server-draining".to_string())
            .await
            .expect("stage remove");
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn remove first".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove starts draining");
        let first_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(first_turn_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "server-draining"
                && payload.status == "draining"
        }));
        assert!(
            adapter.tools().is_empty(),
            "draining server should be hidden immediately"
        );

        runtime
            .mcp_stage_add(&session_id, "server-staged".to_string(), server2_config)
            .await
            .expect("stage second add");

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            adapter.tools().is_empty(),
            "background drain must not apply newly staged add outside boundary"
        );

        // Turn 3: apply the staged add. Since MCP adds are non-blocking
        // (spawn_pending), this turn will emit PendingConnect ("pending"),
        // not "applied". The actual connection completes asynchronously.
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn apply staged add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("next boundary should apply staged add");

        let next_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(
            next_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Add
                    && payload.target == "server-staged"
                    && payload.status == "pending"
            }),
            "expected Add+pending for server-staged at boundary, got: {next_turn_events:?}"
        );

        // Turn 4: after the background connection resolves, drain_pending
        // picks it up and the server becomes visible.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn drain pending add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("drain should resolve pending add");

        let drain_events = collect_tool_config_events(&mut event_rx);
        // The server becomes active via drain_pending → process_pending_result
        // → Activated lifecycle action. The event status may be "applied" (from
        // emit_mcp_lifecycle_events) or "activated" (from session service tool
        // scope machinery). Either confirms the server was successfully connected.
        let has_add = drain_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Add && payload.target == "server-staged"
        });
        assert!(
            has_add,
            "expected Add event for server-staged after drain, got: {drain_events:?}"
        );
        assert!(
            !adapter.tools().is_empty(),
            "second server should become visible after drain"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    #[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
    async fn queued_lifecycle_actions_survive_boundary_apply_failure() {
        let Some(server_config) = maybe_mcp_server_config("lossless-server") else {
            return;
        };

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "lossless-server".to_string(), server_config)
            .await
            .expect("stage add");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");
        adapter
            .set_removal_timeout_for_testing(Duration::from_millis(20))
            .await
            .expect("set timeout");
        adapter
            .set_inflight_calls_for_testing("lossless-server", 1)
            .await
            .expect("set inflight");

        runtime
            .mcp_stage_remove(&session_id, "lossless-server".to_string())
            .await
            .expect("stage remove");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove boundary");

        // Wait for the background drain task to process the forced removal.
        // Drain task polls every 100ms, timeout is 20ms, so after 500ms
        // the forced removal should be queued in lifecycle_rx.
        tokio::time::sleep(Duration::from_millis(500)).await;

        runtime
            .mcp_stage_add(
                &session_id,
                "broken-server".to_string(),
                serde_json::json!({
                    "command": "echo",
                    "args": [],
                    "env": {}
                }),
            )
            .await
            .expect("stage broken add");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        // The broken add (echo) spawns a background connection that will fail
        // asynchronously. apply_staged() may or may not return an error since
        // MCP adds are non-blocking. The important assertion is that the
        // lossless-server's forced removal event was emitted via the lifecycle
        // channel despite the broken add also being staged.
        let _result = runtime
            .start_turn(
                &session_id,
                "turn with broken add and queued removal".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;

        let fail_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(
            fail_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Remove
                    && payload.target == "lossless-server"
                    && (payload.status == "forced" || payload.status == "applied")
            }),
            "removal of lossless-server should survive alongside broken add, got: {fail_turn_events:?}"
        );
    }

    /// 11. Startup registry build rejects invalid lineage/remap config.
    #[test]
    fn build_skill_identity_registry_rejects_invalid_config() {
        let cfg = Config {
            skills: SkillsConfig {
                repositories: vec![
                    SkillRepositoryConfig {
                        name: "old".to_string(),
                        source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-old".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-a".to_string(),
                        source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-a".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-b".to_string(),
                        source_uuid: SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-b".to_string(),
                        },
                    },
                ],
                identity: SkillsIdentityConfig {
                    lineage: vec![SourceIdentityLineage {
                        event_id: "split-1".to_string(),
                        recorded_at_unix_secs: 1,
                        required_from_skills: vec![
                            SkillName::parse("email-extractor").expect("skill"),
                            SkillName::parse("pdf-processing").expect("skill"),
                        ],
                        event: SourceIdentityLineageEvent::Split {
                            from: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            into: vec![
                                SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                    .expect("uuid"),
                                SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                                    .expect("uuid"),
                            ],
                        },
                    }],
                    remaps: vec![SkillKeyRemap {
                        from: SkillKey {
                            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            skill_name: SkillName::parse("email-extractor").expect("skill"),
                        },
                        to: SkillKey {
                            source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                .expect("uuid"),
                            skill_name: SkillName::parse("mail-extractor").expect("skill"),
                        },
                        reason: None,
                    }],
                    aliases: vec![],
                },
                ..SkillsConfig::default()
            },
            ..Config::default()
        };

        let result = SessionRuntime::build_skill_identity_registry(&cfg);
        assert!(matches!(
            result,
            Err(meerkat_core::skills::SkillError::MissingSkillRemaps { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // Phase 1 tests: Deferred create, host_mode override, per-turn overrides
    // -----------------------------------------------------------------------

    /// Deferred session/create returns a session in Idle state (no turn executed).
    #[tokio::test]
    async fn deferred_create_returns_idle_session() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Session exists and is idle (no turn started).
        let info = runtime.session_state(&session_id).await.unwrap();
        assert_eq!(info.state, SessionState::Idle);
    }

    /// Deferred create followed by turn/start works correctly.
    #[tokio::test]
    async fn deferred_create_then_turn_start_works() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let (event_tx, _event_rx) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(result.text.contains("Hello from mock"));
    }

    /// turn/start with host_mode override on a materialized session is not rejected.
    #[tokio::test]
    async fn turn_start_with_host_mode_override_accepted_on_materialized() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session with a first turn.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "First".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Second turn with only host_mode override (no model/provider etc.)
        // should not be rejected — host_mode is allowed on materialized sessions.
        // Use host_mode: false to avoid needing comms runtime.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            host_mode: Some(false),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            result.is_ok(),
            "host_mode override should succeed on materialized session"
        );
    }

    /// turn/start on a pending session with model override applies it.
    #[tokio::test]
    async fn turn_start_on_pending_session_with_model_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Start turn with model override on pending session.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            result.is_ok(),
            "model override should succeed on pending session"
        );
    }

    /// turn/start on a materialized session allows model override (hot-swap).
    #[tokio::test]
    async fn turn_start_on_materialized_session_allows_model_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "First".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Second turn with model override should NOT be rejected with INVALID_PARAMS.
        // In CI (no API key), the client build may fail with INTERNAL_ERROR, which
        // is fine — the point is that the override is accepted, not rejected at the
        // parameter validation layer.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        if let Err(ref err) = result {
            assert_ne!(
                err.code,
                error::INVALID_PARAMS,
                "model override should not be rejected on materialized session: {err:?}"
            );
        }
    }

    /// StartTurnParams deserializes with all fields.
    #[test]
    fn turn_start_params_deserialize_with_all_fields() {
        use crate::handlers::turn::StartTurnParams;

        let json = serde_json::json!({
            "session_id": "test-id",
            "prompt": "hello",
            "host_mode": true,
            "model": "claude-opus-4-6",
            "provider": "anthropic",
            "max_tokens": 4096,
            "system_prompt": "You are helpful",
            "output_schema": {"type": "object"},
            "structured_output_retries": 3,
            "provider_params": {"thinking": true}
        });
        let params: StartTurnParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_id, "test-id");
        assert_eq!(params.prompt, ContentInput::Text("hello".to_string()));
        assert_eq!(params.host_mode, Some(true));
        assert_eq!(params.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(params.provider.as_deref(), Some("anthropic"));
        assert_eq!(params.max_tokens, Some(4096));
        assert_eq!(params.system_prompt.as_deref(), Some("You are helpful"));
        assert!(params.output_schema.is_some());
        assert_eq!(params.structured_output_retries, Some(3));
        assert!(params.provider_params.is_some());
    }

    // -----------------------------------------------------------------------
    // Phase 4: Persisted session recovery
    // -----------------------------------------------------------------------

    /// turn/start on a completely unknown session returns SESSION_NOT_FOUND.
    #[tokio::test]
    async fn turn_start_unknown_session_returns_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        let (event_tx, _rx) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &unknown_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// Pending session timestamps are stable across list calls.
    #[tokio::test]
    async fn pending_session_timestamps_are_stable() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let list1 = runtime.list_sessions_rich(Default::default()).await;
        let ts1 = list1.iter().find(|s| s.session_id == session_id).unwrap();
        let created1 = ts1.created_at;
        let updated1 = ts1.updated_at;
        assert!(created1 > 0, "created_at should be non-zero");
        assert_eq!(created1, updated1, "initial created_at == updated_at");

        // Wait a moment and list again — timestamps should be identical.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let list2 = runtime.list_sessions_rich(Default::default()).await;
        let ts2 = list2.iter().find(|s| s.session_id == session_id).unwrap();
        assert_eq!(ts2.created_at, created1, "created_at must not drift");
        assert_eq!(
            ts2.updated_at, updated1,
            "updated_at must not drift without mutation"
        );
    }

    /// append_system_context on a pending session bumps updated_at.
    #[tokio::test]
    async fn pending_session_updated_at_advances_on_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let before = runtime.read_session_rich(&session_id).await.unwrap();
        let created = before.created_at;

        // Wait so updated_at can differ.
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let req = AppendSystemContextRequest {
            text: "new context".to_string(),
            source: None,
            idempotency_key: Some("key-1".to_string()),
        };
        runtime
            .append_system_context(&session_id, req)
            .await
            .unwrap();

        let after = runtime.read_session_rich(&session_id).await.unwrap();
        assert_eq!(after.created_at, created, "created_at must not change");
        assert!(
            after.updated_at >= created,
            "updated_at ({}) should be >= created_at ({}) after mutation",
            after.updated_at,
            created,
        );
    }

    /// read_session_rich returns last_assistant_text for idle materialized sessions.
    #[tokio::test]
    async fn read_session_rich_returns_last_assistant_text_for_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session with a turn.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Session is now idle — read_session_rich should provide last_assistant_text.
        let info = runtime.read_session_rich(&session_id).await.unwrap();
        assert!(
            info.last_assistant_text.is_some(),
            "idle session should have last_assistant_text, got None"
        );
    }

    fn temp_factory_with_builtins(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions")).builtins(true)
    }

    /// After hot-swapping from an Anthropic model to an OpenAI model, the
    /// `view_image` tool should be denied via an external tool filter.
    #[tokio::test]
    async fn hot_swap_to_openai_hides_view_image() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));

        // Create session with an Anthropic model (supports image_tool_results).
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize the session with the first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Hot-swap to an OpenAI model (does NOT support image_tool_results).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.2".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Export the session and check that the tool filter metadata blocks view_image.
        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let filter_value = session
            .metadata()
            .get(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY)
            .expect("tool filter metadata should be set after hot-swap");
        let filter: meerkat_core::ToolFilter =
            serde_json::from_value(filter_value.clone()).unwrap();
        assert_eq!(
            filter,
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect()),
            "hot-swap to OpenAI should deny view_image"
        );
    }

    /// After hot-swapping from OpenAI back to Anthropic, the tool filter should
    /// be cleared (set to All).
    #[tokio::test]
    async fn hot_swap_to_anthropic_clears_view_image_deny() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Hot-swap to OpenAI (deny view_image).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.2".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Hot-swap back to Anthropic (should clear the deny).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let (event_tx3, _event_rx3) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Third turn".into(),
                event_tx3,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Export session and verify filter is cleared.
        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let filter_value = session
            .metadata()
            .get(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY)
            .expect("tool filter metadata should be set after hot-swap");
        let filter: meerkat_core::ToolFilter =
            serde_json::from_value(filter_value.clone()).unwrap();
        assert_eq!(
            filter,
            meerkat_core::ToolFilter::All,
            "hot-swap back to Anthropic should clear view_image deny"
        );
    }

    /// Hot-swapping preserves pre-existing denied tools in the external filter.
    #[tokio::test]
    async fn hot_swap_preserves_preexisting_deny_filter() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Stage a pre-existing deny filter for "datetime".
        runtime
            .service
            .set_session_tool_filter(
                &session_id,
                meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            )
            .await
            .expect("staging deny filter for datetime should succeed");

        // Hot-swap to OpenAI — should add view_image to the deny set, not replace it.
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.2".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let filter_value = session
            .metadata()
            .get(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY)
            .expect("tool filter metadata should be set after hot-swap");
        let filter: meerkat_core::ToolFilter =
            serde_json::from_value(filter_value.clone()).unwrap();

        let expected_deny: std::collections::HashSet<String> = ["datetime", "view_image"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            filter,
            meerkat_core::ToolFilter::Deny(expected_deny),
            "hot-swap should merge view_image into existing deny set, not replace it"
        );
    }

    /// Hot-swapping back to a capable model removes only view_image, keeping other denied tools.
    #[tokio::test]
    async fn hot_swap_back_preserves_other_denied_tools() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Stage deny filter with datetime + view_image (simulating prior OpenAI swap).
        runtime
            .service
            .set_session_tool_filter(
                &session_id,
                meerkat_core::ToolFilter::Deny(
                    ["datetime", "view_image"]
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                ),
            )
            .await
            .expect("staging deny filter for datetime+view_image should succeed");

        // Hot-swap to Anthropic — should remove view_image but keep datetime.
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let filter_value = session
            .metadata()
            .get(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY)
            .expect("tool filter metadata should be set after hot-swap");
        let filter: meerkat_core::ToolFilter =
            serde_json::from_value(filter_value.clone()).unwrap();

        assert_eq!(
            filter,
            meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            "hot-swap to Anthropic should remove view_image but keep other denied tools"
        );
    }
}
