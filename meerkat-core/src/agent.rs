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
use crate::compact::SessionCompactionCadence;
use crate::config::{AgentConfig, HookRunOverrides};
use crate::error::AgentError;
use crate::event::ExternalToolDelta;
use crate::hooks::HookEngine;
use crate::retry::RetryPolicy;
use crate::schema::{CompiledSchema, SchemaError};
use crate::session::Session;
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_scope::ToolScope;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, OutputSchema, StopReason, ToolCallView,
    ToolDef, Usage,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub use builder::AgentBuilder;
pub use runner::{AgentRunner, HostModePollOutcome};

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
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

/// Result of polling for external tool updates.
///
/// Returned by [`AgentToolDispatcher::poll_external_updates`].
#[derive(Debug, Clone, Default)]
pub struct ExternalToolUpdate {
    /// Notices about completed background operations since last poll.
    pub notices: Vec<ExternalToolDelta>,
    /// Names of servers still connecting in the background.
    pub pending: Vec<String>,
}

/// Trait for tool dispatchers
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;
    /// Execute a tool call, returning the transcript result and any async operations.
    ///
    /// The `ToolDispatchOutcome` separates transcript data (`result`) from
    /// execution metadata (`async_ops`). Most tools return no async ops;
    /// use `ToolDispatchOutcome::from(result)` for synchronous tools.
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError>;

    /// Poll for external tool updates from background operations (e.g. async MCP loading).
    ///
    /// The default implementation returns an empty update. Implementations that
    /// support background tool loading (like `McpRouterAdapter`) override this
    /// to drain completed results and report pending servers.
    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        ExternalToolUpdate::default()
    }

    /// Bind a wait-interrupt receiver into this dispatcher, returning a rebound dispatcher.
    ///
    /// The consuming `Arc<Self>` receiver is required because some dispatchers
    /// (e.g. `CompositeDispatcher`) hold non-Clone state. The factory calls
    /// this once before comms gateway composition, transferring ownership.
    ///
    /// Default returns `Err(Unsupported)`. Dispatchers that contain a `WaitTool`
    /// (e.g. `CompositeDispatcher`) override this to swap in an interrupt-aware
    /// instance. `ToolGateway` and `FilteredToolDispatcher` forward to their
    /// inner dispatchers.
    fn bind_wait_interrupt(
        self: Arc<Self>,
        _rx: crate::wait_interrupt::WaitInterruptReceiver,
    ) -> Result<Arc<dyn AgentToolDispatcher>, crate::wait_interrupt::WaitInterruptBindError> {
        Err(crate::wait_interrupt::WaitInterruptBindError::Unsupported)
    }

    /// Whether this dispatcher supports wait interrupt binding.
    ///
    /// Non-consuming probe that callers check before calling the consuming
    /// `bind_wait_interrupt()`. Default: `false`. Dispatchers that contain a
    /// `WaitTool` (e.g. `CompositeDispatcher`) override this to return `true`.
    fn supports_wait_interrupt(&self) -> bool {
        false
    }

    /// Bind a session-canonical ops registry into this dispatcher.
    ///
    /// Dispatchers that emit session-visible `AsyncOpRef`s must route those
    /// operation IDs into the bound registry. Default returns Unsupported.
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        _registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        _owner_session_id: crate::types::SessionId,
    ) -> Result<Arc<dyn AgentToolDispatcher>, OpsLifecycleBindError> {
        Err(OpsLifecycleBindError::Unsupported)
    }

    /// Whether this dispatcher supports ops lifecycle binding.
    fn supports_ops_lifecycle_binding(&self) -> bool {
        false
    }
}

