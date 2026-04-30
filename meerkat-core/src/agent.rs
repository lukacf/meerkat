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
#[doc(hidden)]
pub mod test_turn_state_handle;
use crate::budget::Budget;
use crate::comms::{
    CommsCommand, EventStream, PeerDirectoryEntry, PeerId, SendAndStreamError, SendError,
    SendReceipt, StreamError, StreamScope, TrustedPeerDescriptor,
};
use crate::compact::SessionCompactionCadence;
use crate::completion_feed::CompletionSeq;
use crate::config::{AgentConfig, HookRunOverrides};
use crate::error::AgentError;
use crate::event::ExternalToolDelta;
use crate::hooks::HookEngine;
use crate::lifecycle::RunId;
use crate::lifecycle::run_primitive::ProviderParamsOverride;
use crate::ops::OperationId;
use crate::ops_lifecycle::{OperationKind, OperationStatus, OperationTerminalOutcome};
use crate::retry::RetryPolicy;
use crate::schema::{CompiledSchema, SchemaError};
use crate::session::Session;
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_catalog::{
    ToolCatalogCapabilities, ToolCatalogEntry, ToolCatalogMode, deferred_session_entry_count,
    select_catalog_mode_from_snapshot,
};
use crate::tool_scope::ToolScope;
use crate::turn_execution_authority::{
    ContentShape, TurnPhase, TurnPrimitiveKind, TurnTerminalOutcome,
};
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, OutputSchema, StopReason, ToolCallView,
    ToolDef, ToolName, ToolNameSet, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

pub use builder::AgentBuilder;
pub use runner::AgentRunner;

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
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Get the provider name
    fn provider(&self) -> &'static str;

    /// Get the current effective model identifier.
    ///
    /// Used by the agent loop for profile-default resolution (e.g., call timeout
    /// defaults that vary per model family). Must reflect the current model even
    /// after hot-swap.
    fn model(&self) -> &str;

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
        BlockAssistantMessage::new(self.blocks, self.stop_reason)
    }

    pub fn into_parts(self) -> (Vec<AssistantBlock>, StopReason, Usage) {
        (self.blocks, self.stop_reason, self.usage)
    }
}

/// Snapshot of the core agent's live execution state.
///
/// When a runtime-backed turn-state handle is attached, this snapshots the
/// runtime-owned turn machine; otherwise it falls back to the in-process
/// standalone turn state used by core-only execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExecutionSnapshot {
    pub loop_state: LoopState,
    pub turn_phase: TurnPhase,
    pub active_run_id: Option<RunId>,
    pub primitive_kind: TurnPrimitiveKind,
    pub admitted_content_shape: Option<ContentShape>,
    pub vision_enabled: bool,
    pub image_tool_results_enabled: bool,
    pub tool_calls_pending: u32,
    pub pending_operation_ids: Option<Vec<OperationId>>,
    pub barrier_operation_ids: Vec<OperationId>,
    pub has_barrier_ops: bool,
    pub barrier_satisfied: bool,
    pub boundary_count: u32,
    pub cancel_after_boundary: bool,
    pub terminal_outcome: TurnTerminalOutcome,
    pub extraction_attempts: u32,
    pub max_extraction_retries: u32,
    pub applied_cursor: CompletionSeq,
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
    /// Detached background operation completions since last poll.
    pub background_completions: Vec<DetachedOpCompletion>,
}

/// Completion notice for a detached background operation, projected from
/// canonical ops-lifecycle terminal state plus dispatcher-owned display metadata.
///
/// This is a rebuildable projection (INV-003), not authoritative state.
/// Terminal class and timing come from `OperationLifecycleSnapshot` (INV-001).
/// Shell-projected detail is supplementary display only (INV-002).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetachedOpCompletion {
    /// App-facing job identifier (the control noun for surfaces).
    pub job_id: String,
    /// Operation kind from canonical ops-lifecycle.
    pub kind: OperationKind,
    /// Terminal status from canonical ops-lifecycle.
    pub status: OperationStatus,
    /// Terminal outcome from canonical ops-lifecycle.
    pub terminal_outcome: Option<OperationTerminalOutcome>,
    /// Canonical display label from ops-lifecycle snapshot.
    pub display_name: String,
    /// Dispatcher-projected summary (exit code, output tail). Display only.
    pub detail: String,
    /// Monotonic elapsed millis from ops-lifecycle snapshot.
    pub elapsed_ms: Option<u64>,
}

