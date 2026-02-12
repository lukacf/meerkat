//! Concrete `EventInjector` implementation wrapping `InboxSender`.

use crate::inbox::{InboxError, InboxSender};
use crate::runtime::comms_runtime::SubscriberRegistry;
use crate::types::InboxItem;
use meerkat_core::PlainEventSource;
use meerkat_core::event_injector::{
    EventInjector, EventInjectorError, InteractionSubscription, SubscribableInjector,
};
use meerkat_core::interaction::InteractionId;
use tokio::sync::mpsc;
use uuid::Uuid;

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
    /// Create a new injector wrapping the given inbox sender.
    pub fn new(sender: InboxSender, subscriber_registry: SubscriberRegistry) -> Self {
        Self {
            sender,
            subscriber_registry,
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

impl SubscribableInjector for CommsEventInjector {
    fn inject_with_subscription(
        &self,
        body: String,
        source: PlainEventSource,
    ) -> Result<InteractionSubscription, EventInjectorError> {
        let interaction_uuid = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(4096);
        self.subscriber_registry.lock().insert(interaction_uuid, tx);

        let send_result = self.sender.send(InboxItem::PlainEvent {
            body,
            source,
            interaction_id: Some(interaction_uuid),
        });
        if let Err(err) = send_result {
            self.subscriber_registry.lock().remove(&interaction_uuid);
            return Err(match err {
                InboxError::Full => EventInjectorError::Full,
                InboxError::Closed => EventInjectorError::Closed,
            });
        }

        Ok(InteractionSubscription {
            id: InteractionId(interaction_uuid),
            events: rx,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::inbox::Inbox;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn test_comms_event_injector_sends_to_inbox() {
        let (mut inbox, sender) = Inbox::new();
        let injector =
            CommsEventInjector::new(sender, Arc::new(parking_lot::Mutex::new(HashMap::new())));

        injector
            .inject("hello".to_string(), PlainEventSource::Tcp)
            .unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::PlainEvent {
                body,
                source,
                interaction_id,
            } => {
                assert_eq!(body, "hello");
                assert_eq!(*source, PlainEventSource::Tcp);
                assert_eq!(*interaction_id, None);
            }
            other => panic!("Expected PlainEvent, got {:?}", other),
        }
    }

    #[test]
    fn test_comms_event_injector_reports_full() {
        // Create inbox with capacity 1 and fill it
        let (_inbox, sender) = Inbox::new_with_capacity(1);
        let injector =
            CommsEventInjector::new(sender, Arc::new(parking_lot::Mutex::new(HashMap::new())));

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
        let (mut inbox, sender) = Inbox::new();
        let injector: Arc<dyn EventInjector> = Arc::new(CommsEventInjector::new(
            sender,
            Arc::new(parking_lot::Mutex::new(HashMap::new())),
        ));

        injector
            .inject("via dyn".to_string(), PlainEventSource::Webhook)
            .unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_inject_with_subscription_registers_and_sends_interaction_id() {
        let (mut inbox, sender) = Inbox::new();
        let registry = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let injector = CommsEventInjector::new(sender, registry.clone());

        let sub = injector
            .inject_with_subscription("scoped".to_string(), PlainEventSource::Webhook)
            .unwrap();
        assert!(registry.lock().contains_key(&sub.id.0));

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::PlainEvent { interaction_id, .. } => {
                assert_eq!(*interaction_id, Some(sub.id.0));
            }
            other => panic!("Expected PlainEvent, got {:?}", other),
        }
    }

    #[test]
    fn test_inject_with_subscription_full_removes_registry_entry() {
        let (_inbox, sender) = Inbox::new_with_capacity(1);
        let registry = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let injector = CommsEventInjector::new(sender, registry.clone());
        injector
            .inject("fill".to_string(), PlainEventSource::Tcp)
            .unwrap();

        let result = injector.inject_with_subscription("scoped".to_string(), PlainEventSource::Tcp);
        assert!(matches!(result, Err(EventInjectorError::Full)));
        assert!(registry.lock().is_empty());
    }

    #[test]
    fn test_inject_with_subscription_closed_removes_registry_entry() {
        let (inbox, sender) = Inbox::new();
        drop(inbox);

        let registry = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let injector = CommsEventInjector::new(sender, registry.clone());
        let result =
            injector.inject_with_subscription("scoped".to_string(), PlainEventSource::Webhook);
        assert!(matches!(result, Err(EventInjectorError::Closed)));
        assert!(registry.lock().is_empty());
    }
}
