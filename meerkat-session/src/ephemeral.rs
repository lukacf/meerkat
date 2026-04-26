//! EphemeralSessionService — in-memory session lifecycle with no persistence.
//!
//! Each session gets a dedicated tokio task that exclusively owns the `Agent`.
//! Communication happens through channels, generalized from `SessionRuntime` in
//! `meerkat-rpc`.

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::error::AgentError;
use meerkat_core::event::{AgentEvent, EventEnvelope};
use meerkat_core::image_content::{MissingBlobBehavior, hydrate_deferred_turn_state};
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreApplyTerminal};
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    DeferredPromptPolicy, MobToolAuthorityContext, SessionControlError, SessionError,
    SessionHistoryPage, SessionHistoryQuery, SessionInfo, SessionQuery, SessionService,
    SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary,
    SessionUsage, SessionView, StageToolResultsRequest, StageToolResultsResult, StartTurnRequest,
    TurnToolOverlay,
};
use meerkat_core::time_compat::SystemTime;
use meerkat_core::types::{ContentInput, RunResult, SessionId, ToolResult, Usage};
use meerkat_core::{
    InputId, PendingDeferredPrompt, PendingSystemContextAppend, PendingToolResultsMessage, RunId,
    SessionDeferredTurnState, SessionLlmIdentity, SessionSystemContextState,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Tokio re-exports: on wasm32, use the crate-level alias (tokio_with_wasm).
#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore, mpsc, oneshot, watch};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore, mpsc, oneshot, watch};

use crate::turn_admission::{TurnAdmissionPhase, TurnAdmissionSlot};

/// Capacity for the internal agent event channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Capacity for session command channel.
const COMMAND_CHANNEL_CAPACITY: usize = 8;

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Observable state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Idle,
    Admitted,
    Running,
    Completing,
    ShuttingDown,
}

/// Snapshot of session metadata for read/list operations.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub total_tokens: u64,
    pub usage: Usage,
    pub last_assistant_text: Option<String>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Commands sent from the service to a session task.
enum SessionCommand {
    StartTurn {
        prompt: meerkat_core::types::ContentInput,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        handling_mode: meerkat_core::types::HandlingMode,
        event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
        result_tx: oneshot::Sender<Result<RunResult, meerkat_core::error::AgentError>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<TurnToolOverlay>,
        execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
    },
    ReplaceClient {
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        reply_tx: oneshot::Sender<()>,
    },
    HotSwapLlmIdentity {
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    StageToolFilter {
        filter: meerkat_core::ToolFilter,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    SetToolVisibilityState {
        state: Option<meerkat_core::SessionToolVisibilityState>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    SyncSystemContextState {
        reply_tx: oneshot::Sender<()>,
    },
    /// Export the full session (messages + metadata) for persistence.
    ExportSession {
        reply_tx: oneshot::Sender<meerkat_core::Session>,
    },
    ExecutionSnapshot {
        reply_tx: oneshot::Sender<Option<meerkat_core::AgentExecutionSnapshot>>,
    },
    ToolScopeSnapshot {
        reply_tx: oneshot::Sender<Option<meerkat_core::ToolScopeSnapshot>>,
    },
    VisibleToolDefs {
        reply_tx: oneshot::Sender<Vec<meerkat_core::ToolDef>>,
    },
    ExternalToolSurfaceSnapshot {
        reply_tx: oneshot::Sender<Option<meerkat_core::ExternalToolSurfaceSnapshot>>,
    },
    ApplyRuntimeSystemContext {
        appends: Vec<PendingSystemContextAppend>,
        reply_tx: oneshot::Sender<()>,
    },
    AppendExternalUserContent {
        content: ContentInput,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    AppendExternalAssistantOutput {
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: Usage,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    DispatchExternalToolCall {
        call: meerkat_core::ToolCall,
        reply_tx: oneshot::Sender<
            Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError>,
        >,
    },
    /// Update the keep_alive flag in durable session metadata.
    UpdateKeepAlive {
        keep_alive: bool,
        reply_tx: oneshot::Sender<()>,
    },
    UpdateMobToolAuthority {
        authority_context: Option<MobToolAuthorityContext>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    UpdateSystemPrompt {
        system_prompt: String,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    Shutdown,
}

/// Lightweight summary updated after each turn, readable without querying the task.
#[derive(Clone)]
struct SessionSummaryCache {
    updated_at: SystemTime,
    message_count: usize,
    total_tokens: u64,
    usage: Usage,
    last_assistant_text: Option<String>,
}

/// Handle stored in the sessions map.
struct SessionHandle {
    command_tx: mpsc::Sender<SessionCommand>,
    state_tx: watch::Sender<SessionState>,
    state_rx: watch::Receiver<SessionState>,
    summary_rx: watch::Receiver<SessionSummaryCache>,
    llm_identity_rx: watch::Receiver<SessionLlmIdentity>,
    /// Canonical owner for session turn admission lifecycle.
    turn_admission: Arc<std::sync::Mutex<TurnAdmissionSlot>>,
    _capacity_permit: OwnedSemaphorePermit,
    created_at: SystemTime,
    /// Key-value labels attached at session creation.
    labels: BTreeMap<String, String>,
    /// Public event injector for pushing external events.
    /// Extracted from the agent before it moves into its task.
    event_injector: Option<Arc<dyn meerkat_core::EventInjector>>,
    /// Internal runtime injector for interaction-scoped streaming.
    interaction_event_injector: Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>>,
    /// Optional comms runtime for keep-alive commands and stream attachment.
    comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    /// Shared runtime control state for system-context appends.
    system_context_state: Arc<std::sync::Mutex<SessionSystemContextState>>,
    /// Shared control state for deferred first-turn prompt and staged tool results.
    deferred_turn_state: Arc<std::sync::Mutex<SessionDeferredTurnState>>,
    /// Wakes the running turn loop when an interrupt is requested.
    interrupt_notify: Arc<tokio::sync::Notify>,
    /// Shared live flag for cancel-after-boundary requests.
    cancel_after_boundary_handle: Option<Arc<AtomicBool>>,
    /// Broadcast channel for session-wide event subscription.
    session_event_tx: tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>,
}

struct SessionTaskControl {
    state_tx: watch::Sender<SessionState>,
    summary_tx: watch::Sender<SessionSummaryCache>,
    llm_identity_tx: watch::Sender<SessionLlmIdentity>,
    turn_admission: Arc<std::sync::Mutex<TurnAdmissionSlot>>,
    interrupt_notify: Arc<tokio::sync::Notify>,
    session_event_tx: tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>,
    /// Session-context DSL handle (W2-E / issue #264). `None` on standalone
    /// / ephemeral builds that have no runtime-backed DSL authority. The
    /// session task's `publish_summary` helper fires
    /// `AdvanceSessionContext` through this handle after every canonical
    /// session-truth mutation so the realtime projection consumer observes
    /// a typed `SessionContextAdvanced` effect.
    session_context: Option<Arc<dyn meerkat_core::handles::SessionContextHandle>>,
}

impl SessionTaskControl {
    /// Update the summary watch channel and fire the DSL
    /// `AdvanceSessionContext` input so the realtime projection consumer
    /// receives a typed `SessionContextAdvanced` effect (W2-E).
    ///
    /// Monotonic: the DSL guard filters out non-advancing ticks, so a
    /// single call site at every mutation is correct even if two sites
    /// fire back-to-back without an intervening advance.
    fn publish_summary(&self, snapshot: SessionSummaryCache) {
        let updated_at_ms = summary_updated_at_ms(snapshot.updated_at);
        self.summary_tx.send_replace(snapshot);
        if let Some(handle) = self.session_context.as_ref()
            && let Err(err) = handle.context_advanced(updated_at_ms)
        {
            tracing::debug!(
                error = %err,
                "AdvanceSessionContext rejected by DSL; projection refresh will rely on later ticks"
            );
        }
    }
}

/// Convert a `SystemTime` summary timestamp into monotonic milliseconds
/// for the DSL's `AdvanceSessionContext` input. `UNIX_EPOCH`-relative.
/// Saturates to `u64::MAX` on dates far in the future (unreachable under
/// any realistic clock).
///
/// Uses [`meerkat_core::time_compat::UNIX_EPOCH`] so the same `SystemTime`
/// alias applies on both native and wasm32 (which uses `web_time`).
fn summary_updated_at_ms(updated_at: SystemTime) -> u64 {
    updated_at
        .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Agent abstraction
// ---------------------------------------------------------------------------

/// Trait for building agents from session creation requests.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionAgentBuilder: Send + Sync {
    /// The concrete agent type.
    #[cfg(not(target_arch = "wasm32"))]
    type Agent: SessionAgent + Send + 'static;
    #[cfg(target_arch = "wasm32")]
    type Agent: SessionAgent + 'static;

    /// Build an agent for a new session.
    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError>;
}

/// Trait abstracting over the agent's run/cancel interface.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionAgent: Send {
    /// Run the agent with the given prompt, streaming events.
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError>;

    /// Run the next turn.
    ///
    /// `handling_mode` and `render_metadata` are runtime-owned semantics:
    /// the runtime routes Queue/Steer BEFORE calling the executor, so by
    /// the time this method runs the routing decision is already made.
    /// These parameters are present on the trait because the session task
    /// forwards them from `StartTurnRequest`, but implementations should
    /// not act on them — the runtime is the canonical owner.
    ///
    /// The default rejects non-Queue handling_mode and non-None render_metadata
    /// to prevent silent flattening on paths that can't honor them (§5).
    async fn run_turn_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        handling_mode: meerkat_core::types::HandlingMode,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        _execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        if handling_mode != meerkat_core::types::HandlingMode::Queue {
            return Err(meerkat_core::error::AgentError::ConfigError(format!(
                "handling_mode {handling_mode:?} requires a runtime-backed surface",
            )));
        }
        if render_metadata.is_some() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "render_metadata requires a runtime-backed surface".to_string(),
            ));
        }
        self.run_with_events(prompt, event_tx).await
    }

    /// Continue from the existing session transcript without pushing a new user
    /// message. Used for deferred first-turn prompts and staged callback
    /// tool-result continuations.
    async fn run_pending_with_events(
        &mut self,
        _execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "run_pending_with_events is not supported by this session agent".to_string(),
        ))
    }

