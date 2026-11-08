//! Persistent operational event log with buffered ingestion and pluggable storage.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

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
    /// A slow append does slow the runtime's event drain once the intake
    /// buffer is full (see [`EventLogConfig::batch_size`]): the drain waits
    /// for room rather than dropping events, and the console projection it
    /// also feeds waits with it.
    ///
    /// A batch carries at most [`EventLogConfig::batch_size`] events, retries
    /// included, oldest first. After a failed append the engine waits before
    /// it calls again: the flush interval, doubled for each further failed
    /// flush in a row, up to 30 seconds (or the flush interval when that is
    /// longer). The events wait in its retry buffer meanwhile, in order.
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
    /// Number of events to buffer before flushing to storage, and the most
    /// events one [`EventLogStore::append_batch`] call carries: a retry
    /// buffer larger than this drains in batches of this size, oldest first.
    /// Default: 64.
    ///
    /// The intake buffer in front of the flush holds four batches (at least
    /// 4 events). When it is full, the runtime's event drain waits for room
    /// before it takes the next event; no event is dropped to make room.
    pub batch_size: usize,
    /// Maximum time between flushes while the store takes them, even if the
    /// batch is not full. Default: 1 second.
    ///
    /// After the store refuses an append, the next flush waits this interval,
    /// doubled for each further refusal in a row, up to 30 seconds (or this
    /// interval when it is longer); while arriving events are dropped at the
    /// retry buffer's 4096-event limit, it waits one interval. A long
    /// interval therefore holds back the next append after a single refusal
    /// for at least that long. On a quiet log a retry runs at the first flush
    /// tick after its wait, up to one interval later.
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
    /// Sender for the intake buffer, which a background task flushes in
    /// batches. The buffer is bounded, and an event is sent only into room
    /// reserved for it ([`Self::room`]), so a full buffer makes intake wait
    /// instead of dropping the event.
    ingress_tx: mpsc::Sender<EventEnvelope<UnifiedEvent>>,
    /// Set once, by shutdown before it closes the event router
    /// ([`Self::close_intake`]): a wait for room then ends, so a store that
    /// never answers cannot hold the router open.
    intake_closed: watch::Sender<bool>,
    /// Whether the end of the flush task has been reported.
    flush_task_end_reported: AtomicBool,
}

/// Room for one event in the event log's intake buffer, reserved by
/// [`EventLogHandle::room`]. Ingesting through it neither waits nor drops;
/// dropping it unused gives the room back.
pub(crate) struct EventLogRoom<'a>(mpsc::Permit<'a, EventEnvelope<UnifiedEvent>>);

impl EventLogRoom<'_> {
    /// Ingest `event` into the room reserved for it.
    pub(crate) fn ingest(self, event: EventEnvelope<UnifiedEvent>) {
        self.0.send(event);
    }
}

/// Why [`EventLogHandle::room`] handed out no room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventLogIntakeEnded {
    /// Shutdown closed the intake ([`EventLogHandle::close_intake`]).
    Closed,
    /// The flush task has ended (a store or a filter panicked), so no event
    /// reaches the store again.
    FlushTaskEnded,
}

impl EventLogHandle {
    /// Return a cloned reference to the underlying store.
    pub fn store(&self) -> std::sync::Arc<dyn EventLogStore> {
        self.store.clone()
    }

