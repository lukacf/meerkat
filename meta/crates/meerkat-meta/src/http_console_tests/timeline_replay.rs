//! Rows for the timeline stream's replay from a cursor: it sends each page
//! as it reads it, ends after the frames it sent when a later page fails (its
//! client resumes from the last one), sends `replay_unavailable` when the log
//! no longer holds what follows, ends between pages at the gateway's
//! shutdown, and reads the roster for unknown members at most once per page.
#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::console_aggregator::{
    AppendOutcome, ConsoleFrameSource, ConsoleFrameStatus, ConsoleTimelinePage,
    ConsoleTimelineWindowPage, InMemoryConsoleLogStore, NewConsoleFrame,
};
use axum::body::Body;
use futures::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

/// The frames each replay row stores: more than one raw scan page.
const FRAMES: u64 = 1_500;
/// A replay's page: the aggregator's raw scan page.
const PAGE: u64 = 1_000;
/// How long a row waits for the stream before it fails.
const DEADLINE: Duration = Duration::from_secs(30);

/// What the replay's second page read does.
enum SecondPage {
    /// Waits for the row to release it.
    Held(Arc<tokio::sync::Notify>),
    /// Fails, as a broken store read does.
    Fails,
    /// Answers that the log no longer holds the frames after the cursor.
    Unavailable,
}

/// An in-memory log whose second `since` page read is held, fails, or is
/// answered unavailable.
struct ReplayStore {
    inner: InMemoryConsoleLogStore,
    second_page: std::sync::Mutex<Option<SecondPage>>,
    since_reads: AtomicUsize,
}

impl ReplayStore {
    async fn with_frames(
        second_page: Option<SecondPage>,
        identity: impl Fn(u64) -> String,
    ) -> Arc<Self> {
        let store = Arc::new(Self {
            inner: InMemoryConsoleLogStore::new(),
            second_page: std::sync::Mutex::new(second_page),
            since_reads: AtomicUsize::new(0),
        });
        for seq in 1..=FRAMES {
            let identity = identity(seq);
            store
                .inner
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("replay-frame-{seq}"),
                    timestamp_ms: seq,
                    runtime_key: "runtime-a".to_string(),
                    identity: identity.clone(),
                    conversation_id: Some(identity),
                    session_id: None,
                    kind: "text_delta".to_string(),
                    status: ConsoleFrameStatus::Completed,
                    payload: json!({ "delta": seq }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("replay-frame-{seq}")),
                    interaction_id: None,
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append a frame");
        }
        store
    }

    fn since_reads(&self) -> usize {
        self.since_reads.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for ReplayStore {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.inner.append_if_absent(frame).await
    }
    async fn update_frame_status(
        &self,
        id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(id, status).await
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
        if query.mode == ConsoleTimelineMode::Since
            && query.after.is_some()
            && self.since_reads.fetch_add(1, Ordering::SeqCst) == 1
        {
            let second_page = self
                .second_page
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            match second_page {
                Some(SecondPage::Held(release)) => release.notified().await,
                Some(SecondPage::Fails) => {
                    return Err(std::io::Error::other("the second page read fails").into());
                }
                Some(SecondPage::Unavailable) => {
                    return Err(Box::new(ConsoleTimelineQueryError::ReplayUnavailable {
                        requested_cursor: query.after,
                        latest_cursor: self.inner.latest_cursor().await?,
                    }));
                }
                None => {}
            }
        }
        self.inner.query_windowed_frames(query).await
    }
    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(key).await
    }
    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.latest_cursor().await
    }
    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }
    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime, kind, cursor)
            .await
    }
    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime, kind).await
    }
}

fn open_console() -> RuntimeDecisionState {
    RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    )
}

/// Open the timeline stream; `None` when its response does not arrive in
/// time.
async fn open_stream(app: &axum::Router, last_event_id: &str) -> Option<Sse> {
    let request = axum::http::Request::builder()
        .uri("/console/timeline/stream")
        .header("last-event-id", last_event_id)
        .body(Body::empty())
        .expect("request");
    let response = tokio::time::timeout(DEADLINE, app.clone().oneshot(request))
        .await
        .ok()?
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
    Some(Sse {
        body: response.into_body().into_data_stream(),
        buffered: String::new(),
    })
}

struct SseEvent {
    name: String,
    id: Option<String>,
    data: String,
}