    /// Stage skill references to resolve and inject on the next turn.
    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>);

    /// Apply or clear a per-turn flow tool overlay.
    fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError>;

    /// Apply staged callback tool results before the next continuation turn.
    fn apply_pending_tool_results(
        &mut self,
        results: Vec<meerkat_core::ToolResult>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        if results.is_empty() {
            return Ok(());
        }
        Err(meerkat_core::error::AgentError::ConfigError(
            "staged tool-result continuations are not supported by this session agent".to_string(),
        ))
    }

    /// Replace the LLM client for subsequent turns.
    fn replace_client(&mut self, _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>) {}

    /// Atomically update the live client and the session's durable LLM identity.
    fn hot_swap_llm_identity(
        &mut self,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError>;

    /// Stage an external tool visibility filter for subsequent turns.
    fn stage_external_tool_filter(
        &mut self,
        _filter: meerkat_core::ToolFilter,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    /// Replace the canonical tool visibility state carried by the live session.
    fn set_tool_visibility_state(
        &mut self,
        _state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "tool visibility updates are not supported by this session agent".to_string(),
        ))
    }

    /// Dispatch an external tool call through the session's canonical dispatcher.
    async fn dispatch_external_tool_call(
        &mut self,
        _call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "external live tool dispatch is not supported by this session agent".to_string(),
        ))
    }

    /// Cancel the currently running turn.
    fn cancel(&mut self);

    /// Shared live control flag for cancel-after-boundary requests.
    fn cancel_after_boundary_handle(&self) -> Option<Arc<AtomicBool>> {
        None
    }

    /// Session-context advancement handle (W2-E / issue #264).
    ///
    /// Runtime-backed agents expose the session's DSL handle here. The
    /// session task fires `context_advanced(updated_at_ms)` on every
    /// `summary_tx.send_replace` so the realtime projection consumer
    /// observes canonical session-truth advancement as a typed effect
    /// instead of polling a watch channel.
    ///
    /// Standalone agents (WASM, ephemeral tests) return `None`; the task
    /// then skips the emit and the typed effect simply never fires —
    /// which is correct, there is nothing to refresh on those paths.
    fn session_context_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::handles::SessionContextHandle>> {
        None
    }

    /// Get the session ID.
    fn session_id(&self) -> SessionId;

    /// Take a snapshot of the current session state.
    fn snapshot(&self) -> SessionSnapshot;

    /// Take a diagnostic snapshot of the live execution state, if supported.
    fn execution_snapshot(&self) -> Option<meerkat_core::AgentExecutionSnapshot> {
        None
    }

    /// Take a diagnostic snapshot of the live tool-scope state, if supported.
    fn tool_scope_snapshot(&self) -> Option<meerkat_core::ToolScopeSnapshot> {
        None
    }

    /// Snapshot the canonical visible tool definitions for the live session.
    fn visible_tool_defs(&self) -> Vec<meerkat_core::ToolDef> {
        Vec::new()
    }

    /// Take a diagnostic snapshot of the live external tool-surface state, if supported.
    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        None
    }

    /// Clone the full session (messages + metadata) for persistence.
    ///
    /// This is more expensive than `snapshot()` because it includes the
    /// full message history. Only called by `PersistentSessionService`
    /// after each turn.
    fn session_clone(&self) -> meerkat_core::Session;

    /// Whether the session has a pending user/tool-results boundary.
    ///
    /// Lightweight check used by the ResumePending no-op guard to avoid
    /// calling `session_clone()` just to inspect the last message.
    fn has_pending_boundary(&self) -> bool {
        false // default: no pending boundary
    }

    /// Update the `keep_alive` flag in the session's durable metadata.
    ///
    /// Called by the session task when the runtime overrides keep-alive on a
    /// live session. This ensures subsequent inheriting calls observe the
    /// updated value.
    fn update_keep_alive(&mut self, _keep_alive: bool) {}

    /// Update the canonical mob operator authority persisted on the session.
    fn update_mob_tool_authority_context(
        &mut self,
        _authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "mob tool authority updates are not supported by this session agent".to_string(),
        ))
    }

    /// Update the session system prompt before the first turn starts.
    fn update_system_prompt(
        &mut self,
        _system_prompt: String,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "system_prompt override is not supported by this session agent".to_string(),
        ))
    }

    /// Apply runtime-owned system-context blocks immediately to the canonical session.
    fn apply_runtime_system_context(&mut self, appends: &[PendingSystemContextAppend]);

    /// Append externally-produced user content into the canonical transcript.
    fn append_external_user_content(
        &mut self,
        _content: ContentInput,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "external user content append is not supported by this session agent".to_string(),
        ))
    }

    /// Append externally-produced assistant output into the canonical transcript.
    fn append_external_assistant_output(
        &mut self,
        _blocks: Vec<meerkat_core::types::AssistantBlock>,
        _stop_reason: meerkat_core::types::StopReason,
        _usage: Usage,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "external assistant output append is not supported by this session agent".to_string(),
        ))
    }

    /// Get shared runtime control state for system-context append requests.
    fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>;

    /// Synchronize the shared system-context control state into the canonical session metadata.
    fn sync_system_context_state(&mut self) {}

    /// Get an event injector for pushing external events.
    ///
    /// Called once before the agent moves into its dedicated task. The returned
    /// injector is stored in the session handle for surfaces to access.
    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        None
    }

    /// Internal runtime injector for interaction-scoped streaming.
    #[doc(hidden)]
    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        None
    }

    /// Get the comms runtime used by this agent, if any.
    ///
    /// Called once before the agent moves into its dedicated task. The returned
    /// runtime is stored in the session handle for surfaces that need comms
    /// command execution and stream attachment.
    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        None
    }
}

// ---------------------------------------------------------------------------
// EphemeralSessionService
// ---------------------------------------------------------------------------

/// In-memory session service with no persistence.
///
/// Sessions are kept alive as tokio tasks. All state is lost on process exit.
pub struct EphemeralSessionService<B: SessionAgentBuilder> {
    sessions: RwLock<IndexMap<SessionId, SessionHandle>>,
    archived_views: RwLock<IndexMap<SessionId, SessionView>>,
    builder: B,
    max_sessions: usize,
    session_capacity: Arc<Semaphore>,
    /// Notified when a new session handle is stored. Used by CLI --stdin
    /// to avoid polling for the session to appear.
    session_registered: tokio::sync::Notify,
}

