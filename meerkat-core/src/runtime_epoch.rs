//! Runtime epoch identity and session runtime bindings.
//!
//! A runtime epoch identifies a continuous async-ordering domain for a session.
//! `SessionRuntimeBindings` bundles the epoch-local runtime facts that the
//! factory consumes but never creates for runtime-backed surfaces.
//!
//! Design rule: one build consumes bindings, it does not create them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::completion_feed::CompletionSeq;
use crate::handles::{
    AuthLeaseHandle, CommsDrainHandle, ExternalToolSurfaceHandle, McpServerLifecycleHandle,
    PeerCommsHandle, PeerInteractionHandle, SessionAdmissionHandle, SessionClaimHandle,
    SessionContextHandle, TurnStateHandle,
};
use crate::ops_lifecycle::OpsLifecycleRegistry;
use crate::tool_scope::ToolVisibilityOwner;
use crate::types::SessionId;

/// Unique identifier for a runtime epoch (UUID v7 for time-ordering).
///
/// A runtime epoch identifies a continuous async-ordering domain. The same
/// session may span multiple epochs (e.g., after reset or process restart
/// without durable recovery). The same identity may span multiple sessions
/// and epochs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeEpochId(pub Uuid);

impl RuntimeEpochId {
    /// Create a new epoch ID using UUID v7.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for RuntimeEpochId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RuntimeEpochId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Shared consumer cursor state for the epoch.
///
/// Written by the agent boundary and runtime loop; read by the persistence
/// channel for snapshotting. Atomics provide lock-free monotonic updates.
///
/// Cursor values may be stale relative to the agent's true position when
/// read for persistence — this is safe (stale cursors produce duplicate
/// notices on recovery, never lost notices).
pub struct EpochCursorState {
    /// Agent's `applied_cursor` — advanced at the CallingLlm boundary.
    pub agent_applied_cursor: AtomicU64,
    /// Runtime loop's `observed_seq` — advanced after feed reads.
    pub runtime_observed_seq: AtomicU64,
    /// Runtime loop's `last_injected_seq` — advanced after continuation injection.
    pub runtime_last_injected_seq: AtomicU64,
}

impl EpochCursorState {
    /// Create fresh cursor state (all zeros).
    pub fn new() -> Self {
        Self {
            agent_applied_cursor: AtomicU64::new(0),
            runtime_observed_seq: AtomicU64::new(0),
            runtime_last_injected_seq: AtomicU64::new(0),
        }
    }

    /// Create from recovered persisted values.
    pub fn from_recovered(
        agent_applied_cursor: CompletionSeq,
        runtime_observed_seq: CompletionSeq,
        runtime_last_injected_seq: CompletionSeq,
    ) -> Self {
        Self {
            agent_applied_cursor: AtomicU64::new(agent_applied_cursor),
            runtime_observed_seq: AtomicU64::new(runtime_observed_seq),
            runtime_last_injected_seq: AtomicU64::new(runtime_last_injected_seq),
        }
    }

    /// Snapshot current cursor values for persistence.
    pub fn snapshot(&self) -> EpochCursorSnapshot {
        EpochCursorSnapshot {
            agent_applied_cursor: self.agent_applied_cursor.load(Ordering::Acquire),
            runtime_observed_seq: self.runtime_observed_seq.load(Ordering::Acquire),
            runtime_last_injected_seq: self.runtime_last_injected_seq.load(Ordering::Acquire),
        }
    }
}

impl Default for EpochCursorState {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for EpochCursorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpochCursorState")
            .field(
                "agent_applied_cursor",
                &self.agent_applied_cursor.load(Ordering::Relaxed),
            )
            .field(
                "runtime_observed_seq",
                &self.runtime_observed_seq.load(Ordering::Relaxed),
            )
            .field(
                "runtime_last_injected_seq",
                &self.runtime_last_injected_seq.load(Ordering::Relaxed),
            )
            .finish()
    }
}