/// Error from [`AgentToolDispatcher::bind_ops_lifecycle`].
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum OpsLifecycleBindError {
    #[error("ops lifecycle binding is unsupported")]
    Unsupported,
    #[error("dispatcher has shared ownership and cannot be rebound")]
    SharedOwnership,
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: AgentToolDispatcher + ?Sized + 'static> AgentToolDispatcher for FilteredToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.filtered_tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        if !self.allowed_tools.contains(call.name) {
            return Err(crate::error::ToolError::access_denied(call.name));
        }
        self.inner.dispatch(call).await
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn bind_wait_interrupt(
        self: Arc<Self>,
        rx: crate::wait_interrupt::WaitInterruptReceiver,
    ) -> Result<Arc<dyn AgentToolDispatcher>, crate::wait_interrupt::WaitInterruptBindError> {
        let owned = Arc::try_unwrap(self)
            .map_err(|_| crate::wait_interrupt::WaitInterruptBindError::SharedOwnership)?;
        let rebound_inner = owned.inner.bind_wait_interrupt(rx)?;
        Ok(Arc::new(FilteredToolDispatcher {
            inner: rebound_inner,
            allowed_tools: owned.allowed_tools,
            filtered_tools: owned.filtered_tools,
        }))
    }

    fn supports_wait_interrupt(&self) -> bool {
        self.inner.supports_wait_interrupt()
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        owner_session_id: crate::types::SessionId,
    ) -> Result<Arc<dyn AgentToolDispatcher>, OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        let rebound_inner = owned.inner.bind_ops_lifecycle(registry, owner_session_id)?;
        Ok(Arc::new(FilteredToolDispatcher {
            inner: rebound_inner,
            allowed_tools: owned.allowed_tools,
            filtered_tools: owned.filtered_tools,
        }))
    }

    fn supports_ops_lifecycle_binding(&self) -> bool {
        self.inner.supports_ops_lifecycle_binding()
    }
}

/// Trait for session stores
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentSessionStore: Send + Sync {
    async fn save(&self, session: &Session) -> Result<(), AgentError>;
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}

/// Runtime policy for inlining peer lifecycle updates into session context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlinePeerNotificationPolicy {
    /// Always inline batched peer lifecycle updates.
    Always,
    /// Never inline batched peer lifecycle updates.
    Never,
    /// Inline only when post-drain peer count is at or below this threshold.
    AtMost(usize),
}

/// Default inline threshold when no explicit value is configured.
pub const DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS: usize = 50;

impl InlinePeerNotificationPolicy {
    /// Resolve policy from transport/build-layer config representation.
    pub fn try_from_raw(raw: Option<i32>) -> Result<Self, i32> {
        match raw {
            None => Ok(Self::AtMost(DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS)),
            Some(-1) => Ok(Self::Always),
            Some(0) => Ok(Self::Never),
            Some(v) if v > 0 => Ok(Self::AtMost(v as usize)),
            Some(v) => Err(v),
        }
    }
}

/// Error returned when a comms runtime capability is not available.
#[derive(Debug, thiserror::Error)]
pub enum CommsCapabilityError {
    /// The runtime does not support this capability.
    #[error("comms capability not supported: {0}")]
    Unsupported(String),
}