/// Dispatcher binding capabilities — what optional bindings this dispatcher supports.
///
/// Returned by [`AgentToolDispatcher::capabilities`]. Replaces individual
/// `supports_*` boolean methods with a single structured query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DispatcherCapabilities {
    /// Whether `bind_ops_lifecycle` is implemented.
    pub ops_lifecycle: bool,
}

/// Result of a dispatcher binding operation.
///
/// Distinguishes "binding was applied" from "binding was skipped" so callers
/// can decide whether to wire downstream side effects (e.g. bridge tasks).
///
/// **Semantics (decision 11 — supported/best-effort/rejected):**
/// - `Ok(Bound(d))` = **supported** — binding succeeded, side effects should be wired
/// - `Ok(Skipped(d))` = **best-effort** — inner shared or incompatible, dispatcher unchanged
/// - `Err(SharedOwnership)` = **rejected** — outer wrapper is shared, caught by factory pre-check
/// - `Err(Unsupported)` = **rejected** — type doesn't support this binding, caught by `capabilities()`
pub enum BindOutcome {
    /// Binding was applied. The dispatcher was rebound.
    Bound(Arc<dyn AgentToolDispatcher>),
    /// Binding was skipped — inner dispatcher was shared or unsupported.
    /// The returned dispatcher is unchanged but safe to use.
    Skipped(Arc<dyn AgentToolDispatcher>),
}

impl BindOutcome {
    /// Extract the dispatcher, regardless of bind status.
    pub fn into_dispatcher(self) -> Arc<dyn AgentToolDispatcher> {
        match self {
            Self::Bound(d) | Self::Skipped(d) => d,
        }
    }

    /// Whether the binding was actually applied.
    pub fn was_bound(&self) -> bool {
        matches!(self, Self::Bound(_))
    }
}

/// Trait for tool dispatchers
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;

    /// Query exact catalog support for this dispatcher.
    ///
    /// Dispatchers report `exact_catalog=true` only when `tool_catalog()`
    /// returns the exact precedence-resolved winner registry for the plane
    /// they own. Wrappers that cannot prove exactness must leave this false.
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities::default()
    }

    /// Return the precedence-resolved tool catalog for this dispatcher.
    ///
    /// The default implementation mirrors `tools()` as a visible-only inline
    /// catalog. Callers must gate any deferred-catalog behavior on
    /// `tool_catalog_capabilities().exact_catalog`.
    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.tools()
            .iter()
            .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>()
            .into()
    }

    /// Return non-draining pending source names for exact-catalog discovery.
    ///
    /// Pending sources are catalog-level discovery metadata rather than
    /// provider-visible tools. The default implementation reports none.
    fn pending_catalog_sources(&self) -> Arc<[String]> {
        Arc::from([])
    }

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

    /// Snapshot the live external tool-surface machine state, if supported.
    ///
    /// This is a hidden diagnostic surface for MeerkatMachine mapping work.
    /// Dispatchers that do not own dynamic external tool mutation should
    /// return `None`.
    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        None
    }

    /// Query which optional bindings this dispatcher supports.
    fn capabilities(&self) -> DispatcherCapabilities {
        DispatcherCapabilities::default()
    }

    /// Bind a session-canonical ops registry into this dispatcher.
    ///
    /// Dispatchers that emit session-visible `AsyncOpRef`s must route those
    /// operation IDs into the bound registry. Under the identity-first Mob
    /// regime the owner binding passed here is the canonical bridge session
    /// binding, even though many compatibility surfaces still spell it
    /// `session_id`. Default returns Unsupported.
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        _registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        _owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        Err(OpsLifecycleBindError::Unsupported)
    }

    /// Return the completion enrichment provider, if available.
    ///
    /// Dispatchers with shell job management return a provider that maps
    /// operation IDs to display details (job ID, status detail string).
    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        None
    }

    /// Bind a session-scoped MCP server lifecycle handle (Phase 5G / T5g).
    ///
    /// Dispatchers that manage per-server MCP handshake lifecycle (like
    /// `McpRouterAdapter`) use the handle to mirror connection state into
    /// the session's MeerkatMachine DSL. The default implementation is a
    /// no-op for dispatchers that have no MCP handshake to route.
    fn bind_mcp_server_lifecycle_handle(
        &self,
        _handle: Arc<dyn crate::handles::McpServerLifecycleHandle>,
    ) {
    }

    /// Bind the session-canonical external tool-surface handle.
    ///
    /// MCP dispatchers use this to route add/remove/reload/call lifecycle
    /// semantics through the session's MeerkatMachine DSL instead of their
    /// standalone compatibility projection. The default implementation is a
    /// no-op for dispatchers that do not own dynamic external tool surfaces.
    fn bind_external_tool_surface_handle(
        &self,
        _handle: Arc<dyn crate::handles::ExternalToolSurfaceHandle>,
    ) {
    }
}

