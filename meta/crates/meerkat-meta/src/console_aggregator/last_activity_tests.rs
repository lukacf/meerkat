//! The aggregator's in-memory last activity ([`super::last_activity`]):
//! each identity is read from the console log once, with no frame bound, and
//! then follows the acting frames the aggregator appends, in both console
//! stores. The store wrapper counts the log reads (walks) and can hold a
//! walk after its read, or an append after its write, to fix an
//! interleaving.
#![allow(clippy::expect_used)]

use std::sync::atomic::AtomicUsize;

use tokio::sync::oneshot;

use super::last_activity::{CAPACITY, LastActivityCache, SEED_CHUNK};
use super::*;

const RUNTIME: &str = "runtime-a";
const SESSION: &str = "0193f5d1-0b5a-7c3e-9a1f-1c2d3e4f5a6b";

fn frame(
    key: &str,
    identity: &str,
    timestamp_ms: u64,
    source: ConsoleFrameSourceKind,
    kind: &str,
) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms,
        runtime_key: RUNTIME.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: None,
        kind: kind.to_string(),
        status: ConsoleFrameStatus::Delivered,
        payload: json!({}),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: source,
            source_cursor: None,
        },
        source_event_id: Some(key.to_string()),
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// A live runtime event of the identity acting.
fn acting(key: &str, identity: &str, timestamp_ms: u64) -> NewConsoleFrame {
    frame(
        key,
        identity,
        timestamp_ms,
        ConsoleFrameSourceKind::ConsoleEvent,
        "text_complete",
    )
}

fn send_request(identity: &str, key: &str) -> ConsoleSendRequest {
    ConsoleSendRequest {
        identity: identity.to_string(),
        content: json!("hello"),
        origin: "console".to_string(),
        idempotency_key: key.to_string(),
        handling_mode: Some("queue".to_string()),
        origin_kind: None,
        skill_refs: Vec::new(),
    }
}

/// Both console stores, each fresh.
fn stores() -> Vec<(&'static str, Arc<dyn ConsoleLogStore>)> {
    let in_memory: Arc<dyn ConsoleLogStore> = Arc::new(InMemoryConsoleLogStore::new());
    let sqlite: Arc<dyn ConsoleLogStore> =
        Arc::new(SqliteConsoleLogStore::in_memory().expect("sqlite store"));
    vec![("in-memory", in_memory), ("sqlite", sqlite)]
}

/// One held step: the store signals `reached` and waits for `resume`.
struct Hold {
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

/// The test's side of a [`Hold`].
struct HoldHandle {
    reached: oneshot::Receiver<()>,
    resume: oneshot::Sender<()>,
}

fn hold() -> (Hold, HoldHandle) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    (
        Hold {
            reached: reached_tx,
            resume: resume_rx,
        },
        HoldHandle {
            reached: reached_rx,
            resume: resume_tx,
        },
    )
}

/// A console store that counts its activity walks and can hold a walk (by
/// its number) after it has read the log, or the next append after it has
/// written the frame.
struct ObservedStore {
    inner: Arc<dyn ConsoleLogStore>,
    walks: AtomicUsize,
    walk_holds: std::sync::Mutex<Vec<(usize, Hold)>>,
    append_hold: std::sync::Mutex<Option<Hold>>,
}

