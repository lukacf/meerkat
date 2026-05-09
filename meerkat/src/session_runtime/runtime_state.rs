//! Runtime-state observers and cleanup primitives.
//!
//! Populated by W1-E (`ArchiveRuntimeCleanup`,
//! `PendingSessionEventStreams`, `PendingSessionEventStreamDrop`),
//! W2-E (`SessionInfo`, `SessionState` enums + state observers), and
//! W3-A (skill-identity registry plumbing).

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