enum Read {
    Event(SseEvent),
    Ended,
    Quiet,
}

/// The events of one stream's body, as a client reads them.
struct Sse {
    body: axum::body::BodyDataStream,
    buffered: String,
}

impl Sse {
    async fn next(&mut self, wait: Duration) -> Read {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(end) = self.buffered.find("\n\n") {
                let raw: String = self.buffered.drain(..end + 2).collect();
                let mut name = "message".to_string();
                let mut id = None;
                let mut data = Vec::new();
                for line in raw.lines() {
                    if let Some(value) = line.strip_prefix("event:") {
                        name = value.trim().to_string();
                    } else if let Some(value) = line.strip_prefix("id:") {
                        id = Some(value.trim().to_string());
                    } else if let Some(value) = line.strip_prefix("data:") {
                        data.push(value.strip_prefix(' ').unwrap_or(value).to_string());
                    }
                }
                // A keep-alive comment carries no data.
                if !data.is_empty() {
                    return Read::Event(SseEvent {
                        name,
                        id,
                        data: data.join("\n"),
                    });
                }
                continue;
            }
            match tokio::time::timeout_at(deadline, self.body.next()).await {
                Err(_) => return Read::Quiet,
                Ok(None) => return Read::Ended,
                Ok(Some(chunk)) => self
                    .buffered
                    .push_str(&String::from_utf8_lossy(&chunk.expect("an SSE chunk"))),
            }
        }
    }

    /// The cursors of the frames read until `done` matches an event, which
    /// is returned with them.
    async fn frames_until(&mut self, done: impl Fn(&SseEvent) -> bool) -> (Vec<u64>, SseEvent) {
        let mut cursors = Vec::new();
        loop {
            match self.next(DEADLINE).await {
                Read::Event(event) if done(&event) => return (cursors, event),
                Read::Event(event) => {
                    if event.name == "console_frame" {
                        cursors.push(cursor_seq(&event));
                    }
                }
                Read::Ended => panic!("the stream ended after {} frames", cursors.len()),
                Read::Quiet => panic!("the stream went quiet after {} frames", cursors.len()),
            }
        }
    }

    /// The cursors of the frames read, and every other event, until the
    /// stream ends.
    async fn until_end(&mut self) -> (Vec<u64>, Vec<SseEvent>) {
        let mut cursors = Vec::new();
        let mut others = Vec::new();
        loop {
            match self.next(DEADLINE).await {
                Read::Event(event) if event.name == "console_frame" => {
                    cursors.push(cursor_seq(&event));
                }
                Read::Event(event) => others.push(event),
                Read::Ended => return (cursors, others),
                Read::Quiet => panic!("the stream did not end after {} frames", cursors.len()),
            }
        }
    }
}

fn cursor_seq(event: &SseEvent) -> u64 {
    event
        .id
        .as_deref()
        .and_then(|id| ConsoleCursor::from(id.to_string()).seq())
        .expect("a frame event names its cursor")
}

fn is_named(name: &'static str) -> impl Fn(&SseEvent) -> bool {
    move |event| event.name == name
}

/// A replay sends its first page before it reads its second, and then the
/// rest. RED on the base: the stream read every page before it answered, so
/// with the second page held no response arrived at all.
#[tokio::test]
async fn a_replay_sends_its_first_page_before_reading_the_next() {
    let release = Arc::new(tokio::sync::Notify::new());
    let store = ReplayStore::with_frames(Some(SecondPage::Held(release.clone())), |_| {
        "agent-a".to_string()
    })
    .await;
    let app = console_json_router_with_aggregator(
        open_console(),
        MetaConsoleAggregator::new(store.clone()),
    );

    let mut sse = open_stream(&app, "console:0")
        .await
        .expect("the stream answers while its second page is held");
    let (_, started) = sse.frames_until(|_| true).await;
    assert_eq!(started.name, "snapshot_started");
    let mut first = Vec::new();
    while first.len() < usize::try_from(PAGE).expect("page") {
        match sse.next(DEADLINE).await {
            Read::Event(event) if event.name == "console_frame" => first.push(cursor_seq(&event)),
            Read::Event(event) => panic!("{} before the first page ended", event.name),
            Read::Ended | Read::Quiet => panic!("the first page stopped at {}", first.len()),
        }
    }
    assert_eq!(first, (1..=PAGE).collect::<Vec<_>>());
    // The stream now waits on its second page and sends nothing else.
    assert!(matches!(
        sse.next(Duration::from_millis(300)).await,
        Read::Quiet
    ));
    assert_eq!(store.since_reads(), 2, "the second page read is held");

    release.notify_one();
    let (rest, complete) = sse.frames_until(is_named("snapshot_complete")).await;
    assert_eq!(rest, (PAGE + 1..=FRAMES).collect::<Vec<_>>());
    assert_eq!(
        complete.id.as_deref(),
        Some(format!("console:{FRAMES}").as_str())
    );
}

