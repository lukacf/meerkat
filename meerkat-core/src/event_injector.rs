//! Transport-agnostic event injection trait.
//!
//! Surfaces (REST, CLI, RPC) use `EventInjector` to push external events
//! into an agent's inbox without depending on comms-specific types.
//! `meerkat-comms` provides the concrete implementation (`CommsEventInjector`)
//! that wraps `InboxSender`.

use crate::PlainEventSource;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// Error returned when event injection fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventInjectorError {
    /// The inbox is full (bounded capacity exceeded).
    Full,
    /// The inbox receiver has been dropped (agent shut down).
    Closed,
}

impl std::fmt::Display for EventInjectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "inbox full"),
            Self::Closed => write!(f, "inbox closed"),
        }
    }
}

impl std::error::Error for EventInjectorError {}

/// Transport-agnostic trait for injecting external events into an agent's inbox.
///
/// This trait lives in `meerkat-core` so that surfaces can depend on it without
/// pulling in `meerkat-comms`. The comms crate provides the concrete implementation.
pub trait EventInjector: Send + Sync {
    /// Inject a plain event into the agent's inbox.
    ///
    /// Returns `Ok(())` on success, `Err(Full)` if the inbox is at capacity,
    /// or `Err(Closed)` if the agent has shut down.
    fn inject(&self, body: String, source: PlainEventSource) -> Result<(), EventInjectorError>;
}

/// A subscription handle returned by `SubscribableInjector::inject_with_subscription`.
///
/// The caller reads `events` to receive streaming `AgentEvent`s scoped to this interaction.
/// The terminal event (`InteractionComplete` or `InteractionFailed`) signals end-of-stream.
pub struct InteractionSubscription {
    /// Unique ID for this interaction (correlates with events).
    pub id: crate::interaction::InteractionId,
    /// Receiver for streaming events scoped to this interaction (buffer: 4096).
    pub events: tokio::sync::mpsc::Receiver<crate::event::AgentEvent>,
}

/// Extended injector that returns a subscription for streaming events.
///
/// Implementors register a subscriber in the registry keyed by interaction ID,
/// then inject the event into the inbox. The host loop looks up the subscriber
/// during `drain_inbox_interactions()` and wires it to the event tap.
pub trait SubscribableInjector: EventInjector {
    /// Inject an event and return a subscription for streaming events.
    ///
    /// 1. Generates a unique `InteractionId`
    /// 2. Creates a channel (buffer: 4096) and stores the sender in the subscriber registry
    /// 3. Injects the event into the inbox with the interaction ID
    /// 4. Returns the subscription handle with the receiver
    ///
    /// On inbox send failure: removes the subscriber from the registry and returns the error.
    fn inject_with_subscription(
        &self,
        body: String,
        source: PlainEventSource,
    ) -> Result<InteractionSubscription, EventInjectorError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Mock EventInjector for testing — records injected events.
    struct MockEventInjector {
        events: Mutex<Vec<(String, PlainEventSource)>>,
    }

    impl MockEventInjector {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }
    }

    impl EventInjector for MockEventInjector {
        fn inject(&self, body: String, source: PlainEventSource) -> Result<(), EventInjectorError> {
            self.events.lock().unwrap().push((body, source));
            Ok(())
        }
    }

    #[test]
    fn test_event_injector_trait_compiles() {
        let injector = MockEventInjector::new();
        injector
            .inject("hello".to_string(), PlainEventSource::Tcp)
            .unwrap();

        let events = injector.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "hello");
        assert_eq!(events[0].1, PlainEventSource::Tcp);
    }

    #[test]
    fn test_event_injector_as_dyn_trait() {
        let injector: Arc<dyn EventInjector> = Arc::new(MockEventInjector::new());
        injector
            .inject("test".to_string(), PlainEventSource::Webhook)
            .unwrap();
    }

    #[test]
    fn test_event_injector_error_display() {
        assert_eq!(EventInjectorError::Full.to_string(), "inbox full");
        assert_eq!(EventInjectorError::Closed.to_string(), "inbox closed");
    }
}