impl<B: SessionAgentBuilder + 'static> EphemeralSessionService<B> {
    fn build_runtime_receipt(
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        session: &meerkat_core::Session,
    ) -> Result<RunBoundaryReceipt, SessionError> {
        let encoded_messages = serde_json::to_vec(session.messages()).map_err(|err| {
            SessionError::Agent(AgentError::InternalError(format!(
                "failed to serialize session for runtime receipt digest: {err}"
            )))
        })?;
        let digest = format!("{:x}", Sha256::digest(encoded_messages));

        Ok(RunBoundaryReceipt {
            run_id,
            boundary,
            contributing_input_ids,
            conversation_digest: Some(digest),
            message_count: session.messages().len(),
            sequence: 0,
        })
    }

    fn callback_pending_terminal(error: &SessionError) -> Option<CoreApplyTerminal> {
        match error {
            SessionError::Agent(AgentError::CallbackPending { tool_name, args }) => {
                Some(CoreApplyTerminal::CallbackPending {
                    tool_name: tool_name.clone(),
                    args: args.clone(),
                })
            }
            _ => None,
        }
    }

    async fn build_runtime_output(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        terminal: Option<CoreApplyTerminal>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let session = self.export_session(id).await?;
        let session_snapshot = serde_json::to_vec(&session).map_err(|err| {
            SessionError::Agent(AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;
        let receipt =
            Self::build_runtime_receipt(run_id, boundary, contributing_input_ids, &session)?;

        Ok(match terminal {
            Some(CoreApplyTerminal::RunResult(run_result)) => {
                CoreApplyOutput::with_run_result(receipt, Some(session_snapshot), run_result)
            }
            Some(CoreApplyTerminal::CallbackPending { tool_name, args }) => {
                CoreApplyOutput::with_callback_pending(
                    receipt,
                    Some(session_snapshot),
                    tool_name,
                    args,
                )
            }
            Some(CoreApplyTerminal::NoPendingBoundary) => CoreApplyOutput {
                receipt,
                session_snapshot: Some(session_snapshot),
                run_result: None,
                terminal: Some(CoreApplyTerminal::NoPendingBoundary),
            },
            None => CoreApplyOutput::without_terminal(receipt, Some(session_snapshot)),
        })
    }

    fn provider_supports_inline_video(identity: &SessionLlmIdentity) -> bool {
        identity.provider == meerkat_core::Provider::Gemini
    }

    fn validate_prompt_video_input(
        prompt: &ContentInput,
        identity: &SessionLlmIdentity,
    ) -> Result<(), SessionError> {
        let blocks = match prompt {
            ContentInput::Text(_) => return Ok(()),
            ContentInput::Blocks(blocks) => blocks,
        };

        meerkat_core::validate_inline_video_blocks(blocks)
            .map_err(|err| SessionError::Agent(AgentError::ConfigError(err)))?;

        if meerkat_core::has_video(blocks) && !Self::provider_supports_inline_video(identity) {
            return Err(SessionError::Agent(AgentError::ConfigError(format!(
                "inline video input requires a Gemini model; current provider is '{}'",
                identity.provider.as_str()
            ))));
        }

        Ok(())
    }

    fn validate_tool_result_video(results: &[ToolResult]) -> Result<(), SessionError> {
        if results.iter().any(ToolResult::has_video) {
            return Err(SessionError::Agent(AgentError::ConfigError(
                "video blocks are not supported in tool results".to_string(),
            )));
        }
        Ok(())
    }

    fn llm_identity_from_create_request(req: &CreateSessionRequest) -> SessionLlmIdentity {
        let provider = req
            .build
            .as_ref()
            .and_then(|build| build.provider)
            .or_else(|| meerkat_core::Provider::infer_from_model(&req.model))
            .unwrap_or(meerkat_core::Provider::Other);
        let provider_params = req
            .build
            .as_ref()
            .and_then(|build| build.provider_params.clone());
        SessionLlmIdentity {
            model: req.model.clone(),
            provider,
            self_hosted_server_id: None,
            provider_params,
            connection_ref: req
                .build
                .as_ref()
                .and_then(|build| build.connection_ref.clone()),
        }
    }

    /// Create a new ephemeral session service.
    pub fn new(builder: B, max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(IndexMap::new()),
            archived_views: RwLock::new(IndexMap::new()),
            builder,
            max_sessions,
            session_capacity: Arc::new(Semaphore::new(max_sessions)),
            session_registered: tokio::sync::Notify::new(),
        }
    }

    fn archived_view_from_handle(id: &SessionId, handle: &SessionHandle) -> SessionView {
        let cache = handle.summary_rx.borrow();
        let llm_identity = handle.llm_identity_rx.borrow().clone();
        SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: handle.created_at,
                updated_at: cache.updated_at,
                message_count: cache.message_count,
                is_active: false,
                model: llm_identity.model,
                provider: llm_identity.provider,
                last_assistant_text: cache.last_assistant_text.clone(),
                labels: handle.labels.clone(),
            },
            billing: SessionUsage {
                total_tokens: cache.total_tokens,
                usage: cache.usage.clone(),
            },
        }
    }

    /// Export the full session (messages + metadata) for persistence.
    ///
    /// Returns the complete `Session` including message history. Used by
    /// `PersistentSessionService` to save full snapshots after each turn.
    pub async fn export_session(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::Session, SessionError> {
        let (command_tx, deferred_turn_state, system_context_state) = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            (
                handle.command_tx.clone(),
                Arc::clone(&handle.deferred_turn_state),
                Arc::clone(&handle.system_context_state),
            )
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ExportSession { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        let mut session = reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })?;

        let state = lock_deferred_turn_state(&deferred_turn_state).clone();
        session.set_deferred_turn_state(state).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize deferred-turn state: {err}"
            )))
        })?;

        let system_context = match system_context_state.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                tracing::warn!("system-context state lock poisoned while exporting session");
                poisoned.into_inner().clone()
            }
        };
        session
            .set_system_context_state(system_context)
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize system-context state: {err}"
                )))
            })?;

        Ok(session)
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::SetToolVisibilityState { state, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Get a diagnostic snapshot of the live execution state for a session.
    pub async fn execution_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ExecutionSnapshot { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Get a diagnostic snapshot of the live tool-scope state for a session.
    pub async fn tool_scope_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ToolScopeSnapshot { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Get the canonical visible tool definitions for a live session.
    pub async fn live_visible_tool_defs(
        &self,
        id: &SessionId,
    ) -> Result<Vec<meerkat_core::ToolDef>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::VisibleToolDefs { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Get a diagnostic snapshot of the live external tool-surface state for a session.
    pub async fn external_tool_surface_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ExternalToolSurfaceSnapshot>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ExternalToolSurfaceSnapshot { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Dispatch an external tool call through the live session task.
    pub async fn dispatch_external_tool_call(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::DispatchExternalToolCall { call, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Get shared deferred-turn control state for a session, if available.
    pub async fn deferred_turn_state(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<std::sync::Mutex<SessionDeferredTurnState>>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|h| Arc::clone(&h.deferred_turn_state))
    }

    /// Drop a live session handle without archiving it.
    ///
    /// This is used when a runtime-backed turn mutates the in-memory session
    /// but fails to durably commit its boundary. Discarding the live handle
    /// forces subsequent access to recover from the last persisted snapshot.
    pub async fn discard_live_session(&self, id: &SessionId) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        let handle = sessions
            .swap_remove(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        drop(sessions);
        let phase = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.request_shutdown().ok()
        };
        if let Some(phase) = phase {
            handle
                .state_tx
                .send_replace(map_turn_phase_to_session_state(phase));
        }
        let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        Ok(())
    }

    pub async fn apply_runtime_system_context(
        &self,
        id: &SessionId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ApplyRuntimeSystemContext { appends, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Append externally-produced user content into the canonical transcript.
    pub async fn append_external_user_content(
        &self,
        id: &SessionId,
        content: ContentInput,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::AppendExternalUserContent { content, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Append externally-produced assistant output into the canonical transcript.
    pub async fn append_external_assistant_output(
        &self,
        id: &SessionId,
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: Usage,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::AppendExternalAssistantOutput {
                blocks,
                stop_reason,
                usage,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    pub(crate) async fn sync_system_context_state(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::SyncSystemContextState { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped reply channel".to_string(),
            ))
        })
    }

    pub async fn apply_runtime_turn(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        match self.start_turn(id, req).await {
            Ok(run_result) => {
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::RunResult(run_result)),
                )
                .await
            }
            Err(SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary)) => {
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::NoPendingBoundary),
                )
                .await
            }
            Err(error) => {
                if let Some(terminal) = Self::callback_pending_terminal(&error) {
                    self.build_runtime_output(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(terminal),
                    )
                    .await
                } else {
                    Err(error)
                }
            }
        }
    }

    pub async fn apply_runtime_context_appends(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_boundary(
            id,
            run_id,
            appends,
            RunApplyBoundary::Immediate,
            contributing_input_ids,
        )
        .await
    }

    pub async fn apply_runtime_context_appends_with_boundary(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_system_context(id, appends).await?;
        self.build_runtime_output(id, run_id, boundary, contributing_input_ids, None)
            .await
    }

    /// Get the event injector for a session, if available.
    ///
    /// Returns `None` if the session doesn't exist, has no comms runtime,
    /// or the comms runtime doesn't support event injection.
    pub async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.event_injector.clone())
    }

    #[doc(hidden)]
    pub async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.interaction_event_injector.clone())
    }

    /// Get shared system-context control state for a session, if available.
    pub async fn system_context_state(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<std::sync::Mutex<SessionSystemContextState>>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|h| Arc::clone(&h.system_context_state))
    }

    /// Get the current live durable LLM identity for a session.
    pub async fn live_session_llm_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLlmIdentity, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.clone(),
            })?;
        Ok(handle.llm_identity_rx.borrow().clone())
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.comms_runtime.clone())
    }

    /// Wait for a session to be registered.
    ///
    /// Returns when the next session handle is stored. Used by CLI `--stdin`
    /// to wait for the session to become available before starting the stdin reader.
    pub async fn wait_session_registered(&self) {
        self.session_registered.notified().await;
    }

    /// Shut down all sessions.
    pub async fn shutdown(&self) {
        let mut sessions = self.sessions.write().await;
        for (_id, handle) in sessions.drain(..) {
            let phase = {
                let mut slot = lock_turn_admission(&handle.turn_admission);
                slot.request_shutdown().ok()
            };
            if let Some(phase) = phase {
                handle
                    .state_tx
                    .send_replace(map_turn_phase_to_session_state(phase));
            }
            let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        }
    }

    /// Subscribe to session-wide events.
    ///
    /// This stream is available as soon as the session is registered and emits
    /// all agent events produced by the session task, regardless of which
    /// interaction triggered them.
    pub async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| meerkat_core::comms::StreamError::NotFound(format!("session {id}")))?;
        let rx = handle.session_event_tx.subscribe();
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(event) => return Some((event, rx)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        })))
    }

    /// Wait until the session summary reflects a mutation newer than `after`.
    ///
    /// This is intentionally summary-backed rather than agent-event-backed:
    /// runtime-owned context-only appends mutate canonical session state but do
    /// not necessarily produce user-visible agent events. Callers that need an
    /// authoritative "session changed" witness, such as realtime projection
    /// refresh, should follow this mutation boundary instead of the event lane.
    pub async fn wait_for_session_mutation_after(
        &self,
        id: &SessionId,
        after: SystemTime,
    ) -> Result<SystemTime, meerkat_core::comms::StreamError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| meerkat_core::comms::StreamError::NotFound(format!("session {id}")))?;
        let mut rx = handle.summary_rx.clone();
        drop(sessions);

        loop {
            let current = rx.borrow().updated_at;
            if current > after {
                return Ok(current);
            }
            rx.changed()
                .await
                .map_err(|_| meerkat_core::comms::StreamError::Closed)?;
        }
    }

    /// Get a raw broadcast receiver for a session's events.
    ///
    /// Unlike [`subscribe_session_events`] which returns an `EventStream`,
    /// this returns the raw `broadcast::Receiver` which supports synchronous
    /// `try_recv()` — useful for WASM where async polling with noop wakers
    /// doesn't work reliably.
    pub async fn subscribe_session_events_raw(
        &self,
        id: &SessionId,
    ) -> Result<
        tokio::sync::broadcast::Receiver<EventEnvelope<AgentEvent>>,
        meerkat_core::comms::StreamError,
    > {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| meerkat_core::comms::StreamError::NotFound(format!("session {id}")))?;
        Ok(handle.session_event_tx.subscribe())
    }

    fn is_session_state_active(state: SessionState) -> bool {
        matches!(
            state,
            SessionState::Admitted | SessionState::Running | SessionState::Completing
        )
    }

    fn request_start_turn(id: &SessionId, handle: &SessionHandle) -> Result<(), SessionError> {
        let phase = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.claim()
                .map_err(|_| SessionError::Busy { id: id.clone() })?
        };
        handle
            .state_tx
            .send_replace(map_turn_phase_to_session_state(phase));
        Ok(())
    }

    fn try_abort_admitted_turn(handle: &SessionHandle) {
        let phase = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.abort_claim().ok()
        };
        if let Some(phase) = phase {
            handle
                .state_tx
                .send_replace(map_turn_phase_to_session_state(phase));
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionService for EphemeralSessionService<B> {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        // Reserve capacity up front so two concurrent create_session calls cannot race
        // past max_sessions between check and insert.
        let capacity_permit = match self.session_capacity.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let active = self.sessions.read().await.len();
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "Max sessions reached ({}/{})",
                        active, self.max_sessions
                    )),
                ));
            }
        };

        let prompt = req.prompt.clone();
        let caller_event_tx = req.event_tx.clone();
        let defer_initial_turn =
            req.initial_turn == meerkat_core::service::InitialTurnPolicy::Defer;
        let llm_identity = Self::llm_identity_from_create_request(&req);
        Self::validate_prompt_video_input(&prompt, &llm_identity)?;
        let labels = req.labels.clone().unwrap_or_default();
        let resumed_session = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref());
        let mut deferred_turn_state = resumed_session
            .and_then(meerkat_core::Session::deferred_turn_state)
            .unwrap_or_default();
        let resumed_session_is_deferred_template = resumed_session.is_some_and(|session| {
            session.messages().is_empty() && session.deferred_turn_state().is_none()
        });
        if let Some(blob_store) = req
            .build
            .as_ref()
            .and_then(|build| build.blob_store_override.clone())
        {
            hydrate_deferred_turn_state(
                blob_store.as_ref(),
                &mut deferred_turn_state,
                MissingBlobBehavior::HistoricalPlaceholder,
            )
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to hydrate deferred-turn state during session creation: {err}"
                )))
            })?;
        }
        if defer_initial_turn && (resumed_session.is_none() || resumed_session_is_deferred_template)
        {
            deferred_turn_state.mark_initial_turn_pending();
        }
        if defer_initial_turn && req.deferred_prompt_policy == DeferredPromptPolicy::Stage {
            deferred_turn_state.stage_initial_prompt(prompt.clone(), SystemTime::now());
        }
        let deferred_turn_state = Arc::new(std::sync::Mutex::new(deferred_turn_state));

        // Create the permanent event channel for this session.
        let (agent_event_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);

        // Build the agent
        let agent = self
            .builder
            .build_agent(&req, agent_event_tx.clone())
            .await?;
        let session_id = agent.session_id();
        let created_at = SystemTime::now();
        let turn_admission = Arc::new(std::sync::Mutex::new(TurnAdmissionSlot::new()));

        // Extract the event injector before the agent moves into its task.
        let event_injector = agent.event_injector();
        let interaction_event_injector = agent.interaction_event_injector();
        let comms_runtime = agent.comms_runtime();
        let cancel_after_boundary_handle = agent.cancel_after_boundary_handle();
        let system_context_state = agent.system_context_state();
        // W2-E: capture the session-context DSL handle so the session task
        // can fire `AdvanceSessionContext` on every summary-publish site.
        let session_context = agent.session_context_handle();
        // Create session task channels
        let (command_tx, command_rx) = mpsc::channel::<SessionCommand>(COMMAND_CHANNEL_CAPACITY);
        let (state_tx, state_rx) = watch::channel(SessionState::Idle);
        let state_tx_handle = state_tx.clone();
        let (summary_tx, summary_rx) = watch::channel(SessionSummaryCache {
            updated_at: created_at,
            message_count: 0,
            total_tokens: 0,
            usage: Usage::default(),
            last_assistant_text: None,
        });
        let (llm_identity_tx, llm_identity_rx) = watch::channel(llm_identity);
        let (session_event_tx, session_event_rx) =
            tokio::sync::broadcast::channel::<EventEnvelope<AgentEvent>>(EVENT_CHANNEL_CAPACITY);
        drop(session_event_rx);
        let interrupt_notify = Arc::new(tokio::sync::Notify::new());

        // Spawn the session task using the platform-appropriate task API.
        #[cfg(not(target_arch = "wasm32"))]
        tokio::spawn(session_task(
            agent,
            agent_event_tx,
            agent_event_rx,
            command_rx,
            Arc::clone(&deferred_turn_state),
            SessionTaskControl {
                state_tx,
                summary_tx,
                llm_identity_tx,
                turn_admission: Arc::clone(&turn_admission),
                interrupt_notify: interrupt_notify.clone(),
                session_event_tx: session_event_tx.clone(),
                session_context: session_context.clone(),
            },
        ));
        #[cfg(target_arch = "wasm32")]
        tokio_with_wasm::alias::task::spawn(session_task(
            agent,
            agent_event_tx,
            agent_event_rx,
            command_rx,
            Arc::clone(&deferred_turn_state),
            SessionTaskControl {
                state_tx,
                summary_tx,
                llm_identity_tx,
                turn_admission: Arc::clone(&turn_admission),
                interrupt_notify: interrupt_notify.clone(),
                session_event_tx: session_event_tx.clone(),
                session_context: session_context.clone(),
            },
        ));

        // Store the handle
        let handle = SessionHandle {
            command_tx: command_tx.clone(),
            state_tx: state_tx_handle,
            state_rx,
            summary_rx,
            llm_identity_rx,
            turn_admission: Arc::clone(&turn_admission),
            _capacity_permit: capacity_permit,
            created_at,
            labels,
            event_injector,
            interaction_event_injector,
            comms_runtime,
            system_context_state,
            deferred_turn_state,
            interrupt_notify,
            cancel_after_boundary_handle,
            session_event_tx,
        };

        let inserted = {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(&session_id) {
                false
            } else {
                sessions.insert(session_id.clone(), handle);
                // Notify waiters (e.g., CLI --stdin) that a session is available.
                self.session_registered.notify_waiters();
                true
            }
        };
        if !inserted {
            // Duplicate IDs are unexpected but can happen if the builder returns a reused ID.
            // Stop the task so it does not leak in the background.
            let _ = command_tx.send(SessionCommand::Shutdown).await;
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "Duplicate session ID generated: {session_id}"
                )),
            ));
        }

        if defer_initial_turn {
            return Ok(RunResult {
                text: String::new(),
                session_id,
                turns: 0,
                tool_calls: 0,
                usage: Usage::default(),
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            });
        }

        // Claim the canonical turn slot for the eager first turn.
        {
            let sessions = self.sessions.read().await;
            let handle = sessions.get(&session_id).ok_or_else(|| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "fresh session handle missing for eager first turn: {session_id}"
                )))
            })?;
            Self::request_start_turn(&session_id, handle).map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "fresh session failed to admit eager first turn: {error}"
                )))
            })?;
        }

        // Run the first turn
        let (result_tx, result_rx) = oneshot::channel();
        if command_tx
            .send(SessionCommand::StartTurn {
                prompt,
                render_metadata: req.render_metadata,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                event_tx: caller_event_tx,
                result_tx,
                skill_references: req.skill_references,
                flow_tool_overlay: None,
                execution_kind: None, // non-runtime substrate-direct path
            })
            .await
            .is_err()
        {
            let sessions = self.sessions.read().await;
            if let Some(handle) = sessions.get(&session_id) {
                Self::try_abort_admitted_turn(handle);
            }
            drop(sessions);
            let mut sessions = self.sessions.write().await;
            sessions.swap_remove(&session_id);
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(
                    "Session task exited before first turn".to_string(),
                ),
            ));
        }

        let result = match result_rx.await {
            Ok(result) => result,
            Err(_) => {
                let mut sessions = self.sessions.write().await;
                sessions.swap_remove(&session_id);
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(
                        "Session task dropped the result channel".to_string(),
                    ),
                ));
            }
        };

        result.map_err(SessionError::Agent)
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let (result_tx, result_rx) = oneshot::channel();

        let prompt: meerkat_core::types::ContentInput = req.prompt.clone();

        {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            let identity = handle.llm_identity_rx.borrow().clone();
            Self::validate_prompt_video_input(&prompt, &identity)?;

            // Atomic busy check via compare-and-swap. This is the single
            // point of admission — if two callers race, exactly one wins.
            Self::request_start_turn(id, handle)?;

            if let Some(system_prompt) = req.system_prompt {
                let allows_override = {
                    let guard = lock_deferred_turn_state(&handle.deferred_turn_state);
                    guard.allows_initial_turn_overrides()
                };
                if !allows_override {
                    Self::try_abort_admitted_turn(handle);
                    return Err(SessionError::Unsupported(
                        "system_prompt override is only allowed on a deferred session's first turn"
                            .to_string(),
                    ));
                }
                let (reply_tx, reply_rx) = oneshot::channel();
                handle
                    .command_tx
                    .send(SessionCommand::UpdateSystemPrompt {
                        system_prompt,
                        reply_tx,
                    })
                    .await
                    .map_err(|_| {
                        Self::try_abort_admitted_turn(handle);
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            "Session task has exited".to_string(),
                        ))
                    })?;
                let update_result = reply_rx.await.map_err(|_| {
                    Self::try_abort_admitted_turn(handle);
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                        "Session task dropped reply channel".to_string(),
                    ))
                })?;
                update_result.map_err(|error| {
                    Self::try_abort_admitted_turn(handle);
                    SessionError::Agent(error)
                })?;
            }

            let metadata = req.turn_metadata;
            let render_metadata = req.render_metadata.or_else(|| {
                metadata
                    .as_ref()
                    .and_then(|metadata| metadata.render_metadata.clone())
            });
            let handling_mode = metadata
                .as_ref()
                .and_then(|metadata| metadata.handling_mode)
                .unwrap_or(req.handling_mode);
            let skill_references = req.skill_references.or_else(|| {
                metadata
                    .as_ref()
                    .and_then(|metadata| metadata.skill_references.clone())
            });
            let flow_tool_overlay = req.flow_tool_overlay.or_else(|| {
                metadata
                    .as_ref()
                    .and_then(|metadata| metadata.flow_tool_overlay.clone())
            });
            let execution_kind = metadata
                .as_ref()
                .and_then(|metadata| metadata.execution_kind);

            handle
                .command_tx
                .send(SessionCommand::StartTurn {
                    prompt,
                    render_metadata,
                    handling_mode,
                    event_tx: req.event_tx,
                    result_tx,
                    skill_references,
                    flow_tool_overlay,
                    execution_kind,
                })
                .await
                .map_err(|_| {
                    Self::try_abort_admitted_turn(handle);
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                        "Session task has exited".to_string(),
                    ))
                })?;
        }

        let result = result_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the result channel".to_string(),
            ))
        })?;

        result.map_err(SessionError::Agent)
    }

    async fn set_session_client(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ReplaceClient { client, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped reply channel".to_string(),
            ))
        })
    }

    async fn hot_swap_session_llm_identity(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::HotSwapLlmIdentity {
                client,
                identity,
                request_policy,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        Self::set_session_tool_visibility_state(self, id, state).await
    }

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::StageToolFilter { filter, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let woke = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.request_interrupt()
                .map_err(|_| SessionError::NotRunning { id: id.clone() })?
        };
        if woke {
            handle.interrupt_notify.notify_waiters();
        }
        Ok(())
    }

    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        let Some(cancel_after_boundary_handle) = handle.cancel_after_boundary_handle.as_ref()
        else {
            return Err(SessionError::Unsupported(
                "cancel_after_boundary".to_string(),
            ));
        };

        let phase = lock_turn_admission(&handle.turn_admission).phase();
        if phase != TurnAdmissionPhase::Running {
            return Err(SessionError::NotRunning { id: id.clone() });
        }

        cancel_after_boundary_handle.store(true, Ordering::SeqCst);
        handle.interrupt_notify.notify_waiters();
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = match sessions.get(id) {
            Some(handle) => handle,
            None => {
                drop(sessions);
                return self
                    .archived_views
                    .read()
                    .await
                    .get(id)
                    .cloned()
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() });
            }
        };

        // Serve live reads from the service-owned summary/watch state instead
        // of round-tripping through the session task. This keeps read-side
        // snapshots responsive on sync surfaces such as wasm exports.
        let state = *handle.state_rx.borrow();
        let summary = handle.summary_rx.borrow().clone();
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: handle.created_at,
                updated_at: summary.updated_at,
                message_count: summary.message_count,
                is_active: Self::is_session_state_active(state),
                model: handle.llm_identity_rx.borrow().model.clone(),
                provider: handle.llm_identity_rx.borrow().provider,
                last_assistant_text: summary.last_assistant_text,
                labels: handle.labels.clone(),
            },
            billing: SessionUsage {
                total_tokens: summary.total_tokens,
                usage: summary.usage,
            },
        })
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.sessions.read().await;
        let mut summaries: Vec<SessionSummary> = sessions
            .iter()
            .map(|(session_id, h)| {
                let state = *h.state_rx.borrow();
                let cache = h.summary_rx.borrow();
                SessionSummary {
                    session_id: session_id.clone(),
                    created_at: h.created_at,
                    updated_at: cache.updated_at,
                    message_count: cache.message_count,
                    total_tokens: cache.total_tokens,
                    is_active: Self::is_session_state_active(state),
                    labels: h.labels.clone(),
                }
            })
            .collect();

        // Filter by labels if specified (all k/v pairs must match).
        if let Some(ref filter_labels) = query.labels {
            summaries.retain(|s| {
                filter_labels
                    .iter()
                    .all(|(k, v)| s.labels.get(k) == Some(v))
            });
        }

        if let Some(offset) = query.offset {
            if offset < summaries.len() {
                summaries = summaries.split_off(offset);
            } else {
                summaries.clear();
            }
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        Ok(self.sessions.read().await.contains_key(id))
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        let handle = sessions
            .swap_remove(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let archived_view = Self::archived_view_from_handle(id, &handle);
        drop(sessions);
        self.archived_views
            .write()
            .await
            .insert(id.clone(), archived_view);

        let phase = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.request_shutdown().ok()
        };
        if let Some(phase) = phase {
            handle
                .state_tx
                .send_replace(map_turn_phase_to_session_state(phase));
        }
        let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        Ok(())
    }

    async fn update_session_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::UpdateKeepAlive {
                keep_alive,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped reply channel".to_string(),
            ))
        })
    }

    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::UpdateMobToolAuthority {
                authority_context,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        EphemeralSessionService::<B>::subscribe_session_events(self, id).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionServiceControlExt for EphemeralSessionService<B> {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        let state = self
            .system_context_state(id)
            .await
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        let status = {
            let mut guard = match state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!(
                        session_id = %id,
                        "system-context state lock poisoned while staging append"
                    );
                    poisoned.into_inner()
                }
            };
            guard
                .stage_append(&req, SystemTime::now())
                .map_err(|err| err.into_control_error(id))?
        };

        self.sync_system_context_state(id)
            .await
            .map_err(SessionControlError::Session)?;

        Ok(AppendSystemContextResult { status })
    }

    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        Self::validate_tool_result_video(&req.results)?;
        let state = self
            .deferred_turn_state(id)
            .await
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let accepted = {
            let mut guard = lock_deferred_turn_state(&state);
            guard.stage_tool_results(req.results, SystemTime::now())
        };
        Ok(StageToolResultsResult {
            accepted_result_count: accepted,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionServiceCommsExt for EphemeralSessionService<B> {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        EphemeralSessionService::<B>::comms_runtime(self, session_id).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        EphemeralSessionService::<B>::event_injector(self, session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        EphemeralSessionService::<B>::interaction_event_injector(self, session_id).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionServiceHistoryExt for EphemeralSessionService<B> {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        match self.export_session(id).await {
            Ok(session) => Ok(SessionHistoryPage::from_messages(
                session.id().clone(),
                session.messages(),
                query,
            )),
            Err(SessionError::NotFound { .. }) => {
                if self.archived_views.read().await.contains_key(id) {
                    Err(SessionError::PersistenceDisabled)
                } else {
                    Err(SessionError::NotFound { id: id.clone() })
                }
            }
            Err(err) => Err(err),
        }
    }
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

/// Long-lived task that exclusively owns a session agent and processes commands.
fn stamp_event_envelope(
    next_seq: &mut u64,
    source_id: &str,
    event: AgentEvent,
) -> EventEnvelope<AgentEvent> {
    *next_seq += 1;
    // mob_id is optional and only set when a surface/runtime has mob context.
    EventEnvelope::new(source_id, *next_seq, None, event)
}

fn render_runtime_system_context_event_prompt(
    appends: &[PendingSystemContextAppend],
) -> Option<String> {
    if appends.is_empty() {
        return None;
    }

    // Mirror the canonical session rendering closely so public session/mob
    // event subscribers can observe runtime-owned ImmediateContextAppend work
    // without inventing a second helper-local witness. The machine already
    // models these appends as real run lifecycle transitions; this prompt is
    // the session-stream projection of that runtime-owned fact.
    let rendered = appends
        .iter()
        .map(|append| {
            let mut text = String::from("[Runtime System Context]");
            if let Some(source) = &append.source {
                text.push_str("\nsource: ");
                text.push_str(source);
            }
            text.push_str("\n\n");
            text.push_str(&append.text);
            text
        })
        .collect::<Vec<_>>()
        .join(meerkat_core::SYSTEM_CONTEXT_SEPARATOR);

    Some(rendered)
}

fn lock_deferred_turn_state(
    state: &Arc<std::sync::Mutex<SessionDeferredTurnState>>,
) -> std::sync::MutexGuard<'_, SessionDeferredTurnState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("deferred-turn state lock poisoned; continuing with inner state");
            poisoned.into_inner()
        }
    }
}

fn map_turn_phase_to_session_state(phase: TurnAdmissionPhase) -> SessionState {
    match phase {
        TurnAdmissionPhase::Idle => SessionState::Idle,
        TurnAdmissionPhase::Admitted => SessionState::Admitted,
        TurnAdmissionPhase::Running => SessionState::Running,
        TurnAdmissionPhase::Completing => SessionState::Completing,
        TurnAdmissionPhase::ShuttingDown => SessionState::ShuttingDown,
    }
}

fn lock_turn_admission(
    slot: &Arc<std::sync::Mutex<TurnAdmissionSlot>>,
) -> std::sync::MutexGuard<'_, TurnAdmissionSlot> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Roll an admitted turn back to `Idle` from within the session task when a
/// preflight check fails before the run starts.
fn abort_admitted_turn(control: &SessionTaskControl) {
    let phase = {
        let mut slot = lock_turn_admission(&control.turn_admission);
        slot.abort_claim().ok()
    };
    if let Some(phase) = phase {
        control
            .state_tx
            .send_replace(map_turn_phase_to_session_state(phase));
    }
}

/// Canonical turn-admissibility disposition.
///
/// This is the single owner of the "can this turn request legally proceed?"
/// decision. It reads both the live session state (has_pending_boundary)
/// AND deferred staging state (has_staged_tool_results) to produce a typed
/// outcome. No shell code should re-derive this from partial projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartTurnDisposition {
    /// Run a content turn (prompt → LLM).
    RunContentTurn,
    /// Resume a pending continuation boundary (ToolResults or User).
    RunPending,
    /// No pending boundary exists and no staged tool results will create one.
    /// The caller should return NoPendingBoundary.
    NoPendingBoundary,
}

