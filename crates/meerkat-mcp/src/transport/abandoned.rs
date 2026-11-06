//! The in-flight requests of one streamable-HTTP connection, by JSON-RPC
//! request id, and the ones the host has abandoned.
//!
//! A server in stateless mode ignores `notifications/cancelled`; what it can
//! see is the client closing the request's own HTTP exchange. The connection
//! abandons a request here when the caller stops waiting for it, and the HTTP
//! client ends that request's exchange: a response it is still waiting for,
//! the JSON body it is reading, or the SSE response stream it opened. Dropping
//! the response closes the HTTP/1.1 connection (an HTTP/2 stream is reset),
//! which the server sees.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rmcp::model::RequestId;
use tokio_util::sync::CancellationToken;

/// How many abandoned ids are remembered for requests whose exchange had not
/// started yet. An abandon normally lands while its request is in flight;
/// this covers one that lands before the transport posts it.
const REMEMBERED_ABANDONED: usize = 256;

/// Numbers each registry: a registry belongs to one connection instance, and
/// a reconnect builds a new connection with a new registry.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// One connection's registry, shared by its handle and its HTTP client. A
/// connection without an HTTP client keeps one that nothing registers in.
#[derive(Clone)]
pub(crate) struct AbandonedRequests(Arc<Registry>);

struct Registry {
    connection: u64,
    state: Mutex<State>,
}

/// A request's key: its connection instance and its JSON-RPC id, never the
/// bare id, so an abandon can only reach the request it was armed for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RequestKey {
    connection: u64,
    id: RequestId,
}

#[derive(Default)]
struct State {
    next: u64,
    in_flight: HashMap<RequestKey, (u64, CancellationToken)>,
    abandoned: VecDeque<RequestKey>,
}

impl Default for AbandonedRequests {
    fn default() -> Self {
        Self(Arc::new(Registry {
            connection: NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed),
            state: Mutex::default(),
        }))
    }
}

impl AbandonedRequests {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn key(&self, id: &RequestId) -> RequestKey {
        RequestKey {
            connection: self.0.connection,
            id: id.clone(),
        }
    }

    /// The caller stopped waiting for `id`: its HTTP exchange ends now, or as
    /// soon as it starts.
    pub(crate) fn abandon(&self, id: &RequestId) {
        let key = self.key(id);
        let mut state = self.state();
        if let Some((_, token)) = state.in_flight.remove(&key) {
            token.cancel();
            return;
        }
        if state.abandoned.len() == REMEMBERED_ABANDONED {
            state.abandoned.pop_front();
        }
        state.abandoned.push_back(key);
    }

    /// Registers `id` as in flight until the returned guard is dropped. Its
    /// token fires when the request is abandoned, at once if it already was.
    pub(crate) fn register(&self, id: &RequestId) -> InFlight {
        let key = self.key(id);
        let token = CancellationToken::new();
        let mut state = self.state();
        let generation = state.next;
        state.next += 1;
        if let Some(at) = state
            .abandoned
            .iter()
            .position(|abandoned| *abandoned == key)
        {
            state.abandoned.remove(at);
            token.cancel();
        } else {
            state
                .in_flight
                .insert(key.clone(), (generation, token.clone()));
        }
        InFlight {
            registry: self.clone(),
            key,
            generation,
            token,
        }
    }

    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.state().in_flight.len()
    }
}

/// A registered request's place in the registry, removed when its exchange
/// ends (its response read, or its response stream dropped).
pub(crate) struct InFlight {
    registry: AbandonedRequests,
    key: RequestKey,
    generation: u64,
    token: CancellationToken,
}

impl InFlight {
    /// Resolves when the request is abandoned.
    pub(crate) async fn abandoned(&self) {
        self.token.cancelled().await;
    }

    /// The token that fires when the request is abandoned.
    pub(crate) fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut state = self.registry.state();
        if state
            .in_flight
            .get(&self.key)
            .is_some_and(|(generation, _)| *generation == self.generation)
        {
            state.in_flight.remove(&self.key);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rmcp::model::NumberOrString;

    fn id(n: i64) -> RequestId {
        NumberOrString::Number(n)
    }

    #[test]
    fn abandoning_an_in_flight_request_fires_its_token_once() {
        let registry = AbandonedRequests::default();
        let in_flight = registry.register(&id(1));
        assert!(!in_flight.token().is_cancelled());
        registry.abandon(&id(1));
        assert!(in_flight.token().is_cancelled());
        assert_eq!(registry.in_flight(), 0);
    }

    #[test]
    fn an_abandon_before_the_exchange_starts_still_applies() {
        let registry = AbandonedRequests::default();
        registry.abandon(&id(2));
        let in_flight = registry.register(&id(2));
        assert!(in_flight.token().is_cancelled());
        // Remembered once only.
        let again = registry.register(&id(2));
        assert!(!again.token().is_cancelled());
    }

    #[test]
    fn a_finished_exchange_leaves_nothing_behind() {
        let registry = AbandonedRequests::default();
        let in_flight = registry.register(&id(3));
        drop(in_flight);
        assert_eq!(registry.in_flight(), 0);
        let other = registry.register(&id(4));
        registry.abandon(&id(3));
        assert!(!other.token().is_cancelled());
    }

    #[test]
    fn an_abandon_reaches_only_its_own_connection() {
        let first = AbandonedRequests::default();
        let second = AbandonedRequests::default();
        let on_second = second.register(&id(5));
        first.abandon(&id(5));
        assert!(!on_second.token().is_cancelled());
        // Nor does it wait for the same id on another connection.
        drop(on_second);
        assert!(!second.register(&id(5)).token().is_cancelled());
    }

    #[test]
    fn the_remembered_abandons_are_bounded() {
        let registry = AbandonedRequests::default();
        for n in 0..(REMEMBERED_ABANDONED as i64 + 10) {
            registry.abandon(&id(n));
        }
        assert_eq!(registry.state().abandoned.len(), REMEMBERED_ABANDONED);
        assert!(!registry.register(&id(0)).token().is_cancelled());
        assert!(registry.register(&id(20)).token().is_cancelled());
    }
}
