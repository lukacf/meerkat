//! Thread-safe inbox for Meerkat comms.
//!
//! The inbox collects incoming messages from IO tasks and subagent results,
//! allowing the agent loop to drain them at turn boundaries.
//!
//! Includes a `Notify` mechanism to wake waiting tasks when new messages arrive.

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use crate::classify::IngressClassificationContext;
use crate::types::InboxItem;
use meerkat_core::PeerInputClass;

const DEFAULT_INBOX_CAPACITY: usize = 1024;

/// A classified inbox entry, pairing an item with its ingress classification.
pub(crate) struct ClassifiedInboxEntry {
    pub(crate) item: InboxItem,
    pub(crate) class: PeerInputClass,
    pub(crate) from_peer: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
}

/// The receiving end of the inbox, held by the agent loop.
pub struct Inbox {
    rx: mpsc::Receiver<InboxItem>,
    /// Notifier to wake waiting tasks when messages arrive.
    notify: Arc<Notify>,
    /// Classified entries channel (parallel to raw channel).
    classified_rx: Option<mpsc::Receiver<ClassifiedInboxEntry>>,
    /// Notifier that fires only for actionable inputs.
    actionable_notify: Option<Arc<Notify>>,
}

/// The sending end of the inbox, cloned to IO tasks.
#[derive(Clone)]
pub struct InboxSender {
    tx: mpsc::Sender<InboxItem>,
    /// Notifier to wake waiting tasks when messages arrive.
    notify: Arc<Notify>,
    /// Classification context (None for non-classified path).
    classification_context: Option<Arc<IngressClassificationContext>>,
    /// Classified entries sender (parallel to raw channel).
    classified_tx: Option<mpsc::Sender<ClassifiedInboxEntry>>,
    /// Notifier that fires only for actionable inputs.
    actionable_notify: Option<Arc<Notify>>,
}

impl Inbox {
    /// Create a new inbox, returning both the inbox and a sender.
    pub fn new() -> (Self, InboxSender) {
        Self::new_with_capacity(DEFAULT_INBOX_CAPACITY)
    }

    /// Create a new inbox with a bounded capacity.
    pub fn new_with_capacity(capacity: usize) -> (Self, InboxSender) {
        let (tx, rx) = mpsc::channel(capacity);
        let notify = Arc::new(Notify::new());
        (
            Inbox {
                rx,
                notify: notify.clone(),
                classified_rx: None,
                actionable_notify: None,
            },
            InboxSender {
                tx,
                notify,
                classification_context: None,
                classified_tx: None,
                actionable_notify: None,
            },
        )
    }

    /// Create a new inbox with ingress classification support.
    ///
    /// The classified path uses its own channel (`classified_tx`/`classified_rx`).
    /// `send_classified()` enqueues only on the classified channel.
    /// The raw `tx`/`rx` are kept for structural compatibility with `InboxSender::send()`
    /// but are not written to on the classified path (0.4.10 vestige; can be removed
    /// once `tx`/`rx` are made `Option` in a minor release).
    pub(crate) fn new_classified(
        context: Arc<IngressClassificationContext>,
    ) -> (Self, InboxSender) {
        let (tx, rx) = mpsc::channel(DEFAULT_INBOX_CAPACITY);
        let (classified_tx, classified_rx) = mpsc::channel(DEFAULT_INBOX_CAPACITY);
        let notify = Arc::new(Notify::new());
        let actionable_notify = Arc::new(Notify::new());
        (
            Inbox {
                rx,
                notify: notify.clone(),
                classified_rx: Some(classified_rx),
                actionable_notify: Some(actionable_notify.clone()),
            },
            InboxSender {
                tx,
                notify,
                classification_context: Some(context),
                classified_tx: Some(classified_tx),
                actionable_notify: Some(actionable_notify),
            },
        )
    }

    /// Get the notifier for waiting on new messages.
    ///
    /// Use this to wake a task when a message arrives, enabling
    /// interrupt-based wait patterns.
    pub fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Receive the next item from the inbox, blocking until one is available.
    pub async fn recv(&mut self) -> Option<InboxItem> {
        self.rx.recv().await
    }

    /// Try to drain all currently available items without blocking.
    pub fn try_drain(&mut self) -> Vec<InboxItem> {
        let mut items = Vec::new();
        while let Ok(item) = self.rx.try_recv() {
            items.push(item);
        }
        items
    }

    /// Try to drain all classified entries without blocking.
    pub(crate) fn try_drain_classified(&mut self) -> Vec<ClassifiedInboxEntry> {
        let mut entries = Vec::new();
        if let Some(ref mut rx) = self.classified_rx {
            while let Ok(entry) = rx.try_recv() {
                entries.push(entry);
            }
        }
        entries
    }

    /// Try to receive a single classified entry without blocking.
    pub(crate) fn try_recv_one_classified(&mut self) -> Option<ClassifiedInboxEntry> {
        self.classified_rx.as_mut()?.try_recv().ok()
    }

