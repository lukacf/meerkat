//! Persistent operational event log with buffered ingestion and pluggable storage.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::types::{EventEnvelope, UnifiedEvent};

/// Boxed error type returned by [`EventLogStore`] methods.
pub type EventLogError = Box<dyn std::error::Error + Send>;

/// Optional event filter predicate.
type EventFilter = Box<dyn Fn(&UnifiedEvent) -> bool + Send + Sync>;

// ---------------------------------------------------------------------------
// Persisted event model
// ---------------------------------------------------------------------------

/// A persisted operational event with monotonic ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedEvent {
    /// Unique event ID (from the original event envelope).
    pub id: String,
    /// Monotonic sequence number assigned at ingestion time.
    /// Deterministic ordering within and across batches.
    pub seq: u64,
    /// Millisecond timestamp from the original event.
    pub timestamp_ms: u64,
    /// Member/agent ID. `None` for module events.
    pub member_id: Option<String>,
    /// The full event payload.
    pub event: UnifiedEvent,
}

// ---------------------------------------------------------------------------
// Query model
// ---------------------------------------------------------------------------

/// Query parameters for historical event retrieval.
///
/// `after_seq` IS the cursor for pagination. Stores that surface a
/// monotonic sequence (`PersistedEvent::seq`, `MobStructuralEventEnvelope::cursor`)
/// MUST treat it as an exclusive lower bound and return events with
/// `seq > after_seq`. Callers paginate by passing the last-seen `seq` as
/// `after_seq` on the next query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventQuery {
    /// Only events after this timestamp (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
    /// Only events before this timestamp (exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_ms: Option<u64>,
    /// Filter to events from a specific member/agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    /// Filter to events from a specific identity when the store supports identity-native rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Filter to events from a specific mob (structural event surface).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<String>,
    /// Filter to events scoped to a specific flow run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Filter to events scoped to a specific flow step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    /// Filter to specific event types (e.g. "run_completed", "run_failed").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_types: Vec<String>,
    /// Maximum number of events to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Resume after this sequence number (exclusive; the cursor for pagination).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<u64>,
}

// ---------------------------------------------------------------------------
// Storage trait
// ---------------------------------------------------------------------------

