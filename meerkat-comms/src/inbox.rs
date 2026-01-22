//! Thread-safe inbox for Meerkat comms.
//!
//! The inbox collects incoming messages from IO tasks and subagent results,
//! allowing the agent loop to drain them at turn boundaries.

use tokio::sync::mpsc;

use crate::types::InboxItem;

/// The receiving end of the inbox, held by the agent loop.
pub struct Inbox {
    rx: mpsc::UnboundedReceiver<InboxItem>,
}

/// The sending end of the inbox, cloned to IO tasks.
#[derive(Clone)]
pub struct InboxSender {
    tx: mpsc::UnboundedSender<InboxItem>,
}

impl Inbox {
    /// Create a new inbox, returning both the inbox and a sender.
    pub fn new() -> (Self, InboxSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Inbox { rx }, InboxSender { tx })
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
}

impl InboxSender {
    /// Send an item to the inbox.
    ///
    /// Returns an error if the inbox has been closed.
    pub fn send(&self, item: InboxItem) -> Result<(), InboxError> {
        self.tx.send(item).map_err(|_| InboxError::Closed)
    }
}

/// Errors that can occur with inbox operations.
#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    #[error("Inbox has been closed")]
    Closed,
}

#[cfg(test)]
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
        let _sender2 = sender.clone();
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

        sender
            .send(InboxItem::External { envelope })
            .unwrap();

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
}