    /// Room for one event in the intake buffer.
    ///
    /// While the buffer is full this waits for the flush task to take an
    /// event out, so a burst is never dropped and a slow store slows the
    /// caller instead. The wait is cancel-safe: reserve the room before
    /// taking the event from its source, and a caller cut off while it
    /// waits (a deadline, an abort) has taken nothing. Once shutdown has
    /// closed the intake ([`Self::close_intake`]) this answers
    /// [`EventLogIntakeEnded::Closed`] at once, room or not: the event router
    /// is closing, and a drain stops there.
    pub(crate) async fn room(&self) -> Result<EventLogRoom<'_>, EventLogIntakeEnded> {
        let closed = self.intake_closed.subscribe();
        tokio::select! {
            biased;
            () = until_intake_closed(closed) => Err(EventLogIntakeEnded::Closed),
            reserved = self.ingress_tx.reserve() => reserved
                .map(EventLogRoom)
                .map_err(|_| EventLogIntakeEnded::FlushTaskEnded),
        }
    }

    /// Close the intake: every wait for room ends, the ones already waiting
    /// included, and no room is handed out again. Shutdown calls this before
    /// it closes the event router, so a drain waiting on a store that never
    /// answers lets go of the router.
    pub(crate) fn close_intake(&self) {
        self.intake_closed.send_replace(true);
    }

    /// True for the first caller that learns the flush task has ended, so
    /// that end is reported once.
    pub(crate) fn first_to_see_flush_task_end(&self) -> bool {
        !self.flush_task_end_reported.swap(true, Ordering::Relaxed)
    }
}

/// Resolves once the intake is closed. The value is read through a guard
/// that is dropped here, so the caller's future stays `Send`.
async fn until_intake_closed(mut closed: watch::Receiver<bool>) {
    // An error means the handle, and with it the sender, is gone: there is
    // nothing left to wait for.
    let _ = closed.wait_for(|closed| *closed).await;
}

/// Hard cap on retained events while the store is unavailable. Beyond
/// this, the oldest events in the retry buffer are dropped — bounded
/// loss is preferable to OOM.
const EVENT_LOG_RETRY_BUFFER_CAP: usize = 4096;

/// Longest wait between two appends to a store that refuses them, unless the
/// flush interval is longer. The first wait is the flush interval, and each
/// further refused flush in a row doubles it; while arriving events are
/// dropped at [`EVENT_LOG_RETRY_BUFFER_CAP`], the wait is one interval.
const EVENT_LOG_RETRY_BACKOFF_MAX: Duration = Duration::from_secs(30);

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
    // tiny batch_size doesn't starve. A full buffer makes intake wait for
    // room (`EventLogHandle::room`), so the capacity bounds how far intake
    // runs ahead of the store, never what reaches it.
    let channel_capacity = (batch_size * 4).max(4);
    let (ingress_tx, ingress_rx) = mpsc::channel(channel_capacity);

    let handle = EventLogHandle {
        store: store.clone(),
        ingress_tx,
        intake_closed: watch::channel(false).0,
        flush_task_end_reported: AtomicBool::new(false),
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
    let mut retry = FlushRetry::new(flush_interval);
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
                        if retry.failing() {
                            // While a refused flush waits out its backoff,
                            // the retry buffer grows here: bound it here.
                            let dropped = enforce_retry_cap(&mut batch);
                            if dropped > 0 {
                                retry.dropped += dropped;
                                retry.dropping();
                            }
                        }
                        if batch.len() >= batch_size && retry.due() {
                            flush_batch(&store, &mut batch, batch_size, &mut retry, &error_hook)
                                .await;
                        }
                    }
                    None => {
                        // Channel closed — best-effort final flush and exit.
                        // If this final flush fails, events on the floor
                        // are unrecoverable; the error hook fires so
                        // operators see it.
                        // It does not wait out an earlier refusal's backoff.
                        if !batch.is_empty() {
                            flush_batch(&store, &mut batch, batch_size, &mut retry, &error_hook)
                                .await;
                        }
                        break;
                    }
                }
            }
            _ = interval.tick() => {
                if !batch.is_empty() && retry.due() {
                    flush_batch(&store, &mut batch, batch_size, &mut retry, &error_hook).await;
                }
            }
        }
    }
}

/// When the flush may ask the store again after appends it refused.
struct FlushRetry {
    /// The flush interval: the wait after the first refused flush.
    interval: Duration,
    /// Flushes refused in a row.
    failures: u32,
    /// No append before this instant; `None` while the store takes them.
    not_before: Option<tokio::time::Instant>,
    /// Events dropped at [`EVENT_LOG_RETRY_BUFFER_CAP`] since the last
    /// report.
    dropped: usize,
}

