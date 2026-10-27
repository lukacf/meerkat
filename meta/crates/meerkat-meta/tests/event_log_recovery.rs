#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Regression tests for the event-log ingestion engine.
//!
//! - `batch_size = 0` must not panic (`mpsc::channel(0)` panics, and a
//!   batch size of zero would defeat batching anyway). The runtime
//!   clamps to 1.
//! - A flush failure must NOT silently drop the events: the next flush
//!   tick should retry the same batch.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_meta::unified_runtime::EventLogError;
use meerkat_meta::{EventLogConfig, EventLogStore, EventQuery, PersistedEvent};

/// Stub store that fails its first N `append_batch` calls and succeeds
/// thereafter, recording everything it eventually accepts.
#[derive(Clone)]
struct FlakyStore {
    failures_remaining: Arc<Mutex<usize>>,
    persisted: Arc<Mutex<Vec<PersistedEvent>>>,
    attempts: Arc<Mutex<usize>>,
}

impl FlakyStore {
    fn new(failures: usize) -> Self {
        Self {
            failures_remaining: Arc::new(Mutex::new(failures)),
            persisted: Arc::new(Mutex::new(Vec::new())),
            attempts: Arc::new(Mutex::new(0)),
        }
    }
}

impl EventLogStore for FlakyStore {
    fn append_batch(
        &self,
        events: Vec<PersistedEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
        let failures = self.failures_remaining.clone();
        let persisted = self.persisted.clone();
        let attempts = self.attempts.clone();
        Box::pin(async move {
            *attempts.lock().expect("attempts lock") += 1;
            let mut left = failures.lock().expect("failures lock");
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
                let err: Box<dyn std::error::Error + Send> = Box::new(Transient);
                return Err(err);
            }
            persisted.lock().expect("persisted lock").extend(events);
            Ok(())
        })
    }

    fn query(
        &self,
        _query: EventQuery,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[tokio::test]
async fn batch_size_zero_does_not_panic() {
    // Regression: `mpsc::channel(0)` panics, and `batch_size = 0`
    // would also defeat batching by triggering an immediate flush per
    // event. The engine clamps to 1.
    let store: Box<dyn EventLogStore> = Box::new(FlakyStore::new(0));
    let config = EventLogConfig {
        store,
        batch_size: 0,
        flush_interval: Duration::from_millis(50),
        filter: None,
    };

    // The bug pre-fix: this call panics inside `start_event_log`.
    let runtime = Box::pin(
        meerkat_meta::UnifiedRuntimeBuilder::default()
            .definition(
                meerkat_mob::MobDefinition::from_toml(
                    "[mob]\nid = \"event-log-zero-batch\"\n\n\
                 [profiles.lead]\nmodel = \"gpt-5.5\"\nexternal_addressable = false",
                )
                .expect("definition"),
            )
            .event_log(config)
            .build(),
    )
    .await;
    assert!(
        runtime.is_ok(),
        "EventLogConfig {{ batch_size: 0 }} must not panic the runtime; got {:?}",
        runtime.as_ref().err()
    );
    let runtime = runtime.expect("runtime");
    let _ = runtime.shutdown().await;
}

// `flush_failure_retries_instead_of_dropping_events`: see the unit
// test of the same name in `crates/meerkat-meta/src/unified_runtime/event_log.rs`.
// `start_event_log` / `EventLogHandle` are `pub(crate)`, so the
// retry-buffer behavior is exercised in a unit test next to the
// implementation rather than from this integration target.
