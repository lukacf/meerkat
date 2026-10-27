//! Projection of meerkat's TRANSACTIONAL WorkGraph facts onto Meta's event
//! and SSE surfaces.
//!
//! # What a fact is, and what it is not
//!
//! `WorkGraphFact` (meerkat 0.8.22, `meerkat-workgraph/src/types.rs`) is
//! recorded by the SAME durable commit as the mutation that first observes it.
//! It is a ledger fact, not a wake instruction, and this module never promotes
//! one into authority:
//!
//! - A projected fact is a **lossy wake accelerant**. It says "something at
//!   this identity may have changed"; it never says what the current state is.
//! - The envelope carries **identifiers only** ([`WorkGraphFactEnvelope`]).
//!   No `WorkItem`, no status, no owner, no claim, no evidence. A consumer
//!   that wants state MUST read it back through the authoritative WorkGraph
//!   read surface (`meta/workgraph/get`, `/list`, `/ready`, `/snapshot`).
//! - Delivery is best-effort. [`WorkGraphFactStream`] is a
//!   [`tokio::sync::broadcast`] channel: a slow subscriber is dropped with
//!   `RecvError::Lagged` and this module does **not** backfill. Losing a wake
//!   costs latency, never correctness, precisely because no decision may be
//!   made from the fact alone.
//! - Nothing inside Meta consumes a projected fact to make a decision. The
//!   projection is emit-only.
//!
//! # Why polling, and why never `list_attention`
//!
//! meerkat-workgraph exposes no subscription seam at 0.8.22 - there is no
//! `subscribe`, `watch`, or `broadcast` anywhere in the crate. The only
//! readable stream is `WorkGraphService::events`, an `after_seq`-cursored,
//! bounded, ascending page over the durable event ledger, so the bridge is a
//! cursor tail ([`poll_workgraph_facts`]).
//!
//! `WorkGraphService::list_attention` is deliberately NOT on this path.
//! Upstream strips the status filter before the store read
//! (`meerkat-workgraph/src/service.rs:364`) and re-applies eligibility-at-now
//! in process (`:378`), so every call reads `MAX_COLLECTION_LIMIT + 1 = 1001`
//! rows (`:368`) and REFUSES outright above 1000 (`:370`) - while superseded
//! and stopped binding rows accumulate permanently. Calling it once per
//! transition is the outage path; this module never calls it.
//!
//! # Cursor semantics
//!
//! [`WorkGraphFactPage::next_after_seq`] is the maximum `seq` of the events
//! this page actually observed. It is safe to resume from ("no visible event
//! is skipped") but it is deliberately NOT advertised as the raw ledger
//! frontier: `list_public_events` omits the internal `ExecutionBound` /
//! `ExecutionTransitioned` kinds, so ledger rows may exist between two
//! consecutive visible sequences.
//!
//! Events that carry no `seq` cannot advance a cursor, and emitting their
//! facts would re-emit the same fact on every poll forever. Both built-in
//! stores stamp `seq` on every read row (`meerkat-workgraph/src/store.rs:1647`
//! for the memory store, `:4583` for SQLite), so a seq-less row means a custom
//! `WorkGraphStore` is not stamping sequences. Such rows are counted into
//! [`WorkGraphFactPage::events_without_seq`] as a defect signal and never
//! emitted.
//!
//! # Subscription activation window
//!
//! Registering an SSE subscriber wakes the idle tail, while the SSE stream
//! independently sends `workgraph.resync_required` with `initial_sync`. The
//! tail then discovers the current durable frontier before it starts live
//! polling. A mutation committed after `initial_sync` is delivered but before
//! that frontier read completes can therefore be included in the starting
//! cursor and produce no fact wake. This loss is intentional and is the same
//! class of loss as broadcast lag: the stream is never a completeness feed.
//! Consumers whose correctness depends on freshness must schedule durable
//! WorkGraph pulls themselves; `initial_sync` and later facts only accelerate
//! those reads.

use std::sync::Arc;
use std::time::Duration;

