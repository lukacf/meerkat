//! EphemeralSessionService — in-memory session lifecycle with no persistence.
//!
//! Each session gets a dedicated tokio task that exclusively owns the `Agent`.
//! Communication happens through channels, generalized from `SessionRuntime` in
//! `meerkat-rpc`.

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::error::AgentError;
use meerkat_core::event::{AgentEvent, EventEnvelope, EventSourceIdentity};
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
    DeferredFirstTurnPhase, InputId, PendingDeferredPrompt, PendingSystemContextAppend,
    PendingToolResultsMessage, RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent,
    RealtimeTranscriptMaterializedMessage, RunId, SessionDeferredTurnState, SessionLlmIdentity,
    SessionSystemContextState,
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
        active_admission: Option<RuntimeContextAdmissionGuard>,
        restore_staged_capacity_on_pre_run_failure: bool,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<TurnToolOverlay>,
        pre_turn_context_appends: Vec<PendingSystemContextAppend>,
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
    PublishRuntimeSystemContextEvents {
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
    AppendRealtimeTranscriptEvent {
        event: RealtimeTranscriptEvent,
        reply_tx: oneshot::Sender<
            Result<RealtimeTranscriptApplyOutcome, meerkat_core::error::AgentError>,
        >,
    },
    DispatchExternalToolCall {
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
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
    /// Capacity reserved for a deferred first turn while the session is staged.
    staged_capacity_permit: Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>,
    /// Capacity currently held by live work for this session. Additional
    /// runtime-routed inputs for the same running session join this lease
    /// instead of consuming another global slot.
    active_capacity_lease: Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
    /// Wakes the running turn loop when an interrupt is requested.
    interrupt_notify: Arc<tokio::sync::Notify>,
    /// Shared live flag for cancel-after-boundary requests.
    cancel_after_boundary_handle: Option<Arc<AtomicBool>>,
    /// Broadcast channel for session-wide event subscription.
    session_event_tx: tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>,
}

pub struct RuntimeContextAdmissionGuard {
    staged_capacity_permit: Option<Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>>,
    active_capacity_lease: Option<Arc<std::sync::Mutex<SessionActiveCapacityLease>>>,
    active_permit: Option<OwnedSemaphorePermit>,
    restore_staged_capacity_on_drop: bool,
}

#[derive(Default)]
struct SessionActiveCapacityLease {
    permit: Option<OwnedSemaphorePermit>,
    leases: usize,
    restore_staged_capacity_on_final_release: bool,
    staged_capacity_permit: Option<Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>>,
}

#[derive(Default)]
struct ActiveCapacityLeaseRelease {
    permit: Option<OwnedSemaphorePermit>,
    staged_capacity_permit: Option<Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>>,
}

impl Drop for RuntimeContextAdmissionGuard {
    fn drop(&mut self) {
        if let Some(active_capacity_lease) = self.active_capacity_lease.take() {
            if self.restore_staged_capacity_on_drop
                && let Some(staged_capacity_permit) = self.staged_capacity_permit.take()
            {
                mark_active_capacity_lease_restore_staged(
                    &active_capacity_lease,
                    staged_capacity_permit,
                );
            }
            let released = release_active_capacity_lease(&active_capacity_lease);
            if let Some(staged_capacity_permit) = released.staged_capacity_permit {
                restore_staged_capacity_permit(&staged_capacity_permit, released.permit);
            }
            return;
        }
        if self.restore_staged_capacity_on_drop
            && let Some(staged_capacity_permit) = self.staged_capacity_permit.take()
        {
            restore_staged_capacity_permit(&staged_capacity_permit, self.active_permit.take());
        }
    }
}

impl RuntimeContextAdmissionGuard {
    fn into_start_turn_parts(mut self) -> (Self, bool) {
        let restore_staged_capacity_on_pre_run_failure = self.restore_staged_capacity_on_drop;
        self.restore_staged_capacity_on_drop = false;
        (self, restore_staged_capacity_on_pre_run_failure)
    }

    pub(crate) fn into_create_session_permit(mut self) -> Option<OwnedSemaphorePermit> {
        self.restore_staged_capacity_on_drop = false;
        self.staged_capacity_permit.take();
        if let Some(active_capacity_lease) = self.active_capacity_lease.take() {
            return release_active_capacity_lease(&active_capacity_lease).permit;
        }
        self.active_permit.take()
    }

    fn restore_staged_capacity(mut self) {
        if let Some(active_capacity_lease) = self.active_capacity_lease.as_ref()
            && let Some(staged_capacity_permit) = self.staged_capacity_permit.as_ref()
        {
            mark_active_capacity_lease_restore_staged(
                active_capacity_lease,
                Arc::clone(staged_capacity_permit),
            );
            self.restore_staged_capacity_on_drop = false;
            return;
        }
        self.restore_staged_capacity_on_drop = true;
    }
}

struct SessionTaskControl {
    state_tx: watch::Sender<SessionState>,
    summary_tx: watch::Sender<SessionSummaryCache>,
    llm_identity_tx: watch::Sender<SessionLlmIdentity>,
    turn_admission: Arc<std::sync::Mutex<TurnAdmissionSlot>>,
    interrupt_notify: Arc<tokio::sync::Notify>,
    cancel_after_boundary_handle: Option<Arc<AtomicBool>>,
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

    /// Resolve whether the selected model accepts inline video user content.
    ///
    /// Implementations with a richer configured model registry can override
    /// this; the default uses the built-in model capability catalog.
    async fn model_supports_inline_video(&self, identity: &SessionLlmIdentity) -> Option<bool> {
        meerkat_core::model_profile::inline_video_support_for(identity.provider, &identity.model)
    }

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

    /// Dispatch an external tool call with a caller-specific timeout policy.
    async fn dispatch_external_tool_call_with_timeout_policy(
        &mut self,
        call: meerkat_core::ToolCall,
        _timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError> {
        self.dispatch_external_tool_call(call).await
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

    /// Return the durable LLM identity authored by the concrete agent builder.
    ///
    /// Factory-backed agents resolve this through the same model registry path
    /// that constructs the runtime client and persisted session metadata. Mock
    /// or legacy agents without durable metadata may return `None`.
    fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
        None
    }

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

    /// Apply an identity-bearing provider realtime transcript event.
    fn append_realtime_transcript_event(
        &mut self,
        _event: RealtimeTranscriptEvent,
    ) -> Result<RealtimeTranscriptApplyOutcome, meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "realtime transcript append is not supported by this session agent".to_string(),
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

#[cfg(test)]
fn validate_prompt_video_input_against_capability(
    prompt: &ContentInput,
    identity: &SessionLlmIdentity,
    supports_inline_video: bool,
) -> Result<(), SessionError> {
    let blocks = match prompt {
        ContentInput::Text(_) => return Ok(()),
        ContentInput::Blocks(blocks) => blocks,
    };

    meerkat_core::validate_inline_video_blocks(blocks)
        .map_err(|err| SessionError::Agent(AgentError::ConfigError(err)))?;

    if meerkat_core::has_video(blocks) && !supports_inline_video {
        return Err(SessionError::Agent(AgentError::ConfigError(format!(
            "inline video input is not supported by model '{}' on provider '{}'",
            identity.model,
            identity.provider.as_str()
        ))));
    }

    Ok(())
}

fn wake_interrupt_notify(notify: &tokio::sync::Notify) {
    notify.notify_waiters();
    notify.notify_one();
}

fn clear_cancel_after_boundary_request(handle: &Option<Arc<AtomicBool>>) {
    if let Some(handle) = handle {
        handle.store(false, Ordering::SeqCst);
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
    max_sessions: Option<usize>,
    active_session_capacity: Option<Arc<Semaphore>>,
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
                terminal: Some(CoreApplyTerminal::NoPendingBoundary),
            },
            None => CoreApplyOutput::without_terminal(receipt, Some(session_snapshot)),
        })
    }

    async fn require_inline_video_support(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<(), meerkat_core::UnsupportedModelCapabilityEvidence> {
        match self.builder.model_supports_inline_video(identity).await {
            Some(true) => Ok(()),
            Some(false) => Err(
                meerkat_core::UnsupportedModelCapabilityEvidence::inline_video(
                    identity.provider,
                    identity.model.clone(),
                    meerkat_core::UnsupportedModelCapabilityReason::CapabilityDisabled,
                ),
            ),
            None => Err(
                meerkat_core::UnsupportedModelCapabilityEvidence::inline_video(
                    identity.provider,
                    identity.model.clone(),
                    meerkat_core::UnsupportedModelCapabilityReason::ProviderModelProfileMissing,
                ),
            ),
        }
    }

    async fn validate_prompt_video_input(
        &self,
        prompt: &ContentInput,
        identity: &SessionLlmIdentity,
    ) -> Result<(), SessionError> {
        let blocks = match prompt {
            ContentInput::Text(_) => return Ok(()),
            ContentInput::Blocks(blocks) => blocks,
        };

        meerkat_core::validate_inline_video_blocks(blocks)
            .map_err(|err| SessionError::Agent(AgentError::ConfigError(err)))?;

        if meerkat_core::has_video(blocks)
            && let Err(evidence) = self.require_inline_video_support(identity).await
        {
            return Err(SessionError::Agent(AgentError::ConfigError(
                evidence.to_string(),
            )));
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

    fn fallback_llm_identity_from_create_request(req: &CreateSessionRequest) -> SessionLlmIdentity {
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
            auth_binding: req
                .build
                .as_ref()
                .and_then(|build| build.auth_binding.clone()),
        }
    }

    /// Create a new ephemeral session service.
    pub fn new(builder: B, max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(IndexMap::new()),
            archived_views: RwLock::new(IndexMap::new()),
            builder,
            max_sessions: Some(max_sessions),
            active_session_capacity: Some(Arc::new(Semaphore::new(max_sessions))),
            session_registered: tokio::sync::Notify::new(),
        }
    }

    fn try_acquire_active_permit(&self) -> Result<Option<OwnedSemaphorePermit>, SessionError> {
        let Some(capacity) = self.active_session_capacity.as_ref() else {
            return Ok(None);
        };
        match capacity.clone().try_acquire_owned() {
            Ok(permit) => Ok(Some(permit)),
            Err(_) => {
                let max_sessions = self.max_sessions.unwrap_or(0);
                let active = max_sessions.saturating_sub(capacity.available_permits());
                Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "Max sessions reached ({active}/{max_sessions})"
                    )),
                ))
            }
        }
    }

    fn acquire_runtime_context_admission_for_handle(
        &self,
        handle: &SessionHandle,
    ) -> Result<RuntimeContextAdmissionGuard, SessionError> {
        if let Some(permit) = take_staged_capacity_permit(&handle.staged_capacity_permit) {
            let restore_staged_capacity_on_drop = {
                let state = lock_deferred_turn_state(&handle.deferred_turn_state);
                matches!(state.first_turn_phase, DeferredFirstTurnPhase::Pending)
            };
            return Ok(acquire_active_capacity_lease(
                Arc::clone(&handle.active_capacity_lease),
                Some(permit),
                Some(Arc::clone(&handle.staged_capacity_permit)),
                restore_staged_capacity_on_drop,
            ));
        }
        if let Some(admission) =
            try_join_active_capacity_lease(Arc::clone(&handle.active_capacity_lease))
        {
            return Ok(admission);
        }
        match self.try_acquire_active_permit() {
            Ok(permit) => Ok(acquire_active_capacity_lease(
                Arc::clone(&handle.active_capacity_lease),
                permit,
                None,
                false,
            )),
            Err(err) => {
                if let Some(admission) =
                    try_join_active_capacity_lease(Arc::clone(&handle.active_capacity_lease))
                {
                    Ok(admission)
                } else {
                    Err(err)
                }
            }
        }
    }

    pub fn ensure_active_capacity_available(&self) -> Result<(), SessionError> {
        let Some(capacity) = self.active_session_capacity.as_ref() else {
            return Ok(());
        };
        if capacity.available_permits() > 0 {
            return Ok(());
        }
        let max_sessions = self.max_sessions.unwrap_or(0);
        let active = max_sessions.saturating_sub(capacity.available_permits());
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "Max sessions reached ({active}/{max_sessions})"
            )),
        ))
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
        self.dispatch_external_tool_call_with_timeout_policy(
            id,
            call,
            meerkat_core::ToolDispatchTimeoutPolicy::Disabled,
        )
        .await
    }

    /// Dispatch an external tool call through the live session task with a
    /// caller-specific timeout policy.
    pub async fn dispatch_external_tool_call_with_timeout_policy(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
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
            .send(SessionCommand::DispatchExternalToolCall {
                call,
                timeout_policy,
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
        release_staged_capacity_permit(&handle.staged_capacity_permit);
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

    pub async fn publish_runtime_system_context_events(
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
            .send(SessionCommand::PublishRuntimeSystemContextEvents { appends, reply_tx })
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

    /// Apply an identity-bearing provider realtime transcript event.
    pub async fn append_realtime_transcript_event(
        &self,
        id: &SessionId,
        event: RealtimeTranscriptEvent,
    ) -> Result<RealtimeTranscriptApplyOutcome, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::AppendRealtimeTranscriptEvent { event, reply_tx })
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
        Self::require_runtime_execution_kind_stamp(&req)?;
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

    fn require_runtime_execution_kind_stamp(req: &StartTurnRequest) -> Result<(), SessionError> {
        if req
            .turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.execution_kind)
            .is_some()
        {
            return Ok(());
        }

        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime_execution_kind not set: runtime-backed turn did not stamp RuntimeTurnMetadata.execution_kind"
                    .to_string(),
            ),
        ))
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
        self.apply_runtime_context_appends_with_admission(
            id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            None,
        )
        .await
    }

    pub(crate) async fn apply_runtime_context_appends_with_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_admission_recovering_not_found(
            id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            admission,
        )
        .await
        .map_err(|(error, _admission)| error)
    }

    pub(crate) async fn apply_runtime_context_appends_with_admission_recovering_not_found(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<CoreApplyOutput, (SessionError, Option<RuntimeContextAdmissionGuard>)> {
        let preserve_reserved_admission = admission.is_some();
        let active_guard = match admission {
            Some(admission) => admission,
            None => self
                .acquire_runtime_context_admission(id)
                .await
                .map_err(|error| (error, None))?,
        };
        if let Err(error) = self.apply_runtime_system_context(id, appends).await {
            let admission =
                if preserve_reserved_admission && matches!(error, SessionError::NotFound { .. }) {
                    Some(active_guard)
                } else {
                    None
                };
            return Err((error, admission));
        }
        self.build_runtime_output(id, run_id, boundary, contributing_input_ids, None)
            .await
            .map_err(|error| (error, None))
    }

    pub async fn acquire_runtime_context_admission(
        &self,
        id: &SessionId,
    ) -> Result<RuntimeContextAdmissionGuard, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        self.acquire_runtime_context_admission_for_handle(handle)
    }

    pub async fn join_active_runtime_context_admission(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeContextAdmissionGuard>, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        Ok(try_join_active_capacity_lease(Arc::clone(
            &handle.active_capacity_lease,
        )))
    }

    #[cfg(feature = "session-store")]
    pub(crate) async fn acquire_runtime_capacity_admission(
        &self,
    ) -> Result<RuntimeContextAdmissionGuard, SessionError> {
        let active_permit = self.try_acquire_active_permit()?;
        Ok(RuntimeContextAdmissionGuard {
            staged_capacity_permit: None,
            active_capacity_lease: None,
            active_permit,
            restore_staged_capacity_on_drop: false,
        })
    }

    #[cfg(feature = "session-store")]
    pub(crate) async fn start_turn_with_runtime_context_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: RuntimeContextAdmissionGuard,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_with_admission_recovering_not_found(id, req, Some(admission))
            .await
            .map_err(|(error, _admission)| error)
    }

    #[cfg(feature = "session-store")]
    pub(crate) async fn start_turn_with_runtime_context_admission_recovering_not_found(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: RuntimeContextAdmissionGuard,
    ) -> Result<RunResult, (SessionError, Option<RuntimeContextAdmissionGuard>)> {
        self.start_turn_with_admission_recovering_not_found(id, req, Some(admission))
            .await
    }

    async fn start_turn_with_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        reserved_admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_with_admission_recovering_not_found(id, req, reserved_admission)
            .await
            .map_err(|(error, _admission)| error)
    }

    async fn start_turn_with_admission_recovering_not_found(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        mut reserved_admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, (SessionError, Option<RuntimeContextAdmissionGuard>)> {
        let (result_tx, result_rx) = oneshot::channel();

        let prompt: meerkat_core::types::ContentInput = req.prompt.clone();

        {
            let sessions = self.sessions.read().await;
            let handle = match sessions.get(id) {
                Some(handle) => handle,
                None => {
                    return Err((
                        SessionError::NotFound { id: id.clone() },
                        reserved_admission.take(),
                    ));
                }
            };
            let identity = handle.llm_identity_rx.borrow().clone();
            self.validate_prompt_video_input(&prompt, &identity)
                .await
                .map_err(|error| (error, None))?;

            // Atomic busy check via compare-and-swap. This is the single
            // point of admission — if two callers race, exactly one wins.
            Self::request_start_turn(id, handle).map_err(|error| (error, None))?;

            if let Some(system_prompt) = req.system_prompt {
                let allows_override = {
                    let guard = lock_deferred_turn_state(&handle.deferred_turn_state);
                    guard.allows_initial_turn_overrides()
                };
                if !allows_override {
                    Self::try_abort_admitted_turn(handle);
                    return Err((
                        SessionError::Unsupported(
                            "system_prompt override is only allowed on a deferred session's first turn"
                                .to_string(),
                        ),
                        None,
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
                        (
                            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                                "Session task has exited".to_string(),
                            )),
                            None,
                        )
                    })?;
                let update_result = reply_rx.await.map_err(|_| {
                    Self::try_abort_admitted_turn(handle);
                    (
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            "Session task dropped reply channel".to_string(),
                        )),
                        None,
                    )
                })?;
                update_result.map_err(|error| {
                    Self::try_abort_admitted_turn(handle);
                    (SessionError::Agent(error), None)
                })?;
            }

            let metadata = req.turn_metadata;
            let render_metadata = metadata
                .as_ref()
                .and_then(|metadata| metadata.render_metadata.clone())
                .or(req.render_metadata);
            let handling_mode = metadata
                .as_ref()
                .and_then(|metadata| metadata.handling_mode)
                .unwrap_or(req.handling_mode);
            let skill_references = metadata
                .as_ref()
                .and_then(|metadata| metadata.skill_references.clone())
                .or(req.skill_references);
            let flow_tool_overlay = metadata
                .as_ref()
                .and_then(|metadata| metadata.flow_tool_overlay.clone())
                .or(req.flow_tool_overlay);
            let pre_turn_context_appends = req.pre_turn_context_appends;
            let execution_kind = metadata
                .as_ref()
                .and_then(|metadata| metadata.execution_kind);
            let (active_admission, restore_staged_capacity_on_pre_run_failure) =
                if let Some(admission) = reserved_admission.take() {
                    admission.into_start_turn_parts()
                } else {
                    match self.acquire_runtime_context_admission_for_handle(handle) {
                        Ok(admission) => admission.into_start_turn_parts(),
                        Err(err) => {
                            Self::try_abort_admitted_turn(handle);
                            return Err((err, None));
                        }
                    }
                };

            let command = SessionCommand::StartTurn {
                prompt,
                render_metadata,
                handling_mode,
                event_tx: req.event_tx,
                result_tx,
                active_admission: Some(active_admission),
                restore_staged_capacity_on_pre_run_failure,
                skill_references,
                flow_tool_overlay,
                pre_turn_context_appends,
                execution_kind,
            };
            if let Err(send_error) = handle.command_tx.send(command).await {
                let SessionCommand::StartTurn {
                    active_admission,
                    restore_staged_capacity_on_pre_run_failure,
                    ..
                } = send_error.0
                else {
                    unreachable!("only StartTurn command was sent")
                };
                if restore_staged_capacity_on_pre_run_failure
                    && let Some(admission) = active_admission
                {
                    admission.restore_staged_capacity();
                }
                Self::try_abort_admitted_turn(handle);
                return Err((
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                        "Session task has exited".to_string(),
                    )),
                    None,
                ));
            }
        }

        let result = result_rx.await.map_err(|_| {
            (
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the result channel".to_string(),
                )),
                None,
            )
        })?;

        result.map_err(|error| (SessionError::Agent(error), None))
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
            release_staged_capacity_permit(&handle.staged_capacity_permit);
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
            let phase = slot
                .claim()
                .map_err(|_| SessionError::Busy { id: id.clone() })?;
            clear_cancel_after_boundary_request(&handle.cancel_after_boundary_handle);
            phase
        };
        handle
            .state_tx
            .send_replace(map_turn_phase_to_session_state(phase));
        Ok(())
    }

    fn try_abort_admitted_turn(handle: &SessionHandle) {
        let phase = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            let phase = slot.abort_claim().ok();
            if phase.is_some() {
                clear_cancel_after_boundary_request(&handle.cancel_after_boundary_handle);
            }
            phase
        };
        if let Some(phase) = phase {
            handle
                .state_tx
                .send_replace(map_turn_phase_to_session_state(phase));
        }
    }
}