/// Trait for comms runtime that can be used with the agent
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

    #[doc(hidden)]
    fn stream(&self, scope: StreamScope) -> Result<EventStream, StreamError> {
        let scope_desc = match scope {
            StreamScope::Session(session_id) => format!("session {session_id}"),
            StreamScope::Interaction(interaction_id) => format!("interaction {}", interaction_id.0),
        };
        Err(StreamError::NotFound(scope_desc))
    }

    /// List peers visible to this runtime.
    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        Vec::new()
    }

    /// Count peers visible to this runtime.
    ///
    /// Implementations can override this to avoid materializing a full peer list.
    async fn peer_count(&self) -> usize {
        self.peers().await.len()
    }

    #[doc(hidden)]
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
    /// Get an event injector for this runtime's inbox.
    ///
    /// Surfaces use this to push external events into the agent inbox.
    /// Returns `None` if the implementation doesn't support event injection.
    fn event_injector(&self) -> Option<Arc<dyn crate::EventInjector>> {
        None
    }

    /// Internal runtime seam for interaction-scoped streaming.
    #[doc(hidden)]
    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn crate::event_injector::SubscribableInjector>> {
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
                content: crate::interaction::InteractionContent::Message {
                    body: text.clone(),
                    blocks: None,
                },
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

    /// Drain classified inbox interactions.
    ///
    /// Returns interactions with pre-computed classification from ingress.
    /// The host loop routes on the stored `PeerInputClass` instead of
    /// re-classifying after drain.
    ///
    /// Default returns `Unsupported`. Comms-enabled runtimes must override.
    async fn drain_classified_inbox_interactions(
        &self,
    ) -> Result<Vec<crate::interaction::ClassifiedInboxInteraction>, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "drain_classified_inbox_interactions".to_string(),
        ))
    }

    /// Get a notification that fires only for actionable peer input.
    ///
    /// Default returns `Unsupported`. Comms-enabled runtimes must override.
    /// Used by the factory to bridge into `WaitTool` interrupt.
    fn actionable_input_notify(&self) -> Result<Arc<tokio::sync::Notify>, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "actionable_input_notify".to_string(),
        ))
    }
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
    tool_scope: ToolScope,
    store: Arc<S>,
    session: Session,
    budget: Budget,
    retry_policy: RetryPolicy,
    state: LoopState,
    depth: u32,
    pub(super) comms_runtime: Option<Arc<dyn CommsRuntime>>,
    pub(super) hook_engine: Option<Arc<dyn HookEngine>>,
    pub(super) hook_run_overrides: HookRunOverrides,
    /// Optional context compaction strategy.
    pub(crate) compactor: Option<Arc<dyn crate::compact::Compactor>>,
    /// Input tokens from the last LLM response (for compaction trigger).
    pub(crate) last_input_tokens: u64,
    /// Session-scoped compaction cadence tracked across runs.
    pub(crate) compaction_cadence: SessionCompactionCadence,
    /// Optional memory store for indexing compaction discards.
    pub(crate) memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
    /// Optional skill engine for per-turn `/skill-ref` activation.
    pub(crate) skill_engine: Option<Arc<crate::skills::SkillRuntime>>,
    /// Skill references to resolve and inject for the next turn.
    /// Set by surfaces before calling `run()`, consumed on run start.
    pub pending_skill_references: Option<Vec<crate::skills::SkillKey>>,
    /// Per-interaction event tap for streaming events to subscribers.
    pub(crate) event_tap: crate::event_tap::EventTap,
    /// Shared control state for runtime system-context appends.
    pub(crate) system_context_state:
        Arc<std::sync::Mutex<crate::session::SessionSystemContextState>>,
    /// Optional default event channel configured at build time.
    /// Used by run methods when no per-call event channel is provided.
    pub(crate) default_event_tx: Option<tokio::sync::mpsc::Sender<crate::event::AgentEvent>>,
    /// Optional session checkpointer for host-mode persistence.
    #[allow(dead_code)] // Used by persistent session service; Phase 9-10 wiring pending
    pub(crate) checkpointer: Option<Arc<dyn crate::checkpoint::SessionCheckpointer>>,
    /// Comms intents that should be silently injected into the session
    /// without triggering an LLM turn. Matched against `InteractionContent::Request.intent`.
    #[allow(dead_code)] // Used by comms_impl when comms feature is enabled
    pub(crate) silent_comms_intents: Vec<String>,
    /// Runtime policy for inline peer lifecycle context injection.
    pub(crate) inline_peer_notification_policy: InlinePeerNotificationPolicy,
    /// Whether peer lifecycle updates are currently suppressed due to threshold policy.
    /// Used to inject suppression notice only on transition into suppressed mode.
    pub(crate) peer_notification_suppression_active: bool,
    /// Optional shared lifecycle registry for async operations.
    ///
    /// When set, the agent loop waits on the exact turn-local operation IDs
    /// registered in `turn_authority.pending_op_refs()`.
    pub(crate) ops_lifecycle: Option<Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>>,
    /// Machine authority for turn-execution state transitions (RMAT).
    pub(crate) turn_authority: crate::turn_execution_authority::TurnExecutionAuthority,
    /// True after the agentic loop completes when `output_schema` is set.
    /// Causes the next `CallingLlm` iteration to use extraction parameters
    /// (no tools, temperature 0.0, structured_output provider params).
    pub(crate) extraction_mode: bool,
    /// Populated on successful extraction validation — carried into RunResult.
    pub(crate) extraction_result: Option<serde_json::Value>,
    /// Schema warnings from compilation — carried into RunResult.
    pub(crate) extraction_schema_warnings: Option<Vec<crate::schema::SchemaWarning>>,
    /// Last validation error (for retry prompt).
    pub(crate) extraction_last_error: Option<String>,
    /// Session-level infrastructure delegation flag.
    ///
    /// When `true`, turn-boundary `drain_comms_inbox()` is suppressed because
    /// an external or session-owned host-mode drain owns inbox consumption.
    /// Shared so service/runtime shells can realize the canonical drain
    /// lifecycle without mutating the `Agent` directly.
    pub(crate) comms_drain_active: Arc<AtomicBool>,
}