/// Evaluate turn admissibility from the canonical inputs.
///
/// This function is the single source of truth for deciding how a StartTurn
/// request should be dispatched. It considers:
/// - `execution_kind`: typed intent from the runtime layer (or None for substrate-direct)
/// - `prompt`: the content to send (for the None/substrate-direct fallback)
/// - `session_has_pending_boundary`: whether the live session ends in User/ToolResults
/// - `has_staged_tool_results`: whether deferred tool results will create a boundary
fn evaluate_start_turn_disposition(
    execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
    prompt: &meerkat_core::types::ContentInput,
    session_has_pending_boundary: bool,
    has_staged_tool_results: bool,
) -> StartTurnDisposition {
    // A pending boundary exists if the live session has one OR if staged
    // tool results will materialize one when applied.
    let effective_pending_boundary = session_has_pending_boundary || has_staged_tool_results;

    match execution_kind {
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn) => {
            StartTurnDisposition::RunContentTurn
        }
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending) => {
            if effective_pending_boundary {
                StartTurnDisposition::RunPending
            } else {
                StartTurnDisposition::NoPendingBoundary
            }
        }
        None => {
            // Non-runtime substrate-direct paths: infer from prompt content.
            let has_prompt =
                prompt.has_non_text_content() || !prompt.text_content().trim().is_empty();
            if has_prompt {
                StartTurnDisposition::RunContentTurn
            } else if effective_pending_boundary {
                StartTurnDisposition::RunPending
            } else {
                StartTurnDisposition::NoPendingBoundary
            }
        }
    }
}

