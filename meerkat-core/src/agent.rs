//! Agent - the core agent orchestrator
//!
//! The Agent struct ties together all components and runs the agent loop.

mod builder;
pub mod comms_impl;
pub mod compact;
mod extraction;
mod hook_impl;
mod runner;
pub mod skills;
mod state;

use crate::budget::Budget;
use crate::comms::{
    CommsCommand, EventStream, PeerDirectoryEntry, SendAndStreamError, SendError, SendReceipt,
    StreamError, StreamScope, TrustedPeerSpec,
};
use crate::config::{AgentConfig, HookRunOverrides};
use crate::error::AgentError;
use crate::hooks::HookEngine;
use crate::retry::RetryPolicy;
use crate::schema::{CompiledSchema, SchemaError};
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::SubAgentManager;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, OutputSchema, StopReason, ToolCallView,
    ToolDef, ToolResult, Usage,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

pub use builder::AgentBuilder;
pub use runner::AgentRunner;

/// Special error prefix to signal tool calls that must be routed externally.
///
/// DEPRECATED: Use `ToolError::CallbackPending` or `AgentError::CallbackPending` instead.
/// This constant is kept for backward compatibility but will be removed in a future version.
#[deprecated(
    since = "0.2.0",
    note = "Use ToolError::CallbackPending or AgentError::CallbackPending instead"
)]
pub const CALLBACK_TOOL_PREFIX: &str = "CALLBACK_TOOL_PENDING:";

/// Trait for LLM clients that can be used with the agent
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    /// Stream a response from the LLM
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Get the provider name
    fn provider(&self) -> &'static str;

    /// Compile an output schema for this provider.
    ///
    /// Default implementation normalizes the schema without provider-specific lowering.
    /// Adapters override this to apply provider-specific transformations (e.g.,
    /// Anthropic adds `additionalProperties: false`, Gemini strips unsupported keywords).
    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        // Default passthrough: normalized clone, no provider-specific lowering
        Ok(CompiledSchema {
            schema: output_schema.schema.as_value().clone(),
            warnings: Vec::new(),
        })
    }
}

/// Result of streaming from the LLM
pub struct LlmStreamResult {
    blocks: Vec<AssistantBlock>,
    stop_reason: StopReason,
    usage: Usage,
}

impl LlmStreamResult {
    pub fn new(blocks: Vec<AssistantBlock>, stop_reason: StopReason, usage: Usage) -> Self {
        Self {
            blocks,
            stop_reason,
            usage,
        }
    }

    pub fn blocks(&self) -> &[AssistantBlock] {
        &self.blocks
    }
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }
    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    pub fn into_message(self) -> BlockAssistantMessage {
        BlockAssistantMessage {
            blocks: self.blocks,
            stop_reason: self.stop_reason,
        }
    }

    pub fn into_parts(self) -> (Vec<AssistantBlock>, StopReason, Usage) {
        (self.blocks, self.stop_reason, self.usage)
    }
}

/// Trait for tool dispatchers
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;
    /// Execute a tool call
    async fn dispatch(&self, call: ToolCallView<'_>)
    -> Result<ToolResult, crate::error::ToolError>;
}

/// A tool dispatcher that filters tools based on a policy
///
/// Tools are filtered once at construction time based on the allowed_tools list.
/// The inner dispatcher is used for actual dispatch, but only allowed tools are
/// exposed via tools() and dispatch() returns AccessDenied for filtered tools.
pub struct FilteredToolDispatcher<T: AgentToolDispatcher + ?Sized> {
    inner: Arc<T>,
    allowed_tools: HashSet<String>,
    /// Pre-computed filtered tool list (computed once at construction)
    filtered_tools: Arc<[Arc<ToolDef>]>,
}

impl<T: AgentToolDispatcher + ?Sized> FilteredToolDispatcher<T> {
    pub fn new(inner: Arc<T>, allowed_tools: Vec<String>) -> Self {
        let allowed_set: HashSet<String> = allowed_tools.into_iter().collect();

        // Filter tools once at construction - the tool registry is static for agent lifetime
        let inner_tools = inner.tools();
        let filtered: Vec<Arc<ToolDef>> = inner_tools
            .iter()
            .filter(|t| allowed_set.contains(t.name.as_str()))
            .map(Arc::clone)
            .collect();

        Self {
            inner,
            allowed_tools: allowed_set,
            filtered_tools: filtered.into(),
        }
    }
}

