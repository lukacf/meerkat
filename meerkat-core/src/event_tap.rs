//! Interaction-scoped event tap for streaming events to per-interaction subscribers.
//!
//! The `EventTap` is a shared mutex checked by both the `LlmClientAdapter` (for TextDelta)
//! and the agent's `run_loop` (for lifecycle events). The host loop installs/clears a
//! subscriber per interaction.

use crate::event::AgentEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

/// State held inside an active event tap subscriber.
pub struct EventTapState {
    /// Channel sender for streaming events to the subscriber.
    pub tx: mpsc::Sender<AgentEvent>,
    /// Whether any events were dropped due to channel backpressure.
    pub truncated: AtomicBool,
}

/// A shared, optionally-active event tap.
///
/// When `Some(EventTapState)`, events are forwarded to the subscriber.
/// When `None`, no subscriber is active and tap operations are no-ops.
pub type EventTap = Arc<parking_lot::Mutex<Option<EventTapState>>>;

/// Create a new event tap with no active subscriber.
pub fn new_event_tap() -> EventTap {
    Arc::new(parking_lot::Mutex::new(None))
}

/// Best-effort send to the tap for streaming events (TextDelta, lifecycle).
///
/// Takes a reference to avoid unconditional cloning — the event is only cloned
/// when a subscriber is active and the channel has capacity.
///
/// On `TrySendError::Full`: sets `truncated = true`, attempts a one-time
/// `StreamTruncated` marker via direct `tx.try_send(...)`.
/// On `TrySendError::Closed`: no-op.
///
/// This is synchronous (no await) — safe to call from both sync and async contexts.
pub fn tap_try_send(tap: &EventTap, event: &AgentEvent) {
    let guard = tap.lock();
    let Some(state) = guard.as_ref() else {
        return;
    };
    match state.tx.try_send(event.clone()) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            if !state.truncated.swap(true, Ordering::Relaxed) {
                // First truncation — attempt one-time marker (also best-effort)
                let _ = state.tx.try_send(AgentEvent::StreamTruncated {
                    reason: "channel full".to_string(),
                });
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // Subscriber dropped — no-op
        }
    }
}

/// Send a terminal event (InteractionComplete/InteractionFailed) to the tap.
///
/// Clones the sender outside the lock, then uses `tokio::time::timeout(5s)`
/// to avoid stalling the host loop. On timeout: logs a warning and continues.
pub async fn tap_send_terminal(tap: &EventTap, event: AgentEvent) {
    let tx = {
        let guard = tap.lock();
        match guard.as_ref() {
            Some(state) => state.tx.clone(),
            None => return,
        }
    };

    match tokio::time::timeout(std::time::Duration::from_secs(5), tx.send(event)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            // Receiver dropped
        }
        Err(_) => {
            tracing::warn!("tap_send_terminal timed out after 5s; continuing");
        }
    }
}

/// Convenience: send event to tap (best-effort), then to primary channel.
///
/// The event is only cloned for the tap when a subscriber is active.
/// The owned event is moved into the primary channel send.
///
/// Returns `false` if the primary send failed (receiver dropped).
pub async fn tap_emit(
    tap: &EventTap,
    tx: Option<&mpsc::Sender<AgentEvent>>,
    event: AgentEvent,
) -> bool {
    tap_try_send(tap, &event);
    if let Some(tx) = tx {
        return tx.send(event).await.is_ok();
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn tap_try_send_none_is_noop() {
        let tap = new_event_tap();
        // Should not panic
        tap_try_send(
            &tap,
            &AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );
    }

    #[test]
    fn tap_try_send_delivers_to_active_subscriber() {
        let tap = new_event_tap();
        let (tx, mut rx) = mpsc::channel(16);
        {
            let mut guard = tap.lock();
            *guard = Some(EventTapState {
                tx,
                truncated: AtomicBool::new(false),
            });
        }

        tap_try_send(
            &tap,
            &AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );

        let event = rx.try_recv().unwrap();
        match event {
            AgentEvent::TextDelta { delta } => assert_eq!(delta, "hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn tap_try_send_full_channel_sets_truncated_and_sends_marker() {
        let tap = new_event_tap();
        // Channel with capacity 1
        let (tx, mut rx) = mpsc::channel(1);
        {
            let mut guard = tap.lock();
            *guard = Some(EventTapState {
                tx,
                truncated: AtomicBool::new(false),
            });
        }

        // Fill the channel
        tap_try_send(
            &tap,
            &AgentEvent::TextDelta {
                delta: "first".to_string(),
            },
        );

        // This should trigger truncation
        tap_try_send(
            &tap,
            &AgentEvent::TextDelta {
                delta: "second".to_string(),
            },
        );

        // Verify truncated flag
        {
            let guard = tap.lock();
            assert!(guard.as_ref().unwrap().truncated.load(Ordering::Relaxed));
        }

        // Drain and check: first event should be the TextDelta "first"
        let first = rx.try_recv().unwrap();
        assert!(
            matches!(&first, AgentEvent::TextDelta { delta } if delta == "first"),
            "Expected first TextDelta, got {:?}",
            first
        );
    }

    #[tokio::test]
    async fn tap_send_terminal_delivers_within_timeout() {
        let tap = new_event_tap();
        let (tx, mut rx) = mpsc::channel(16);
        {
            let mut guard = tap.lock();
            *guard = Some(EventTapState {
                tx,
                truncated: AtomicBool::new(false),
            });
        }

        tap_send_terminal(
            &tap,
            AgentEvent::RunCompleted {
                session_id: crate::types::SessionId::new(),
                result: "done".to_string(),
                usage: crate::types::Usage::default(),
            },
        )
        .await;

        let event = rx.try_recv().unwrap();
        assert!(matches!(event, AgentEvent::RunCompleted { .. }));
    }

    #[tokio::test]
    async fn tap_send_terminal_no_subscriber_is_noop() {
        let tap = new_event_tap();
        // Should not panic or hang
        tap_send_terminal(
            &tap,
            AgentEvent::RunCompleted {
                session_id: crate::types::SessionId::new(),
                result: "done".to_string(),
                usage: crate::types::Usage::default(),
            },
        )
        .await;
    }

    #[tokio::test]
    async fn tap_emit_sends_to_both_tap_and_primary() {
        let tap = new_event_tap();
        let (tap_tx, mut tap_rx) = mpsc::channel(16);
        let (primary_tx, mut primary_rx) = mpsc::channel(16);
        {
            let mut guard = tap.lock();
            *guard = Some(EventTapState {
                tx: tap_tx,
                truncated: AtomicBool::new(false),
            });
        }

        let ok = tap_emit(
            &tap,
            Some(&primary_tx),
            AgentEvent::TextDelta {
                delta: "both".to_string(),
            },
        )
        .await;
        assert!(ok);

        // Both channels should have the event
        let tap_event = tap_rx.try_recv().unwrap();
        let primary_event = primary_rx.try_recv().unwrap();
        assert!(matches!(tap_event, AgentEvent::TextDelta { delta } if delta == "both"));
        assert!(matches!(primary_event, AgentEvent::TextDelta { delta } if delta == "both"));
    }

    #[tokio::test]
    async fn tap_emit_primary_none_returns_true() {
        let tap = new_event_tap();
        let ok = tap_emit(
            &tap,
            None,
            AgentEvent::TextDelta {
                delta: "x".to_string(),
            },
        )
        .await;
        assert!(ok);
    }
}