fn merge_content_inputs(
    deferred: meerkat_core::types::ContentInput,
    turn: meerkat_core::types::ContentInput,
) -> meerkat_core::types::ContentInput {
    match (&deferred, &turn) {
        (
            meerkat_core::types::ContentInput::Text(deferred_text),
            meerkat_core::types::ContentInput::Text(turn_text),
        ) => meerkat_core::types::ContentInput::Text(format!("{deferred_text}\n\n{turn_text}")),
        _ => {
            let mut blocks = deferred.into_blocks();
            blocks.extend(turn.into_blocks());
            meerkat_core::types::ContentInput::Blocks(blocks)
        }
    }
}

fn restore_deferred_turn_inputs(
    deferred_turn_state: &Arc<std::sync::Mutex<SessionDeferredTurnState>>,
    restore_first_turn_pending: bool,
    pending_initial_prompt: Option<PendingDeferredPrompt>,
    pending_tool_results: Vec<PendingToolResultsMessage>,
) {
    if !restore_first_turn_pending
        && pending_initial_prompt.is_none()
        && pending_tool_results.is_empty()
    {
        return;
    }
    let mut guard = lock_deferred_turn_state(deferred_turn_state);
    if restore_first_turn_pending {
        guard.restore_initial_turn_pending();
    }
    if guard.pending_initial_prompt.is_none() {
        guard.pending_initial_prompt = pending_initial_prompt;
    }
    if !pending_tool_results.is_empty() {
        let mut restored = pending_tool_results;
        restored.extend(std::mem::take(&mut guard.pending_tool_results));
        guard.pending_tool_results = restored;
    }
}