impl<B: SessionAgentBuilder + 'static> EphemeralSessionService<B> {
    pub(crate) async fn create_session_with_admission(
        &self,
        req: CreateSessionRequest,
        reserved_create_admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, SessionError> {
        let prompt = req.prompt.clone();
        let caller_event_tx = req.event_tx.clone();
        let defer_initial_turn =
            req.initial_turn == meerkat_core::service::InitialTurnPolicy::Defer;
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

        let create_capacity_permit = match reserved_create_admission {
            Some(admission) => admission.into_create_session_permit(),
            None => self.try_acquire_active_permit()?,
        };

        // Create the permanent event channel for this session.
        let (agent_event_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);

        // Build the agent
        let agent = self
            .builder
            .build_agent(&req, agent_event_tx.clone())
            .await?;
        let llm_identity = agent
            .durable_llm_identity()
            .unwrap_or_else(|| Self::fallback_llm_identity_from_create_request(&req));
        self.validate_prompt_video_input(&prompt, &llm_identity)
            .await?;
        let session_id = agent.session_id();
        let created_at = SystemTime::now();
        let turn_admission = Arc::new(std::sync::Mutex::new(TurnAdmissionSlot::new()));
        let staged_capacity_permit = Arc::new(std::sync::Mutex::new(None));
        let active_capacity_lease =
            Arc::new(std::sync::Mutex::new(SessionActiveCapacityLease::default()));
        let eager_active_admission = if defer_initial_turn {
            *lock_staged_capacity_permit(&staged_capacity_permit) = create_capacity_permit;
            None
        } else {
            Some(acquire_active_capacity_lease(
                Arc::clone(&active_capacity_lease),
                create_capacity_permit,
                None,
                false,
            ))
        };

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
                cancel_after_boundary_handle: cancel_after_boundary_handle.clone(),
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
                cancel_after_boundary_handle: cancel_after_boundary_handle.clone(),
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
            created_at,
            labels,
            event_injector,
            interaction_event_injector,
            comms_runtime,
            system_context_state,
            deferred_turn_state,
            staged_capacity_permit,
            active_capacity_lease,
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
                terminal_cause_kind: None,
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
            if let Err(error) = Self::request_start_turn(&session_id, handle) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "fresh session failed to admit eager first turn: {error}"
                    )),
                ));
            }
        }

        let initial_turn_metadata = req
            .build
            .as_ref()
            .and_then(|build| build.initial_turn_metadata.as_ref())
            .cloned();
        let initial_render_metadata = initial_turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.render_metadata.clone())
            .or(req.render_metadata);
        let initial_handling_mode = initial_turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.handling_mode)
            .unwrap_or(meerkat_core::types::HandlingMode::Queue);
        let initial_skill_references = initial_turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.skill_references.clone())
            .or(req.skill_references);
        let initial_flow_tool_overlay = initial_turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.flow_tool_overlay.clone());
        let initial_execution_kind = initial_turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.execution_kind);

        // Run the first turn
        let (result_tx, result_rx) = oneshot::channel();
        if command_tx
            .send(SessionCommand::StartTurn {
                prompt,
                render_metadata: initial_render_metadata,
                handling_mode: initial_handling_mode,
                event_tx: caller_event_tx,
                result_tx,
                active_admission: eager_active_admission,
                restore_staged_capacity_on_pre_run_failure: false,
                skill_references: initial_skill_references,
                flow_tool_overlay: initial_flow_tool_overlay,
                pre_turn_context_appends: Vec::new(),
                execution_kind: initial_execution_kind,
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
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionService for EphemeralSessionService<B> {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.create_session_with_admission(req, None).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_with_admission(id, req, None).await
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
            wake_interrupt_notify(&handle.interrupt_notify);
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

        {
            let slot = lock_turn_admission(&handle.turn_admission);
            let phase = slot.phase();
            if !matches!(
                phase,
                TurnAdmissionPhase::Admitted | TurnAdmissionPhase::Running
            ) {
                return Err(SessionError::NotRunning { id: id.clone() });
            }
            cancel_after_boundary_handle.store(true, Ordering::SeqCst);
        }
        wake_interrupt_notify(&handle.interrupt_notify);
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
        release_staged_capacity_permit(&handle.staged_capacity_permit);
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
    source: &EventSourceIdentity,
    event: AgentEvent,
) -> EventEnvelope<AgentEvent> {
    *next_seq += 1;
    // mob_id is optional and only set when a surface/runtime has mob context.
    EventEnvelope::new_with_source(source.clone(), *next_seq, None, event)
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

fn apply_runtime_system_context_and_publish<A: SessionAgent>(
    agent: &mut A,
    appends: &[PendingSystemContextAppend],
    control: &SessionTaskControl,
    next_seq: &mut u64,
    source: &EventSourceIdentity,
) {
    agent.apply_runtime_system_context(appends);
    let snap = agent.snapshot();
    control.publish_summary(SessionSummaryCache {
        updated_at: snap.updated_at,
        message_count: snap.message_count,
        total_tokens: snap.total_tokens,
        usage: snap.usage,
        last_assistant_text: snap.last_assistant_text,
    });
    if let Some(prompt) = render_runtime_system_context_event_prompt(appends) {
        let session_id = agent.session_id();
        let started = stamp_event_envelope(
            next_seq,
            source,
            AgentEvent::RunStarted {
                session_id: session_id.clone(),
                prompt: ContentInput::Text(prompt),
            },
        );
        let _ = control.session_event_tx.send(started);

        let completed = stamp_event_envelope(
            next_seq,
            source,
            AgentEvent::RunCompleted {
                session_id,
                result: String::new(),
                structured_output: None,
                usage: Usage::default(),
                terminal_cause_kind: None,
            },
        );
        let _ = control.session_event_tx.send(completed);
    }
}

fn publish_runtime_system_context_events<A: SessionAgent>(
    agent: &A,
    appends: &[PendingSystemContextAppend],
    control: &SessionTaskControl,
    next_seq: &mut u64,
    source: &EventSourceIdentity,
) {
    if let Some(prompt) = render_runtime_system_context_event_prompt(appends) {
        let session_id = agent.session_id();
        let started = stamp_event_envelope(
            next_seq,
            source,
            AgentEvent::RunStarted {
                session_id: session_id.clone(),
                prompt: ContentInput::Text(prompt),
            },
        );
        let _ = control.session_event_tx.send(started);

        let completed = stamp_event_envelope(
            next_seq,
            source,
            AgentEvent::RunCompleted {
                session_id,
                result: String::new(),
                structured_output: None,
                usage: Usage::default(),
                terminal_cause_kind: None,
            },
        );
        let _ = control.session_event_tx.send(completed);
    }
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

fn lock_staged_capacity_permit(
    permit: &Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>,
) -> std::sync::MutexGuard<'_, Option<OwnedSemaphorePermit>> {
    permit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_active_capacity_lease(
    lease: &Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
) -> std::sync::MutexGuard<'_, SessionActiveCapacityLease> {
    lease
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn take_staged_capacity_permit(
    permit: &Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>,
) -> Option<OwnedSemaphorePermit> {
    lock_staged_capacity_permit(permit).take()
}

fn restore_staged_capacity_permit(
    slot: &Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>,
    permit: Option<OwnedSemaphorePermit>,
) {
    let Some(permit) = permit else {
        return;
    };
    let mut guard = lock_staged_capacity_permit(slot);
    if guard.is_none() {
        *guard = Some(permit);
    }
}

fn release_staged_capacity_permit(permit: &Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>) {
    let _ = lock_staged_capacity_permit(permit).take();
}

fn acquire_active_capacity_lease(
    active_capacity_lease: Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
    permit: Option<OwnedSemaphorePermit>,
    staged_capacity_permit: Option<Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>>,
    restore_staged_capacity_on_drop: bool,
) -> RuntimeContextAdmissionGuard {
    let mut lease = lock_active_capacity_lease(&active_capacity_lease);
    if lease.leases == 0 {
        lease.permit = permit;
        lease.restore_staged_capacity_on_final_release = false;
        lease.staged_capacity_permit = None;
    } else {
        drop(permit);
    }
    lease.leases = lease.leases.saturating_add(1);
    drop(lease);
    RuntimeContextAdmissionGuard {
        staged_capacity_permit,
        active_capacity_lease: Some(active_capacity_lease),
        active_permit: None,
        restore_staged_capacity_on_drop,
    }
}

fn try_join_active_capacity_lease(
    active_capacity_lease: Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
) -> Option<RuntimeContextAdmissionGuard> {
    let mut lease = lock_active_capacity_lease(&active_capacity_lease);
    if lease.leases == 0 {
        return None;
    }
    lease.leases = lease.leases.saturating_add(1);
    drop(lease);
    Some(RuntimeContextAdmissionGuard {
        staged_capacity_permit: None,
        active_capacity_lease: Some(active_capacity_lease),
        active_permit: None,
        restore_staged_capacity_on_drop: false,
    })
}

fn mark_active_capacity_lease_restore_staged(
    active_capacity_lease: &Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
    staged_capacity_permit: Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>,
) {
    let mut lease = lock_active_capacity_lease(active_capacity_lease);
    lease.restore_staged_capacity_on_final_release = true;
    if lease.staged_capacity_permit.is_none() {
        lease.staged_capacity_permit = Some(staged_capacity_permit);
    }
}

fn release_active_capacity_lease(
    active_capacity_lease: &Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
) -> ActiveCapacityLeaseRelease {
    let mut lease = lock_active_capacity_lease(active_capacity_lease);
    if lease.leases == 0 {
        return ActiveCapacityLeaseRelease::default();
    }
    lease.leases -= 1;
    if lease.leases == 0 {
        let permit = lease.permit.take();
        if lease.restore_staged_capacity_on_final_release {
            lease.restore_staged_capacity_on_final_release = false;
            ActiveCapacityLeaseRelease {
                permit,
                staged_capacity_permit: lease.staged_capacity_permit.take(),
            }
        } else {
            lease.staged_capacity_permit = None;
            ActiveCapacityLeaseRelease {
                permit,
                staged_capacity_permit: None,
            }
        }
    } else {
        ActiveCapacityLeaseRelease::default()
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
        let phase = slot.abort_claim().ok();
        if phase.is_some() {
            clear_cancel_after_boundary_request(&control.cancel_after_boundary_handle);
        }
        phase
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
    let source = EventSourceIdentity::session(agent.session_id());

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
                active_admission,
                restore_staged_capacity_on_pre_run_failure,
                skill_references,
                flow_tool_overlay,
                pre_turn_context_appends,
                execution_kind,
            } => {
                let mut active_admission = active_admission;
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
                    if restore_staged_capacity_on_pre_run_failure
                        && let Some(admission) = active_admission.take()
                    {
                        admission.restore_staged_capacity();
                    }
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
                    if restore_staged_capacity_on_pre_run_failure
                        && let Some(admission) = active_admission.take()
                    {
                        admission.restore_staged_capacity();
                    }
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
                    if restore_staged_capacity_on_pre_run_failure
                        && let Some(admission) = active_admission.take()
                    {
                        admission.restore_staged_capacity();
                    }
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
                        if restore_staged_capacity_on_pre_run_failure
                            && let Some(admission) = active_admission.take()
                        {
                            admission.restore_staged_capacity();
                        }
                        let _ =
                            result_tx.send(Err(meerkat_core::error::AgentError::InternalError(
                                format!("illegal begin-run transition: {error}"),
                            )));
                        continue;
                    }
                }
                if !pre_turn_context_appends.is_empty() {
                    agent.apply_runtime_system_context(&pre_turn_context_appends);
                }
                let mut event_stream_open = true;

                // Scope the pinned future so its mutable borrow of `agent` is
                // released before we call `agent.snapshot()`.
                let (result, resolved_phase) = {
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
                    let mut resolved_phase = None;

                    let r = loop {
                        if lock_turn_admission(&control.turn_admission).interrupt_pending() {
                            interrupted = true;
                            break Err(meerkat_core::error::AgentError::Cancelled);
                        }
                        let interrupt_wait = control.interrupt_notify.notified();
                        tokio::pin!(interrupt_wait);
                        if lock_turn_admission(&control.turn_admission).interrupt_pending() {
                            interrupted = true;
                            break Err(meerkat_core::error::AgentError::Cancelled);
                        }
                        tokio::select! {
                            result = &mut run_fut => {
                                let mut slot = lock_turn_admission(&control.turn_admission);
                                if slot.interrupt_pending() {
                                    interrupted = true;
                                    break Err(meerkat_core::error::AgentError::Cancelled);
                                }
                                resolved_phase = slot.resolve().ok();
                                break result;
                            }
                            () = &mut interrupt_wait => {
                                let interrupt_pending =
                                    lock_turn_admission(&control.turn_admission).interrupt_pending();
                                if interrupt_pending {
                                    interrupted = true;
                                    break Err(meerkat_core::error::AgentError::Cancelled);
                                }
                            }
                            Some(event) = agent_event_rx.recv() => {
                                let envelope = stamp_event_envelope(&mut next_seq, &source, event);
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
                        let envelope = stamp_event_envelope(&mut next_seq, &source, event);
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

                    (r, resolved_phase)
                }; // run_fut dropped here

                let resolve_phase = resolved_phase.or_else(|| {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.resolve().ok()
                });
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
                    let finalize = slot.finalize();
                    if finalize.is_ok() {
                        clear_cancel_after_boundary_request(&control.cancel_after_boundary_handle);
                    }
                    finalize
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
                drop(active_admission);
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
                apply_runtime_system_context_and_publish(
                    &mut agent,
                    &appends,
                    &control,
                    &mut next_seq,
                    &source,
                );
                let _ = reply_tx.send(());
            }
            SessionCommand::PublishRuntimeSystemContextEvents { appends, reply_tx } => {
                publish_runtime_system_context_events(
                    &agent,
                    &appends,
                    &control,
                    &mut next_seq,
                    &source,
                );
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
                            &source,
                            AgentEvent::TextComplete {
                                content: text_content,
                            },
                        );
                        let _ = control.session_event_tx.send(envelope);
                    }
                    let envelope = stamp_event_envelope(
                        &mut next_seq,
                        &source,
                        AgentEvent::TurnCompleted {
                            stop_reason,
                            usage: usage_for_event,
                        },
                    );
                    let _ = control.session_event_tx.send(envelope);
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::AppendRealtimeTranscriptEvent { event, reply_tx } => {
                let result = agent.append_realtime_transcript_event(event);
                if let Ok(outcome) = &result {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                    for materialized in &outcome.materialized_messages {
                        if let RealtimeTranscriptMaterializedMessage::Assistant {
                            text,
                            stop_reason,
                            usage,
                            ..
                        } = materialized
                        {
                            if !text.is_empty() {
                                let envelope = stamp_event_envelope(
                                    &mut next_seq,
                                    &source,
                                    AgentEvent::TextComplete {
                                        content: text.clone(),
                                    },
                                );
                                let _ = control.session_event_tx.send(envelope);
                            }
                            let envelope = stamp_event_envelope(
                                &mut next_seq,
                                &source,
                                AgentEvent::TurnCompleted {
                                    stop_reason: *stop_reason,
                                    usage: usage.clone(),
                                },
                            );
                            let _ = control.session_event_tx.send(envelope);
                        }
                    }
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::DispatchExternalToolCall {
                call,
                timeout_policy,
                reply_tx,
            } => {
                let result = agent
                    .dispatch_external_tool_call_with_timeout_policy(call, timeout_policy)
                    .await;
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
                        let next_phase = slot.request_shutdown().ok();
                        if matches!(next_phase, Some(TurnAdmissionPhase::ShuttingDown)) {
                            clear_cancel_after_boundary_request(
                                &control.cancel_after_boundary_handle,
                            );
                        }
                        next_phase
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
#[allow(clippy::expect_used)]
mod runtime_turn_metadata_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::lifecycle::RuntimeExecutionKind;
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService,
    };
    use meerkat_core::skills::{SkillKey, SkillName};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct MetadataProbeBuilder {
        observed_skill_references: Arc<Mutex<Vec<Option<Vec<SkillKey>>>>>,
        observed_context_texts: Arc<Mutex<Vec<String>>>,
        run_context_counts: Arc<Mutex<Vec<usize>>>,
        fail_flow_overlay_set: bool,
    }

    struct MetadataProbeAgent {
        session_id: SessionId,
        observed_skill_references: Arc<Mutex<Vec<Option<Vec<SkillKey>>>>>,
        observed_context_texts: Arc<Mutex<Vec<String>>>,
        run_context_counts: Arc<Mutex<Vec<usize>>>,
        fail_flow_overlay_set: bool,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for MetadataProbeBuilder {
        type Agent = MetadataProbeAgent;

        async fn build_agent(
            &self,
            _req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(MetadataProbeAgent {
                session_id: SessionId::new(),
                observed_skill_references: Arc::clone(&self.observed_skill_references),
                observed_context_texts: Arc::clone(&self.observed_context_texts),
                run_context_counts: Arc::clone(&self.run_context_counts),
                fail_flow_overlay_set: self.fail_flow_overlay_set,
                system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for MetadataProbeAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            self.run_context_counts
                .lock()
                .expect("run context counts lock poisoned")
                .push(
                    self.observed_context_texts
                        .lock()
                        .expect("observed context texts lock poisoned")
                        .len(),
                );
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, refs: Option<Vec<SkillKey>>) {
            self.observed_skill_references
                .lock()
                .expect("observed skill references lock poisoned")
                .push(refs);
        }

        fn set_flow_tool_overlay(
            &mut self,
            overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            if self.fail_flow_overlay_set && overlay.is_some() {
                return Err(AgentError::ConfigError(
                    "synthetic flow overlay failure".to_string(),
                ));
            }
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> meerkat_core::Session {
            meerkat_core::Session::new()
        }

        fn apply_runtime_system_context(&mut self, appends: &[PendingSystemContextAppend]) {
            self.observed_context_texts
                .lock()
                .expect("observed context texts lock poisoned")
                .extend(appends.iter().map(|append| append.text.clone()));
        }

        fn system_context_state(
            &self,
        ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
            Arc::clone(&self.system_context_state)
        }
    }

    #[tokio::test]
    async fn eager_initial_turn_forwards_full_runtime_metadata_carrier() {
        let observed_skill_references = Arc::new(Mutex::new(Vec::new()));
        let observed_context_texts = Arc::new(Mutex::new(Vec::new()));
        let run_context_counts = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references: Arc::clone(&observed_skill_references),
                observed_context_texts,
                run_context_counts,
                fail_flow_overlay_set: false,
            },
            1,
        );
        let skill = SkillKey::builtin(SkillName::parse("metadata-probe").expect("valid skill"));

        service
            .create_session(CreateSessionRequest {
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("hello".to_string()),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    initial_turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        skill_references: Some(vec![skill.clone()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("eager first turn should run");

        assert_eq!(
            *observed_skill_references
                .lock()
                .expect("observed skill references lock poisoned"),
            vec![Some(vec![skill])],
            "eager first turn must forward the full runtime metadata carrier"
        );
    }

    #[tokio::test]
    async fn start_turn_runtime_metadata_prevents_stale_split_skill_replay() {
        let observed_skill_references = Arc::new(Mutex::new(Vec::new()));
        let observed_context_texts = Arc::new(Mutex::new(Vec::new()));
        let run_context_counts = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references: Arc::clone(&observed_skill_references),
                observed_context_texts,
                run_context_counts,
                fail_flow_overlay_set: false,
            },
            1,
        );
        let stale_split =
            SkillKey::builtin(SkillName::parse("stale-split-skill").expect("valid skill"));
        let canonical =
            SkillKey::builtin(SkillName::parse("runtime-canonical-skill").expect("valid skill"));

        let result = service
            .create_session(CreateSessionRequest {
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("defer".to_string()),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session should create");

        service
            .start_turn(
                &result.session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("go".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    event_tx: None,
                    skill_references: Some(vec![stale_split]),
                    flow_tool_overlay: None,
                    pre_turn_context_appends: Vec::new(),
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        skill_references: Some(vec![canonical.clone()]),
                        ..Default::default()
                    }),
                },
            )
            .await
            .expect("turn should run with canonical runtime metadata");

        assert_eq!(
            *observed_skill_references
                .lock()
                .expect("observed skill references lock poisoned"),
            vec![Some(vec![canonical])],
            "canonical RuntimeTurnMetadata must be the only skill carrier once present"
        );
    }

    #[tokio::test]
    async fn start_turn_applies_pre_turn_context_before_run() {
        let observed_skill_references = Arc::new(Mutex::new(Vec::new()));
        let observed_context_texts = Arc::new(Mutex::new(Vec::new()));
        let run_context_counts = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references,
                observed_context_texts: Arc::clone(&observed_context_texts),
                run_context_counts: Arc::clone(&run_context_counts),
                fail_flow_overlay_set: false,
            },
            1,
        );

        let result = service
            .create_session(CreateSessionRequest {
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("defer".to_string()),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session should create");

        service
            .start_turn(
                &result.session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("reaction".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: None,
                    pre_turn_context_appends: vec![PendingSystemContextAppend {
                        text: "terminal peer context".to_string(),
                        source: Some("peer_response_terminal:test:req".to_string()),
                        idempotency_key: Some("peer_response_terminal:test:req".to_string()),
                        accepted_at: meerkat_core::time_compat::SystemTime::now(),
                    }],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                },
            )
            .await
            .expect("pre-turn context turn should run");

        assert_eq!(
            *observed_context_texts
                .lock()
                .expect("observed context texts lock poisoned"),
            vec!["terminal peer context".to_string()]
        );
        assert_eq!(
            *run_context_counts
                .lock()
                .expect("run context counts lock poisoned"),
            vec![1],
            "pre-turn context must be applied before the agent run starts"
        );
    }

    #[tokio::test]
    async fn start_turn_does_not_apply_pre_turn_context_when_setup_fails() {
        let observed_skill_references = Arc::new(Mutex::new(Vec::new()));
        let observed_context_texts = Arc::new(Mutex::new(Vec::new()));
        let run_context_counts = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references,
                observed_context_texts: Arc::clone(&observed_context_texts),
                run_context_counts: Arc::clone(&run_context_counts),
                fail_flow_overlay_set: true,
            },
            1,
        );

        let result = service
            .create_session(CreateSessionRequest {
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("defer".to_string()),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session should create");

        let error = service
            .start_turn(
                &result.session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("reaction".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: Some(TurnToolOverlay {
                        allowed_tools: Some(vec!["flow_tool".to_string()]),
                        blocked_tools: None,
                    }),
                    pre_turn_context_appends: vec![PendingSystemContextAppend {
                        text: "must not leak before setup succeeds".to_string(),
                        source: Some("peer_response_terminal:test:req".to_string()),
                        idempotency_key: Some("peer_response_terminal:test:req".to_string()),
                        accepted_at: meerkat_core::time_compat::SystemTime::now(),
                    }],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                },
            )
            .await
            .expect_err("flow overlay setup should fail");

        assert!(
            error.to_string().contains("synthetic flow overlay failure"),
            "unexpected error: {error}"
        );
        assert!(
            observed_context_texts
                .lock()
                .expect("observed context texts lock poisoned")
                .is_empty(),
            "pre-turn context must not be visible when setup fails before run"
        );
        assert!(
            run_context_counts
                .lock()
                .expect("run context counts lock poisoned")
                .is_empty(),
            "agent run must not start after setup failure"
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod admission_window_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::service::{
        InitialTurnPolicy, SessionBuildOptions, SessionService, StartTurnRequest,
    };
    use meerkat_core::types::HandlingMode;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct AdmissionProbeBuilder {
        run_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        cancel_after_boundary: Arc<AtomicBool>,
        turn_admission_for_run: Arc<Mutex<Option<Arc<Mutex<TurnAdmissionSlot>>>>>,
        interrupt_before_success: bool,
    }

    struct AdmissionProbeAgent {
        session_id: SessionId,
        run_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        cancel_after_boundary: Arc<AtomicBool>,
        turn_admission_for_run: Arc<Mutex<Option<Arc<Mutex<TurnAdmissionSlot>>>>>,
        interrupt_before_success: bool,
        system_context_state: Arc<Mutex<SessionSystemContextState>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for AdmissionProbeBuilder {
        type Agent = AdmissionProbeAgent;

        async fn build_agent(
            &self,
            _req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(AdmissionProbeAgent {
                session_id: SessionId::new(),
                run_calls: Arc::clone(&self.run_calls),
                cancel_calls: Arc::clone(&self.cancel_calls),
                cancel_after_boundary: Arc::clone(&self.cancel_after_boundary),
                turn_admission_for_run: Arc::clone(&self.turn_admission_for_run),
                interrupt_before_success: self.interrupt_before_success,
                system_context_state: Arc::new(Mutex::new(SessionSystemContextState::default())),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for AdmissionProbeAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            if self.interrupt_before_success {
                let turn_admission = self
                    .turn_admission_for_run
                    .lock()
                    .expect("turn admission probe lock poisoned")
                    .clone()
                    .expect("turn admission probe installed");
                let mut slot = lock_turn_admission(&turn_admission);
                slot.request_interrupt()
                    .expect("running turn should accept interrupt probe");
            }
            self.run_calls.fetch_add(1, Ordering::SeqCst);
            let text = if self.cancel_after_boundary.load(Ordering::SeqCst) {
                "boundary requested"
            } else {
                "ran"
            };
            Ok(RunResult {
                text: text.to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        }

        fn cancel_after_boundary_handle(&self) -> Option<Arc<AtomicBool>> {
            Some(Arc::clone(&self.cancel_after_boundary))
        }

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> meerkat_core::Session {
            meerkat_core::Session::new()
        }

        fn apply_runtime_system_context(&mut self, _appends: &[PendingSystemContextAppend]) {}

        fn system_context_state(&self) -> Arc<Mutex<SessionSystemContextState>> {
            Arc::clone(&self.system_context_state)
        }
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            model: "admission-window-test".to_string(),
            prompt: ContentInput::Text("defer".to_string()),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    fn start_turn_request() -> StartTurnRequest {
        StartTurnRequest {
            prompt: ContentInput::Text("go".to_string()),
            system_prompt: None,
            render_metadata: None,
            handling_mode: HandlingMode::Queue,
            event_tx: None,
            skill_references: None,
            flow_tool_overlay: None,
            pre_turn_context_appends: Vec::new(),
            turn_metadata: None,
        }
    }

    fn probe_builder(
        run_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        cancel_after_boundary: Arc<AtomicBool>,
    ) -> AdmissionProbeBuilder {
        AdmissionProbeBuilder {
            run_calls,
            cancel_calls,
            cancel_after_boundary,
            turn_admission_for_run: Arc::new(Mutex::new(None)),
            interrupt_before_success: false,
        }
    }

    async fn create_admitted_session(
        service: &EphemeralSessionService<AdmissionProbeBuilder>,
    ) -> (SessionId, mpsc::Sender<SessionCommand>) {
        let result = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let command_tx = {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(&result.session_id).expect("session handle");
            EphemeralSessionService::<AdmissionProbeBuilder>::request_start_turn(
                &result.session_id,
                handle,
            )
            .expect("admit turn before command delivery");
            handle.command_tx.clone()
        };
        (result.session_id, command_tx)
    }

    async fn deliver_start_turn(
        command_tx: mpsc::Sender<SessionCommand>,
    ) -> Result<RunResult, AgentError> {
        let (result_tx, result_rx) = oneshot::channel();
        let request = start_turn_request();
        command_tx
            .send(SessionCommand::StartTurn {
                prompt: request.prompt,
                render_metadata: request.render_metadata,
                handling_mode: request.handling_mode,
                event_tx: request.event_tx,
                result_tx,
                active_admission: None,
                restore_staged_capacity_on_pre_run_failure: false,
                skill_references: request.skill_references,
                flow_tool_overlay: request.flow_tool_overlay,
                pre_turn_context_appends: request.pre_turn_context_appends,
                execution_kind: None,
            })
            .await
            .expect("send start turn");
        result_rx.await.expect("receive start turn result")
    }

    #[tokio::test]
    async fn hard_interrupt_during_admission_cancels_before_agent_poll() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let service = EphemeralSessionService::new(
            probe_builder(
                Arc::clone(&run_calls),
                Arc::clone(&cancel_calls),
                Arc::new(AtomicBool::new(false)),
            ),
            1,
        );
        let (session_id, command_tx) = create_admitted_session(&service).await;

        service
            .interrupt(&session_id)
            .await
            .expect("admitted turn accepts hard interrupt");
        let result = deliver_start_turn(command_tx).await;

        assert!(matches!(result, Err(AgentError::Cancelled)));
        assert_eq!(run_calls.load(Ordering::SeqCst), 0);
        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn boundary_cancel_during_admission_sets_flag_for_delivered_turn() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let cancel_after_boundary = Arc::new(AtomicBool::new(false));
        let service = EphemeralSessionService::new(
            probe_builder(
                Arc::clone(&run_calls),
                Arc::new(AtomicUsize::new(0)),
                Arc::clone(&cancel_after_boundary),
            ),
            1,
        );
        let (session_id, command_tx) = create_admitted_session(&service).await;

        service
            .cancel_after_boundary(&session_id)
            .await
            .expect("admitted turn accepts boundary cancel");
        assert!(cancel_after_boundary.load(Ordering::SeqCst));

        let result = deliver_start_turn(command_tx)
            .await
            .expect("start turn should run cooperatively");
        assert_eq!(result.text, "boundary requested");
        assert_eq!(run_calls.load(Ordering::SeqCst), 1);
        assert!(!cancel_after_boundary.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn hard_interrupt_pending_when_run_result_is_ready_wins_over_success() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let turn_admission_for_run = Arc::new(Mutex::new(None));
        let mut builder = probe_builder(
            Arc::clone(&run_calls),
            Arc::clone(&cancel_calls),
            Arc::new(AtomicBool::new(false)),
        );
        builder.turn_admission_for_run = Arc::clone(&turn_admission_for_run);
        builder.interrupt_before_success = true;
        let service = EphemeralSessionService::new(builder, 1);
        let result = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(&result.session_id).expect("session handle");
            *turn_admission_for_run
                .lock()
                .expect("turn admission probe lock poisoned") =
                Some(Arc::clone(&handle.turn_admission));
        }

        let result = service
            .start_turn(&result.session_id, start_turn_request())
            .await;

        assert!(matches!(
            result,
            Err(SessionError::Agent(AgentError::Cancelled))
        ));
        assert_eq!(run_calls.load(Ordering::SeqCst), 1);
        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn boundary_cancel_on_aborted_admission_does_not_leak_to_next_turn() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let cancel_after_boundary = Arc::new(AtomicBool::new(false));
        let service = EphemeralSessionService::new(
            probe_builder(
                Arc::clone(&run_calls),
                Arc::new(AtomicUsize::new(0)),
                Arc::clone(&cancel_after_boundary),
            ),
            1,
        );
        let (session_id, _command_tx) = create_admitted_session(&service).await;

        service
            .cancel_after_boundary(&session_id)
            .await
            .expect("admitted turn accepts boundary cancel");
        assert!(cancel_after_boundary.load(Ordering::SeqCst));
        {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(&session_id).expect("session handle");
            EphemeralSessionService::<AdmissionProbeBuilder>::try_abort_admitted_turn(handle);
        }
        assert!(!cancel_after_boundary.load(Ordering::SeqCst));

        let result = service
            .start_turn(&session_id, start_turn_request())
            .await
            .expect("next turn should run");
        assert_eq!(result.text, "ran");
        assert_eq!(run_calls.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod inline_video_admission_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::Provider;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService,
        StartTurnRequest,
    };
    use meerkat_core::types::{ContentBlock, HandlingMode, VideoData};
    use std::sync::{Arc, Mutex};

    fn identity(provider: Provider, model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    fn inline_video_prompt() -> ContentInput {
        ContentInput::Blocks(vec![
            ContentBlock::Text {
                text: "describe this".to_string(),
            },
            ContentBlock::Video {
                media_type: "video/mp4".to_string(),
                duration_ms: 1_000,
                data: VideoData::Inline {
                    data: "AAAA".to_string(),
                },
            },
        ])
    }

    struct BuilderIdentityProbe {
        identity: SessionLlmIdentity,
        validated_identities: Arc<Mutex<Vec<SessionLlmIdentity>>>,
    }

    struct BuilderIdentityAgent {
        session_id: SessionId,
        identity: SessionLlmIdentity,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    }

    struct NoopAgentLlmClient {
        model: String,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl meerkat_core::AgentLlmClient for NoopAgentLlmClient {
        async fn stream_response(
            &self,
            _messages: &[meerkat_core::Message],
            _tools: &[Arc<meerkat_core::ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<
                &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
            >,
        ) -> Result<meerkat_core::LlmStreamResult, AgentError> {
            Err(AgentError::ConfigError(
                "noop test client should not be called".to_string(),
            ))
        }

        fn provider(&self) -> &'static str {
            "noop"
        }

        fn model(&self) -> &str {
            &self.model
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for BuilderIdentityProbe {
        type Agent = BuilderIdentityAgent;

        async fn model_supports_inline_video(&self, identity: &SessionLlmIdentity) -> Option<bool> {
            self.validated_identities
                .lock()
                .expect("validated identities lock poisoned")
                .push(identity.clone());
            Some(identity == &self.identity)
        }

        async fn build_agent(
            &self,
            _req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(BuilderIdentityAgent {
                session_id: SessionId::new(),
                identity: self.identity.clone(),
                system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for BuilderIdentityAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            self.identity = identity;
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> meerkat_core::Session {
            meerkat_core::Session::new()
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(self.identity.clone())
        }

        fn apply_runtime_system_context(&mut self, _appends: &[PendingSystemContextAppend]) {}

        fn system_context_state(
            &self,
        ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
            Arc::clone(&self.system_context_state)
        }
    }

    fn create_request(
        prompt: ContentInput,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "providerless-video-alias".to_string(),
            prompt,
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    fn start_turn_request(prompt: ContentInput) -> StartTurnRequest {
        StartTurnRequest {
            prompt,
            system_prompt: None,
            render_metadata: None,
            handling_mode: HandlingMode::Queue,
            event_tx: None,
            skill_references: None,
            flow_tool_overlay: None,
            pre_turn_context_appends: Vec::new(),
            turn_metadata: None,
        }
    }

    #[test]
    fn provider_gemini_capability_false_rejects_inline_video() {
        let result = validate_prompt_video_input_against_capability(
            &inline_video_prompt(),
            &identity(Provider::Gemini, "gemini-3-flash-preview"),
            false,
        );

        let message = match result {
            Err(SessionError::Agent(AgentError::ConfigError(message))) => Some(message),
            _ => None,
        };

        assert!(
            message
                .as_deref()
                .is_some_and(|message| message.contains("not supported by model"))
        );
    }

    #[test]
    fn provider_not_gemini_capability_true_accepts_inline_video() {
        let result = validate_prompt_video_input_against_capability(
            &inline_video_prompt(),
            &identity(Provider::OpenAI, "openai-video-capable-test-model"),
            true,
        );

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_session_validates_initial_video_against_builder_identity() {
        let durable_identity = identity(Provider::Gemini, "providerless-video-alias");
        let validated_identities = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: durable_identity.clone(),
                validated_identities: Arc::clone(&validated_identities),
            },
            1,
        );

        let result = service
            .create_session(create_request(
                inline_video_prompt(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("builder-owned Gemini identity should allow inline video");

        let live_identity = service
            .live_session_llm_identity(&result.session_id)
            .await
            .expect("live identity");
        assert_eq!(live_identity, durable_identity);
        assert_eq!(
            *validated_identities
                .lock()
                .expect("validated identities lock poisoned"),
            vec![durable_identity]
        );
    }

    #[tokio::test]
    async fn start_turn_validates_video_against_builder_seeded_live_identity() {
        let durable_identity = identity(Provider::Gemini, "providerless-video-alias");
        let validated_identities = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: durable_identity.clone(),
                validated_identities: Arc::clone(&validated_identities),
            },
            1,
        );

        let result = service
            .create_session(create_request(
                ContentInput::Text("defer".to_string()),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        validated_identities
            .lock()
            .expect("validated identities lock poisoned")
            .clear();

        service
            .start_turn(
                &result.session_id,
                start_turn_request(inline_video_prompt()),
            )
            .await
            .expect("builder-seeded live identity should allow inline video turn");

        assert_eq!(
            *validated_identities
                .lock()
                .expect("validated identities lock poisoned"),
            vec![durable_identity]
        );
    }

    #[tokio::test]
    async fn hot_swap_replaces_builder_seeded_live_identity() {
        let durable_identity = identity(Provider::Gemini, "providerless-video-alias");
        let validated_identities = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: durable_identity,
                validated_identities,
            },
            1,
        );
        let result = service
            .create_session(create_request(
                ContentInput::Text("defer".to_string()),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");

        let target_identity = identity(Provider::OpenAI, "gpt-5.4");
        service
            .hot_swap_session_llm_identity(
                &result.session_id,
                Arc::new(NoopAgentLlmClient {
                    model: target_identity.model.clone(),
                }),
                target_identity.clone(),
                meerkat_core::SessionLlmRequestPolicy {
                    model: target_identity.model.clone(),
                    provider_params: None,
                    provider_tool_defaults: None,
                },
            )
            .await
            .expect("hot-swap should update the live identity watch");

        let live_identity = service
            .live_session_llm_identity(&result.session_id)
            .await
            .expect("live identity");
        assert_eq!(live_identity, target_identity);
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