impl FlushRetry {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            failures: 0,
            not_before: None,
            dropped: 0,
        }
    }

    /// Whether the last flush was refused: the buffer is a retry buffer.
    fn failing(&self) -> bool {
        self.failures > 0
    }

    /// Whether the flush may ask the store now.
    fn due(&self) -> bool {
        self.not_before
            .is_none_or(|at| tokio::time::Instant::now() >= at)
    }

    /// The store took every buffered event: the next flush asks at once.
    fn succeeded(&mut self) {
        self.failures = 0;
        self.not_before = None;
    }

    /// Arriving events push older ones out of the full retry buffer: a longer
    /// wait only loses more of them, so the next flush asks the store within
    /// one interval.
    fn dropping(&mut self) {
        let within_an_interval = tokio::time::Instant::now() + self.interval;
        if self.not_before.is_none_or(|at| at > within_an_interval) {
            self.not_before = Some(within_an_interval);
        }
    }

    /// The store refused a flush: the wait before the next one. A flush that
    /// got events into the store before the refusal counts as the first
    /// refusal in a row. While events are being dropped at the cap
    /// (`dropping`), the wait is one interval.
    fn refused(&mut self, made_progress: bool, dropping: bool) -> Duration {
        if made_progress {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        let wait = if dropping {
            self.interval
        } else {
            flush_retry_wait(self.interval, self.failures)
        };
        self.not_before = Some(tokio::time::Instant::now() + wait);
        wait
    }
}

