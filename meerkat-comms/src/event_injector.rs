//! Concrete `EventInjector` implementation wrapping `InboxSender`.

use crate::inbox::{InboxError, InboxSender};
use crate::types::InboxItem;
use meerkat_core::event_injector::{EventInjector, EventInjectorError};
use meerkat_core::PlainEventSource;

/// Shared subscriber registry for interaction-scoped event streaming.
///
/// Maps interaction UUIDs to event senders. Entries are inserted by
/// `inject_with_subscription()` and removed (one-shot) by
/// `CommsRuntime::interaction_subscriber()`.
pub type SubscriberRegistry =
    std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<uuid::Uuid, tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>>>>;

/// Create an empty subscriber registry.
pub fn new_subscriber_registry() -> SubscriberRegistry {
    std::sync::Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()))
}

/// Injects plain events into the comms inbox.
///
/// This is the concrete implementation of `EventInjector` that wraps an
/// `InboxSender`. Surfaces receive `Arc<dyn EventInjector>` — this type
/// never leaks into meerkat-core.
pub struct CommsEventInjector {
    sender: InboxSender,
    subscriber_registry: SubscriberRegistry,
}

impl CommsEventInjector {
    /// Create a new injector wrapping the given inbox sender and subscriber registry.
    pub fn new(sender: InboxSender, registry: SubscriberRegistry) -> Self {
        Self {
            sender,
            subscriber_registry: registry,
        }
    }
}

impl EventInjector for CommsEventInjector {
    fn inject(&self, body: String, source: PlainEventSource) -> Result<(), EventInjectorError> {
        self.sender
            .send(InboxItem::PlainEvent {
                body,
                source,
                interaction_id: None,
            })
            .map_err(|e| match e {
                InboxError::Full => EventInjectorError::Full,
                InboxError::Closed => EventInjectorError::Closed,
            })
    }
}

impl meerkat_core::SubscribableInjector for CommsEventInjector {
    fn inject_with_subscription(
        &self,
        body: String,
        source: PlainEventSource,
    ) -> Result<meerkat_core::InteractionSubscription, EventInjectorError> {
        let id = uuid::Uuid::new_v4();
        let (tx, rx) = tokio::sync::mpsc::channel(4096);

        // Store subscriber in registry
        self.subscriber_registry.lock().insert(id, tx);

        // Send to inbox with interaction_id
        if let Err(e) = self.sender.send(InboxItem::PlainEvent {
            body,
            source,
            interaction_id: Some(id),
        }) {
            // Clean up subscriber on send failure
            self.subscriber_registry.lock().remove(&id);
            return Err(match e {
                InboxError::Full => EventInjectorError::Full,
                InboxError::Closed => EventInjectorError::Closed,
            });
        }

        Ok(meerkat_core::InteractionSubscription {
            id: meerkat_core::InteractionId(id),
            events: rx,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::inbox::Inbox;

    #[test]
    fn test_comms_event_injector_sends_to_inbox() {
        let (mut inbox, sender) = Inbox::new();
        let injector = CommsEventInjector::new(sender, new_subscriber_registry());

        injector
            .inject("hello".to_string(), PlainEventSource::Tcp)
            .unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::PlainEvent { body, source, .. } => {
                assert_eq!(body, "hello");
                assert_eq!(*source, PlainEventSource::Tcp);
            }
            other => panic!("Expected PlainEvent, got {:?}", other),
        }
    }

    #[test]
    fn test_comms_event_injector_reports_full() {
        // Create inbox with capacity 1 and fill it
        let (_inbox, sender) = Inbox::new_with_capacity(1);
        let injector = CommsEventInjector::new(sender, new_subscriber_registry());

        // First should succeed
        injector
            .inject("first".to_string(), PlainEventSource::Tcp)
            .unwrap();

        // Second should fail with Full
        let result = injector.inject("second".to_string(), PlainEventSource::Tcp);
        assert!(
            matches!(result, Err(EventInjectorError::Full)),
            "Expected Full error"
        );
    }

    #[test]
    fn test_comms_event_injector_as_dyn() {
        use std::sync::Arc;

        let (mut inbox, sender) = Inbox::new();
        let injector: Arc<dyn EventInjector> = Arc::new(CommsEventInjector::new(sender, new_subscriber_registry()));

        injector
            .inject("via dyn".to_string(), PlainEventSource::Webhook)
            .unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_inject_with_subscription_stores_subscriber_and_sends_item() {
        use meerkat_core::SubscribableInjector;

        let registry = new_subscriber_registry();
        let (mut inbox, sender) = Inbox::new();
        let injector = CommsEventInjector::new(sender, registry.clone());

        let sub = injector
            .inject_with_subscription("tracked".to_string(), PlainEventSource::Rpc)
            .unwrap();

        // Verify item in inbox has interaction_id
        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::PlainEvent {
                body,
                interaction_id,
                ..
            } => {
                assert_eq!(body, "tracked");
                assert_eq!(*interaction_id, Some(sub.id.0));
            }
            other => panic!("Expected PlainEvent, got {:?}", other),
        }

        // Subscriber should be in registry
        assert!(registry.lock().contains_key(&sub.id.0));
    }

    #[test]
    fn test_inject_with_subscription_cleans_up_on_full() {
        use meerkat_core::SubscribableInjector;

        let registry = new_subscriber_registry();
        let (_inbox, sender) = Inbox::new_with_capacity(1);
        let injector = CommsEventInjector::new(sender, registry.clone());

        // Fill the inbox
        injector
            .inject("first".to_string(), PlainEventSource::Tcp)
            .unwrap();

        // This should fail and clean up registry
        let result = injector.inject_with_subscription("second".to_string(), PlainEventSource::Tcp);
        assert!(matches!(result, Err(EventInjectorError::Full)));

        // Registry should be empty (cleaned up)
        assert!(registry.lock().is_empty());
    }
}
