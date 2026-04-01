//! Detached-op wake task for idle keep-alive sessions.
//!
//! When a `BackgroundToolOp` reaches terminal in the ops lifecycle registry,
//! the registry sets `pending = true`. This task waits for the session to
//! become quiescent, then injects a derived `ContinuationInput` through the
//! canonical ingress seam to wake the agent.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use meerkat_core::types::SessionId;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::input::{ContinuationInput, Input};
use crate::session_adapter::RuntimeSessionAdapter;

/// Per-session wake state for detached background operations.
///
/// Shared between the ops lifecycle registry (sets `pending`) and the
/// session adapter (checks quiescence and signals).
#[derive(Debug)]
pub struct DetachedWakeState {
    /// A background op reached terminal since last drain.
    pub pending: Arc<AtomicBool>,
    /// A wake signal has been sent and not yet consumed.
    pub signaled: Arc<AtomicBool>,
    /// Notification handle for the waker task.
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

/// Spawn the detached-op waker task for a session.
///
/// The task loops: await notify -> re-check pending -> admit continuation.
/// Exits when the adapter rejects the input (session gone).
pub fn spawn_detached_op_waker(
    adapter: Arc<RuntimeSessionAdapter>,
    session_id: SessionId,
    state: Arc<DetachedWakeState>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            state.notify.notified().await;

            // Re-check: is there actually pending work?
            if !state.pending.load(Ordering::Acquire) {
                state.signaled.store(false, Ordering::Release);
                continue;
            }

            let input = Input::Continuation(ContinuationInput::detached_background_op_completed());

            match adapter
                .accept_input_with_completion(&session_id, input)
                .await
            {
                Ok(_) => {
                    // Continuation admitted — clear both flags
                    state.pending.store(false, Ordering::Release);
                    state.signaled.store(false, Ordering::Release);
                }
                Err(_) => {
                    // Session gone or rejected — exit the waker task
                    break;
                }
            }
        }
    })
}