/// The wait after `failures` refused flushes in a row: the flush interval,
/// doubled for each refusal after the first, at most
/// [`EVENT_LOG_RETRY_BACKOFF_MAX`] (or the flush interval when that is
/// longer).
fn flush_retry_wait(interval: Duration, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    interval
        .saturating_mul(1 << doublings)
        .min(EVENT_LOG_RETRY_BACKOFF_MAX.max(interval))
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

/// Send the buffered events to the store in requests of at most
/// `batch_size` events, oldest first, until the buffer is empty or the store
/// refuses a request. The refused request and every event after it stay
/// buffered, in order, for the next flush, and `retry` says when that may ask
/// the store again.
async fn flush_batch(
    store: &Arc<dyn EventLogStore>,
    batch: &mut Vec<PersistedEvent>,
    batch_size: usize,
    retry: &mut FlushRetry,
    error_hook: &Option<super::ErrorHook>,
) {
    let mut sent = 0;
    let mut refusal = None;
    while sent < batch.len() {
        let end = batch.len().min(sent.saturating_add(batch_size));
        match store.append_batch(batch[sent..end].to_vec()).await {
            Ok(()) => sent = end,
            Err(err) => {
                refusal = Some(err);
                break;
            }
        }
    }
    // Only what the store took leaves the buffer: silent loss on transient
    // store errors was the prior bug.
    batch.drain(..sent);
    let msg = match refusal {
        None => {
            retry.succeeded();
            let dropped = std::mem::take(&mut retry.dropped);
            if dropped == 0 {
                return;
            }
            format!(
                "event log dropped {dropped} oldest events to bound the retry buffer at {EVENT_LOG_RETRY_BUFFER_CAP} while the store refused appends"
            )
        }
        Some(err) => {
            // Bound the retry buffer so a persistently failing store can't
            // drive meta OOM.
            let dropped = std::mem::take(&mut retry.dropped) + enforce_retry_cap(batch);
            let wait = retry.refused(sent > 0, dropped > 0);
            if dropped > 0 {
                format!(
                    "event log flush failed: {err}; dropped {dropped} oldest events to bound the retry buffer at {EVENT_LOG_RETRY_BUFFER_CAP}; will retry in {wait:?}"
                )
            } else {
                format!("event log flush failed: {err}; will retry in {wait:?}")
            }
        }
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
        // Called directly, each flush asks the store: the backoff it records
        // holds back only the flush loop.
        let mut retry = FlushRetry::new(Duration::from_secs(1));

        // First flush — fails. Batch must remain non-empty (events
        // pushed back into the retry buffer).
        flush_batch(&store, &mut batch, 64, &mut retry, &None).await;
        assert_eq!(batch.len(), 2, "events must be retained on flush failure");

        // Second flush — fails again. Still retained.
        flush_batch(&store, &mut batch, 64, &mut retry, &None).await;
        assert_eq!(batch.len(), 2);

        // Third flush — succeeds. Batch drained, events persisted.
        flush_batch(&store, &mut batch, 64, &mut retry, &None).await;
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
        handle
            .room()
            .await
            .expect("an empty intake has room")
            .ingest(EventEnvelope {
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

    /// A burst larger than the intake buffer reaches the store whole and in
    /// order. With `batch_size: 1` the buffer holds 4 events. Six are
    /// ingested back to back on a current-thread runtime, so the flush task
    /// first runs when the fifth waits for room. Intake used to `try_send`
    /// without waiting: the fifth and sixth found the buffer full and were
    /// dropped with no error, log or count, and the store kept 4.
    #[tokio::test(flavor = "current_thread")]
    async fn a_burst_larger_than_the_intake_buffer_reaches_the_store_whole() {
        let recorded = Arc::new(FlakyStore {
            failures_remaining: Mutex::new(0),
            persisted: Mutex::new(Vec::new()),
            attempts: Mutex::new(0),
        });
        let handle = start_event_log(
            EventLogConfig {
                store: Box::new(SharedStore(recorded.clone())),
                filter: None,
                batch_size: 1,
                flush_interval: Duration::from_secs(1),
            },
            None,
        );
        let burst: Vec<String> = (1..=6).map(|n| format!("evt-burst-{n}")).collect();
        for id in &burst {
            handle
                .room()
                .await
                .expect("the intake hands out room, waiting for it while full")
                .ingest(EventEnvelope {
                    event_id: id.clone(),
                    source: "module".to_string(),
                    timestamp_ms: 0,
                    event: UnifiedEvent::Module(crate::types::ModuleEvent {
                        module: "test-module".into(),
                        event_type: "x".into(),
                        payload: serde_json::Value::Null,
                    }),
                });
        }

        let stored = || -> Vec<String> {
            recorded
                .persisted
                .lock()
                .expect("persisted")
                .iter()
                .map(|event| event.id.clone())
                .collect()
        };
        for _ in 0..400 {
            if stored().len() >= burst.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            stored(),
            burst,
            "every event of the burst reaches the store, in order"
        );
    }

    /// Ingest one module event through the intake, waiting for room as the
    /// runtime's event drain does.
    async fn ingest_module_event(handle: &EventLogHandle, id: &str) {
        handle
            .room()
            .await
            .expect("the intake hands out room, waiting for it while full")
            .ingest(EventEnvelope {
                event_id: id.to_string(),
                source: "module".to_string(),
                timestamp_ms: 0,
                event: UnifiedEvent::Module(crate::types::ModuleEvent {
                    module: "test-module".into(),
                    event_type: "x".into(),
                    payload: serde_json::Value::Null,
                }),
            });
    }

    /// A failing store is not asked again for every event that arrives
    /// while it fails: after a failed append the flush waits out a backoff
    /// before it asks the store again. The flush used to re-send the whole
    /// retry buffer for every event that arrived while the buffer held
    /// `batch_size` or more, so 100 events arriving within half a second
    /// made about 101 failing appends.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_failing_store_is_not_called_once_per_arriving_event() {
        let failing = Arc::new(FlakyStore {
            failures_remaining: Mutex::new(usize::MAX),
            persisted: Mutex::new(Vec::new()),
            attempts: Mutex::new(0),
        });
        let handle = start_event_log(
            EventLogConfig {
                store: Box::new(SharedStore(failing.clone())),
                filter: None,
                batch_size: 4,
                flush_interval: Duration::from_secs(1),
            },
            None,
        );
        // The first four events make the first, failing, append. The next
        // 100 arrive one every 4 ms: all of them within the first half
        // second.
        for n in 0..104 {
            ingest_module_event(&handle, &format!("evt-failing-{n}")).await;
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        let attempts = *failing.attempts.lock().expect("attempts");
        assert!(
            attempts <= 2,
            "a failing store was asked {attempts} times while 104 events arrived within half a second"
        );
    }

    /// A store that refuses every append while it is down, and any append of
    /// more than `cap` events (a request size limit), keeping the ids it
    /// accepts, the largest request it was sent and how many it was sent.
    #[derive(Clone)]
    struct CappedStore(Arc<CappedState>);

    struct CappedState {
        cap: usize,
        down: AtomicBool,
        largest_request: std::sync::atomic::AtomicUsize,
        calls: std::sync::atomic::AtomicUsize,
        persisted: Mutex<Vec<String>>,
    }

    impl CappedStore {
        fn down_with_cap(cap: usize) -> Self {
            Self(Arc::new(CappedState {
                cap,
                down: AtomicBool::new(true),
                largest_request: std::sync::atomic::AtomicUsize::new(0),
                calls: std::sync::atomic::AtomicUsize::new(0),
                persisted: Mutex::new(Vec::new()),
            }))
        }

        fn recover(&self) {
            self.0.down.store(false, Ordering::SeqCst);
        }

        fn persisted(&self) -> Vec<String> {
            self.0.persisted.lock().expect("persisted").clone()
        }

        fn largest_request(&self) -> usize {
            self.0.largest_request.load(Ordering::SeqCst)
        }

        fn calls(&self) -> usize {
            self.0.calls.load(Ordering::SeqCst)
        }
    }

    impl EventLogStore for CappedStore {
        fn append_batch(
            &self,
            events: Vec<PersistedEvent>,
        ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
            Box::pin(async move {
                self.0.calls.fetch_add(1, Ordering::SeqCst);
                self.0
                    .largest_request
                    .fetch_max(events.len(), Ordering::SeqCst);
                if self.0.down.load(Ordering::SeqCst) || events.len() > self.0.cap {
                    return Err(
                        Box::new(std::io::Error::other("the store refused the append"))
                            as Box<dyn std::error::Error + Send>,
                    );
                }
                self.0
                    .persisted
                    .lock()
                    .expect("persisted")
                    .extend(events.into_iter().map(|event| event.id));
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

    /// After an outage the retry buffer drains in requests of at most
    /// `batch_size` events, oldest first. The flush used to re-send the whole
    /// buffer in one append, so once the buffer held more than the store
    /// takes in one request, no event reached the store again, however
    /// healthy the store was.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_recovered_store_drains_the_retry_buffer_in_batch_size_requests() {
        let store = CappedStore::down_with_cap(4);
        let handle = start_event_log(
            EventLogConfig {
                store: Box::new(store.clone()),
                filter: None,
                batch_size: 4,
                flush_interval: Duration::from_secs(1),
            },
            None,
        );
        let ids: Vec<String> = (0..12).map(|n| format!("evt-outage-{n}")).collect();
        for id in &ids {
            ingest_module_event(&handle, id).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        store.recover();
        // Two minutes is far past every retry a backoff schedules here.
        tokio::time::sleep(Duration::from_mins(2)).await;
        assert_eq!(
            store.persisted(),
            ids,
            "every event buffered during the outage reaches the recovered store, in order"
        );
        assert!(
            store.largest_request() <= 4,
            "no append carried more than batch_size events: largest {}",
            store.largest_request()
        );
    }

    /// While the retry buffer drops events at its cap, a longer wait only
    /// loses more of them: the flush asks the store again within one flush
    /// interval, still one request at a time. The wait used to keep doubling
    /// to 30 s while each arriving event pushed out an older one, so a store
    /// that recovered just after a refusal was asked again up to 30 s later,
    /// and the events dropped in that time were lost.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_store_that_recovers_while_events_drop_at_the_cap_is_asked_within_an_interval() {
        let store = CappedStore::down_with_cap(4);
        let handle = start_event_log(
            EventLogConfig {
                store: Box::new(store.clone()),
                filter: None,
                batch_size: 4,
                flush_interval: Duration::from_secs(1),
            },
            None,
        );
        // 100 events a second: the wait reaches 30 s after about 31 s of
        // refusals, and the retry buffer reaches its cap after about 41 s.
        let outage = Duration::from_secs(62);
        let started = tokio::time::Instant::now();
        let mut n = 0;
        while started.elapsed() < outage {
            ingest_module_event(&handle, &format!("evt-cap-{n}")).await;
            n += 1;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let asked_while_down = store.calls();
        store.recover();
        let recovered_at = tokio::time::Instant::now();
        while store.persisted().is_empty() && recovered_at.elapsed() < Duration::from_secs(40) {
            ingest_module_event(&handle, &format!("evt-cap-{n}")).await;
            n += 1;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let asked_again_after = recovered_at.elapsed();
        assert!(
            asked_again_after <= Duration::from_secs(2),
            "the recovered store was asked again {asked_again_after:?} after it recovered, while events were dropping at the cap; the wait is one flush interval (1 s)"
        );
        assert!(
            asked_while_down <= 62,
            "the failing store was asked {asked_while_down} times in a 62 s outage: at most once per flush interval"
        );
    }

    #[test]
    fn the_retry_wait_doubles_from_the_flush_interval_up_to_its_cap() {
        let second = Duration::from_secs(1);
        let waits: Vec<Duration> = (1..=7).map(|n| flush_retry_wait(second, n)).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30].map(Duration::from_secs));
        assert_eq!(flush_retry_wait(second, u32::MAX), Duration::from_secs(30));
        assert_eq!(
            flush_retry_wait(Duration::from_secs(45), 3),
            Duration::from_secs(45),
            "a flush interval longer than the cap is the wait"
        );
    }

    /// A store that takes its first append and refuses every later one.
    #[derive(Default)]
    struct TakesFirstAppend {
        calls: std::sync::atomic::AtomicUsize,
        persisted: Mutex<Vec<String>>,
    }

    impl EventLogStore for TakesFirstAppend {
        fn append_batch(
            &self,
            events: Vec<PersistedEvent>,
        ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
            Box::pin(async move {
                if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
                    return Err(
                        Box::new(std::io::Error::other("the store refused the append"))
                            as Box<dyn std::error::Error + Send>,
                    );
                }
                self.persisted
                    .lock()
                    .expect("persisted")
                    .extend(events.into_iter().map(|event| event.id));
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

    /// A flush the store refuses part way keeps the refused request and every
    /// later event buffered, in order. The store took events, so the refusal
    /// counts as the first in a row: the wait starts again at the interval.
    /// The clock is paused, so the wait has not passed when the row looks.
    #[tokio::test(start_paused = true)]
    async fn a_flush_refused_part_way_keeps_the_rest_in_order() {
        let taker = Arc::new(TakesFirstAppend::default());
        let store: Arc<dyn EventLogStore> = taker.clone();
        let mut batch: Vec<PersistedEvent> =
            (0..5).map(|n| sample_event(&format!("evt-{n}"))).collect();
        let mut retry = FlushRetry::new(Duration::from_secs(1));
        retry.refused(false, false);
        retry.refused(false, false);

        flush_batch(&store, &mut batch, 2, &mut retry, &None).await;
        let buffered = |batch: &[PersistedEvent]| -> Vec<String> {
            batch.iter().map(|event| event.id.clone()).collect()
        };
        assert_eq!(
            taker.persisted.lock().expect("persisted").clone(),
            ["evt-0", "evt-1"]
        );
        assert_eq!(buffered(&batch), ["evt-2", "evt-3", "evt-4"]);
        assert_eq!(retry.failures, 1);
        assert!(!retry.due(), "the next flush waits");

        flush_batch(&store, &mut batch, 2, &mut retry, &None).await;
        assert_eq!(buffered(&batch), ["evt-2", "evt-3", "evt-4"]);
        assert_eq!(retry.failures, 2);
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