/// Serializable snapshot of cursor values, captured for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochCursorSnapshot {
    pub agent_applied_cursor: CompletionSeq,
    pub runtime_observed_seq: CompletionSeq,
    pub runtime_last_injected_seq: CompletionSeq,
}

/// Bundle of epoch-local runtime facts.
///
/// Created by the runtime epoch owner ([`MeerkatMachine::prepare_bindings`]),
/// consumed by the factory. The factory never creates competing registries
/// when it receives this bundle.
///
/// The `session_id` field acts as an identity witness: the factory validates
/// that `bindings.session_id == session.id()` to catch cross-wired bindings.
pub struct SessionRuntimeBindings {
    /// Session this binding was prepared for. Factory validates this matches
    /// the session being built.
    pub session_id: SessionId,
    /// Epoch identity — stable across rebuilds within the same epoch,
    /// rotated on reset/restart-without-recovery.
    pub epoch_id: RuntimeEpochId,
    /// Canonical ops lifecycle registry for this epoch.
    pub ops_lifecycle: Arc<dyn OpsLifecycleRegistry>,
    /// Shared consumer cursor state for this epoch.
    pub cursor_state: Arc<EpochCursorState>,
    /// Canonical durable tool-visibility owner for this session/runtime binding.
    pub tool_visibility_owner: Arc<dyn ToolVisibilityOwner>,
    /// Turn-execution DSL handle (Phase 5F/0 addition).
    pub turn_state: Arc<dyn TurnStateHandle>,
    /// Comms drain lifecycle DSL handle (Phase 5F/0 addition).
    pub comms_drain: Arc<dyn CommsDrainHandle>,
    /// External tool surface DSL handle (Phase 5F/0 addition).
    pub external_tool_surface: Arc<dyn ExternalToolSurfaceHandle>,
    /// Peer comms classification DSL handle (Phase 5F/0 addition).
    pub peer_comms: Arc<dyn PeerCommsHandle>,
    /// Session turn-admission DSL handle (Phase 5F/0 addition).
    pub session_admission: Arc<dyn SessionAdmissionHandle>,
    /// Auth lease lifecycle DSL handle (Phase 1.5-rev addition).
    pub auth_lease: Arc<dyn AuthLeaseHandle>,
    /// MCP server lifecycle DSL handle (Phase 5G / T5g addition).
    ///
    /// Routes per-server MCP handshake events into the session's MeerkatMachine
    /// DSL (`mcp_server_states` substate) and exposes the `PendingConnect` set
    /// to the agent loop for the `[MCP_PENDING]` system-notice toggle.
    pub mcp_server_lifecycle: Arc<dyn McpServerLifecycleHandle>,
    /// Peer interaction lifecycle DSL handle (W1-A / issue #264).
    ///
    /// Optional: surfaces without a runtime-owned session DSL (WASM,
    /// standalone ephemeral tests) leave this `None` and the comms runtime
    /// falls back to inline channel bookkeeping. Surfaces with a real session
    /// runtime populate this with [`RuntimePeerInteractionHandle`] sharing the
    /// same `HandleDslAuthority` as the other five handles.
    pub peer_interaction: Option<Arc<dyn PeerInteractionHandle>>,
    /// Session-context advancement DSL handle (W2-E / issue #264).
    ///
    /// Fires `AdvanceSessionContext` at every canonical session-truth
    /// mutation site so the realtime projection consumer can subscribe to
    /// a typed `SessionContextAdvanced` effect instead of polling a watch
    /// channel. Shares the same `HandleDslAuthority` as the other handles.
    pub session_context: Arc<dyn SessionContextHandle>,
    /// Session-identity claim handle owned by the runtime (dogma #2).
    ///
    /// Comms runtimes built for this session acquire their typed
    /// [`SessionClaim`] through this handle. The canonical owner lives on
    /// `MeerkatMachine` so per-process uniqueness is enforced by the
    /// runtime, not by process-global shell statics.
    ///
    /// [`SessionClaim`]: crate::handles::SessionClaim
    pub session_claim_handle: Arc<dyn SessionClaimHandle>,
}

