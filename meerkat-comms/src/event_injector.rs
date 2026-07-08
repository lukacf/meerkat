//! Concrete `EventInjector` implementation wrapping `InboxSender`.

use crate::inbox::{AdmissionOutcome, DropReason, InboxSender};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::types::InboxItem;
use meerkat_core::PlainEventSource;
use meerkat_core::event_injector::{EventInjector, EventInjectorError};
use meerkat_core::types::{ContentInput, HandlingMode, RenderMetadata};

/// Shared subscriber registry for interaction-scoped event streaming.
///
/// Maps interaction UUIDs to event senders. Entries are inserted by
/// `inject_with_subscription()` and removed (one-shot) by
/// `CommsRuntime::interaction_subscriber()`.
pub type SubscriberRegistry = std::sync::Arc<
    parking_lot::Mutex<
        std::collections::HashMap<uuid::Uuid, tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>>,
    >,
>;

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

    /// Inject a plain event stamped with a caller-chosen interaction id.
    ///
    /// Unlike [`EventInjector::inject`] (which injects with `interaction_id:
    /// None`), this stamps the injected inbox item with `interaction_id`, so a
    /// caller that returns the same id as a public handle yields a *correlated*
    /// interaction id — the returned handle correlates with the injected event
    /// rather than being a fresh unrelated UUID. No subscriber channel is
    /// reserved (that is [`SubscribableInjector::inject_with_subscription`]'s
    /// job); this is the non-stream correlation case.
    pub fn inject_with_interaction_id(
        &self,
        interaction_id: uuid::Uuid,
        content: ContentInput,
        source: PlainEventSource,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<(), EventInjectorError> {
        let body = content.text_content();
        let blocks = match content {
            ContentInput::Text(_) => None,
            ContentInput::Blocks(blocks) => Some(blocks),
        };
        match self.sender.send_classified(InboxItem::PlainEvent {
            body,
            source,
            handling_mode,
            interaction_id: Some(interaction_id),
            blocks,
            render_metadata,
        }) {
            AdmissionOutcome::Admitted => Ok(()),
            AdmissionOutcome::Dropped { reason } => Err(drop_reason_to_injector_error(reason)),
        }
    }
}

