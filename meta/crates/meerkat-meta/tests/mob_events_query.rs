//! Live-subscribe contract for the structural mob-events surface.
//!
//! The historical query coverage in this file exercised the in-memory
//! deque the store no longer maintains — every projection-level query
//! now scans the meerkat ledger directly. Ledger-backed query coverage
//! (forward / backward / stale-cursor / batch-scan with sparse filters)
//! lives in `mob_events_streaming.rs` and `mob_events_query_ledger.rs`,
//! which build a real `UnifiedRuntime`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::redundant_clone
)]

use chrono::Utc;
use meerkat_meta::unified_runtime::mob_events::MobEventsStore;
use meerkat_mob::event::{MobEvent, MobEventKind};
use meerkat_mob::ids::{FlowId, MobId, RunId};

fn flow_started(cursor: u64, mob: &str, run: &RunId) -> MobEvent {
    MobEvent {
        cursor,
        timestamp: Utc::now(),
        mob_id: MobId::from(mob),
        kind: MobEventKind::FlowStarted {
            run_id: run.clone(),
            flow_id: FlowId::from("flow-x"),
            params: serde_json::json!({}),
        },
    }
}

#[tokio::test]
async fn live_subscribe_receives_new_events() {
    let store = MobEventsStore::new();
    let mut rx = store.subscribe();
    let run = RunId::new();
    store
        .project_mob_event(&flow_started(1, "mob-A", &run))
        .await;
    let received = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .expect("recv timed out")
        .expect("recv ok");
    assert_eq!(received.kind, "flow_started");
    assert_eq!(received.mob_id, "mob-A");
    assert_eq!(received.cursor, 1);
}

#[tokio::test]
async fn project_event_for_query_does_not_broadcast() {
    let store = MobEventsStore::new();
    let mut rx = store.subscribe();
    let run = RunId::new();
    let _ = store
        .project_event_for_query(&flow_started(7, "mob-A", &run))
        .await;
    assert!(rx.try_recv().is_err());
}