impl Clone for SessionRuntimeBindings {
    fn clone(&self) -> Self {
        Self {
            session_id: self.session_id.clone(),
            epoch_id: self.epoch_id.clone(),
            ops_lifecycle: Arc::clone(&self.ops_lifecycle),
            cursor_state: Arc::clone(&self.cursor_state),
            tool_visibility_owner: Arc::clone(&self.tool_visibility_owner),
            turn_state: Arc::clone(&self.turn_state),
            comms_drain: Arc::clone(&self.comms_drain),
            external_tool_surface: Arc::clone(&self.external_tool_surface),
            peer_comms: Arc::clone(&self.peer_comms),
            session_admission: Arc::clone(&self.session_admission),
            auth_lease: Arc::clone(&self.auth_lease),
            mcp_server_lifecycle: Arc::clone(&self.mcp_server_lifecycle),
            peer_interaction: self.peer_interaction.as_ref().map(Arc::clone),
            session_context: Arc::clone(&self.session_context),
            session_claim_handle: Arc::clone(&self.session_claim_handle),
        }
    }
}

impl std::fmt::Debug for SessionRuntimeBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRuntimeBindings")
            .field("session_id", &self.session_id)
            .field("epoch_id", &self.epoch_id)
            .field("ops_lifecycle", &"<dyn OpsLifecycleRegistry>")
            .field("cursor_state", &self.cursor_state)
            .field("tool_visibility_owner", &"<dyn ToolVisibilityOwner>")
            .field("turn_state", &"<dyn TurnStateHandle>")
            .field("comms_drain", &"<dyn CommsDrainHandle>")
            .field("external_tool_surface", &"<dyn ExternalToolSurfaceHandle>")
            .field("peer_comms", &"<dyn PeerCommsHandle>")
            .field("session_admission", &"<dyn SessionAdmissionHandle>")
            .field("auth_lease", &"<dyn AuthLeaseHandle>")
            .field("mcp_server_lifecycle", &"<dyn McpServerLifecycleHandle>")
            .field(
                "peer_interaction",
                &self
                    .peer_interaction
                    .as_ref()
                    .map(|_| "<dyn PeerInteractionHandle>"),
            )
            .field("session_context", &"<dyn SessionContextHandle>")
            .field("session_claim_handle", &"<dyn SessionClaimHandle>")
            .finish()
    }
}

/// Discriminant for how the factory should resolve async-operation lifecycle resources.
///
/// - `StandaloneEphemeral`: factory creates local-only ephemeral bindings.
///   Suitable for WASM, tests, embedded, and doc examples.
/// - `SessionOwned`: factory consumes pre-created bindings from the runtime
///   epoch owner. Never creates a competing registry.
///
/// The `SessionOwned` variant is intentionally large — it carries the full
/// bundle of Arc-wrapped DSL handles and registries. Every value passes
/// through the factory exactly once, never lands in a collection, so paying
/// for an extra heap indirection on every construction would regress the hot
/// path. The `StandaloneEphemeral` path only appears in WASM, tests, and
/// standalone embedded runs.
#[allow(clippy::large_enum_variant)]
pub enum RuntimeBuildMode {
    /// Standalone: factory creates local-only ephemeral bindings.
    StandaloneEphemeral,
    /// Runtime-backed: factory consumes pre-created bindings. The epoch_id
    /// and session_id serve as identity witnesses.
    SessionOwned(SessionRuntimeBindings),
}

impl Clone for RuntimeBuildMode {
    fn clone(&self) -> Self {
        match self {
            Self::StandaloneEphemeral => Self::StandaloneEphemeral,
            Self::SessionOwned(b) => Self::SessionOwned(b.clone()),
        }
    }
}

impl std::fmt::Debug for RuntimeBuildMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StandaloneEphemeral => write!(f, "StandaloneEphemeral"),
            Self::SessionOwned(b) => f
                .debug_tuple("SessionOwned")
                .field(&b.session_id)
                .field(&b.epoch_id)
                .finish(),
        }
    }
}