impl ObservedStore {
    fn new(inner: Arc<dyn ConsoleLogStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            walks: AtomicUsize::new(0),
            walk_holds: std::sync::Mutex::new(Vec::new()),
            append_hold: std::sync::Mutex::new(None),
        })
    }

    fn walks(&self) -> usize {
        self.walks.load(Ordering::SeqCst)
    }

    fn hold_next_walk(&self) -> HoldHandle {
        self.hold_walk(self.walks() + 1)
    }

    /// Hold the walk with this number, counting every walk from the first.
    fn hold_walk(&self, number: usize) -> HoldHandle {
        let (held, handle) = hold();
        self.walk_holds
            .lock()
            .expect("walk holds")
            .push((number, held));
        handle
    }

    fn take_walk_hold(&self, number: usize) -> Option<Hold> {
        let mut holds = self.walk_holds.lock().expect("walk holds");
        let index = holds.iter().position(|(held, _)| *held == number)?;
        Some(holds.remove(index).1)
    }

    fn hold_next_append(&self) -> HoldHandle {
        let (held, handle) = hold();
        *self.append_hold.lock().expect("append hold") = Some(held);
        handle
    }

    async fn pass(held: Option<Hold>) {
        if let Some(held) = held {
            let _ = held.reached.send(());
            let _ = held.resume.await;
        }
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for ObservedStore {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        let outcome = self.inner.append_if_absent(frame).await?;
        let held = self.append_hold.lock().expect("append hold").take();
        Self::pass(held).await;
        Ok(outcome)
    }

    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(frame_id, status).await
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        self.inner.query_frames(query).await
    }

    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.inner.query_windowed_frames(query).await
    }

    async fn frame_by_dedupe_key(
        &self,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(dedupe_key).await
    }

    async fn unsettled_send_frames(
        &self,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        self.inner.unsettled_send_frames(window, limit).await
    }

    async fn latest_activity_times(
        &self,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        let number = self.walks.fetch_add(1, Ordering::SeqCst) + 1;
        let times = self.inner.latest_activity_times(identities).await?;
        Self::pass(self.take_walk_hold(number)).await;
        Ok(times)
    }

    fn send_dedupe_durability(&self) -> ConsoleSendDedupeDurability {
        self.inner.send_dedupe_durability()
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.latest_cursor().await
    }

    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        self.inner.history_prefix_revision().await
    }

    async fn compact_message_deltas(
        &self,
        compaction: &ConsoleDeltaCompaction,
    ) -> ConsoleLogResult<usize> {
        self.inner.compact_message_deltas(compaction).await
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }

    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime_key, source_kind, source_cursor)
            .await
    }

    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime_key, source_kind).await
    }
}

/// A frame the log held before this aggregator started, as from an earlier
/// process: written to the store directly.
async fn stored_before(store: &ObservedStore, frame: NewConsoleFrame) {
    store
        .inner
        .append_if_absent(frame)
        .await
        .expect("stored frame");
}

/// A frame the aggregator appends itself, through the projection's append.
async fn append(aggregator: &MetaConsoleAggregator, frame: NewConsoleFrame) -> AppendOutcome {
    append_and_emit(&aggregator.inner, frame)
        .await
        .expect("aggregator append")
}

async fn read(aggregator: &MetaConsoleAggregator, identities: &[&str]) -> HashMap<String, u64> {
    let identities = identities
        .iter()
        .map(|identity| (*identity).to_string())
        .collect::<Vec<_>>();
    aggregator
        .last_activity_for_identities(&identities)
        .await
        .expect("last activity")
}

/// A read through a cache built for the test, as the aggregator reads.
async fn read_cache(
    cache: &LastActivityCache,
    store: &ObservedStore,
    identities: &[&str],
) -> HashMap<String, u64> {
    let identities = identities
        .iter()
        .map(|identity| (*identity).to_string())
        .collect::<Vec<_>>();
    cache.read(store, &identities).await.expect("last activity")
}

/// An append through a cache built for the test, as the aggregator appends.
async fn append_cache(
    cache: &LastActivityCache,
    store: &ObservedStore,
    frame: NewConsoleFrame,
) -> AppendOutcome {
    cache.append(store, frame).await.expect("cache append")
}

fn times(entries: &[(&str, u64)]) -> HashMap<String, u64> {
    entries
        .iter()
        .map(|(identity, time)| ((*identity).to_string(), *time))
        .collect()
}

/// An identity whose one acting frame sits behind 600 newer frames of it not
/// acting (as after a history backfill) reads that frame's time: the first
/// read walks the log with no frame bound.
#[tokio::test]
async fn an_acting_frame_behind_hundreds_of_newer_frames_reads_its_time() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("archivist-acted", "archivist", 100)).await;
        for index in 0..600_u64 {
            stored_before(
                &observed,
                frame(
                    &format!("archivist-history-{index}"),
                    "archivist",
                    200 + index,
                    ConsoleFrameSourceKind::SessionHistory,
                    "assistant_message",
                ),
            )
            .await;
        }
        let aggregator = MetaConsoleAggregator::new(observed);
        assert_eq!(
            read(&aggregator, &["archivist"]).await,
            times(&[("archivist", 100)]),
            "{name}"
        );
    }
}

