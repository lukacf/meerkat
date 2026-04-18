//! Thread-safe inbox for Meerkat comms.
//!
//! The inbox collects incoming peer and external-event traffic,
//! allowing the agent loop to drain them at turn boundaries.
//!
//! Includes a `Notify` mechanism to wake waiting tasks when new messages arrive.

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use crate::classify::IngressClassificationContext;
use crate::classify::PreparedIngressItem;
use crate::peer_types::{PeerId, PeerIngressState};
use crate::types::InboxItem;
use meerkat_core::{
    InteractionId, PeerIngressEntrySnapshot, PeerIngressKind, PeerIngressQueueSnapshot,
    PeerInputClass,
};
use std::collections::BTreeSet;

const DEFAULT_INBOX_CAPACITY: usize = 1024;

/// A classified inbox entry, pairing an item with its ingress classification.
#[derive(Debug)]
pub(crate) struct ClassifiedInboxEntry {
    pub(crate) raw_item_id: String,
    pub(crate) item: InboxItem,
    pub(crate) class: PeerInputClass,
    pub(crate) auth_exempt: bool,
    pub(crate) kind: PeerIngressKind,
    pub(crate) from_peer: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
    pub(crate) request_id: Option<String>,
    pub(crate) trusted_snapshot: Option<bool>,
    pub(crate) text_projection: String,
}

#[derive(Debug)]
struct ClassifiedInboxQueue {
    capacity: usize,
    closed: bool,
    entries: VecDeque<ClassifiedInboxEntry>,
    auth_required: bool,
    trusted_peers: BTreeSet<PeerId>,
    phase: PeerIngressState,
}

impl ClassifiedInboxQueue {
    fn new(capacity: usize, auth_required: bool) -> Self {
        Self {
            capacity,
            closed: false,
            entries: VecDeque::new(),
            auth_required,
            trusted_peers: BTreeSet::new(),
            phase: PeerIngressState::Absent,
        }
    }

    fn sync_trusted_peer_added(&mut self, peer_id: &str) {
        self.trusted_peers.insert(PeerId(peer_id.to_string()));
    }

    fn sync_trusted_peer_removed(&mut self, peer_id: &str) {
        self.trusted_peers.remove(&PeerId(peer_id.to_string()));
    }

    /// Admit a prepared item into the peer-ingress lifecycle.
    ///
    /// Returns `(should_enqueue, trusted_snapshot)`:
    /// - For plain events: always enqueue, no trust snapshot.
    /// - For external envelopes: consult the trust set + auth-required policy.
    ///   Untrusted senders with auth required are dropped (no enqueue); the
    ///   trust snapshot still records the decision for queue visibility.
    ///
    /// Updates `phase` to track the observable lifecycle:
    /// `Absent → Received` (admitted), `Received → Received` (more work),
    /// `Absent → Dropped` (untrusted with empty queue), untrusted with queued
    /// work keeps `Received`.
    fn admit_peer_receive(&mut self, prepared: &PreparedIngressItem) -> (bool, Option<bool>) {
        let InboxItem::External { envelope } = &prepared.item else {
            return (true, None);
        };
        let peer_id = PeerId(envelope.from.to_peer_id());
        let trusted = self.trusted_peers.contains(&peer_id);
        let admitted = trusted || !self.auth_required;
        let had_queued_work = !self.entries.is_empty();

        if admitted {
            self.phase = PeerIngressState::Received;
            (true, Some(trusted))
        } else if had_queued_work {
            self.phase = PeerIngressState::Received;
            (false, Some(false))
        } else {
            self.phase = PeerIngressState::Dropped;
            (false, Some(false))
        }
    }

    /// Advance `phase` on dequeue.
    ///
    /// External items drive the transition: an empty queue after dequeue moves
    /// to `Delivered`; otherwise we stay in `Received`. Plain events do not
    /// drive the peer-ingress phase. Auth-exempt entries (bridge bootstrap
    /// / idempotency-ack envelopes) bypass the phase machinery entirely.
    fn note_peer_dequeue(&mut self, entry: &ClassifiedInboxEntry) {
        if entry.auth_exempt {
            return;
        }
        let InboxItem::External { .. } = &entry.item else {
            return;
        };
        if self.entries.is_empty() {
            self.phase = PeerIngressState::Delivered;
        } else {
            self.phase = PeerIngressState::Received;
        }
    }