async fn session_task<A: SessionAgent>(
    mut agent: A,
    agent_event_tx: mpsc::Sender<AgentEvent>,
    mut agent_event_rx: mpsc::Receiver<AgentEvent>,
    mut commands: mpsc::Receiver<SessionCommand>,
    deferred_turn_state: Arc<std::sync::Mutex<SessionDeferredTurnState>>,
    control: SessionTaskControl,
) {
    let mut next_seq: u64 = 0;
    let source_id = format!("session:{}", agent.session_id());

    loop {
        let Some(cmd) = commands.recv().await else {
            break;
        };

        match cmd {
            SessionCommand::ReplaceClient { client, reply_tx } => {
                agent.replace_client(client);
                let _ = reply_tx.send(());
                continue;
            }
            SessionCommand::HotSwapLlmIdentity {
                client,
                identity,
                request_policy,
                reply_tx,
            } => {
                let result = agent.hot_swap_llm_identity(client, identity.clone(), request_policy);
                if result.is_ok() {
                    control.llm_identity_tx.send_replace(identity);
                }
                let _ = reply_tx.send(result);
                continue;
            }
            SessionCommand::StageToolFilter { filter, reply_tx } => {
                let _ = reply_tx.send(agent.stage_external_tool_filter(filter));
                continue;
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::SetToolVisibilityState { state, reply_tx } => {
                let _ = reply_tx.send(agent.set_tool_visibility_state(state));
                continue;
            }
            SessionCommand::SyncSystemContextState { reply_tx } => {
                agent.sync_system_context_state();
                let _ = reply_tx.send(());
                continue;
            }
            SessionCommand::StartTurn {
                prompt,
                render_metadata,
                handling_mode,
                event_tx,
                result_tx,
                skill_references,
                flow_tool_overlay,
                execution_kind,
            } => {
                let current_phase = lock_turn_admission(&control.turn_admission).phase();
                if current_phase == TurnAdmissionPhase::ShuttingDown {
                    let _ = result_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
                    continue;
                }

                let (restore_first_turn_pending, pending_initial_prompt, pending_tool_results) = {
                    let mut guard = lock_deferred_turn_state(&deferred_turn_state);
                    (
                        guard.mark_initial_turn_started(),
                        guard.pending_initial_prompt.take(),
                        std::mem::take(&mut guard.pending_tool_results),
                    )
                };
                let prompt = match pending_initial_prompt.as_ref() {
                    Some(staged_prompt) => {
                        merge_content_inputs(staged_prompt.prompt.clone(), prompt)
                    }
                    None => prompt,
                };
                let flattened_tool_results = pending_tool_results
                    .iter()
                    .flat_map(|pending| pending.results.clone())
                    .collect::<Vec<_>>();

                // Canonical turn-preflight: evaluate whether this turn request
                // can legally proceed. This is the single owner of the
                // admissibility decision — it reads both the live session state
                // AND deferred staging state to produce a typed disposition.
                let disposition = evaluate_start_turn_disposition(
                    execution_kind,
                    &prompt,
                    agent.has_pending_boundary(),
                    !flattened_tool_results.is_empty(),
                );
                if matches!(disposition, StartTurnDisposition::NoPendingBoundary) {
                    restore_deferred_turn_inputs(
                        &deferred_turn_state,
                        restore_first_turn_pending,
                        pending_initial_prompt,
                        pending_tool_results,
                    );
                    abort_admitted_turn(&control);
                    let _ = result_tx.send(Err(meerkat_core::error::AgentError::NoPendingBoundary));
                    continue;
                }

                agent.set_skill_references(skill_references);
                if let Err(error) = agent.set_flow_tool_overlay(flow_tool_overlay) {
                    restore_deferred_turn_inputs(
                        &deferred_turn_state,
                        restore_first_turn_pending,
                        pending_initial_prompt,
                        pending_tool_results,
                    );
                    abort_admitted_turn(&control);
                    let _ = result_tx.send(Err(error));
                    continue;
                }
                if let Err(error) = agent.apply_pending_tool_results(flattened_tool_results) {
                    let _ = agent.set_flow_tool_overlay(None);
                    restore_deferred_turn_inputs(
                        &deferred_turn_state,
                        restore_first_turn_pending,
                        pending_initial_prompt,
                        pending_tool_results,
                    );
                    abort_admitted_turn(&control);
                    let _ = result_tx.send(Err(error));
                    continue;
                }
                let begin_phase = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.begin()
                };
                match begin_phase {
                    Ok(phase) => {
                        control
                            .state_tx
                            .send_replace(map_turn_phase_to_session_state(phase));
                    }
                    Err(error) => {
                        let _ = agent.set_flow_tool_overlay(None);
                        restore_deferred_turn_inputs(
                            &deferred_turn_state,
                            restore_first_turn_pending,
                            pending_initial_prompt,
                            pending_tool_results,
                        );
                        let _ =
                            result_tx.send(Err(meerkat_core::error::AgentError::InternalError(
                                format!("illegal begin-run transition: {error}"),
                            )));
                        continue;
                    }
                }
                let mut event_stream_open = true;

                // Scope the pinned future so its mutable borrow of `agent` is
                // released before we call `agent.snapshot()`.
                let result = {
                    #[cfg(not(target_arch = "wasm32"))]
                    type RunFut<'a> = std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<RunResult, meerkat_core::error::AgentError>,
                                > + Send
                                + 'a,
                        >,
                    >;
                    #[cfg(target_arch = "wasm32")]
                    type RunFut<'a> = std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<RunResult, meerkat_core::error::AgentError>,
                                > + 'a,
                        >,
                    >;
                    // Dispatch based on the canonical disposition computed above.
                    // NoPendingBoundary was already handled before Running state.
                    let run_fut: RunFut<'_> = match disposition {
                        StartTurnDisposition::RunContentTurn => {
                            Box::pin(agent.run_turn_with_events(
                                prompt,
                                handling_mode,
                                render_metadata,
                                execution_kind,
                                agent_event_tx.clone(),
                            ))
                        }
                        StartTurnDisposition::RunPending => Box::pin(
                            agent.run_pending_with_events(execution_kind, agent_event_tx.clone()),
                        ),
                        StartTurnDisposition::NoPendingBoundary => {
                            // Already handled above — unreachable here.
                            unreachable!("NoPendingBoundary handled before Running state")
                        }
                    };
                    // run_fut is already Pin<Box<...>>, no tokio::pin! needed.
                    let mut run_fut = run_fut;
                    let mut interrupted = false;

                    let r = loop {
                        let interrupt_wait = control.interrupt_notify.notified();
                        tokio::select! {
                            result = &mut run_fut => break result,
                            () = interrupt_wait => {
                                let interrupt_pending =
                                    lock_turn_admission(&control.turn_admission).interrupt_pending();
                                if interrupt_pending {
                                    interrupted = true;
                                    break Err(meerkat_core::error::AgentError::Cancelled);
                                }
                            }
                            Some(event) = agent_event_rx.recv() => {
                                let envelope = stamp_event_envelope(&mut next_seq, &source_id, event);
                                let _ = control.session_event_tx.send(envelope.clone());
                                if event_stream_open
                                    && let Some(ref tx) = event_tx
                                    && tx.send(envelope).await.is_err()
                                {
                                    event_stream_open = false;
                                    tracing::warn!("session event stream receiver dropped; continuing without streaming events");
                                }
                            }
                        }
                    };
                    drop(run_fut);
                    if interrupted {
                        agent.cancel();
                    }

                    // Drain any remaining events
                    while let Ok(event) = agent_event_rx.try_recv() {
                        let envelope = stamp_event_envelope(&mut next_seq, &source_id, event);
                        let _ = control.session_event_tx.send(envelope.clone());
                        if event_stream_open
                            && let Some(ref tx) = event_tx
                            && tx.send(envelope).await.is_err()
                        {
                            event_stream_open = false;
                            tracing::warn!(
                                "session event stream receiver dropped while draining events"
                            );
                        }
                    }

                    r
                }; // run_fut dropped here

                let resolve_phase = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.resolve().ok()
                };
                if let Some(phase) = resolve_phase {
                    control
                        .state_tx
                        .send_replace(map_turn_phase_to_session_state(phase));
                }

                // Update cached summary
                let snap = agent.snapshot();
                control.publish_summary(SessionSummaryCache {
                    updated_at: snap.updated_at,
                    message_count: snap.message_count,
                    total_tokens: snap.total_tokens,
                    usage: snap.usage,
                    last_assistant_text: snap.last_assistant_text,
                });

                // Release the turn lock AFTER setting state to Idle and
                // updating the summary, so the next caller sees consistent state.
                let result = if let Err(error) = agent.set_flow_tool_overlay(None) {
                    tracing::error!(
                        error = %error,
                        "failed to clear flow tool overlay; failing turn to avoid stale scope"
                    );
                    Err(error)
                } else {
                    result
                };
                let finalize = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.finalize()
                };
                let shutting_down = match finalize {
                    Ok(outcome) => {
                        control
                            .state_tx
                            .send_replace(map_turn_phase_to_session_state(outcome.next_phase));
                        outcome.next_phase == TurnAdmissionPhase::ShuttingDown
                    }
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            "failed to finalize session turn admission state"
                        );
                        false
                    }
                };
                let _ = result_tx.send(result);
                if shutting_down {
                    break;
                }
            }
            SessionCommand::ExportSession { reply_tx } => {
                let _ = reply_tx.send(agent.session_clone());
            }
            SessionCommand::ExecutionSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.execution_snapshot());
            }
            SessionCommand::ToolScopeSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.tool_scope_snapshot());
            }
            SessionCommand::VisibleToolDefs { reply_tx } => {
                let _ = reply_tx.send(agent.visible_tool_defs());
            }
            SessionCommand::ExternalToolSurfaceSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.external_tool_surface_snapshot());
            }
            SessionCommand::ApplyRuntimeSystemContext { appends, reply_tx } => {
                agent.apply_runtime_system_context(&appends);
                let snap = agent.snapshot();
                control.publish_summary(SessionSummaryCache {
                    updated_at: snap.updated_at,
                    message_count: snap.message_count,
                    total_tokens: snap.total_tokens,
                    usage: snap.usage,
                    last_assistant_text: snap.last_assistant_text,
                });
                if let Some(prompt) = render_runtime_system_context_event_prompt(&appends) {
                    let session_id = agent.session_id();
                    let started = stamp_event_envelope(
                        &mut next_seq,
                        &source_id,
                        AgentEvent::RunStarted {
                            session_id: session_id.clone(),
                            prompt: ContentInput::Text(prompt),
                        },
                    );
                    let _ = control.session_event_tx.send(started);

                    let completed = stamp_event_envelope(
                        &mut next_seq,
                        &source_id,
                        AgentEvent::RunCompleted {
                            session_id,
                            result: String::new(),
                            usage: Usage::default(),
                        },
                    );
                    let _ = control.session_event_tx.send(completed);
                }
                let _ = reply_tx.send(());
            }
            SessionCommand::AppendExternalUserContent { content, reply_tx } => {
                let result = agent.append_external_user_content(content);
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::AppendExternalAssistantOutput {
                blocks,
                stop_reason,
                usage,
                reply_tx,
            } => {
                let text_content = blocks
                    .iter()
                    .filter_map(|block| match block {
                        meerkat_core::types::AssistantBlock::Text { text, .. } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<String>();
                let usage_for_event = usage.clone();
                let result = agent.append_external_assistant_output(blocks, stop_reason, usage);
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                    if !text_content.is_empty() {
                        let envelope = stamp_event_envelope(
                            &mut next_seq,
                            &source_id,
                            AgentEvent::TextComplete {
                                content: text_content,
                            },
                        );
                        let _ = control.session_event_tx.send(envelope);
                    }
                    let envelope = stamp_event_envelope(
                        &mut next_seq,
                        &source_id,
                        AgentEvent::TurnCompleted {
                            stop_reason,
                            usage: usage_for_event,
                        },
                    );
                    let _ = control.session_event_tx.send(envelope);
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::DispatchExternalToolCall { call, reply_tx } => {
                let result = agent.dispatch_external_tool_call(call).await;
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::UpdateKeepAlive {
                keep_alive,
                reply_tx,
            } => {
                agent.update_keep_alive(keep_alive);
                let _ = reply_tx.send(());
            }
            SessionCommand::UpdateMobToolAuthority {
                authority_context,
                reply_tx,
            } => {
                let _ = reply_tx.send(agent.update_mob_tool_authority_context(authority_context));
            }
            SessionCommand::UpdateSystemPrompt {
                system_prompt,
                reply_tx,
            } => {
                let _ = reply_tx.send(agent.update_system_prompt(system_prompt));
            }
            SessionCommand::Shutdown => {
                let next_phase = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    if slot.phase() == TurnAdmissionPhase::ShuttingDown {
                        None
                    } else {
                        slot.request_shutdown().ok()
                    }
                };
                if let Some(phase) = next_phase {
                    control
                        .state_tx
                        .send_replace(map_turn_phase_to_session_state(phase));
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod disposition_tests {
    use super::*;
    use meerkat_core::lifecycle::RuntimeExecutionKind;

    #[test]
    fn content_turn_always_runs() {
        let d = evaluate_start_turn_disposition(
            Some(RuntimeExecutionKind::ContentTurn),
            &meerkat_core::types::ContentInput::Text(String::new()),
            false,
            false,
        );
        assert_eq!(d, StartTurnDisposition::RunContentTurn);
    }

    #[test]
    fn resume_pending_with_session_boundary() {
        let d = evaluate_start_turn_disposition(
            Some(RuntimeExecutionKind::ResumePending),
            &meerkat_core::types::ContentInput::Text(String::new()),
            true,
            false,
        );
        assert_eq!(d, StartTurnDisposition::RunPending);
    }

    #[test]
    fn resume_pending_with_staged_tool_results() {
        let d = evaluate_start_turn_disposition(
            Some(RuntimeExecutionKind::ResumePending),
            &meerkat_core::types::ContentInput::Text(String::new()),
            false,
            true,
        );
        assert_eq!(d, StartTurnDisposition::RunPending);
    }

    #[test]
    fn resume_pending_no_boundary_no_staged() {
        let d = evaluate_start_turn_disposition(
            Some(RuntimeExecutionKind::ResumePending),
            &meerkat_core::types::ContentInput::Text(String::new()),
            false,
            false,
        );
        assert_eq!(d, StartTurnDisposition::NoPendingBoundary);
    }

    #[test]
    fn none_with_prompt_runs_content_turn() {
        let d = evaluate_start_turn_disposition(
            None,
            &meerkat_core::types::ContentInput::Text("hello".into()),
            false,
            false,
        );
        assert_eq!(d, StartTurnDisposition::RunContentTurn);
    }

    #[test]
    fn none_empty_prompt_with_boundary_runs_pending() {
        let d = evaluate_start_turn_disposition(
            None,
            &meerkat_core::types::ContentInput::Text(String::new()),
            true,
            false,
        );
        assert_eq!(d, StartTurnDisposition::RunPending);
    }

    #[test]
    fn none_empty_prompt_no_boundary_is_no_pending() {
        let d = evaluate_start_turn_disposition(
            None,
            &meerkat_core::types::ContentInput::Text(String::new()),
            false,
            false,
        );
        assert_eq!(d, StartTurnDisposition::NoPendingBoundary);
    }

    #[test]
    fn none_empty_prompt_staged_tool_results_runs_pending() {
        let d = evaluate_start_turn_disposition(
            None,
            &meerkat_core::types::ContentInput::Text(String::new()),
            false,
            true,
        );
        assert_eq!(d, StartTurnDisposition::RunPending);
    }
}