/// After its first read, an identity follows the acting frames the
/// aggregator appends (a projected event, a console send) with no further
/// store read, and a policy view reads the same times.
#[tokio::test]
async fn a_read_identity_follows_acting_appends_and_sends_without_a_store_read() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("worker-earlier", "worker", 100)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 100)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 1, "{name}: the first read walks the log");

        append(&aggregator, acting("worker-later", "worker", 300)).await;
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 300)]),
            "{name}"
        );

        let before = current_time_ms();
        let reservation = aggregator
            .reserve_identity_first_interaction(send_request("worker", "k1"), Some(SESSION))
            .await
            .expect("reservation");
        assert!(
            matches!(reservation, IdentityFirstReservation::Fresh(_)),
            "{name}: {reservation:?}"
        );
        let after = current_time_ms();
        let sent = read(&aggregator, &["worker"]).await["worker"];
        assert!(
            (before..=after).contains(&sent),
            "{name}: a console send is the identity acting ({before} <= {sent} <= {after})"
        );

        let view = aggregator.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
        assert_eq!(
            view.last_activity_for_identities(&["worker".to_string()])
                .await
                .expect("view last activity"),
            times(&[("worker", sent)]),
            "{name}: a policy view shares the times"
        );
        assert_eq!(observed.walks(), 1, "{name}: no read after the first walks");
    }
}

/// Appends of frames that do not record the identity acting change neither
/// its time nor the store reads, and do not stand in for an unread
/// identity's walk.
#[tokio::test]
async fn appends_that_do_not_act_change_neither_the_read_nor_the_store_reads() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("worker-acted", "worker", 100)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 100)]),
            "{name}"
        );

        for (key, time, source, kind) in [
            (
                "history",
                500,
                ConsoleFrameSourceKind::SessionHistory,
                "assistant_message",
            ),
            (
                "synthetic",
                510,
                ConsoleFrameSourceKind::Synthetic,
                "frame_updated",
            ),
            (
                "reset",
                520,
                ConsoleFrameSourceKind::ConsoleEvent,
                "identity_reset",
            ),
            (
                "respawned",
                525,
                ConsoleFrameSourceKind::ConsoleEvent,
                "identity_respawned",
            ),
            (
                "memory",
                530,
                ConsoleFrameSourceKind::ConsoleEvent,
                "memory.record.promoted",
            ),
            (
                "failed",
                540,
                ConsoleFrameSourceKind::ConsoleEvent,
                "interaction_failed",
            ),
        ] {
            append(&aggregator, frame(key, "worker", time, source, kind)).await;
        }
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 100)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 1, "{name}");

        // An identity seen only not acting is still walked on its first
        // read, and has no time.
        append(
            &aggregator,
            frame(
                "ghost-history",
                "ghost",
                600,
                ConsoleFrameSourceKind::SessionHistory,
                "assistant_message",
            ),
        )
        .await;
        assert_eq!(
            read(&aggregator, &["ghost"]).await,
            HashMap::new(),
            "{name}"
        );
        assert_eq!(observed.walks(), 2, "{name}");
    }
}

/// An identity that acted in this process before its first read is answered
/// from memory: the frame the aggregator stored is its newest acting frame.
#[tokio::test]
async fn an_identity_that_acted_here_is_read_without_a_walk() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("fresh-earlier", "fresh", 100)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        append(&aggregator, acting("fresh-now", "fresh", 400)).await;
        assert_eq!(
            read(&aggregator, &["fresh"]).await,
            times(&[("fresh", 400)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 0, "{name}");
    }
}

/// A replayed append (a dedupe key the log already holds) is not merged: an
/// unread identity is still walked, so its newer acting frame wins.
#[tokio::test]
async fn a_replayed_acting_frame_does_not_stand_in_for_the_walk() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("router-first", "router", 100)).await;
        stored_before(&observed, acting("router-second", "router", 200)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        let replay = append(&aggregator, acting("router-first", "router", 100)).await;
        assert_eq!(replay.disposition, AppendDisposition::Existing, "{name}");
        assert_eq!(
            read(&aggregator, &["router"]).await,
            times(&[("router", 200)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 1, "{name}");
    }
}

/// An acting append that lands while the first read's walk is between its
/// log read and its merge is kept: the walk found the older time, and the
/// merge keeps the newer one whichever lands first. The store holds the walk
/// after its read, so the interleaving is fixed.
#[tokio::test]
async fn an_acting_append_racing_the_seed_is_kept() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("worker-earlier", "worker", 100)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        let shared = &aggregator;
        let HoldHandle { reached, resume } = observed.hold_next_walk();
        let (first, ()) = tokio::join!(read(shared, &["worker"]), async move {
            reached.await.expect("the walk read the log");
            append(shared, acting("worker-racing", "worker", 300)).await;
            resume.send(()).expect("resume the walk");
        });
        assert_eq!(first, times(&[("worker", 300)]), "{name}");
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 300)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 1, "{name}");
    }
}