    fn try_push(&mut self, entry: ClassifiedInboxEntry) -> Result<(), InboxError> {
        if self.closed {
            return Err(InboxError::Closed);
        }
        if self.entries.len() >= self.capacity {
            return Err(InboxError::Full);
        }
        self.entries.push_back(entry);
        Ok(())
    }

    fn drain(&mut self) -> Vec<ClassifiedInboxEntry> {
        let drained: Vec<_> = self.entries.drain(..).collect();
        for entry in &drained {
            self.note_peer_dequeue(entry);
        }
        drained
    }

    fn pop_front(&mut self) -> Option<ClassifiedInboxEntry> {
        let entry = self.entries.pop_front();
        if let Some(entry_ref) = entry.as_ref() {
            self.note_peer_dequeue(entry_ref);
        }
        entry
    }

    fn close(&mut self) {
        self.closed = true;
    }

    fn snapshot(&self) -> PeerIngressQueueSnapshot {
        let queued_entries: Vec<_> = self.entries.iter().map(snapshot_entry).collect();
        let actionable_count = queued_entries
            .iter()
            .filter(|entry| entry.class.is_actionable())
            .count();
        let response_count = queued_entries
            .iter()
            .filter(|entry| entry.class == PeerInputClass::Response)
            .count();
        let lifecycle_count = queued_entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.class,
                    PeerInputClass::PeerLifecycleAdded | PeerInputClass::PeerLifecycleRetired
                )
            })
            .count();
        let silent_request_count = queued_entries
            .iter()
            .filter(|entry| entry.class == PeerInputClass::SilentRequest)
            .count();
        let ack_count = queued_entries
            .iter()
            .filter(|entry| entry.class == PeerInputClass::Ack)
            .count();
        let plain_event_count = queued_entries
            .iter()
            .filter(|entry| entry.class == PeerInputClass::PlainEvent)
            .count();

        PeerIngressQueueSnapshot {
            total_count: queued_entries.len(),
            actionable_count,
            response_count,
            lifecycle_count,
            silent_request_count,
            ack_count,
            plain_event_count,
            queued_entries,
        }
    }

    fn runtime_snapshot(&self) -> (PeerIngressQueueSnapshot, PeerIngressState, usize) {
        let queue_len = self.entries.len();
        (self.snapshot(), self.phase, queue_len)
    }
}