#[cfg(test)]
mod tests {
    use super::{
        CommsRuntime, DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS, InlinePeerNotificationPolicy,
    };
    use crate::comms::{SendError, TrustedPeerSpec};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Notify;

    struct NoopCommsRuntime {
        notify: Arc<Notify>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

    #[test]
    fn test_inline_peer_notification_policy_from_raw() {
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(None),
            Ok(InlinePeerNotificationPolicy::AtMost(
                DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS
            ))
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(-1)),
            Ok(InlinePeerNotificationPolicy::Always)
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(0)),
            Ok(InlinePeerNotificationPolicy::Never)
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(25)),
            Ok(InlinePeerNotificationPolicy::AtMost(25))
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(-42)),
            Err(-42)
        );
    }

    #[test]
    fn test_filtered_dispatcher_bind_wait_interrupt_forwards() {
        use super::{AgentToolDispatcher, FilteredToolDispatcher};
        use crate::error::ToolError;
        use crate::ops::ToolDispatchOutcome;
        use crate::types::{ToolCallView, ToolDef};
        use serde_json::json;

        struct MockTool;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl AgentToolDispatcher for MockTool {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                vec![Arc::new(ToolDef {
                    name: "test".to_string(),
                    description: "test tool".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "required": []}),
                })]
                .into()
            }
            async fn dispatch(
                &self,
                _call: ToolCallView<'_>,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                Err(ToolError::not_found("test"))
            }
        }

        let inner: Arc<dyn AgentToolDispatcher> = Arc::new(MockTool);
        let filtered = Arc::new(FilteredToolDispatcher::new(inner, vec!["test".to_string()]));

        let (_tx, rx) = tokio::sync::watch::channel(None::<crate::wait_interrupt::WaitInterrupt>);

        // MockTool returns Unsupported (default); FilteredToolDispatcher
        // forwards to inner which also returns Unsupported.
        let result = filtered.bind_wait_interrupt(rx);
        assert!(matches!(
            result,
            Err(crate::wait_interrupt::WaitInterruptBindError::Unsupported)
        ));
    }

    #[test]
    #[allow(clippy::panic)]
    fn test_filtered_dispatcher_bind_wait_interrupt_shared_ownership() {
        use super::{AgentToolDispatcher, FilteredToolDispatcher};
        use crate::error::ToolError;
        use crate::ops::ToolDispatchOutcome;
        use crate::types::{ToolCallView, ToolDef};
        use serde_json::json;

        struct MockTool;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl AgentToolDispatcher for MockTool {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                vec![Arc::new(ToolDef {
                    name: "test".to_string(),
                    description: "test tool".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "required": []}),
                })]
                .into()
            }
            async fn dispatch(
                &self,
                _call: ToolCallView<'_>,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                Err(ToolError::not_found("test"))
            }
        }

        let inner: Arc<dyn AgentToolDispatcher> = Arc::new(MockTool);
        let filtered = Arc::new(FilteredToolDispatcher::new(inner, vec!["test".to_string()]));
        let _clone = Arc::clone(&filtered);

        let (_tx, rx) = tokio::sync::watch::channel(None::<crate::wait_interrupt::WaitInterrupt>);
        match filtered.bind_wait_interrupt(rx) {
            Err(crate::wait_interrupt::WaitInterruptBindError::SharedOwnership) => {}
            Ok(_) => panic!("expected SharedOwnership error, got Ok"),
            Err(e) => panic!("expected SharedOwnership, got {e:?}"),
        }
    }
}