/// Concurrent first reads walk an identity once: the second waits for the
/// first's walk and reads its result.
#[tokio::test]
async fn concurrent_first_reads_walk_an_identity_once() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("worker-acted", "worker", 100)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        let shared = &aggregator;
        let HoldHandle { reached, resume } = observed.hold_next_walk();
        let (first, (second, ())) = tokio::join!(read(shared, &["worker"]), async move {
            reached.await.expect("the first walk read the log");
            tokio::join!(read(shared, &["worker"]), async move {
                tokio::task::yield_now().await;
                resume.send(()).expect("resume the first walk");
            })
        });
        assert_eq!(first, times(&[("worker", 100)]), "{name}");
        assert_eq!(second, times(&[("worker", 100)]), "{name}");
        assert_eq!(observed.walks(), 1, "{name}");
    }
}

/// A clear waits for an append in flight, then drops every time with the
/// frames: the next read walks the cleared log, and a later acting append
/// counts again. The store holds the append after its write, so the clear
/// would otherwise run between the write and the merge.
#[tokio::test]
async fn a_clear_waits_for_appends_in_flight_and_drops_every_time() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        append(&aggregator, acting("worker-first", "worker", 100)).await;
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 100)]),
            "{name}"
        );

        let shared = &aggregator;
        let HoldHandle { reached, resume } = observed.hold_next_append();
        tokio::join!(
            async move {
                append(shared, acting("worker-second", "worker", 300)).await;
            },
            async move {
                reached.await.expect("the append wrote its frame");
                let mut clear = std::pin::pin!(shared.clear_timeline_frames());
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), clear.as_mut())
                        .await
                        .is_err(),
                    "{name}: the clear waits for the append in flight"
                );
                resume.send(()).expect("resume the append");
                clear.await.expect("clear");
            }
        );

        assert_eq!(
            read(&aggregator, &["worker"]).await,
            HashMap::new(),
            "{name}"
        );
        assert_eq!(
            observed.walks(),
            1,
            "{name}: the read after the clear walks"
        );
        append(&aggregator, acting("worker-third", "worker", 500)).await;
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 500)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 1, "{name}");
    }
}

/// A departure (the identity retired or deleted) drops its entry: the next
/// read walks the log again, finds the identity's newest acting frame, and is
/// kept in memory again.
#[tokio::test]
async fn a_departure_drops_the_entry_and_a_later_read_walks_the_log_again() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        for (identity, departure) in [
            ("worker", "identity_retired"),
            ("router", "identity_deleted"),
        ] {
            append(
                &aggregator,
                acting(&format!("{identity}-acted"), identity, 100),
            )
            .await;
            assert_eq!(
                read(&aggregator, &[identity]).await,
                times(&[(identity, 100)]),
                "{name}"
            );
            let walks = observed.walks();
            append(
                &aggregator,
                frame(
                    &format!("{identity}-left"),
                    identity,
                    200,
                    ConsoleFrameSourceKind::ConsoleEvent,
                    departure,
                ),
            )
            .await;
            assert_eq!(
                read(&aggregator, &[identity]).await,
                times(&[(identity, 100)]),
                "{name}: {departure}"
            );
            assert_eq!(
                observed.walks(),
                walks + 1,
                "{name}: {departure} drops the entry, so the next read walks the log"
            );
            assert_eq!(
                read(&aggregator, &[identity]).await,
                times(&[(identity, 100)]),
                "{name}: {departure}"
            );
            assert_eq!(
                observed.walks(),
                walks + 1,
                "{name}: the walk's result is kept again"
            );
        }
    }
}

/// A departure drops its entry only once a seed in flight has merged. The
/// seed walked the older time, an acting append merged a newer one, and the
/// drop waits for the seed, so the seed cannot leave its older time behind
/// after the drop. The store holds the walk after its read.
#[tokio::test]
async fn a_departure_waits_for_a_seed_in_flight_before_dropping() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("worker-earlier", "worker", 100)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        let shared = &aggregator;
        let HoldHandle { reached, resume } = observed.hold_next_walk();
        let (first, ((), ())) = tokio::join!(read(shared, &["worker"]), async move {
            reached.await.expect("the walk read the log");
            append(shared, acting("worker-racing", "worker", 300)).await;
            tokio::join!(
                async move {
                    append(
                        shared,
                        frame(
                            "worker-left",
                            "worker",
                            400,
                            ConsoleFrameSourceKind::ConsoleEvent,
                            "identity_retired",
                        ),
                    )
                    .await;
                },
                async move {
                    tokio::task::yield_now().await;
                    resume.send(()).expect("resume the walk");
                },
            )
        });
        assert_eq!(first, times(&[("worker", 300)]), "{name}");
        // The drop came after the seed's merge, so the next read walks the
        // log and finds the racing append, not the seed's older time.
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 300)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 2, "{name}");
    }
}

