use crate::event::AgentEvent;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::{Duration, timeout};

pub struct EventTapState {
    pub tx: mpsc::Sender<AgentEvent>,
    pub truncated: AtomicBool,
}

pub type EventTap = Arc<Mutex<Option<EventTapState>>>;

pub fn new_event_tap() -> EventTap {
    Arc::new(Mutex::new(None))
}

pub fn tap_try_send(tap: &EventTap, event: AgentEvent) {
    let guard = tap.lock();
    let Some(state) = guard.as_ref() else {
        return;
    };
    match state.tx.try_send(event) {
        Ok(()) => {}
        Err(TrySendError::Closed(_)) => {}
        Err(TrySendError::Full(_)) => {
            if !state.truncated.swap(true, Ordering::SeqCst) {
                let _ = state.tx.try_send(AgentEvent::StreamTruncated {
                    reason: "event tap channel is full".to_string(),
                });
            }
        }
    }
}

pub async fn tap_send_terminal(tap: &EventTap, event: AgentEvent) {
    let tx = tap.lock().as_ref().map(|state| state.tx.clone());
    let Some(tx) = tx else {
        return;
    };

    match timeout(Duration::from_secs(5), tx.send(event)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {}
        Err(_) => {
            tracing::warn!("timed out sending terminal event to interaction tap");
        }
    }
}

pub async fn tap_emit(
    tap: &EventTap,
    tx: Option<&mpsc::Sender<AgentEvent>>,
    event: AgentEvent,
) -> bool {
    tap_try_send(tap, event.clone());
    if let Some(primary) = tx {
        return primary.send(event).await.is_ok();
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tap_try_send_none_is_noop() {
        let tap = new_event_tap();
        tap_try_send(
            &tap,
            AgentEvent::TextDelta {
                delta: "x".to_string(),
            },
        );
    }

    #[tokio::test]
    async fn tap_try_send_active_delivers_event() {
        let tap = new_event_tap();
        let (tx, mut rx) = mpsc::channel(2);
        tap.lock().replace(EventTapState {
            tx,
            truncated: AtomicBool::new(false),
        });

        tap_try_send(
            &tap,
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );

        let event = rx.recv().await.expect("event");
        match event {
            AgentEvent::TextDelta { delta } => assert_eq!(delta, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tap_try_send_full_sets_truncated_flag() {
        let tap = new_event_tap();
        let (tx, mut rx) = mpsc::channel(1);
        tap.lock().replace(EventTapState {
            tx,
            truncated: AtomicBool::new(false),
        });

        tap_try_send(
            &tap,
            AgentEvent::TextDelta {
                delta: "first".to_string(),
            },
        );
        tap_try_send(
            &tap,
            AgentEvent::TextDelta {
                delta: "second".to_string(),
            },
        );

        let is_truncated = tap
            .lock()
            .as_ref()
            .map(|state| state.truncated.load(Ordering::SeqCst))
            .unwrap_or(false);
        assert!(is_truncated);

        let _ = rx.recv().await;
    }

    #[tokio::test]
    async fn tap_try_send_closed_channel_is_noop() {
        let tap = new_event_tap();
        let (tx, rx) = mpsc::channel(1);
        tap.lock().replace(EventTapState {
            tx,
            truncated: AtomicBool::new(false),
        });
        drop(rx);

        tap_try_send(
            &tap,
            AgentEvent::TextDelta {
                delta: "x".to_string(),
            },
        );
        let truncated = tap
            .lock()
            .as_ref()
            .map(|s| s.truncated.load(Ordering::SeqCst))
            .unwrap_or(false);
        assert!(!truncated);
    }

    #[tokio::test]
    async fn tap_send_terminal_delivers_within_timeout() {
        let tap = new_event_tap();
        let (tx, mut rx) = mpsc::channel(1);
        tap.lock().replace(EventTapState {
            tx,
            truncated: AtomicBool::new(false),
        });

        tap_send_terminal(
            &tap,
            AgentEvent::InteractionComplete {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::now_v7()),
                result: "ok".to_string(),
            },
        )
        .await;

        let got = rx.recv().await.expect("terminal event");
        assert!(matches!(got, AgentEvent::InteractionComplete { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn tap_send_terminal_timeout_does_not_stall() {
        let tap = new_event_tap();
        let (tx, _rx) = mpsc::channel(1);
        tap.lock().replace(EventTapState {
            tx: tx.clone(),
            truncated: AtomicBool::new(false),
        });

        tx.try_send(AgentEvent::TextDelta {
            delta: "fill".to_string(),
        })
        .expect("fill channel");

        let fut = tap_send_terminal(
            &tap,
            AgentEvent::InteractionComplete {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::now_v7()),
                result: "done".to_string(),
            },
        );

        tokio::pin!(fut);
        tokio::time::advance(Duration::from_secs(6)).await;
        fut.await;
    }

    #[tokio::test]
    async fn tap_emit_sends_to_tap_and_primary() {
        let tap = new_event_tap();
        let (tap_tx, mut tap_rx) = mpsc::channel(1);
        tap.lock().replace(EventTapState {
            tx: tap_tx,
            truncated: AtomicBool::new(false),
        });
        let (primary_tx, mut primary_rx) = mpsc::channel(1);

        let ok = tap_emit(
            &tap,
            Some(&primary_tx),
            AgentEvent::TextDelta {
                delta: "z".to_string(),
            },
        )
        .await;
        assert!(ok);
        assert!(matches!(
            tap_rx.recv().await,
            Some(AgentEvent::TextDelta { .. })
        ));
        assert!(matches!(
            primary_rx.recv().await,
            Some(AgentEvent::TextDelta { .. })
        ));
    }
}