/// A later page that fails ends the stream after the frames it sent, with
/// no `snapshot_complete`; the client resumes from the last frame it got
/// and receives the rest. On the base the same failure answered HTTP 500
/// before any frame, so this row is RED there: no stream opened.
#[tokio::test]
async fn a_failed_later_page_ends_the_stream_and_the_client_resumes_from_its_cursor() {
    let store = ReplayStore::with_frames(Some(SecondPage::Fails), |_| "agent-a".to_string()).await;
    let app =
        console_json_router_with_aggregator(open_console(), MetaConsoleAggregator::new(store));

    let mut sse = open_stream(&app, "console:0")
        .await
        .expect("the stream opens on its first page");
    let (sent, others) = sse.until_end().await;
    assert_eq!(sent, (1..=PAGE).collect::<Vec<_>>());
    let names: Vec<_> = others.iter().map(|event| event.name.as_str()).collect();
    assert_eq!(names, ["snapshot_started"], "no snapshot_complete");

    let last = format!("console:{}", sent.last().copied().expect("a frame"));
    let mut resumed = open_stream(&app, &last)
        .await
        .expect("the resumed stream opens");
    let (rest, complete) = resumed.frames_until(is_named("snapshot_complete")).await;
    assert_eq!(rest, (PAGE + 1..=FRAMES).collect::<Vec<_>>());
    assert_eq!(
        complete.id.as_deref(),
        Some(format!("console:{FRAMES}").as_str())
    );
}

/// When the log no longer holds what follows a sent page, the stream sends
/// the existing `replay_unavailable` event, naming where it stopped, and
/// ends. On the base the same answer was an HTTP 409 before any frame.
#[tokio::test]
async fn a_replay_the_log_can_no_longer_continue_sends_replay_unavailable() {
    let store =
        ReplayStore::with_frames(Some(SecondPage::Unavailable), |_| "agent-a".to_string()).await;
    let app =
        console_json_router_with_aggregator(open_console(), MetaConsoleAggregator::new(store));

    let mut sse = open_stream(&app, "console:0")
        .await
        .expect("the stream opens on its first page");
    let (sent, others) = sse.until_end().await;
    assert_eq!(sent, (1..=PAGE).collect::<Vec<_>>());
    let names: Vec<_> = others.iter().map(|event| event.name.as_str()).collect();
    assert_eq!(names, ["snapshot_started", "replay_unavailable"]);
    let unavailable: Value =
        serde_json::from_str(&others[1].data).expect("replay_unavailable carries JSON");
    assert_eq!(
        unavailable["requested_cursor"],
        json!(format!("console:{PAGE}"))
    );
    assert_eq!(
        unavailable["latest_cursor"],
        json!(format!("console:{FRAMES}"))
    );
}