/// The receiving end of the inbox, held by the agent loop.
pub struct Inbox {
    rx: mpsc::Receiver<InboxItem>,
    /// Notifier to wake waiting tasks when messages arrive.
    notify: Arc<Notify>,
    /// Classified entries queue (parallel to raw channel).
    classified_queue: Option<Arc<Mutex<ClassifiedInboxQueue>>>,
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
    /// Classified entries queue (parallel to raw channel).
    classified_queue: Option<Arc<Mutex<ClassifiedInboxQueue>>>,
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
                classified_queue: None,
                actionable_notify: None,
            },
            InboxSender {
                tx,
                notify,
                classification_context: None,
                classified_queue: None,
                actionable_notify: None,
            },
        )
    }

    /// Create a new inbox with ingress classification support.
    ///
    /// The classified path uses its own owned queue.
    /// `send_classified()` enqueues only on that classified queue.
    /// The raw `tx`/`rx` are kept for structural compatibility with `InboxSender::send()`
    /// but are not written to on the classified path (0.4.10 vestige; can be removed
    /// once `tx`/`rx` are made `Option` in a minor release).
    pub(crate) fn new_classified(
        context: Arc<IngressClassificationContext>,
    ) -> (Self, InboxSender) {
        let (tx, rx) = mpsc::channel(DEFAULT_INBOX_CAPACITY);
        let notify = Arc::new(Notify::new());
        let actionable_notify = Arc::new(Notify::new());
        let mut queue =
            ClassifiedInboxQueue::new(DEFAULT_INBOX_CAPACITY, context.require_peer_auth);
        for trusted_peer in &context.trusted_peers.read().peers {
            queue.sync_trusted_peer_added(&trusted_peer.pubkey.to_peer_id());
        }
        let classified_queue = Arc::new(Mutex::new(queue));
        (
            Inbox {
                rx,
                notify: notify.clone(),
                classified_queue: Some(classified_queue.clone()),
                actionable_notify: Some(actionable_notify.clone()),
            },
            InboxSender {
                tx,
                notify,
                classification_context: Some(context),
                classified_queue: Some(classified_queue),
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
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().drain())
            .unwrap_or_default()
    }

    /// Try to receive a single classified entry without blocking.
    pub(crate) fn try_recv_one_classified(&mut self) -> Option<ClassifiedInboxEntry> {
        self.classified_queue.as_ref()?.lock().pop_front()
    }

    /// Get the actionable-input notifier (fires only for actionable classes).
    pub(crate) fn classified_actionable_notify(&self) -> Option<Arc<Notify>> {
        self.actionable_notify.clone()
    }

    /// Snapshot the classified ingress queue without draining it.
    pub(crate) fn classified_snapshot(&self) -> Option<PeerIngressQueueSnapshot> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().snapshot())
    }

    pub(crate) fn peer_runtime_snapshot(
        &self,
    ) -> Option<(PeerIngressQueueSnapshot, PeerIngressState, usize)> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().runtime_snapshot())
    }

    pub(crate) fn note_trusted_peer_added(&mut self, peer_id: &str) {
        if let Some(queue) = &self.classified_queue {
            queue.lock().sync_trusted_peer_added(peer_id);
        }
    }

    pub(crate) fn note_trusted_peer_removed(&mut self, peer_id: &str) {
        if let Some(queue) = &self.classified_queue {
            queue.lock().sync_trusted_peer_removed(peer_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn peer_authority_test_snapshot(&self) -> Option<(PeerIngressState, usize)> {
        self.classified_queue.as_ref().map(|queue| {
            let queue = queue.lock();
            (queue.phase, queue.entries.len())
        })
    }

    #[cfg(test)]
    pub(crate) fn peer_authority_trusts_peer_for_test(&self, peer_id: &str) -> Option<bool> {
        self.classified_queue.as_ref().map(|queue| {
            queue
                .lock()
                .trusted_peers
                .contains(&PeerId(peer_id.to_string()))
        })
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

    /// Send an item with classification through the classified queue.
    ///
    /// Classifies the item using the ingress context. Items that classify
    /// as `None` (e.g., untrusted senders with `require_peer_auth`) are
    /// silently dropped — snapshot semantics, no resurrection.
    ///
    /// Classified items are enqueued only on the classified queue, then the
    /// appropriate notify is fired.
    pub(crate) fn send_classified(&self, item: InboxItem) -> Result<(), InboxError> {
        if let (Some(ctx), Some(classified_queue)) =
            (&self.classification_context, &self.classified_queue)
        {
            let result = match ctx.prepare(item) {
                Some(r) => r,
                None => {
                    // Dropped at ingress — untrusted or otherwise rejected.
                    // Do not enqueue, do not notify.
                    return Ok(());
                }
            };
            let kind = result.ingress_kind();
            let (should_enqueue, trusted_snapshot) = {
                let mut queue = classified_queue.lock();
                if result.auth_exempt && ctx.require_peer_auth && !result.trusted_sender {
                    // Auth-exempt bridge traffic (bootstrap / idempotency-ack)
                    // bypasses the peer-trust gate so bootstrapping can
                    // complete before trust edges exist.
                    (true, Some(false))
                } else {
                    queue.admit_peer_receive(&result)
                }
            };
            if !should_enqueue {
                return Ok(());
            }
            let entry = ClassifiedInboxEntry {
                raw_item_id: result.raw_item_id,
                item: result.item,
                class: result.class,
                auth_exempt: result.auth_exempt,
                kind,
                from_peer: result.from_peer,
                lifecycle_peer: result.lifecycle_peer,
                request_id: result.request_id,
                trusted_snapshot,
                text_projection: result.text_projection,
            };
            // Enqueue only on classified queue (no raw double-enqueue).
            // drain_classified_inbox_interactions() is the sole consumer.
            let is_actionable = entry.class.is_actionable();
            classified_queue.lock().try_push(entry)?;
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

impl Drop for Inbox {
    fn drop(&mut self) {
        if let Some(queue) = &self.classified_queue {
            queue.lock().close();
        }
    }
}

fn snapshot_entry(entry: &ClassifiedInboxEntry) -> PeerIngressEntrySnapshot {
    PeerIngressEntrySnapshot {
        raw_item_id: entry.raw_item_id.clone(),
        interaction_id: match &entry.item {
            InboxItem::External { envelope } => Some(InteractionId(envelope.id)),
            InboxItem::PlainEvent { interaction_id, .. } => interaction_id.map(InteractionId),
        },
        class: entry.class,
        kind: entry.kind,
        from_peer: entry.from_peer.clone(),
        lifecycle_peer: entry.lifecycle_peer.clone(),
        request_id: entry.request_id.clone(),
        trusted_snapshot: entry.trusted_snapshot,
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
                blocks: None,
                body: "test".to_string(),
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        }
    }

    fn make_request_envelope() -> Envelope {
        Envelope {
            id: Uuid::new_v4(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 42}),
                handling_mode: None,
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

    #[test]
    fn test_classified_sender_error_on_closed_inbox() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        drop(inbox);

        let result = sender.send_classified(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert!(matches!(result, Err(InboxError::Closed)));
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
            handling_mode: None,
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
            handling_mode: None,
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
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
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
    async fn test_classified_inbox_no_actionable_notify_for_lifecycle() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Request {
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({"peer": "new-agent"}),
            handling_mode: None,
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
        assert!(!entries[0].raw_item_id.is_empty());
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

    #[tokio::test]
    async fn test_classified_snapshot_is_non_destructive() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .unwrap();
        sender
            .send_classified(InboxItem::PlainEvent {
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            })
            .unwrap();

        let snapshot = inbox
            .classified_snapshot()
            .expect("classified snapshot should exist");
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(snapshot.actionable_count, 2);
        assert_eq!(snapshot.plain_event_count, 1);
        assert_eq!(snapshot.queued_entries.len(), 2);
        assert_eq!(snapshot.queued_entries[0].kind, PeerIngressKind::Message);
        assert_eq!(snapshot.queued_entries[1].kind, PeerIngressKind::PlainEvent);
        assert_eq!(snapshot.queued_entries[0].trusted_snapshot, Some(true));
        assert_eq!(snapshot.queued_entries[1].trusted_snapshot, None);
        assert!(
            !snapshot.queued_entries[0].raw_item_id.is_empty(),
            "external items should keep a stable ingress raw item id"
        );
        assert!(
            snapshot.queued_entries[1].interaction_id.is_some(),
            "plain events without an explicit interaction id should be assigned one at ingress"
        );
        assert_eq!(
            snapshot.queued_entries[1].raw_item_id,
            snapshot.queued_entries[1]
                .interaction_id
                .expect("plain event interaction id should be present")
                .to_string()
        );

        let drained = inbox.try_drain_classified();
        assert_eq!(drained.len(), 2, "snapshot must not drain queued ingress");
    }

    #[tokio::test]
    async fn test_classified_snapshot_preserves_request_correlation() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);

        let envelope = make_request_envelope();
        let request_id = envelope.id.to_string();

        sender
            .send_classified(InboxItem::External { envelope })
            .unwrap();

        let snapshot = inbox
            .classified_snapshot()
            .expect("classified snapshot should exist");
        assert_eq!(snapshot.total_count, 1);
        assert_eq!(snapshot.queued_entries.len(), 1);
        assert_eq!(snapshot.queued_entries[0].kind, PeerIngressKind::Request);
        assert_eq!(snapshot.queued_entries[0].request_id, Some(request_id));
        assert_eq!(snapshot.queued_entries[0].trusted_snapshot, Some(true));
    }
}