/// A first read seeds a chunk of identities per store call, each under its
/// own shared hold, so a departure waits for one chunk, not the whole seed.
/// With a chunk of one identity, a departure for the first identity, made
/// while the first chunk's walk is held, has finished by the time the second
/// chunk walks.
#[tokio::test]
async fn a_first_read_seeds_in_chunks_so_a_departure_waits_for_one_chunk_only() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("first-acted", "first", 100)).await;
        stored_before(&observed, acting("second-acted", "second", 200)).await;
        let cache = LastActivityCache::with_limits(1, CAPACITY);
        let (cache, store) = (&cache, observed.as_ref());
        let HoldHandle {
            reached: first_walked,
            resume: resume_first,
        } = observed.hold_walk(1);
        let HoldHandle {
            reached: second_walked,
            resume: resume_second,
        } = observed.hold_walk(2);
        let departed = std::cell::Cell::new(false);
        let departed = &departed;
        let (seeded, ((), ())) =
            tokio::join!(read_cache(cache, store, &["first", "second"]), async move {
                first_walked.await.expect("the first chunk walked");
                tokio::join!(
                    async move {
                        append_cache(
                            cache,
                            store,
                            frame(
                                "first-left",
                                "first",
                                300,
                                ConsoleFrameSourceKind::ConsoleEvent,
                                "identity_retired",
                            ),
                        )
                        .await;
                        departed.set(true);
                    },
                    async move {
                        tokio::task::yield_now().await;
                        resume_first.send(()).expect("resume the first chunk");
                        tokio::time::timeout(Duration::from_secs(5), second_walked)
                            .await
                            .expect("the seed walks a second chunk")
                            .expect("the second chunk walked");
                        assert!(
                            departed.get(),
                            "{name}: the departure finished between the chunks"
                        );
                        resume_second.send(()).expect("resume the second chunk");
                    },
                )
            });
        // The departure dropped the first identity's entry between the chunks.
        assert_eq!(seeded, times(&[("second", 200)]), "{name}");
        assert_eq!(observed.walks(), 2, "{name}");
        assert_eq!(
            read_cache(cache, store, &["first", "second"]).await,
            times(&[("first", 100), ("second", 200)]),
            "{name}"
        );
        assert_eq!(observed.walks(), 3, "{name}");
    }
}

/// The map keeps at most its capacity of entries. Past it, the least
/// recently used are evicted, and an evicted identity's next read walks the
/// log again; the identities a read returns are used, so they stay.
#[tokio::test]
async fn past_its_capacity_the_least_recently_used_entries_are_evicted() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        let cache = LastActivityCache::with_limits(SEED_CHUNK, 4);
        let store = observed.as_ref();
        for (identity, time) in [("a", 100), ("b", 200), ("c", 300), ("d", 400)] {
            append_cache(
                &cache,
                store,
                acting(&format!("{identity}-acted"), identity, time),
            )
            .await;
        }
        // A read uses "a", so "b" and "c" are now the least recently used.
        assert_eq!(
            read_cache(&cache, store, &["a"]).await,
            times(&[("a", 100)]),
            "{name}"
        );
        // A fifth entry: evicted down to three quarters of four, the two
        // least recently used, "b" and "c", go.
        append_cache(&cache, store, acting("e-acted", "e", 500)).await;
        assert_eq!(
            read_cache(&cache, store, &["a", "d", "e"]).await,
            times(&[("a", 100), ("d", 400), ("e", 500)]),
            "{name}"
        );
        assert_eq!(
            observed.walks(),
            0,
            "{name}: kept entries answer from memory"
        );
        assert_eq!(
            read_cache(&cache, store, &["b"]).await,
            times(&[("b", 200)]),
            "{name}"
        );
        assert_eq!(
            observed.walks(),
            1,
            "{name}: an evicted identity walks the log again"
        );
    }
}