impl EventInjector for CommsEventInjector {
    fn inject(
        &self,
        content: ContentInput,
        source: PlainEventSource,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<(), EventInjectorError> {
        let body = content.text_content();
        let blocks = match content {
            ContentInput::Text(_) => None,
            ContentInput::Blocks(blocks) => Some(blocks),
        };
        match self.sender.send_classified(InboxItem::PlainEvent {
            body,
            source,
            handling_mode,
            interaction_id: None,
            blocks,
            render_metadata,
        }) {
            AdmissionOutcome::Admitted => Ok(()),
            AdmissionOutcome::Dropped { reason } => Err(drop_reason_to_injector_error(reason)),
        }
    }
}

fn drop_reason_to_injector_error(reason: DropReason) -> EventInjectorError {
    match reason {
        DropReason::InboxFull => EventInjectorError::Full,
        DropReason::SessionClosed => EventInjectorError::Closed,
        // Plain events never hit the peer-auth gate (auth classification is
        // external-envelope-only), but an ingress-stage rejection still maps
        // to Closed from the caller's perspective — the event did not reach
        // the agent.
        DropReason::UntrustedSender | DropReason::ClassificationRejected => {
            EventInjectorError::Closed
        }
    }
}

impl meerkat_core::event_injector::SubscribableInjector for CommsEventInjector {
    fn inject_with_subscription(
        &self,
        content: ContentInput,
        source: PlainEventSource,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<meerkat_core::event_injector::InteractionSubscription, EventInjectorError> {
        let id = uuid::Uuid::new_v4();
        let (tx, rx) = tokio::sync::mpsc::channel(4096);
        let body = content.text_content();
        let blocks = match content {
            ContentInput::Text(_) => None,
            ContentInput::Blocks(blocks) => Some(blocks),
        };

        // Store subscriber in registry
        self.subscriber_registry.lock().insert(id, tx);

        // Send to inbox with interaction_id
        if let AdmissionOutcome::Dropped { reason } =
            self.sender.send_classified(InboxItem::PlainEvent {
                body,
                source,
                handling_mode,
                interaction_id: Some(id),
                blocks,
                render_metadata,
            })
        {
            // Clean up subscriber on send failure
            self.subscriber_registry.lock().remove(&id);
            return Err(drop_reason_to_injector_error(reason));
        }

        Ok(meerkat_core::event_injector::InteractionSubscription {
            id: meerkat_core::InteractionId(id),
            events: rx,
        })
    }

    fn inject_with_interaction_id(
        &self,
        interaction_id: meerkat_core::InteractionId,
        content: ContentInput,
        source: PlainEventSource,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<(), EventInjectorError> {
        CommsEventInjector::inject_with_interaction_id(
            self,
            interaction_id.0,
            content,
            source,
            handling_mode,
            render_metadata,
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::classify::test_support;
    use crate::inbox::Inbox;
    use crate::trust::TrustStore;

    fn classified_inbox() -> (Inbox, InboxSender) {
        Inbox::new_classified(test_support::classification_context(
            TrustStore::new(),
            false,
        ))
    }

    #[test]
    fn test_comms_event_injector_sends_to_inbox() {
        let (mut inbox, sender) = classified_inbox();
        let injector = CommsEventInjector::new(sender, new_subscriber_registry());

        injector
            .inject(
                "hello".to_string().into(),
                PlainEventSource::Tcp,
                HandlingMode::Queue,
                None,
            )
            .unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
            InboxItem::PlainEvent { body, source, .. } => {
                assert_eq!(body, "hello");
                assert_eq!(*source, PlainEventSource::Tcp);
            }
            other => panic!("Expected PlainEvent, got {other:?}"),
        }
    }

    #[test]
    fn test_comms_event_injector_reports_full() {
        let (_inbox, sender) = Inbox::new_classified_with_capacity_for_test(
            test_support::classification_context(TrustStore::new(), false),
            1,
        );
        let injector = CommsEventInjector::new(sender, new_subscriber_registry());

        // First should succeed
        injector
            .inject(
                "first".to_string().into(),
                PlainEventSource::Tcp,
                HandlingMode::Queue,
                None,
            )
            .unwrap();

        // Second should fail with Full
        let result = injector.inject(
            "second".to_string().into(),
            PlainEventSource::Tcp,
            HandlingMode::Queue,
            None,
        );
        assert!(
            matches!(result, Err(EventInjectorError::Full)),
            "Expected Full error"
        );
    }

    #[test]
    fn test_comms_event_injector_as_dyn() {
        use std::sync::Arc;

        let (mut inbox, sender) = classified_inbox();
        let injector: Arc<dyn EventInjector> =
            Arc::new(CommsEventInjector::new(sender, new_subscriber_registry()));

        injector
            .inject(
                "via dyn".to_string().into(),
                PlainEventSource::Webhook,
                HandlingMode::Queue,
                None,
            )
            .unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_inject_with_subscription_stores_subscriber_and_sends_item() {
        use meerkat_core::event_injector::SubscribableInjector;

        let registry = new_subscriber_registry();
        let (mut inbox, sender) = classified_inbox();
        let injector = CommsEventInjector::new(sender, registry.clone());

        let sub = injector
            .inject_with_subscription(
                "tracked".to_string().into(),
                PlainEventSource::Rpc,
                HandlingMode::Queue,
                None,
            )
            .unwrap();

        // Verify item in inbox has interaction_id
        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
            InboxItem::PlainEvent {
                body,
                interaction_id,
                ..
            } => {
                assert_eq!(body, "tracked");
                assert_eq!(*interaction_id, Some(sub.id.0));
            }
            other => panic!("Expected PlainEvent, got {other:?}"),
        }

        // Subscriber should be in registry
        assert!(registry.lock().contains_key(&sub.id.0));
    }

    #[test]
    fn test_inject_with_subscription_cleans_up_on_full() {
        use meerkat_core::event_injector::SubscribableInjector;

        let registry = new_subscriber_registry();
        let (_inbox, sender) = Inbox::new_classified_with_capacity_for_test(
            test_support::classification_context(TrustStore::new(), false),
            1,
        );
        let injector = CommsEventInjector::new(sender, registry.clone());

        // Fill the inbox
        injector
            .inject(
                "first".to_string().into(),
                PlainEventSource::Tcp,
                HandlingMode::Queue,
                None,
            )
            .unwrap();

        // This should fail and clean up registry
        let result = injector.inject_with_subscription(
            "second".to_string().into(),
            PlainEventSource::Tcp,
            HandlingMode::Queue,
            None,
        );
        assert!(matches!(result, Err(EventInjectorError::Full)));

        // Registry should be empty (cleaned up)
        assert!(registry.lock().is_empty());
    }

    #[test]
    fn test_inject_with_subscription_cleans_up_on_closed_inbox() {
        use meerkat_core::event_injector::SubscribableInjector;

        let registry = new_subscriber_registry();
        let (inbox, sender) = Inbox::new();
        drop(inbox); // close receiver

        let injector = CommsEventInjector::new(sender, registry.clone());
        let result = injector.inject_with_subscription(
            "closed".to_string().into(),
            PlainEventSource::Tcp,
            HandlingMode::Queue,
            None,
        );

        assert!(matches!(result, Err(EventInjectorError::Closed)));
        assert!(registry.lock().is_empty(), "registry should be cleaned up");
    }
}
