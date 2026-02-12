//! Transport-agnostic event injection trait.
//!
//! Surfaces (REST, CLI, RPC) use `EventInjector` to push external events
//! into an agent's inbox without depending on comms-specific types.
//! `meerkat-comms` provides the concrete implementation (`CommsEventInjector`)
//! that wraps `InboxSender`.

use crate::PlainEventSource;
use crate::event::AgentEvent;
use crate::interaction::InteractionId;
use tokio::sync::mpsc;

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

#[derive(Debug)]
pub struct InteractionSubscription {
    pub id: InteractionId,
    pub events: mpsc::Receiver<AgentEvent>,
}

pub trait SubscribableInjector: EventInjector {
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