use meerkat::{
    WorkGraphError, WorkGraphEvent, WorkGraphEventFilter, WorkGraphFact, WorkGraphService,
    WorkGraphStoreKind, WorkItemId, WorkNamespace,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Notify, broadcast};

use crate::types::{ModuleEvent, UnifiedEvent};

/// Per-poll page size used when a caller does not choose one.
pub const DEFAULT_FACT_POLL_LIMIT: usize = 256;

/// Hard ceiling on a single poll, mirroring upstream's `MAX_COLLECTION_LIMIT`
/// (`meerkat-workgraph/src/service.rs:71`). `WorkGraphService::events` turns a
/// larger limit into `InvalidInput`, so callers are clamped rather than
/// failed - a wake accelerant must not be able to error a tail loop through an
/// over-eager page size.
pub const MAX_FACT_POLL_LIMIT: usize = 1000;

/// Default interval between tail polls.
pub const DEFAULT_FACT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Broadcast capacity for in-process fact subscribers.
pub const WORKGRAPH_FACTS_CHANNEL_CAP: usize = 512;

/// Module identity used on the existing [`UnifiedEvent::Module`] surface.
pub const WORKGRAPH_EVENT_MODULE: &str = "meta.workgraph";

/// A lossy transactional fact wake. Its payload is a
/// [`WorkGraphFactEnvelope`], not authoritative item state.
pub const WORKGRAPH_FACT_EVENT_TYPE: &str = "workgraph.fact";

/// The subscriber must re-read durable WorkGraph state before acting.
pub const WORKGRAPH_RESYNC_REQUIRED_EVENT_TYPE: &str = "workgraph.resync_required";

/// One transactional WorkGraph fact, addressed by the ledger sequence that
/// observed it.
///
/// Identifiers only, by construction. See the module docs: adding item state
/// here would let a consumer treat the accelerant as authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGraphFactEnvelope {
    /// Ledger sequence of the mutation event that recorded the fact. Usable
    /// as a resume cursor; see [`WorkGraphFactPage::next_after_seq`].
    pub seq: i64,
    /// Realm of the recording event. Constant for a realm-scoped runtime.
    pub realm_id: String,
    /// Namespace of the recording event.
    pub namespace: WorkNamespace,
    /// Item the recording event was addressed to, when it had one. This is
    /// the event's subject, which is not necessarily the item named inside
    /// `fact` (a `Closed` child records `ItemReady` for its parent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<WorkItemId>,
    /// The transactional fact, verbatim from meerkat.
    pub fact: WorkGraphFact,
}

/// One bounded page of projected facts plus its resume cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGraphFactPage {
    /// Projected facts in ledger order.
    pub facts: Vec<WorkGraphFactEnvelope>,
    /// Cursor to pass as the next `after_seq`. `None` only when nothing with
    /// a sequence has been observed yet; a caller must then keep its previous
    /// cursor rather than restart from the beginning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after_seq: Option<i64>,
    /// The page filled its limit, so more may already be pending. Poll again
    /// immediately instead of waiting out the interval.
    pub more_may_be_pending: bool,
    /// Events observed with no ledger sequence. Always `0` against the
    /// built-in stores; a non-zero count is a custom-store defect signal, and
    /// those events contributed no facts.
    pub events_without_seq: usize,
}

impl WorkGraphFactPage {
    /// Whether the page carries no facts at all.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }
}

/// Project the facts carried by an ordered page of WorkGraph events.
///
/// Pure and store-free so the cursor rules are testable in isolation.
/// `after_seq` is the cursor the page was read with and is preserved when the
/// page observed nothing sequenced.
pub fn project_workgraph_fact_page(
    events: &[WorkGraphEvent],
    after_seq: Option<i64>,
    limit: usize,
) -> WorkGraphFactPage {
    let limit = limit.max(1);
    let mut facts = Vec::new();
    let mut next_after_seq = after_seq;
    let mut events_without_seq = 0usize;
    for event in events {
        // A row with no sequence cannot advance the cursor. Emitting its facts
        // would replay them on every subsequent poll forever, so it is counted
        // and skipped instead.
        let Some(seq) = event.seq else {
            events_without_seq = events_without_seq.saturating_add(1);
            continue;
        };
        next_after_seq = Some(match next_after_seq {
            Some(current) => current.max(seq),
            None => seq,
        });
        for fact in &event.facts {
            facts.push(WorkGraphFactEnvelope {
                seq,
                realm_id: event.realm_id.clone(),
                namespace: event.namespace.clone(),
                item_id: event.item_id.clone(),
                fact: fact.clone(),
            });
        }
    }
    WorkGraphFactPage {
        facts,
        next_after_seq,
        more_may_be_pending: events.len() >= limit,
        events_without_seq,
    }
}