#[async_trait]
impl<T: AgentToolDispatcher + ?Sized + 'static> AgentToolDispatcher for FilteredToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.filtered_tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<ToolResult, crate::error::ToolError> {
        if !self.allowed_tools.contains(call.name) {
            return Err(crate::error::ToolError::access_denied(call.name));
        }
        self.inner.dispatch(call).await
    }
}

/// Trait for session stores
#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    async fn save(&self, session: &Session) -> Result<(), AgentError>;
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}

/// Trait for comms runtime that can be used with the agent
#[async_trait]
pub trait CommsRuntime: Send + Sync {
    /// Runtime-local public key identifier, if available.
    ///
    /// Returns a peer ID string in `ed25519:<base64>` format.
    fn public_key(&self) -> Option<String> {
        None
    }

    /// Register a trusted peer for future peer sends.
    ///
    /// Runtimes that manage trust dynamically should accept this as a mutable
    /// control-plane operation and return `SendError::Unsupported` if not
    /// available.
    async fn add_trusted_peer(&self, _peer: TrustedPeerSpec) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "add_trusted_peer not supported for this CommsRuntime".to_string(),
        ))
    }

    /// Remove a previously trusted peer by peer ID.
    ///
    /// Returns `true` if the peer was found and removed, `false` if it
    /// was not present. After removal, messages from this peer should be
    /// rejected and `peers()` should no longer return it.
    async fn remove_trusted_peer(&self, _peer_id: &str) -> Result<bool, SendError> {
        Err(SendError::Unsupported(
            "remove_trusted_peer not supported for this CommsRuntime".to_string(),
        ))
    }

    /// Dispatch a canonical comms command.
    async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        Err(SendError::Unsupported(
            "send not implemented for this CommsRuntime".to_string(),
        ))
    }

    /// Open a stream for a session or interaction scope.
    fn stream(&self, scope: StreamScope) -> Result<EventStream, StreamError> {
        let scope_desc = match scope {
            StreamScope::Session(session_id) => format!("session {}", session_id),
            StreamScope::Interaction(interaction_id) => format!("interaction {}", interaction_id.0),
        };
        Err(StreamError::NotFound(scope_desc))
    }

    /// List peers visible to this runtime.
    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        Vec::new()
    }

    /// Send a command and open a scoped stream in one call.
    async fn send_and_stream(
        &self,
        cmd: CommsCommand,
    ) -> Result<(SendReceipt, EventStream), SendAndStreamError> {
        let receipt = self.send(cmd).await?;
        Err(SendAndStreamError::StreamAttach {
            receipt,
            error: StreamError::Internal(
                "send_and_stream is not implemented for this runtime".to_string(),
            ),
        })
    }

    /// Drain comms inbox and return messages formatted for the LLM
    async fn drain_messages(&self) -> Vec<String>;
    /// Get a notification when new messages arrive
    fn inbox_notify(&self) -> Arc<tokio::sync::Notify>;
    /// Returns true if a DISMISS signal was seen during the last `drain_messages` call.
    fn dismiss_received(&self) -> bool {
        false
    }
    /// Get a subscribable event injector for this runtime's inbox.
    ///
    /// Surfaces use this to push external events and optionally subscribe to
    /// interaction-scoped streaming responses. Returns `None` if the
    /// implementation doesn't support event injection.
    fn event_injector(&self) -> Option<Arc<dyn crate::SubscribableInjector>> {
        None
    }

    /// Drain comms inbox and return structured interactions.
    ///
    /// Default implementation wraps `drain_messages()` results as `InteractionContent::Message`
    /// with generated IDs.
    async fn drain_inbox_interactions(&self) -> Vec<crate::interaction::InboxInteraction> {
        self.drain_messages()
            .await
            .into_iter()
            .map(|text| crate::interaction::InboxInteraction {
                id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                from: "unknown".into(),
                content: crate::interaction::InteractionContent::Message { body: text.clone() },
                rendered_text: text,
            })
            .collect()
    }

    /// Look up and remove a one-shot subscriber for the given interaction.
    ///
    /// Returns the event sender if a subscriber was registered (via `inject_with_subscription`).
    /// The entry is removed from the registry on lookup (one-shot).
    fn interaction_subscriber(
        &self,
        _id: &crate::interaction::InteractionId,
    ) -> Option<tokio::sync::mpsc::Sender<crate::event::AgentEvent>> {
        None
    }

    /// Take and clear the one-shot sender for an interaction-scoped stream.
    fn take_interaction_stream_sender(
        &self,
        _id: &crate::interaction::InteractionId,
    ) -> Option<tokio::sync::mpsc::Sender<crate::event::AgentEvent>> {
        self.interaction_subscriber(_id)
    }

    /// Signal that an interaction has reached a terminal state (complete or failed).
    ///
    /// Implementations should transition the reservation FSM to `Completed` and
    /// clean up registry entries. Called from the host-mode loop after sending
    /// terminal events to the tap.
    fn mark_interaction_complete(&self, _id: &crate::interaction::InteractionId) {}
}