/// Trait for persisting and querying operational events.
///
/// Meta defines the contract; apps provide the implementation for their
/// storage backend (BigQuery, Postgres, SQLite, in-memory, etc.).
///
/// Same pattern as `Discovery`, `EdgeDiscovery`, `SessionAgentBuilder`.
pub trait EventLogStore: Send + Sync {
    /// Persist a batch of events. Called periodically by the ingestion engine.
    ///
    /// Must be idempotent — duplicate events (same `id`) should be ignored.
    /// Failures are logged via the error hook but never block agent execution.
    fn append_batch(
        &self,
        events: Vec<PersistedEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>>;

    /// Query historical events matching the given criteria.
    fn query(
        &self,
        query: EventQuery,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>>;
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the event log ingestion engine.
pub struct EventLogConfig {
    /// App-provided storage backend.
    pub store: Box<dyn EventLogStore>,
    /// Optional filter — return `true` to persist, `false` to skip.
    /// If `None`, all events are persisted.
    pub filter: Option<EventFilter>,
    /// Number of events to buffer before flushing to storage.
    /// Default: 64.
    pub batch_size: usize,
    /// Maximum time between flushes, even if batch is not full.
    /// Default: 1 second.
    pub flush_interval: Duration,
}

impl Default for EventLogConfig {
    fn default() -> Self {
        Self {
            store: Box::new(NullEventLogStore),
            filter: None,
            batch_size: 64,
            flush_interval: Duration::from_secs(1),
        }
    }
}

/// No-op store that drops every event and serves empty queries.
///
/// A legitimate **declared** choice (tests, demos, gateways answering
/// `runtime_options.event_log = {"storage": "null"}`), never an unconfigured
/// default the composition falls back to silently — the silent case is the
/// absence of event-log configuration, which means no ingestion at all.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullEventLogStore;

impl EventLogStore for NullEventLogStore {
    fn append_batch(
        &self,
        _events: Vec<PersistedEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn query(
        &self,
        _query: EventQuery,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

// ---------------------------------------------------------------------------
// Ingestion engine
// ---------------------------------------------------------------------------

/// Shared handle to the event log, held by UnifiedRuntime.
pub(crate) struct EventLogHandle {
    store: Arc<dyn EventLogStore>,
    /// Sender for the ingestion buffer. Events are sent here and flushed
    /// in batches by a background task.
    ingress_tx: mpsc::Sender<EventEnvelope<UnifiedEvent>>,
}

impl EventLogHandle {
    /// Return a cloned reference to the underlying store.
    pub fn store(&self) -> std::sync::Arc<dyn EventLogStore> {
        self.store.clone()
    }

    /// Ingest an event into the log (non-blocking, buffered).
    pub fn ingest(&self, event: EventEnvelope<UnifiedEvent>) {
        // Non-blocking: drop if the buffer is full (backpressure protection)
        let _ = self.ingress_tx.try_send(event);
    }
}

/// Hard cap on retained events while the store is unavailable. Beyond
/// this, the oldest events in the retry buffer are dropped — bounded
/// loss is preferable to OOM.
const EVENT_LOG_RETRY_BUFFER_CAP: usize = 4096;

/// Start the event log ingestion engine. Returns a handle for the runtime
/// and spawns a background flush task.
pub(crate) fn start_event_log(
    config: EventLogConfig,
    error_hook: Option<super::ErrorHook>,
) -> EventLogHandle {
    let store: Arc<dyn EventLogStore> = Arc::from(config.store);
    let seq = Arc::new(AtomicU64::new(1));
    // Clamp batch_size — `mpsc::channel(0)` panics, and `batch_size = 0`
    // would also defeat batching by triggering an immediate flush per
    // event.
    let batch_size = config.batch_size.max(1);
    // Clamp flush_interval — `tokio::time::interval` panics on a zero
    // period, which would kill the ingestion task. The gateway rejects
    // zero at the wire; this guards embedders constructing
    // `EventLogConfig` directly.
    let flush_interval = config.flush_interval.max(Duration::from_millis(1));
    // Buffer capacity: 4x batch size to absorb bursts; floor at 4 so a
    // tiny batch_size doesn't starve.
    let channel_capacity = (batch_size * 4).max(4);
    let (ingress_tx, ingress_rx) = mpsc::channel(channel_capacity);

    let handle = EventLogHandle {
        store: store.clone(),
        ingress_tx,
    };

    tokio::spawn(run_flush_loop(
        ingress_rx,
        store,
        seq,
        config.filter,
        batch_size,
        flush_interval,
        error_hook,
    ));

    handle
}

async fn run_flush_loop(
    mut rx: mpsc::Receiver<EventEnvelope<UnifiedEvent>>,
    store: Arc<dyn EventLogStore>,
    seq: Arc<AtomicU64>,
    filter: Option<EventFilter>,
    batch_size: usize,
    flush_interval: Duration,
    error_hook: Option<super::ErrorHook>,
) {
    let mut batch: Vec<PersistedEvent> = Vec::with_capacity(batch_size);
    let mut interval = tokio::time::interval(flush_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(envelope) => {
                        if let Some(ref f) = filter
                            && !f(&envelope.event)
                        {
                            continue;
                        }
                        let persisted = to_persisted(&seq, &envelope);
                        batch.push(persisted);
                        if batch.len() >= batch_size {
                            flush_batch(&store, &mut batch, &error_hook).await;
                        }
                    }
                    None => {
                        // Channel closed — best-effort final flush and exit.
                        // If this final flush fails, events on the floor
                        // are unrecoverable; the error hook fires so
                        // operators see it.
                        if !batch.is_empty() {
                            flush_batch(&store, &mut batch, &error_hook).await;
                        }
                        break;
                    }
                }
            }
            _ = interval.tick() => {
                if !batch.is_empty() {
                    flush_batch(&store, &mut batch, &error_hook).await;
                }
            }
        }
    }
}

/// Drop the oldest events from a retry buffer when it exceeds
/// [`EVENT_LOG_RETRY_BUFFER_CAP`]. Returns the count dropped. Bounded
/// loss is preferable to OOM under sustained store failure.
fn enforce_retry_cap(batch: &mut Vec<PersistedEvent>) -> usize {
    if batch.len() <= EVENT_LOG_RETRY_BUFFER_CAP {
        return 0;
    }
    let drop = batch.len() - EVENT_LOG_RETRY_BUFFER_CAP;
    batch.drain(0..drop);
    drop
}

fn to_persisted(seq: &AtomicU64, envelope: &EventEnvelope<UnifiedEvent>) -> PersistedEvent {
    let member_id = match &envelope.event {
        UnifiedEvent::Agent { agent_id, .. } => Some(agent_id.clone()),
        UnifiedEvent::Module(_) => None,
    };
    PersistedEvent {
        id: envelope.event_id.clone(),
        seq: seq.fetch_add(1, Ordering::Relaxed),
        timestamp_ms: envelope.timestamp_ms,
        member_id,
        event: envelope.event.clone(),
    }
}

async fn flush_batch(
    store: &Arc<dyn EventLogStore>,
    batch: &mut Vec<PersistedEvent>,
    error_hook: &Option<super::ErrorHook>,
) {
    let events = std::mem::take(batch);
    if let Err(err) = store.append_batch(events.clone()).await {
        // Restore the failed batch so the next tick / arrival retries
        // — silent loss on transient store errors was the prior bug.
        // Bound the retry buffer so a persistently-failing store can't
        // drive meta OOM.
        let mut restored = events;
        restored.append(batch); // events that arrived after the failure
        let dropped = enforce_retry_cap(&mut restored);
        *batch = restored;

        let msg = if dropped > 0 {
            format!(
                "event log flush failed: {err}; dropped {dropped} oldest events to bound the retry buffer at {EVENT_LOG_RETRY_BUFFER_CAP}"
            )
        } else {
            format!("event log flush failed: {err}; will retry")
        };
        let event = super::types::ErrorEvent::EventLogFlushFailure { error: msg };
        // Third fire point with its own hook Option, so it needs the same
        // default sink: a flush failure nobody is listening for is still a
        // flush failure.
        super::log_error_event(&event, error_hook.is_some());
        if let Some(hook) = error_hook {
            let hook = hook.clone();
            tokio::spawn(async move {
                let () = hook(event).await;
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Stub store that fails its first N `append_batch` calls and
    /// succeeds thereafter, recording every event it eventually accepts.
    struct FlakyStore {
        failures_remaining: Mutex<usize>,
        persisted: Mutex<Vec<PersistedEvent>>,
        attempts: Mutex<usize>,
    }

    impl EventLogStore for FlakyStore {
        fn append_batch(
            &self,
            events: Vec<PersistedEvent>,
        ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
            Box::pin(async move {
                *self.attempts.lock().expect("attempts") += 1;
                let mut left = self.failures_remaining.lock().expect("failures");
                if *left > 0 {
                    *left -= 1;
                    #[derive(Debug)]
                    struct Transient;
                    impl std::fmt::Display for Transient {
                        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            write!(f, "transient")
                        }
                    }
                    impl std::error::Error for Transient {}
                    return Err(Box::new(Transient) as Box<dyn std::error::Error + Send>);
                }
                self.persisted.lock().expect("persisted").extend(events);
                Ok(())
            })
        }

        fn query(
            &self,
            _query: EventQuery,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>>
        {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn sample_event(id: &str) -> PersistedEvent {
        PersistedEvent {
            id: id.to_string(),
            seq: 0,
            timestamp_ms: 0,
            member_id: None,
            event: UnifiedEvent::Module(crate::types::ModuleEvent {
                module: "test-module".into(),
                event_type: "x".into(),
                payload: serde_json::Value::Null,
            }),
        }
    }

    /// Regression: pre-fix, `flush_batch` did `mem::take(batch)` and
    /// dropped the events on store error. After the fix, the events
    /// stay in the batch until a subsequent flush succeeds.
    #[tokio::test]
    async fn flush_failure_retries_instead_of_dropping_events() {
        let flaky = Arc::new(FlakyStore {
            failures_remaining: Mutex::new(2),
            persisted: Mutex::new(Vec::new()),
            attempts: Mutex::new(0),
        });
        let store: Arc<dyn EventLogStore> = flaky.clone();
        let mut batch = vec![sample_event("a"), sample_event("b")];

        // First flush — fails. Batch must remain non-empty (events
        // pushed back into the retry buffer).
        flush_batch(&store, &mut batch, &None).await;
        assert_eq!(batch.len(), 2, "events must be retained on flush failure");

        // Second flush — fails again. Still retained.
        flush_batch(&store, &mut batch, &None).await;
        assert_eq!(batch.len(), 2);

        // Third flush — succeeds. Batch drained, events persisted.
        flush_batch(&store, &mut batch, &None).await;
        assert!(batch.is_empty(), "batch must drain on successful flush");

        assert_eq!(*flaky.attempts.lock().expect("attempts"), 3);
        assert_eq!(flaky.persisted.lock().expect("persisted").len(), 2);
    }

    /// Delegating wrapper so a test can hand the engine a boxed store
    /// while keeping a shared handle for inspection.
    struct SharedStore(Arc<FlakyStore>);

    impl EventLogStore for SharedStore {
        fn append_batch(
            &self,
            events: Vec<PersistedEvent>,
        ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
            self.0.append_batch(events)
        }

        fn query(
            &self,
            query: EventQuery,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>>
        {
            self.0.query(query)
        }
    }

    /// Regression: `tokio::time::interval(Duration::ZERO)` panics, which
    /// killed the ingestion task before it processed a single event. The
    /// engine clamps the interval, so a zero-interval config still flushes.
    #[tokio::test]
    async fn zero_flush_interval_does_not_kill_the_flush_loop() {
        let flaky = Arc::new(FlakyStore {
            failures_remaining: Mutex::new(0),
            persisted: Mutex::new(Vec::new()),
            attempts: Mutex::new(0),
        });
        // batch_size > 1 so only the interval tick can flush the single
        // ingested event — the path the zero interval used to kill.
        let handle = start_event_log(
            EventLogConfig {
                store: Box::new(SharedStore(flaky.clone())),
                filter: None,
                batch_size: 64,
                flush_interval: Duration::ZERO,
            },
            None,
        );
        handle.ingest(EventEnvelope {
            event_id: "evt-zero-interval".to_string(),
            source: "test".to_string(),
            timestamp_ms: 0,
            event: UnifiedEvent::Module(crate::types::ModuleEvent {
                module: "test-module".into(),
                event_type: "x".into(),
                payload: serde_json::Value::Null,
            }),
        });

        for _ in 0..400 {
            if !flaky.persisted.lock().expect("persisted").is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("event never flushed: zero flush_interval killed the ingestion task");
    }

    #[test]
    fn enforce_retry_cap_drops_oldest() {
        let mut batch: Vec<PersistedEvent> = (0..(EVENT_LOG_RETRY_BUFFER_CAP + 100))
            .map(|i| sample_event(&format!("evt-{i}")))
            .collect();
        let dropped = enforce_retry_cap(&mut batch);
        assert_eq!(dropped, 100);
        assert_eq!(batch.len(), EVENT_LOG_RETRY_BUFFER_CAP);
        // Newest 4096 retained — first remaining id is evt-100.
        assert_eq!(batch.first().expect("first").id, "evt-100");
    }
}