/// Read one bounded page of transactional facts after `after_seq`.
///
/// Routes through `WorkGraphService::events`, which applies this runtime's
/// realm/namespace grant. `limit` is clamped into
/// `1..=`[`MAX_FACT_POLL_LIMIT`].
pub async fn poll_workgraph_facts(
    service: &WorkGraphService,
    after_seq: Option<i64>,
    limit: usize,
) -> Result<WorkGraphFactPage, WorkGraphError> {
    let limit = limit.clamp(1, MAX_FACT_POLL_LIMIT);
    // realm_id/namespace stay `None`: the service fills its own scoped
    // defaults and then validates them against its grant.
    let events = service
        .events(WorkGraphEventFilter {
            realm_id: None,
            namespace: None,
            all_namespaces: false,
            after_seq,
            limit: Some(limit),
        })
        .await?;
    Ok(project_workgraph_fact_page(&events, after_seq, limit))
}

/// Current ledger frontier for this service's scope, without replaying
/// history.
///
/// Intended for tail startup ("only wake me for what happens from now on").
/// The built-in stores answer this with a single `MAX(seq)`
/// (`meerkat-workgraph/src/store.rs:3394` for SQLite); a custom store that
/// does not override `WorkGraphStore::latest_event_seq` falls back to reading
/// the whole event list, so call it once at startup, not per tick.
///
/// This reads the store directly because the service exposes no frontier
/// query. The filter is built from the service's OWN realm/namespace, and
/// every `WorkGraphService` constructor (`new`, `with_scope`,
/// `with_namespace_grant`) sets `default_realm_id`/`default_namespace` from
/// the same values as `namespace_grant`, so this cannot address a scope the
/// service would have refused. The frontier counts internal execution rows
/// the public page omits, which is harmless: it is only ever used as a "skip
/// everything before now" starting cursor.
///
/// Cost is store-class dependent. The runtime tail calls this only after a
/// subscriber arrives, and only for the built-in stores that override the
/// default implementation.
pub async fn latest_workgraph_fact_seq(
    service: &WorkGraphService,
) -> Result<Option<i64>, WorkGraphError> {
    service
        .store()
        .latest_event_seq(WorkGraphEventFilter {
            realm_id: Some(service.default_realm_id().to_string()),
            namespace: Some(service.default_namespace().clone()),
            all_namespaces: false,
            after_seq: None,
            limit: None,
        })
        .await
}

/// Convert a fact wake into the existing unified module-event shape.
///
/// The SSE route deliberately does not add an SSE `id` field and offers no
/// replay-by-id contract. The durable `seq` inside the payload is the only
/// cursor, and even that cursor is useful only with the authoritative pull
/// surface.
pub fn workgraph_fact_event(fact: &WorkGraphFactEnvelope) -> UnifiedEvent {
    UnifiedEvent::Module(ModuleEvent {
        module: WORKGRAPH_EVENT_MODULE.to_string(),
        event_type: WORKGRAPH_FACT_EVENT_TYPE.to_string(),
        payload: serde_json::to_value(fact).unwrap_or_else(|_| {
            json!({
                "seq": fact.seq,
                "projection_error": true,
            })
        }),
    })
}