    /// Get the actionable-input notifier (fires only for actionable classes).
    pub(crate) fn classified_actionable_notify(&self) -> Option<Arc<Notify>> {
        self.actionable_notify.clone()
    }
}

impl InboxSender {
    /// Send an item to the inbox.
    ///
    /// On classified runtimes, delegates to `send_classified()` so the item
    /// goes through the classified queue (the sole consumer). On non-classified
    /// runtimes, enqueues on the raw channel directly.
    ///
    /// Returns an error if the inbox has been closed.
    pub fn send(&self, item: InboxItem) -> Result<(), InboxError> {
        // If classification context is available, route through classified path
        if self.classification_context.is_some() {
            return self.send_classified(item);
        }
        self.tx.try_send(item).map_err(|err| match err {
            mpsc::error::TrySendError::Closed(_) => InboxError::Closed,
            mpsc::error::TrySendError::Full(_) => InboxError::Full,
        })?;
        // Notify any waiting tasks
        self.notify.notify_waiters();
        Ok(())
    }

    /// Send an item with classification through the classified channel.
    ///
    /// Classifies the item using the ingress context. Items that classify
    /// as `None` (e.g., untrusted senders with `require_peer_auth`) are
    /// silently dropped — snapshot semantics, no resurrection.
    ///
    /// Classified items are enqueued on both the raw channel (backward compat)
    /// and the classified channel, then the appropriate notify is fired.
    pub(crate) fn send_classified(&self, item: InboxItem) -> Result<(), InboxError> {
        if let (Some(ctx), Some(classified_tx)) =
            (&self.classification_context, &self.classified_tx)
        {
            let result = match ctx.classify(&item) {
                Some(r) => r,
                None => {
                    // Dropped at ingress — untrusted or otherwise rejected.
                    // Do not enqueue, do not notify.
                    return Ok(());
                }
            };
            let entry = ClassifiedInboxEntry {
                item,
                class: result.class,
                from_peer: result.from_peer,
                lifecycle_peer: result.lifecycle_peer,
            };
            // Enqueue only on classified channel (no raw double-enqueue).
            // drain_classified_inbox_interactions() is the sole consumer.
            let is_actionable = entry.class.is_actionable();
            classified_tx.try_send(entry).map_err(|err| match err {
                mpsc::error::TrySendError::Closed(_) => InboxError::Closed,
                mpsc::error::TrySendError::Full(_) => InboxError::Full,
            })?;
            // Fire actionable notify only for actionable classes
            if is_actionable && let Some(ref actionable) = self.actionable_notify {
                actionable.notify_waiters();
            }
            // Always fire broad notify
            self.notify.notify_waiters();
            Ok(())
        } else {
            // Fallback: no classification context, just send raw
            self.send(item)
        }
    }
}