/// Compute whether the current exact catalog should stay inline or switch to deferred mode.
pub fn select_tool_catalog_mode<T>(dispatcher: &T) -> ToolCatalogMode
where
    T: AgentToolDispatcher + ?Sized,
{
    let capabilities = dispatcher.tool_catalog_capabilities();
    if !capabilities.exact_catalog {
        return ToolCatalogMode::Inline;
    }
    let pending_sources = dispatcher.pending_catalog_sources();
    let catalog = dispatcher.tool_catalog();
    select_catalog_mode_from_snapshot(
        capabilities.exact_catalog,
        catalog.as_ref(),
        pending_sources.as_ref(),
    )
}

/// Compute whether the catalog control plane should be composed for this
/// dispatcher, even if the current adaptive snapshot remains inline.
pub fn should_compose_tool_catalog_control_plane<T>(dispatcher: &T) -> bool
where
    T: AgentToolDispatcher + ?Sized,
{
    let capabilities = dispatcher.tool_catalog_capabilities();
    if !capabilities.exact_catalog {
        return false;
    }
    if capabilities.may_require_catalog_control_plane {
        return true;
    }

    let pending_sources = dispatcher.pending_catalog_sources();
    if !pending_sources.is_empty() {
        return true;
    }

    let catalog = dispatcher.tool_catalog();
    deferred_session_entry_count(catalog.as_ref()) > 0
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
/// Legacy tool lists are filtered once at construction time based on the
/// allowed_tools list. Exact-catalog dispatchers keep catalog callability live.
/// The inner dispatcher is used for actual dispatch, but only allowed tools are
/// exposed via tools() and dispatch() returns AccessDenied for filtered tools.
pub struct FilteredToolDispatcher<T: AgentToolDispatcher + ?Sized> {
    inner: Arc<T>,
    allowed_tools: ToolNameSet,
    /// Pre-computed filtered tool list for non-exact dispatchers.
    filtered_tools: Arc<[Arc<ToolDef>]>,
}

impl<T: AgentToolDispatcher + ?Sized> FilteredToolDispatcher<T> {
    pub fn new<I, N>(inner: Arc<T>, allowed_tools: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<ToolName>,
    {
        let allowed_set: ToolNameSet = allowed_tools
            .into_iter()
            .map(Into::into)
            .collect::<ToolNameSet>();

        let filtered: Vec<Arc<ToolDef>> = if inner.tool_catalog_capabilities().exact_catalog {
            inner
                .tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .filter(|t| allowed_set.contains(t.name.as_str()))
                .collect()
        } else {
            inner
                .tools()
                .iter()
                .filter(|t| allowed_set.contains(t.name.as_str()))
                .map(Arc::clone)
                .collect()
        };

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
        if self.inner.tool_catalog_capabilities().exact_catalog {
            return self
                .inner
                .tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .filter(|tool| self.allowed_tools.contains(tool.name.as_str()))
                .collect::<Vec<_>>()
                .into();
        }
        Arc::clone(&self.filtered_tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        if !self.allowed_tools.contains(call.name) {
            let inner_knows_tool = if self.inner.tool_catalog_capabilities().exact_catalog {
                self.inner
                    .tool_catalog()
                    .iter()
                    .any(|entry| entry.tool.name == call.name)
            } else {
                self.inner.tools().iter().any(|tool| tool.name == call.name)
            };
            if !inner_knows_tool {
                return Err(crate::error::ToolError::not_found(call.name));
            }
            return Err(crate::error::ToolError::access_denied(call.name));
        }
        self.inner.dispatch(call).await
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        if !self.inner.tool_catalog_capabilities().exact_catalog {
            return self
                .tools()
                .iter()
                .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
                .collect::<Vec<_>>()
                .into();
        }
        self.inner
            .tool_catalog()
            .iter()
            .filter(|entry| self.allowed_tools.contains(entry.tool.name.as_str()))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        self.inner.capabilities()
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        if Arc::strong_count(&owned.inner) == 1 {
            let outcome = owned
                .inner
                .bind_ops_lifecycle(registry, owner_bridge_session_id)?;
            let bound = outcome.was_bound();
            let d = outcome.into_dispatcher();
            let allowed_tools = owned.allowed_tools.into_iter().collect::<Vec<_>>();
            Ok(if bound {
                BindOutcome::Bound(Arc::new(FilteredToolDispatcher::new(d, allowed_tools)))
            } else {
                BindOutcome::Skipped(Arc::new(FilteredToolDispatcher::new(d, allowed_tools)))
            })
        } else {
            Ok(BindOutcome::Skipped(Arc::new(FilteredToolDispatcher {
                inner: owned.inner,
                allowed_tools: owned.allowed_tools,
                filtered_tools: owned.filtered_tools,
            })))
        }
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        self.inner.completion_enrichment()
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
    /// Canonical runtime routing identity for this peer, if available.
    ///
    /// `PeerId` is the UUID-shaped routing key used by peer directories and
    /// trust stores. Implementations that only have the legacy string carrier
    /// may return a parsed UUID-shaped `public_key()` value; implementations
    /// with Ed25519 public keys should override this and return the pubkey-
    /// derived canonical [`PeerId`].
    fn peer_id(&self) -> Option<PeerId> {
        self.public_key()
            .as_deref()
            .and_then(|public_key| PeerId::parse(public_key).ok())
    }

    /// Runtime-local transport/auth public key, if available.
    ///
    /// Returns an Ed25519 public key string in `ed25519:<base64>` format.
    /// This is not the canonical routing [`PeerId`]; use [`Self::peer_id`]
    /// for roster/projection identity and peer-directory lookups.
    fn public_key(&self) -> Option<String> {
        None
    }

    /// Runtime-local advertised comms address, if available.
    ///
    /// This is the canonical address the runtime expects peers to use when
    /// constructing a [`TrustedPeerDescriptor`]. Implementations that do not
    /// expose a stable advertised address can return `None`.
    fn advertised_address(&self) -> Option<String> {
        None
    }

    /// Runtime-local bootstrap proof for the initial supervisor bind, if
    /// available.
    fn bridge_bootstrap_token(&self) -> Option<String> {
        None
    }

    /// Register a trusted peer for future peer sends.
    ///
    /// Runtimes that manage trust dynamically should accept this as a mutable
    /// control-plane operation and return `SendError::Unsupported` if not
    /// available.
    async fn add_trusted_peer(&self, _peer: TrustedPeerDescriptor) -> Result<(), SendError> {
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

    /// Register a peer for admission-only trust without listing it in the
    /// directory.
    ///
    /// Used for control-plane edges — the canonical case is the supervisor
    /// bridge for session-backed mob members: lifecycle notifications
    /// (`mob.peer_added`, `mob.peer_retired`, …) must land at the member's
    /// inbox, but the supervisor must not appear as an ordinary sendable
    /// peer in `comms.peers` / REST / RPC / MCP. The admission gate consults
    /// both the public and private trust sets; `resolve_peer_directory()`
    /// consults only the public set.
    async fn add_private_trusted_peer(
        &self,
        _peer: TrustedPeerDescriptor,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "add_private_trusted_peer not supported for this CommsRuntime".to_string(),
        ))
    }

    /// Remove a previously registered private-trust edge by peer ID.
    ///
    /// Returns `true` if the edge was present and removed, `false` if it
    /// was not.
    async fn remove_private_trusted_peer(&self, _peer_id: &str) -> Result<bool, SendError> {
        Err(SendError::Unsupported(
            "remove_private_trusted_peer not supported for this CommsRuntime".to_string(),
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
                from_route: None,
                from: "unknown".into(),
                content: crate::interaction::InteractionContent::Message {
                    body: text.clone(),
                    blocks: None,
                },
                rendered_text: text,
                handling_mode: crate::types::HandlingMode::Queue,
                render_metadata: None,
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
    /// clean up registry entries. Called from the keep-alive loop after sending
    /// terminal events to the tap.
    fn mark_interaction_complete(&self, _id: &crate::interaction::InteractionId) {}

    /// Access the session's peer-interaction DSL handle (W1-A).
    ///
    /// Returns `None` for transport-only comms runtimes. A runtime that emits
    /// semantic peer request/response receipts must return `Some` after the
    /// surface installs machine authority.
    fn peer_interaction_handle(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::handles::PeerInteractionHandle>> {
        None
    }

    /// Access peer request/response authority only when the runtime has the
    /// complete machine-owned lifecycle pair.
    ///
    /// Semantic peer request/response ingress requires both the peer
    /// interaction handle and the paired interaction-stream handle. The stream
    /// handle itself stays hidden behind runtime ownership; this witness lets
    /// authority boundaries fail closed instead of treating a lone peer handle
    /// as sufficient.
    fn peer_request_response_authority_handle(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::handles::PeerInteractionHandle>> {
        None
    }

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

    /// Drain canonical peer/event ingress candidates.
    ///
    /// This remains the live runtime drain bridge for call sites that consume
    /// the `PeerInputCandidate` noun directly. The underlying drain unit is
    /// identical to `ClassifiedInboxInteraction`, so the default
    /// implementation simply forwards the classified drain path.
    async fn drain_peer_input_candidates(&self) -> Vec<crate::interaction::PeerInputCandidate> {
        self.drain_classified_inbox_interactions()
            .await
            .unwrap_or_default()
    }

    /// Snapshot the currently queued peer-ingress surface without draining it.
    ///
    /// This is a hidden diagnostic capability used while mapping the internal
    /// MeerkatMachine boundary onto existing comms ownership.
    async fn peer_ingress_queue_snapshot(
        &self,
    ) -> Result<crate::interaction::PeerIngressQueueSnapshot, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "peer_ingress_queue_snapshot".to_string(),
        ))
    }

    /// Snapshot the current peer runtime surface for MeerkatMachine mapping.
    ///
    /// This extends the queued ingress snapshot with the local trust membership
    /// that governs peer admission.
    async fn peer_ingress_runtime_snapshot(
        &self,
    ) -> Result<crate::interaction::PeerIngressRuntimeSnapshot, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "peer_ingress_runtime_snapshot".to_string(),
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
    /// Optional session checkpointer for keep-alive persistence.
    ///
    /// Wired by `AgentBuilder::with_checkpointer`, installed by
    /// `PersistentSessionService`, and consumed by
    /// `Agent::checkpoint_current_session`.
    pub(crate) checkpointer: Option<Arc<dyn crate::checkpoint::SessionCheckpointer>>,
    /// Optional blob store used to hydrate image refs at execution seams.
    pub(crate) blob_store: Option<Arc<dyn crate::BlobStore>>,
    /// Run-scoped diagnostic stash for originating hard-failure errors.
    ///
    /// When an exhausted hard LLM-call failure (e.g., CallTimeout, NetworkTimeout)
    /// routes through machine-owned FatalFailure, the originating `AgentError` is
    /// stashed here so `build_result()` can surface it instead of a generic
    /// `TerminalFailure`. This is ephemeral, consumed once, and non-authoritative
    /// for terminal phase/classification.
    pub(crate) pending_fatal_diagnostic: Option<AgentError>,
    /// True once the current run has accepted `RunCompleted` hooks.
    pub(crate) run_completed_hooks_applied: bool,
    /// Comms intents that should be silently injected into the session
    /// without triggering an LLM turn. Matched against `InteractionContent::Request.intent`.
    #[allow(dead_code)] // Used by comms_impl when comms feature is enabled
    pub(crate) silent_comms_intents: Vec<String>,
    /// Optional shared lifecycle registry for async operations.
    pub(crate) ops_lifecycle: Option<Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>>,
    /// Optional completion feed for cursor-based completion delivery.
    pub(crate) completion_feed: Option<Arc<dyn crate::completion_feed::CompletionFeed>>,
    /// Shared epoch cursor state for runtime-backed cursor writeback.
    pub(crate) epoch_cursor_state: Option<Arc<crate::runtime_epoch::EpochCursorState>>,
    /// Local cursor into the completion feed — only the agent boundary advances this.
    pub(crate) applied_cursor: crate::completion_feed::CompletionSeq,
    /// Optional enrichment provider for completion display details.
    pub(crate) completion_enrichment:
        Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>>,
    /// Shared effective mob authority handle. Owned by the agent, passed to
    /// mob tools at construction for authorization reads. Updated by
    /// `apply_session_effects` after each tool batch as a derived projection
    /// of the canonical `session.build_state().mob_tool_authority_context`.
    pub(crate) mob_authority_handle:
        Option<Arc<std::sync::RwLock<crate::service::MobToolAuthorityContext>>>,
    /// Runtime-backed turn-state handle, provided by the session runtime bindings.
    pub(crate) turn_state_handle: Option<Arc<dyn crate::TurnStateHandle>>,
    /// True when the runtime control plane must stamp execution kind metadata.
    pub(crate) runtime_execution_kind_required: bool,
    /// Typed execution intent for the current run, when this turn is owned by
    /// the runtime control plane rather than a direct surface call.
    pub(crate) runtime_execution_kind: Option<crate::lifecycle::RuntimeExecutionKind>,
    /// Runtime-backed external tool-surface diagnostic handle, when provided
    /// by the session runtime bindings.
    pub(crate) external_tool_surface_handle: Option<Arc<dyn crate::ExternalToolSurfaceHandle>>,
    /// Runtime-backed auth lease handle (Phase 1.5-rev).
    pub(crate) auth_lease_handle: Option<Arc<dyn crate::handles::AuthLeaseHandle>>,
    /// Runtime-backed MCP server lifecycle handle (Phase 5G / T5g). When set,
    /// the agent loop reads `pending_server_ids()` at each CallingLlm boundary
    /// to decide whether to emit the `[MCP_PENDING]` system notice.
    pub(crate) mcp_server_lifecycle_handle:
        Option<Arc<dyn crate::handles::McpServerLifecycleHandle>>,
    /// Shared live flag for cancellation at the next turn boundary.
    pub(crate) cancel_after_boundary_requested: Arc<std::sync::atomic::AtomicBool>,
    /// Optional resolver for model-specific operational defaults (e.g., call timeout).
    /// Consulted at each LLM call for hot-swap-aware profile default resolution.
    pub(crate) model_defaults_resolver:
        Option<Arc<dyn crate::model_defaults::ModelOperationalDefaultsResolver>>,
    /// Explicit call-timeout override from the build/config composition seam.
    /// Takes precedence over profile-derived defaults.
    pub(crate) call_timeout_override: crate::config::CallTimeoutOverride,
    /// Structured-output extraction state carried into RunResult.
    pub(crate) extraction_state: extraction::ExtractionState,
    /// Last published hidden deferred-catalog names.
    pub(crate) last_hidden_deferred_catalog_names: BTreeSet<String>,
    /// Last published pending catalog sources.
    pub(crate) last_pending_catalog_sources: BTreeSet<String>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::{
        CommsRuntime, DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS, InlinePeerNotificationPolicy,
    };
    use crate::comms::{
        PeerAddress, PeerId, PeerName, PeerTransport, SendError, TrustedPeerDescriptor,
    };
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
        let peer = TrustedPeerDescriptor {
            peer_id: PeerId::new(),
            name: PeerName::new("peer-a").expect("valid peer name"),
            address: PeerAddress::new(PeerTransport::Inproc, "peer-a"),
            pubkey: [0u8; 32],
        };
        let result = <NoopCommsRuntime as CommsRuntime>::add_trusted_peer(&runtime, peer).await;
        assert!(matches!(result, Err(SendError::Unsupported(_))));
    }

    #[tokio::test]
    async fn test_remove_trusted_peer_default_unsupported() {
        let runtime = NoopCommsRuntime {
            notify: Arc::new(Notify::new()),
        };
        let peer_id = PeerId::new().to_string();
        let result =
            <NoopCommsRuntime as CommsRuntime>::remove_trusted_peer(&runtime, &peer_id).await;
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

    /// UNIT-001: OperationStatus::is_terminal() returns true for all terminal
    /// variants and false for non-terminal ones.
    #[test]
    fn unit_001_terminal_status_values() {
        use crate::ops_lifecycle::OperationStatus;
        assert!(OperationStatus::Completed.is_terminal());
        assert!(OperationStatus::Failed.is_terminal());
        assert!(OperationStatus::Cancelled.is_terminal());
        assert!(OperationStatus::Aborted.is_terminal());
        assert!(OperationStatus::Retired.is_terminal());
        assert!(OperationStatus::Terminated.is_terminal());
        assert!(!OperationStatus::Running.is_terminal());
        assert!(!OperationStatus::Provisioning.is_terminal());
        assert!(!OperationStatus::Retiring.is_terminal());
        assert!(!OperationStatus::Absent.is_terminal());
    }

    /// UNIT-002: DetachedOpCompletion serializes without operation_id.
    /// The app-facing control noun is job_id (CONTRACT-003).
    #[test]
    fn unit_002_detached_op_completion_has_no_operation_id() {
        use crate::agent::DetachedOpCompletion;
        use crate::ops_lifecycle::{OperationKind, OperationStatus};

        let completion = DetachedOpCompletion {
            job_id: "j_test".into(),
            kind: OperationKind::BackgroundToolOp,
            status: OperationStatus::Completed,
            terminal_outcome: None,
            display_name: "test cmd".into(),
            detail: "ok".into(),
            elapsed_ms: None,
        };
        #[allow(clippy::unwrap_used)]
        let json = serde_json::to_value(&completion).unwrap();
        assert!(
            json.get("operation_id").is_none(),
            "operation_id must not appear in serialized DetachedOpCompletion (CONTRACT-003)"
        );
        assert!(
            json.get("job_id").is_some(),
            "job_id must be the app-facing control noun"
        );
    }
}