/// The main Agent struct
pub struct Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
{
    config: AgentConfig,
    client: Arc<C>,
    tools: Arc<T>,
    store: Arc<S>,
    session: Session,
    budget: Budget,
    retry_policy: RetryPolicy,
    state: LoopState,
    sub_agent_manager: Arc<SubAgentManager>,
    depth: u32,
    pub(super) comms_runtime: Option<Arc<dyn CommsRuntime>>,
    pub(super) hook_engine: Option<Arc<dyn HookEngine>>,
    pub(super) hook_run_overrides: HookRunOverrides,
    /// Optional context compaction strategy.
    pub(crate) compactor: Option<Arc<dyn crate::compact::Compactor>>,
    /// Input tokens from the last LLM response (for compaction trigger).
    pub(crate) last_input_tokens: u64,
    /// Turn number when compaction last occurred.
    pub(crate) last_compaction_turn: Option<u32>,
    /// Optional memory store for indexing compaction discards.
    pub(crate) memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
    /// Optional skill engine for per-turn `/skill-ref` activation.
    pub(crate) skill_engine: Option<Arc<dyn crate::skills::SkillEngine>>,
    /// Skill references to resolve and inject for the next turn.
    /// Set by surfaces before calling `run()`, consumed on run start.
    pub pending_skill_references: Option<Vec<crate::skills::SkillId>>,
    /// Per-interaction event tap for streaming events to subscribers.
    pub(crate) event_tap: crate::event_tap::EventTap,
    /// Optional default event channel configured at build time.
    /// Used by run methods when no per-call event channel is provided.
    pub(crate) default_event_tx: Option<tokio::sync::mpsc::Sender<crate::event::AgentEvent>>,
    /// When true, the host loop owns the inbox drain cycle.
    /// `drain_comms_inbox()` becomes a no-op to avoid stealing
    /// interaction-scoped messages through the legacy path.
    pub(crate) host_drain_active: bool,
}

#[cfg(test)]
mod tests {
    use super::CommsRuntime;
    use crate::comms::{SendError, TrustedPeerSpec};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Notify;

    struct NoopCommsRuntime {
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl CommsRuntime for NoopCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> std::sync::Arc<Notify> {
            self.notify.clone()
        }
    }

    #[tokio::test]
    async fn test_comms_runtime_trait_defaults_hide_unimplemented_features() {
        let runtime = NoopCommsRuntime {
            notify: Arc::new(Notify::new()),
        };
        assert!(<NoopCommsRuntime as CommsRuntime>::public_key(&runtime).is_none());
        let peer = TrustedPeerSpec {
            name: "peer-a".to_string(),
            peer_id: "ed25519:test".to_string(),
            address: "inproc://peer-a".to_string(),
        };
        let result = <NoopCommsRuntime as CommsRuntime>::add_trusted_peer(&runtime, peer).await;
        assert!(matches!(result, Err(SendError::Unsupported(_))));
    }

    #[tokio::test]
    async fn test_remove_trusted_peer_default_unsupported() {
        let runtime = NoopCommsRuntime {
            notify: Arc::new(Notify::new()),
        };
        let result =
            <NoopCommsRuntime as CommsRuntime>::remove_trusted_peer(&runtime, "ed25519:test").await;
        assert!(matches!(result, Err(SendError::Unsupported(_))));
    }
}