/// Errors that can occur with inbox operations.
#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    #[error("Inbox has been closed")]
    Closed,
    #[error("Inbox is full")]
    Full,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::identity::PubKey;
    use crate::types::{Envelope, MessageKind};
    use uuid::Uuid;

    fn make_test_envelope() -> Envelope {
        Envelope {
            id: Uuid::new_v4(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "test".to_string(),
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        }
    }

    #[test]
    fn test_inbox_struct() {
        let (inbox, _sender) = Inbox::new();
        // Inbox exists and has the receiver
        drop(inbox);
    }

    #[test]
    fn test_inbox_sender_struct() {
        let (_inbox, sender) = Inbox::new();
        // Sender exists and can be cloned
        let _sender2 = sender;
    }

    #[test]
    fn test_inbox_new() {
        let (inbox, sender) = Inbox::new();
        // Both parts exist
        drop(inbox);
        drop(sender);
    }

    #[tokio::test]
    async fn test_inbox_sender_send() {
        let (_inbox, sender) = Inbox::new();
        let item = InboxItem::External {
            envelope: make_test_envelope(),
        };
        let result = sender.send(item);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_inbox_recv() {
        let (mut inbox, sender) = Inbox::new();
        let envelope = make_test_envelope();
        let envelope_id = envelope.id;

        sender.send(InboxItem::External { envelope }).unwrap();

        let received = inbox.recv().await;
        assert!(received.is_some());
        match received.unwrap() {
            InboxItem::External { envelope } => {
                assert_eq!(envelope.id, envelope_id);
            }
            _ => panic!("expected External variant"),
        }
    }

    #[tokio::test]
    async fn test_inbox_try_drain() {
        let (mut inbox, sender) = Inbox::new();

        // Send multiple items
        for i in 0..3 {
            let mut envelope = make_test_envelope();
            envelope.id = Uuid::from_u128(i as u128);
            sender.send(InboxItem::External { envelope }).unwrap();
        }

        // Give a moment for items to be queued
        tokio::task::yield_now().await;

        // Drain all at once
        let items = inbox.try_drain();
        assert_eq!(items.len(), 3);

        // Verify IDs
        for (i, item) in items.into_iter().enumerate() {
            match item {
                InboxItem::External { envelope } => {
                    assert_eq!(envelope.id.as_u128(), i as u128);
                }
                _ => panic!("expected External variant"),
            }
        }

        // No more items
        let items = inbox.try_drain();
        assert!(items.is_empty());
    }

    #[test]
    fn test_sender_error_on_closed_inbox() {
        let (inbox, sender) = Inbox::new();
        drop(inbox); // Close the inbox

        let result = sender.send(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert!(result.is_err());
    }

    // Phase 3: Wait interruption tests

    #[test]
    fn test_inbox_has_notify() {
        let (inbox, _sender) = Inbox::new();
        // Inbox should expose a Notify
        let notify = inbox.notify();
        // Arc should be clonable
        drop(notify);
    }

    #[tokio::test]
    async fn test_sender_notifies_on_send() {
        let (inbox, sender) = Inbox::new();
        let notify = inbox.notify();

        // Spawn a task that waits for notification
        let notified = notify.notified();

        // Send a message - this should notify waiters
        sender
            .send(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .unwrap();

        // The notification should complete immediately (message was sent before we awaited)
        // Use timeout to ensure we don't hang if notify doesn't work
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), notified).await;

        assert!(result.is_ok(), "Should have been notified after send");
    }

    #[tokio::test]
    async fn test_notify_wakes_waiting_task() {
        let (inbox, sender) = Inbox::new();
        let notify = inbox.notify();

        // Spawn a task that waits for notification
        let handle = tokio::spawn(async move {
            notify.notified().await;
            "woken"
        });

        // Give the task time to start waiting
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Send a message - this should wake the waiting task
        sender
            .send(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .unwrap();

        // The task should complete
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), handle).await;

        assert!(result.is_ok(), "Task should have completed");
        assert_eq!(result.unwrap().unwrap(), "woken");
    }

    // === Classified inbox tests ===

    use crate::classify::IngressClassificationContext;
    use crate::trust::{TrustedPeer, TrustedPeers};

    fn make_classification_context(
        trusted: TrustedPeers,
        require_auth: bool,
    ) -> Arc<IngressClassificationContext> {
        Arc::new(IngressClassificationContext {
            require_peer_auth: require_auth,
            trusted_peers: Arc::new(parking_lot::RwLock::new(trusted)),
            silent_intents: Arc::new(std::collections::HashSet::new()),
        })
    }

    fn make_trusted(name: &str, pubkey: &PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: "inproc://test".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        }
    }

    #[tokio::test]
    async fn test_classified_inbox_actionable_notify_fires_for_message() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();
        let notified = actionable.notified();

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(100), notified).await;
        assert!(result.is_ok(), "Actionable notify should fire for messages");
    }

    #[tokio::test]
    async fn test_classified_inbox_actionable_notify_fires_for_request() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();
        let notified = actionable.notified();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Request {
            intent: "review".to_string(),
            params: serde_json::json!({}),
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(100), notified).await;
        assert!(result.is_ok(), "Actionable notify should fire for requests");
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_response() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Response {
            in_reply_to: Uuid::new_v4(),
            status: crate::types::Status::Completed,
            result: serde_json::json!({}),
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for responses"
        );
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_ack() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Ack {
            in_reply_to: Uuid::new_v4(),
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for acks"
        );
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_plain_event() {
        let ctx = make_classification_context(TrustedPeers::new(), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        sender
            .send_classified(InboxItem::PlainEvent {
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                interaction_id: None,
            })
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for plain events"
        );
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_subagent_result() {
        let ctx = make_classification_context(TrustedPeers::new(), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        sender
            .send_classified(InboxItem::SubagentResult {
                subagent_id: Uuid::new_v4(),
                result: serde_json::json!({}),
                summary: "done".to_string(),
            })
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for subagent results"
        );
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_lifecycle() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Request {
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({"peer": "new-agent"}),
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for lifecycle events"
        );
    }

    #[tokio::test]
    async fn test_classified_try_drain() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .unwrap();

        let entries = inbox.try_drain_classified();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].class,
            meerkat_core::PeerInputClass::ActionableMessage
        );
    }

    #[tokio::test]
    async fn test_classified_send_does_not_populate_raw_channel() {
        // Classified send only enqueues on the classified channel.
        // No raw double-enqueue; drain_classified_inbox_interactions() is the sole consumer.
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .unwrap();

        // Raw channel should be empty
        let raw = inbox.try_drain();
        assert_eq!(
            raw.len(),
            0,
            "classified send should not double-enqueue to raw channel"
        );
        // Classified channel should have the item
        let classified = inbox.try_drain_classified();
        assert_eq!(classified.len(), 1);
    }
}