/// Construct the explicit resynchronization signal used at subscription and
/// after broadcast lag. No state rides this event; clients must pull
/// `meta/workgraph/get`, `/list`, `/ready`, or `/snapshot`.
pub fn workgraph_resync_required_event(reason: &'static str, skipped: Option<u64>) -> UnifiedEvent {
    UnifiedEvent::Module(ModuleEvent {
        module: WORKGRAPH_EVENT_MODULE.to_string(),
        event_type: WORKGRAPH_RESYNC_REQUIRED_EVENT_TYPE.to_string(),
        payload: json!({
            "reason": reason,
            "skipped": skipped,
            "authority": "durable_workgraph_pull",
        }),
    })
}

/// In-process, runtime-owned fan-out for projected WorkGraph module events.
///
/// Lossy on purpose: subscribers that fall behind receive
/// [`broadcast::error::RecvError::Lagged`] and MUST recover by re-reading
/// WorkGraph. `subscribe` notifies the one runtime-owned tail so the idle
/// state can remain entirely store-free.
#[derive(Clone)]
pub struct WorkGraphFactHub {
    tx: broadcast::Sender<UnifiedEvent>,
    subscriber_arrived: Arc<Notify>,
    tail_ready: Arc<Notify>,
}

impl Default for WorkGraphFactHub {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WorkGraphFactHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkGraphFactHub")
            .field("receiver_count", &self.tx.receiver_count())
            .finish()
    }
}

impl WorkGraphFactHub {
    pub fn new() -> Self {
        Self::with_capacity(WORKGRAPH_FACTS_CHANNEL_CAP)
    }

    /// Construct a hub with an explicit broadcast capacity. Primarily useful
    /// for deterministic lag tests.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            subscriber_arrived: Arc::new(Notify::new()),
            tail_ready: Arc::new(Notify::new()),
        }
    }

    /// Subscribe to subsequently published module events. Nothing is
    /// replayed. The HTTP route sends `initial_sync` before reading this
    /// receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<UnifiedEvent> {
        let rx = self.tx.subscribe();
        self.subscriber_arrived.notify_one();
        rx
    }

    /// Live subscriber count.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Wait until at least one subscriber exists without reading WorkGraph.
    pub async fn wait_for_subscriber(&self) {
        while self.receiver_count() == 0 {
            self.subscriber_arrived.notified().await;
        }
    }

    /// Wait until the runtime tail has completed cursor discovery and its
    /// first live poll for the current activation. This is a diagnostic/test
    /// barrier only; it grants no synchronization or state authority.
    #[doc(hidden)]
    pub async fn wait_for_tail_ready(&self) {
        self.tail_ready.notified().await;
    }

    fn mark_tail_ready(&self) {
        self.tail_ready.notify_one();
    }

    fn publish(&self, event: UnifiedEvent) -> bool {
        self.receiver_count() > 0 && self.tx.send(event).is_ok()
    }

    /// Publish one fact wake through the module-event contract. Zero
    /// subscribers is the normal idle state and is not an error.
    pub fn publish_fact(&self, fact: &WorkGraphFactEnvelope) -> bool {
        self.publish(workgraph_fact_event(fact))
    }

    /// Publish every fact in `page`, returning how many were accepted by the
    /// channel.
    pub fn publish_page(&self, page: &WorkGraphFactPage) -> usize {
        if self.receiver_count() == 0 {
            return 0;
        }
        page.facts
            .iter()
            .filter(|fact| self.publish_fact(fact))
            .count()
    }
}

/// Backwards-compatible name for the public in-process fact stream.
pub type WorkGraphFactStream = WorkGraphFactHub;

/// Tail behaviour for [`spawn_workgraph_fact_tail`].
#[derive(Debug, Clone, Copy)]
pub struct WorkGraphFactTailOptions {
    /// Delay between polls once the tail is caught up.
    pub poll_interval: Duration,
    /// Per-poll page size, clamped into `1..=`[`MAX_FACT_POLL_LIMIT`].
    pub page_limit: usize,
}

impl Default for WorkGraphFactTailOptions {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_FACT_POLL_INTERVAL,
            page_limit: DEFAULT_FACT_POLL_LIMIT,
        }
    }
}

