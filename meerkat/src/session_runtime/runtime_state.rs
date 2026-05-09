//! Runtime-state observers and cleanup primitives.
//!
//! Populated by W1-E (`PendingSessionEventStreams`,
//! `PendingSessionEventStreamDrop`), W2-E (`SessionInfo`,
//! `SessionState` enums + state observers), and W3-A (skill-identity
//! registry plumbing).
//!
//! `ArchiveRuntimeCleanup` deliberately stays in `meerkat-rpc` until
//! W3-A: its fields reference `SessionMcpState` (RPC-private) and
//! `meerkat_mob_mcp::MobMcpState` (a crate `meerkat` does not yet
//! depend on). Moving it now would require generics + trait bounds
//! (behaviour-changing) or new `meerkat` deps; either is broader than
//! Wave 1's "pure type move" charter. Tracked in W3-A.

use std::sync::Arc;

use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use tokio::sync::{Notify, broadcast};

/// Coarse session lifecycle marker for protocol surfaces.
///
/// Populated body lands in W2-E. Provided as a placeholder so the
/// public re-export from `super` compiles during F2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Session is staged or live in the runtime; populated body in W2-E.
    Active,
    /// Session has been archived or never existed.
    Archived,
}

/// Surface-facing summary of a session's state.
///
/// Populated body lands in W2-E.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Lifecycle marker.
    pub state: SessionState,
}

/// Per-session pending event-stream channel pair.
///
/// While a session is staged or archived but still emitting tail
/// events, the runtime keeps a broadcast sender alive so subscribers
/// drain remaining envelopes; `receiver_dropped` notifies the runtime
/// when the last subscriber drops so the channel can be reaped.
#[derive(Clone)]
pub struct PendingSessionEventStreams {
    /// Broadcast sender that fans out [`AgentEvent`] envelopes to live
    /// subscribers.
    pub events: broadcast::Sender<EventEnvelope<AgentEvent>>,
    /// Notifier that fires when the receiver-side handle is dropped, so
    /// the runtime can prune the entry from its event-stream map.
    pub receiver_dropped: Arc<Notify>,
}

/// RAII drop guard that fires `receiver_dropped` exactly once when the
/// receiver-side handle is dropped. Paired with
/// [`PendingSessionEventStreams::receiver_dropped`].
pub struct PendingSessionEventStreamDrop {
    /// Notifier shared with the corresponding
    /// [`PendingSessionEventStreams`] entry.
    pub receiver_dropped: Arc<Notify>,
}

impl Drop for PendingSessionEventStreamDrop {
    fn drop(&mut self) {
        self.receiver_dropped.notify_one();
    }
}
