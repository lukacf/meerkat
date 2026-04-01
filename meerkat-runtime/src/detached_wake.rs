//! Detached-op wake state for idle keep-alive sessions.
//!
//! When a `BackgroundToolOp` reaches terminal in the ops lifecycle registry,
//! the registry sets `pending = true` and fires `notify`. The runtime loop
//! selects on the Notify directly and injects a `ContinuationInput` to wake
//! the agent. No separate waker task is needed — the runtime loop owns the
//! wake lifecycle, making it surface-agnostic (dogma: surfaces are skins,
//! not authorities).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::tokio::sync::Notify;

/// Per-session wake state for detached background operations.
///
/// Shared between:
/// - The ops lifecycle registry (`execute_effects` sets `pending` + fires `notify`)
/// - The runtime loop (selects on `notify`, checks `pending`, injects continuation)
///
/// `signaled` is used by `maybe_signal_detached_wake` as a secondary fallback
/// after queue processing, ensuring at most one outstanding wake cycle (INV-006).
#[derive(Debug)]
pub struct DetachedWakeState {
    /// A background op reached terminal since last drain.
    pub pending: Arc<AtomicBool>,
    /// A wake signal has been sent and not yet consumed (INV-006).
    pub signaled: Arc<AtomicBool>,
    /// Notification handle: fired by ops lifecycle, selected by runtime loop.
    pub notify: Arc<Notify>,
}

impl DetachedWakeState {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(AtomicBool::new(false)),
            signaled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl Default for DetachedWakeState {
    fn default() -> Self {
        Self::new()
    }
}