/// Spawn the cursor tail that feeds `hub`.
///
/// The unified runtime creates exactly one when WorkGraph is configured and
/// owns the returned task through shutdown. The tail remains in a notified
/// no-read idle state until a subscriber arrives.
///
/// Every failure is logged and retried. A poll error must not take down a
/// host: no correctness depends on the accelerant arriving.
pub fn spawn_workgraph_fact_tail(
    service: WorkGraphService,
    hub: WorkGraphFactHub,
    options: WorkGraphFactTailOptions,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // The idle state is an actual state, not a periodic emptiness
            // check: it performs zero page reads and zero frontier reads.
            hub.wait_for_subscriber().await;

            // A fresh subscriber has already been told to resynchronize. Set
            // the live cursor at the durable frontier so historical facts are
            // not replayed as if they were newly observed. Meta's supported
            // Memory and SQLite stores override `latest_event_seq` with a
            // non-materializing read. Custom stores cannot prove that bound,
            // so their discovery uses bounded public pages and discards them.
            let mut after_seq = match service.store().kind() {
                WorkGraphStoreKind::Memory | WorkGraphStoreKind::Sqlite => {
                    match latest_workgraph_fact_seq(&service).await {
                        Ok(seq) => seq,
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                "workgraph fact tail could not discover the ledger frontier",
                            );
                            None
                        }
                    }
                }
                WorkGraphStoreKind::Disabled => None,
                WorkGraphStoreKind::Custom => {
                    let mut cursor = None;
                    while hub.receiver_count() > 0 {
                        match poll_workgraph_facts(&service, cursor, options.page_limit).await {
                            Ok(page) => {
                                cursor = page.next_after_seq;
                                if !page.more_may_be_pending {
                                    break;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(
                                    %error,
                                    "workgraph fact tail custom-store cursor discovery failed",
                                );
                                break;
                            }
                        }
                        // Cursor discovery is bounded to one page per
                        // scheduled interval. It never becomes a hot
                        // full-history scan.
                        tokio::time::sleep(options.poll_interval).await;
                    }
                    cursor
                }
            };

            // If every subscriber disappeared during discovery, return to the
            // no-read idle state before touching the store again.
            if hub.receiver_count() == 0 {
                continue;
            }

            let mut first_live_poll_pending = true;
            loop {
                if hub.receiver_count() == 0 {
                    break;
                }
                let first_live_poll = first_live_poll_pending;
                let polled_after_seq = after_seq;
                match poll_workgraph_facts(&service, after_seq, options.page_limit).await {
                    Ok(page) => {
                        if page.events_without_seq > 0 {
                            tracing::warn!(
                                events_without_seq = page.events_without_seq,
                                "workgraph store returned events with no ledger sequence; \
                                 their facts were not projected",
                            );
                        }
                        hub.publish_page(&page);
                        let cursor_advanced = page.next_after_seq != polled_after_seq;
                        after_seq = page.next_after_seq;
                        // Fast-drain a backlog ONLY when the cursor actually
                        // moved. Fullness alone is never progress authority.
                        if page.more_may_be_pending && cursor_advanced {
                            tokio::task::yield_now().await;
                            continue;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "workgraph fact poll failed; retrying after the poll interval",
                        );
                    }
                }
                if first_live_poll {
                    hub.mark_tail_ready();
                    first_live_poll_pending = false;
                }
                tokio::time::sleep(options.poll_interval).await;
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use meerkat::WorkGraphEventKind;

    fn item_id(value: &str) -> WorkItemId {
        WorkItemId::new(value).expect("valid work item id")
    }

    fn ready_fact(id: &str, revision: u64) -> WorkGraphFact {
        WorkGraphFact::ItemReady {
            item_id: item_id(id),
            item_revision: revision,
        }
    }

    fn event(seq: Option<i64>, id: &str, facts: Vec<WorkGraphFact>) -> WorkGraphEvent {
        let mut event = WorkGraphEvent::item(
            "realm".to_string(),
            WorkNamespace::default(),
            item_id(id),
            WorkGraphEventKind::Updated,
            chrono::Utc::now(),
            serde_json::Value::Null,
        );
        event.seq = seq;
        event.facts = facts;
        event
    }

    #[test]
    fn fact_envelope_carries_identifiers_only() {
        let envelope = WorkGraphFactEnvelope {
            seq: 7,
            realm_id: "realm".to_string(),
            namespace: WorkNamespace::default(),
            item_id: Some(item_id("work_a")),
            fact: ready_fact("work_a", 3),
        };
        let value = serde_json::to_value(&envelope).expect("envelope serializes");
        let object = value.as_object().expect("envelope is a JSON object");
        // Positive control: the projection ran and the fact IS on the wire.
        assert!(object.contains_key("fact"), "fact must be projected");
        let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, ["fact", "item_id", "namespace", "realm_id", "seq"]);
        // Negative assertion: no authoritative item state ever rides a wake
        // accelerant. Consumers must read WorkGraph back for state.
        for forbidden in [
            "item", "status", "owner", "claim", "evidence", "payload", "title", "revision",
        ] {
            assert!(
                !object.contains_key(forbidden),
                "'{forbidden}' would let a consumer treat the accelerant as authority",
            );
        }
    }

    #[test]
    fn projection_emits_one_envelope_per_fact_and_advances_the_cursor() {
        let events = vec![
            event(Some(4), "work_a", vec![ready_fact("work_a", 1)]),
            event(
                Some(9),
                "work_b",
                vec![ready_fact("work_b", 2), ready_fact("work_parent", 5)],
            ),
        ];
        let page = project_workgraph_fact_page(&events, Some(1), 64);
        assert_eq!(page.facts.len(), 3);
        assert_eq!(page.next_after_seq, Some(9));
        assert!(!page.more_may_be_pending);
        assert_eq!(page.events_without_seq, 0);
        assert_eq!(page.facts[0].seq, 4);
        assert_eq!(page.facts[2].seq, 9);
        // The recording event's subject is preserved even when the fact names
        // a different item.
        assert_eq!(page.facts[2].item_id, Some(item_id("work_b")));
    }

    #[test]
    fn events_without_facts_still_advance_the_cursor() {
        let events = vec![event(Some(11), "work_a", Vec::new())];
        let page = project_workgraph_fact_page(&events, Some(2), 64);
        assert!(page.is_empty());
        assert_eq!(page.next_after_seq, Some(11));
    }

    #[test]
    fn seq_less_events_never_emit_and_never_stall_a_live_cursor() {
        let events = vec![
            event(None, "work_a", vec![ready_fact("work_a", 1)]),
            event(Some(6), "work_b", vec![ready_fact("work_b", 1)]),
        ];
        let page = project_workgraph_fact_page(&events, Some(3), 64);
        assert_eq!(page.events_without_seq, 1);
        assert_eq!(page.facts.len(), 1, "the seq-less fact must not be emitted");
        assert_eq!(page.facts[0].seq, 6);
        assert_eq!(page.next_after_seq, Some(6));
    }

    #[test]
    fn a_page_of_only_seq_less_events_preserves_the_incoming_cursor() {
        let events = vec![event(None, "work_a", vec![ready_fact("work_a", 1)])];
        let page = project_workgraph_fact_page(&events, Some(3), 64);
        assert!(page.is_empty());
        assert_eq!(
            page.next_after_seq,
            Some(3),
            "an unsequenced page must never rewind the cursor",
        );
    }

    #[test]
    fn the_cursor_never_moves_backwards() {
        let events = vec![event(Some(2), "work_a", vec![ready_fact("work_a", 1)])];
        let page = project_workgraph_fact_page(&events, Some(40), 64);
        assert_eq!(page.next_after_seq, Some(40));
    }

    #[test]
    fn a_full_page_reports_that_more_may_be_pending() {
        let events = vec![
            event(Some(1), "work_a", Vec::new()),
            event(Some(2), "work_b", Vec::new()),
        ];
        assert!(project_workgraph_fact_page(&events, None, 2).more_may_be_pending);
        assert!(!project_workgraph_fact_page(&events, None, 3).more_may_be_pending);
        // A zero limit is not a "everything is pending" signal.
        assert!(!project_workgraph_fact_page(&[], None, 0).more_may_be_pending);
    }

    /// The tail's fast-drain condition. `more_may_be_pending` is a fullness
    /// fact, NOT a progress fact - and a full page of seq-less rows produces
    /// fullness with zero progress. `spawn_workgraph_fact_tail` must
    /// therefore require BOTH before it skips the interval sleep; requiring
    /// fullness alone re-issues a byte-identical read forever.
    #[test]
    fn a_full_page_of_seq_less_rows_reports_fullness_without_progress() {
        let events = vec![
            event(None, "work_a", vec![ready_fact("work_a", 1)]),
            event(None, "work_b", vec![ready_fact("work_b", 1)]),
        ];
        let page = project_workgraph_fact_page(&events, Some(12), 2);
        assert!(page.more_may_be_pending, "a filled page reports fullness");
        assert_eq!(
            page.next_after_seq,
            Some(12),
            "no sequenced row was observed, so the cursor cannot have moved",
        );
        assert_eq!(page.events_without_seq, 2);
        assert!(page.is_empty());

        // Positive control: the same full page WITH sequences does advance,
        // so the assertion above is about the seq-less rows and not about
        // `project_workgraph_fact_page` never advancing at all.
        let sequenced = vec![
            event(Some(13), "work_a", vec![ready_fact("work_a", 1)]),
            event(Some(14), "work_b", vec![ready_fact("work_b", 1)]),
        ];
        let progressed = project_workgraph_fact_page(&sequenced, Some(12), 2);
        assert!(progressed.more_may_be_pending);
        assert_eq!(progressed.next_after_seq, Some(14));
    }

    #[tokio::test]
    async fn publishing_without_subscribers_is_not_an_error() {
        let stream = WorkGraphFactStream::new();
        let page = project_workgraph_fact_page(
            &[event(Some(1), "work_a", vec![ready_fact("work_a", 1)])],
            None,
            64,
        );
        assert_eq!(stream.receiver_count(), 0);
        assert_eq!(stream.publish_page(&page), 0);
    }

    #[test]
    fn idle_hub_waits_for_notification_instead_of_scheduling_a_poll() {
        let hub = WorkGraphFactHub::new();
        assert_eq!(hub.receiver_count(), 0);
        assert!(
            hub.wait_for_subscriber().now_or_never().is_none(),
            "zero-subscriber state must remain pending until subscribe notifies it",
        );
    }

    #[tokio::test]
    async fn subscribers_receive_projected_facts() {
        let stream = WorkGraphFactStream::new();
        let mut rx = stream.subscribe();
        let page = project_workgraph_fact_page(
            &[event(Some(5), "work_a", vec![ready_fact("work_a", 2)])],
            None,
            64,
        );
        assert_eq!(stream.publish_page(&page), 1);
        let event = rx.try_recv().expect("published fact is delivered");
        let UnifiedEvent::Module(module) = event else {
            panic!("fact hub emitted a non-module event");
        };
        assert_eq!(module.module, WORKGRAPH_EVENT_MODULE);
        assert_eq!(module.event_type, WORKGRAPH_FACT_EVENT_TYPE);
        assert_eq!(module.payload["seq"], 5);
        assert_eq!(
            module.payload["fact"],
            serde_json::to_value(ready_fact("work_a", 2)).expect("fact JSON"),
        );
    }

    #[test]
    fn tail_defaults_stay_inside_the_upstream_collection_ceiling() {
        let options = WorkGraphFactTailOptions::default();
        assert!(options.page_limit <= MAX_FACT_POLL_LIMIT);
        // Both operands are consts, so this is a compile-time fact rather
        // than a runtime observation - `const {}` makes a future edit that
        // raises the default above the ceiling fail to BUILD rather than
        // fail this test.
        const { assert!(DEFAULT_FACT_POLL_LIMIT <= MAX_FACT_POLL_LIMIT) };
        assert!(!options.poll_interval.is_zero());
    }
}
