//! Concrete `EventInjector` implementation wrapping `InboxSender`.

use crate::inbox::{InboxError, InboxSender};
use crate::types::InboxItem;
use meerkat_core::event_injector::{EventInjector, EventInjectorError};
use meerkat_core::PlainEventSource;

/// Injects plain events into the comms inbox.
///
/// This is the concrete implementation of `EventInjector` that wraps an
/// `InboxSender`. Surfaces receive `Arc<dyn EventInjector>` — this type
/// never leaks into meerkat-core.
pub struct CommsEventInjector {
    sender: InboxSender,
}

impl CommsEventInjector {
    /// Create a new injector wrapping the given inbox sender.
    pub fn new(sender: InboxSender) -> Self {
        Self { sender }
    }
}

impl EventInjector for CommsEventInjector {
    fn inject(&self, body: String, source: PlainEventSource) -> Result<(), EventInjectorError> {
        self.sender
            .send(InboxItem::PlainEvent { body, source })
            .map_err(|e| match e {
                InboxError::Full => EventInjectorError::Full,
                InboxError::Closed => EventInjectorError::Closed,
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
        let injector = CommsEventInjector::new(sender);

        injector
            .inject("hello".to_string(), PlainEventSource::Tcp)
            .unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::PlainEvent { body, source } => {
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
        let injector = CommsEventInjector::new(sender);

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
        let injector: Arc<dyn EventInjector> = Arc::new(CommsEventInjector::new(sender));

        injector
            .inject("via dyn".to_string(), PlainEventSource::Webhook)
            .unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
    }
}
