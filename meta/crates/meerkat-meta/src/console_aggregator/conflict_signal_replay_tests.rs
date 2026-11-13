//! The memory steward's conflict signal after the console catches up from a
//! lag. A console that fell behind its runtime's events reads the retained
//! ones again and stores what it had missed; the conflict signal it stores
//! that way is decided like every other one: it is shown to an administrator
//! only, the other records on the system page to every caller who may view
//! that page, and reading the events again stores nothing twice.
#![allow(clippy::expect_used)]

use super::*;
use crate::access::{AccessControlConfig, AccessController, AccessRule};
use crate::console_contracts::SYSTEM_EVENT_IDENTITY;
use crate::memory::events::MemoryTimelineEvent;

const RUNTIME: &str = "runtime-a";
const ROOT: &str = "root@example.test";
const BOB: &str = "bob@example.test";

/// A console over `store` that has `runtime` registered and reads no live
/// event: what it stores, it stores from the events it reads again.
async fn console_behind_its_runtime(
    store: Arc<dyn ConsoleLogStore>,
) -> (
    tempfile::TempDir,
    Arc<UnifiedRuntime>,
    MetaConsoleAggregator,
    ConsoleEventStore,
) {
    let (temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let events = ConsoleEventStore::new();
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    entry.console_events.clone_from(&events);
    let aggregator = MetaConsoleAggregator::new_with_options(
        store,
        ConsoleAggregatorOptions {
            session_history_backfill_enabled: false,
            ..ConsoleAggregatorOptions::default()
        },
    );
    aggregator
        .inner
        .runtimes
        .write()
        .expect("registry")
        .insert(RUNTIME.into(), entry);
    (temp, runtime, aggregator, events)
}

/// The runtime's memory event as its sink appends it for the console: an
/// event that names no member goes to the system page.
async fn runtime_emits(events: &ConsoleEventStore, event: MemoryTimelineEvent) {
    assert!(event.identity().is_none(), "a realm-level memory event");
    events
        .append_runtime_plane_event(event.event_type().to_string(), event.data())
        .await;
}

/// The memory records stored on the system page, oldest first.
async fn system_page(aggregator: &MetaConsoleAggregator) -> Vec<ConsoleFrame> {
    aggregator
        .store()
        .query_windowed_frames(ConsoleTimelineWindowQuery {
            identity: Some(SYSTEM_EVENT_IDENTITY.into()),
            limit: 1_000,
            ..ConsoleTimelineWindowQuery::default()
        })
        .await
        .expect("read")
        .frames
        .into_iter()
        .filter(|frame| frame.kind.starts_with("memory."))
        .collect()
}

#[tokio::test]
async fn a_conflict_signal_stored_after_a_lag_is_shown_to_an_administrator_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stores: [Arc<dyn ConsoleLogStore>; 2] = [
        Arc::new(
            SqliteConsoleLogStore::open(dir.path().join("console.sqlite3")).expect("sqlite store"),
        ),
        Arc::new(InMemoryConsoleLogStore::new()),
    ];
    // Every person may view every agent, the system page included; root is
    // the one administrator.
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-views-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..AccessControlConfig::default()
    })
    .expect("access controller");
    let rules_off = AccessController::new(AccessControlConfig::default()).expect("controller");
    let person = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let admin = controller.view_for_caller(Some(ROOT), None);
    // A caller who sent no token, by what its console is.
    let no_token = |identifies_people: bool| {
        rules_off
            .view_for_caller(None, None)
            .on_console(identifies_people)
    };

    for store in stores {
        let (_temp, runtime, aggregator, events) = console_behind_its_runtime(store).await;
        // The runtime emitted these while the console was behind: none of
        // them reached it live.
        runtime_emits(
            &events,
            MemoryTimelineEvent::DreamStarted {
                realm: "lagged".to_string(),
                run_id: "dream-started-record".to_string(),
            },
        )
        .await;
        runtime_emits(
            &events,
            MemoryTimelineEvent::ConflictSignal {
                realm: "lagged".to_string(),
                entity: "ledger".to_string(),
                topic: "balance".to_string(),
                reason: "conflict-finding-text".to_string(),
            },
        )
        .await;
        runtime_emits(
            &events,
            MemoryTimelineEvent::DreamSkipped {
                realm: "lagged".to_string(),
                reason: "dream-skipped-record".to_string(),
            },
        )
        .await;
        assert!(
            system_page(&aggregator).await.is_empty(),
            "the console stored nothing live"
        );

        // The console catches up twice: the second pass finds the same
        // retained events again.
        for pass in ["the first catch-up", "a second catch-up"] {
            recover_lagged_source_events(aggregator.inner.clone(), RUNTIME, &events)
                .await
                .expect("lag recovery");
            let frames = system_page(&aggregator).await;
            let kinds: Vec<&str> = frames.iter().map(|frame| frame.kind.as_str()).collect();
            assert_eq!(
                kinds,
                [
                    "memory.dream.started",
                    "memory.conflict.signal",
                    "memory.dream.skipped"
                ],
                "{pass}: each record is stored once"
            );
            for frame in &frames {
                let signal = frame.kind == "memory.conflict.signal";
                assert_eq!(
                    signal,
                    frame.payload.to_string().contains("conflict-finding-text"),
                    "{pass}: only the signal holds the finding's text"
                );
                assert!(
                    timeline_frame_visible_to(&admin, frame),
                    "{pass}: an administrator reads {}",
                    frame.kind
                );
                assert_eq!(
                    timeline_frame_visible_to(&person, frame),
                    !signal,
                    "{pass}: a person who is not an administrator, {}",
                    frame.kind
                );
                assert_eq!(
                    timeline_frame_visible_to(&no_token(true), frame),
                    !signal,
                    "{pass}: a caller with no token on a console that can identify a person, {}",
                    frame.kind
                );
                assert!(
                    timeline_frame_visible_to(&no_token(false), frame),
                    "{pass}: the one operator of a console that can identify nobody reads {}",
                    frame.kind
                );
            }
            // A page read keeps one caller's decisions for the request: read
            // in either order, it decides each of these records as above.
            for (reader, view) in [("an administrator", &admin), ("a person", &person)] {
                for order in [
                    frames.iter().collect::<Vec<_>>(),
                    frames.iter().rev().collect::<Vec<_>>(),
                ] {
                    let decider = TimelineFrameDecider::new(view.clone());
                    for frame in order {
                        assert_eq!(
                            decider.visible(frame),
                            timeline_frame_visible_to(view, frame),
                            "{pass}: {} on a page read by {reader}",
                            frame.kind
                        );
                    }
                }
            }
        }
        let _ = runtime.mob_handle().stop().await;
    }
}