/// Under an enforced access view, a replay reads the roster again for
/// members its view does not know at most once per page: once at open, once
/// for the three unknown members of the first page, once for the two of the
/// second. RED on the base: one read per unknown member, six in all (and
/// with the per-page cap removed from `ReplayPriming::primes_for`).
#[tokio::test]
async fn enforced_access_reads_the_roster_at_most_once_per_replay_page() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let mob_id = format!(
        "timeline-replay-primes-{}",
        NEXT.fetch_add(1, Ordering::SeqCst)
    );
    let (_temp, runtime) = super::tests::build_empty_console_test_runtime(&mob_id)
        .await
        .expect("runtime");
    let store = ReplayStore::with_frames(None, |seq| {
        if seq <= PAGE {
            format!("gone-{}", seq % 3 + 1)
        } else {
            format!("gone-{}", seq % 2 + 4)
        }
    })
    .await;
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.com".to_string()],
        rules: vec![crate::access::AccessRule {
            id: "everyone-views".to_string(),
            actions: vec!["agent.view".to_string()],
            agents: vec!["*".to_string()],
            ..crate::access::AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("controller");
    let app = console_json_router_with_state(ConsoleJsonState {
        decisions: open_console(),
        session_fences: crate::auth::session_fence::ConsoleSessionFences::detached(),
        runtime: Some(runtime.clone()),
        module_runtime: None,
        contact_directory: None,
        event_log: None,
        gateway_peer_keys: None,
        identity_runtime: None,
        console_events: None,
        console_aggregator: Some(MetaConsoleAggregator::new(store)),
        mob_events: None,
        metadata_table: None,
        visibility_policy: Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
        snapshot_read_model: ConsoleSnapshotReadModel::default(),
        access: Some(controller),
        memory_panel: None,
        operator_resolver: None,
        identity_roster: None,
        workgraph: None,
        topology: None,
        job_health_projection: None,
        governed_ingress: Default::default(),
        mob_announcements: Default::default(),
    });

    let mut sse = open_stream(&app, "console:0")
        .await
        .expect("the stream opens");
    let (sent, _) = sse.frames_until(is_named("snapshot_complete")).await;
    assert_eq!(sent, (1..=FRAMES).collect::<Vec<_>>());
    assert_eq!(
        super::stream_primes::count(&mob_id),
        3,
        "one read at open, then one per page naming members the view does not know"
    );
    runtime.handle().stop().await.expect("stop");
}

/// The priming rule itself: a page's first unknown member reads the roster
/// and the page's other unknown members do not; a member the roster was read
/// for is never read for again, while an unknown member another member's
/// read covered is read for on a later page that has not read the roster
/// yet. RED for a rule that records every member a page's read covered
/// (`gone-2` on the second page) and for one with no per-page cap.
#[test]
fn replay_priming_reads_once_per_page_and_once_per_member() {
    let mut priming = ReplayPriming::default();
    priming.start_page();
    let first: Vec<_> = ["gone-1", "gone-2", "gone-1", "gone-3"]
        .into_iter()
        .map(|identity| priming.primes_for(identity))
        .collect();
    assert_eq!(first, [true, false, false, false]);
    priming.start_page();
    let second: Vec<_> = ["gone-1", "gone-2", "gone-4"]
        .into_iter()
        .map(|identity| priming.primes_for(identity))
        .collect();
    assert_eq!(second, [false, true, false]);
    priming.start_page();
    let third: Vec<_> = ["gone-1", "gone-2", "gone-4"]
        .into_iter()
        .map(|identity| priming.primes_for(identity))
        .collect();
    assert_eq!(third, [false, false, true]);
}

/// A gateway shutdown ends a replay between pages: the stream that waits on
/// its second page ends once the gateway's drain begins. RED with the page
/// read outside `until_stream_shutdown`: the stream stays open on the held
/// page.
#[tokio::test]
async fn a_gateway_shutdown_ends_a_replay_between_pages() {
    let release = Arc::new(tokio::sync::Notify::new());
    let store =
        ReplayStore::with_frames(Some(SecondPage::Held(release)), |_| "agent-a".to_string()).await;
    let (stopping, stop_watch) = tokio::sync::watch::channel(false);
    let app = console_json_router_with_aggregator(
        open_console(),
        MetaConsoleAggregator::new(store.clone()),
    )
    .layer(axum::Extension(crate::http_sse::StreamShutdown::new(
        stop_watch,
    )));

    let mut sse = open_stream(&app, "console:0")
        .await
        .expect("the stream answers on its first page");
    let mut first = Vec::new();
    while first.len() < usize::try_from(PAGE).expect("page") {
        match sse.next(DEADLINE).await {
            Read::Event(event) if event.name == "console_frame" => first.push(cursor_seq(&event)),
            Read::Event(_) => {}
            Read::Ended | Read::Quiet => panic!("the first page stopped at {}", first.len()),
        }
    }
    assert!(matches!(
        sse.next(Duration::from_millis(300)).await,
        Read::Quiet
    ));
    assert_eq!(
        store.since_reads(),
        2,
        "the stream waits on its second page"
    );

    stopping.send(true).expect("the stream watches the drain");
    let (rest, others) = sse.until_end().await;
    assert!(rest.is_empty(), "no frame after the drain began: {rest:?}");
    assert!(
        others.iter().all(|event| event.name != "snapshot_complete"),
        "the replay ended before it completed"
    );
}