/// A read counts its identities before it walks, so an eviction an append
/// starts between its chunks keeps room for every identity it lists.
#[tokio::test]
async fn a_read_larger_than_the_capacity_keeps_its_own_earlier_chunks() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        let identities = ["a", "b", "c", "d", "e", "f"];
        for (identity, time) in identities.iter().zip(100_u64..) {
            stored_before(
                &observed,
                acting(&format!("{identity}-acted"), identity, time),
            )
            .await;
        }
        let cache = LastActivityCache::with_limits(1, 4);
        let (cache, store) = (&cache, observed.as_ref());
        let HoldHandle { reached, resume } = observed.hold_walk(5);
        let (seeded, (_, ())) = tokio::join!(read_cache(cache, store, &identities), async move {
            reached.await.expect("the fifth chunk walked");
            tokio::join!(
                append_cache(cache, store, acting("z-acted", "z", 900)),
                async move {
                    tokio::task::yield_now().await;
                    resume.send(()).expect("resume the fifth chunk");
                }
            )
        });
        assert_eq!(seeded.len(), identities.len(), "{name}: {seeded:?}");
    }
}

/// A replayed departure (a dedupe key the log already holds) keeps the
/// entry: the departure dropped it when it was first stored.
#[tokio::test]
async fn a_replayed_departure_keeps_the_entry() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        append(&aggregator, acting("worker-acted", "worker", 100)).await;
        let departure = || {
            frame(
                "worker-left",
                "worker",
                200,
                ConsoleFrameSourceKind::ConsoleEvent,
                "identity_retired",
            )
        };
        append(&aggregator, departure()).await;
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 100)]),
            "{name}"
        );
        assert_eq!(
            observed.walks(),
            1,
            "{name}: the departure dropped the entry"
        );
        let replay = append(&aggregator, departure()).await;
        assert_eq!(replay.disposition, AppendDisposition::Existing, "{name}");
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 100)]),
            "{name}"
        );
        assert_eq!(
            observed.walks(),
            1,
            "{name}: a replayed departure keeps the entry"
        );
    }
}

/// A clear waits for a first read's walk in flight, then clears what it
/// merged with the log: the next read walks the cleared log.
#[tokio::test]
async fn a_clear_waits_for_a_first_read_in_flight() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        stored_before(&observed, acting("worker-acted", "worker", 100)).await;
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        let shared = &aggregator;
        let HoldHandle { reached, resume } = observed.hold_next_walk();
        let (_seeded, ()) = tokio::join!(read(shared, &["worker"]), async move {
            reached.await.expect("the first read's walk read the log");
            let mut clear = std::pin::pin!(shared.clear_timeline_frames());
            assert!(
                tokio::time::timeout(Duration::from_millis(100), clear.as_mut())
                    .await
                    .is_err(),
                "{name}: the clear waits for the walk in flight"
            );
            resume.send(()).expect("resume the walk");
            clear.await.expect("clear");
        });
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            HashMap::new(),
            "{name}: the next read walks the cleared log"
        );
        assert_eq!(observed.walks(), 2, "{name}");
    }
}

/// An acting append that lands between a departure's store write and its
/// drop is not lost: the drop removes the entry, and the next read walks the
/// log and finds the append. The store holds the departure after its write.
#[tokio::test]
async fn an_append_between_a_departures_write_and_its_drop_is_found_by_the_next_read() {
    for (name, store) in stores() {
        let observed = ObservedStore::new(store);
        let aggregator = MetaConsoleAggregator::new(observed.clone());
        append(&aggregator, acting("worker-acted", "worker", 100)).await;
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 100)]),
            "{name}"
        );
        let shared = &aggregator;
        let HoldHandle { reached, resume } = observed.hold_next_append();
        tokio::join!(
            async move {
                append(
                    shared,
                    frame(
                        "worker-left",
                        "worker",
                        200,
                        ConsoleFrameSourceKind::ConsoleEvent,
                        "identity_retired",
                    ),
                )
                .await;
            },
            async move {
                reached.await.expect("the departure wrote its frame");
                append(shared, acting("worker-after", "worker", 300)).await;
                resume.send(()).expect("resume the departure");
            }
        );
        assert_eq!(
            read(&aggregator, &["worker"]).await,
            times(&[("worker", 300)]),
            "{name}"
        );
        assert_eq!(
            observed.walks(),
            1,
            "{name}: the drop made the next read walk the log"
        );
    }
}
