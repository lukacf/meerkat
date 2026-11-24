//! Rows of the console log of a host that serves a mob beside its default
//! mob: the console events of such a mob are stored in the host's one
//! console log under that mob, and read by a read that names it.
//!
//! The fixture is the one of the entries' rows: a launch that declares its
//! mob to its mob set, whose set builds a second mob of the host. Each row
//! opens the resolving of that mob for its own runtime and builds the
//! runtime's console projection, as a gateway does with its first console
//! router.

#![allow(clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::{MobDefinition, MobId, SpawnMemberSpec};
use serde_json::{Value, json};

use super::UnifiedRuntime;
use super::mob_entries::lazy_start_tests::{StoredHost, asleep, known};
use super::mob_entries_tests::{
    definition, launch, launch_composed, second_mob, set_of, shut_down,
};
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, AppendOutcome, ConsoleCursor, ConsoleFrame,
    ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleIdentityRecord,
    ConsoleLogResult, ConsoleLogStore, ConsoleSendError, ConsoleSendRequest, ConsoleSendScope,
    ConsoleTimelineEvent, ConsoleTimelinePage, ConsoleTimelineQuery, ConsoleTimelineWindowPage,
    ConsoleTimelineWindowQuery, ConsoleVisibilityPolicy,
    HideImplicitDelegateMembersConsoleVisibilityPolicy, InMemoryConsoleLogStore,
    MetaConsoleAggregator, NewConsoleFrame, ScopedReservation, SqliteConsoleLogStore,
};
use crate::member_key::DefaultMob;

/// A launch whose console projection is built, and whose requests resolve
/// the mobs its mob set builds for the host. With `log` its console log is
/// the SQLite log at that path, opened as a gateway opens its own.
async fn console_host(temp: &tempfile::TempDir, log: Option<&Path>) -> UnifiedRuntime {
    console_host_of(temp, definition("host"), log).await
}

/// [`console_host`] for the launch of `definition`: a host started again on
/// the same state launches the same mob over the same console log.
async fn console_host_of(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    log: Option<&Path>,
) -> UnifiedRuntime {
    let (mut host, _sessions) = launch(temp, definition).await;
    if let Some(log) = log {
        host.set_console_log_store(Arc::new(
            SqliteConsoleLogStore::open(log).expect("the console log opens"),
        ));
    }
    host.initialize_console_projection_with_policy(Arc::new(
        HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ))
    .expect("the runtime's console projection");
    host
}

/// Record one console event of `identity` in the console events of `mob`
/// (none: the default mob), as a request that changes an agent records it.
/// Its payload carries `text`, by which a row finds its frame.
async fn record(host: &UnifiedRuntime, mob: Option<&MobId>, identity: &str, text: &str) {
    host.console_events()
        .of_mob(mob)
        .record_lifecycle(identity, "identity_respawned", json!({ "text": text }))
        .await;
}

/// The text a row's event gave a frame, for the frames that carry one.
fn text_of(frame: &ConsoleFrame) -> Option<&str> {
    frame.payload.get("text").and_then(Value::as_str)
}

/// The frames of `mob` (none: the default mob) the host's console log
/// holds now, by the read of one mob: those a row's event made, in order.
async fn frames_now(host: &UnifiedRuntime, mob: Option<&MobId>) -> Vec<ConsoleFrame> {
    host.console_log_store()
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: mob.cloned(),
            ..Default::default()
        })
        .await
        .expect("the read of one mob")
        .frames
        .into_iter()
        .filter(|frame| text_of(frame).is_some())
        .collect()
}

/// [`frames_now`], read once a frame with the text `awaited` is among
/// them, or once the wait for it ran out.
async fn frames_showing(
    host: &UnifiedRuntime,
    mob: Option<&MobId>,
    awaited: &str,
) -> Vec<ConsoleFrame> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = frames_now(host, mob).await;
        if frames.iter().any(|frame| text_of(frame) == Some(awaited))
            || tokio::time::Instant::now() >= deadline
        {
            return frames;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The texts of `frames`, in order.
fn texts(frames: &[ConsoleFrame]) -> Vec<&str> {
    frames.iter().filter_map(text_of).collect()
}

/// The console log's ledger version in the file at `path`.
fn ledger_version(path: &Path) -> Option<i64> {
    let conn = rusqlite::Connection::open(path).expect("probe");
    meerkat_sqlite::domain_version(&conn, "meta-console").expect("ledger")
}

/// Whether the console log in the file at `path` has the frame's mob
/// column.
fn has_the_mob_column(path: &Path) -> bool {
    let conn = rusqlite::Connection::open(path).expect("probe");
    conn.prepare("SELECT mob_id FROM console_frames LIMIT 0")
        .is_ok()
}

/// The console events of a mob the host serves beside its default mob are
/// stored in the host's console log, under that mob: the frame names the
/// mob, the read of that mob answers it, and the read of the default mob
/// does not. The default mob's own event is stored with no mob, as it
/// always was, and no read of the second mob answers it. The first stored
/// frame of the second mob is what moves a SQLite log to schema version 4;
/// the frames are in the file after the runtime shut down.
#[tokio::test]
async fn the_console_events_of_a_second_mob_are_stored_under_that_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("console-log-rows.sqlite");
    let host = console_host(&temp, Some(&path)).await;
    let second_id = second_mob(&host, definition("second")).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );

    record(&host, None, "alice", "of-the-default-mob").await;
    let of_the_default = frames_showing(&host, None, "of-the-default-mob").await;
    assert_eq!(texts(&of_the_default), ["of-the-default-mob"]);
    assert_eq!(of_the_default[0].mob_id, None);
    assert_eq!(
        ledger_version(&path),
        Some(3),
        "the default mob's frame moves nothing"
    );

    record(&host, Some(&second_id), "alice", "of-the-second-mob").await;
    let of_the_second = frames_showing(&host, Some(&second_id), "of-the-second-mob").await;
    assert_eq!(
        texts(&of_the_second),
        ["of-the-second-mob"],
        "the second mob's console event is a stored frame of that mob"
    );
    if let Some(frame) = of_the_second.first() {
        assert_eq!(frame.mob_id.as_ref(), Some(&second_id));
        assert_eq!(frame.identity, "alice");
        assert_ne!(
            frame.runtime_key, of_the_default[0].runtime_key,
            "the second mob's frames are recorded under a runtime key of their own"
        );
    }
    assert_eq!(
        texts(&frames_now(&host, None).await),
        ["of-the-default-mob"],
        "the read of the default mob answers no frame of the second mob"
    );
    assert_eq!(
        ledger_version(&path),
        Some(4),
        "the first stored frame of another mob moves the log"
    );

    shut_down(&host, &[&second_id]).await;
    let reopened = SqliteConsoleLogStore::open(&path).expect("the log opens again");
    let kept = reopened
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(second_id.clone()),
            ..Default::default()
        })
        .await
        .expect("the read of one mob")
        .frames;
    assert_eq!(
        texts(&kept),
        ["of-the-second-mob"],
        "the second mob's frame is in the log after the runtime shut down"
    );
}

/// Two mobs of one host with a member of one name keep separate frames:
/// each mob's events are stored under that mob, the read of a mob answers
/// its own member's frames in the order they were recorded, and a member's
/// read (the name with its mob) answers that member's alone.
#[tokio::test]
async fn two_mobs_with_a_member_of_one_name_keep_separate_console_frames() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let second_id = second_mob(&host, definition("second")).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );

    record(&host, None, "alice", "default-1").await;
    record(&host, Some(&second_id), "alice", "second-1").await;
    record(&host, None, "alice", "default-2").await;
    record(&host, Some(&second_id), "alice", "second-2").await;

    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "second-2").await),
        ["second-1", "second-2"]
    );
    assert_eq!(
        texts(&frames_showing(&host, None, "default-2").await),
        ["default-1", "default-2"]
    );
    let store = host.console_log_store();
    for (mob, own) in [
        (None, ["default-1", "default-2"]),
        (Some(&second_id), ["second-1", "second-2"]),
    ] {
        let of_the_member = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("alice".to_string()),
                mob_id: mob.cloned(),
                ..Default::default()
            })
            .await
            .expect("a member's read")
            .frames;
        assert_eq!(
            texts(&of_the_member),
            own,
            "the read of alice in {mob:?} answers that member's frames alone"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// A frame of a second mob's member is witnessed by that mob's own member.
/// Two mobs each seat a member `desk`; an event of each that names its
/// member's session is stored with the witness of the member of ITS mob:
/// the witness names that mob and that session, never the member of the
/// same name in the other mob. What a caller may read of the frame is then
/// decided by the member the frame captured, in the frame's own mob.
#[tokio::test]
async fn a_second_mobs_members_frame_is_witnessed_by_that_mobs_own_member() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
        assert!(
            seated.is_ok(),
            "the spawn through the resolved mob: {:?}",
            seated.as_ref().err()
        );
    }
    let of_the_default = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
    assert!(
        of_the_default.is_ok(),
        "the default mob's member of the same name: {:?}",
        of_the_default.as_ref().err()
    );
    let desk = meerkat_mob::AgentIdentity::from("desk");
    let (in_the_default, in_the_second) = (
        host.mob_handle().resolve_bridge_session_id(&desk).await,
        second.resolve_bridge_session_id(&desk).await,
    );
    assert!(
        in_the_default.is_some() && in_the_second.is_some() && in_the_default != in_the_second,
        "two members, one name: {in_the_default:?}, {in_the_second:?}"
    );
    let (Some(in_the_default), Some(in_the_second)) = (in_the_default, in_the_second) else {
        shut_down(&host, &[&second_id]).await;
        return;
    };

    let events = host.console_events();
    for (mob, session, text) in [
        (None, &in_the_default, "run-of-the-default-desk"),
        (Some(&second_id), &in_the_second, "run-of-the-second-desk"),
    ] {
        events
            .of_mob(mob)
            .append(
                "desk",
                None,
                "run_started",
                json!({ "session_id": session.to_string(), "text": text }),
            )
            .await;
    }
    let mut witnessed = Vec::new();
    for (mob, text) in [
        (None, "run-of-the-default-desk"),
        (Some(&second_id), "run-of-the-second-desk"),
    ] {
        let frame = frames_showing(&host, mob, text)
            .await
            .into_iter()
            .find(|frame| text_of(frame) == Some(text));
        witnessed.push(frame.and_then(|frame| {
            frame.source.member_provenance.map(|witness| {
                (
                    frame.mob_id,
                    witness.primary_mob_id,
                    witness.identity.session_id,
                )
            })
        }));
    }
    assert_eq!(
        witnessed,
        [
            Some((
                None,
                default_id.to_string(),
                Some(in_the_default.to_string())
            )),
            Some((
                Some(second_id.clone()),
                second_id.to_string(),
                Some(in_the_second.to_string())
            )),
        ],
        "each frame is witnessed by the member of its own mob"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The projection of a second mob's console events follows the mob's
/// entry. What the mob recorded before its entry went is stored; an event
/// that reaches the ended mob's console events afterwards is not; and a
/// mob created again under the same id has its events stored again, under
/// that id.
#[tokio::test]
async fn the_projection_of_a_second_mobs_console_events_follows_its_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    let of_the_first = host.console_events().of_mob(Some(&second_id));
    record(&host, Some(&second_id), "alice", "before-the-end").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "before-the-end").await),
        ["before-the-end"]
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_err(),
        "a destroyed mob resolves no mob"
    );
    of_the_first
        .record_lifecycle(
            "alice",
            "identity_respawned",
            json!({ "text": "after-the-end" }),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        texts(&frames_now(&host, Some(&second_id)).await),
        ["before-the-end"],
        "nothing projects the console events of a mob whose entry went"
    );

    let again = second_mob(&host, second_definition).await;
    assert_eq!(again, second_id);
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "a mob created again under the id resolves"
    );
    record(&host, Some(&second_id), "alice", "of-the-mob-created-again").await;
    assert!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-mob-created-again").await)
            .contains(&"of-the-mob-created-again"),
        "the events of the mob created again are stored under its id"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A host of one mob keeps its console log's place and form: through the
/// default mob's events, a second mob that is created, resolved and silent,
/// the default mob named by its id, and the runtime's shutdown, the SQLite
/// log stays at schema version 3 without the mob column, and the default
/// mob's frames are stored with no mob.
#[tokio::test]
async fn a_host_whose_other_mobs_are_silent_keeps_its_console_logs_place_and_form() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("console-log-rows.sqlite");
    let host = console_host(&temp, Some(&path)).await;
    let default_id = host.mob_handle().mob_id().clone();
    let unmoved = |when: &str| {
        assert_eq!(ledger_version(&path), Some(3), "{when}");
        assert!(!has_the_mob_column(&path), "{when}");
    };

    record(&host, None, "alice", "default-1").await;
    assert_eq!(
        texts(&frames_showing(&host, None, "default-1").await),
        ["default-1"]
    );
    unmoved("after the default mob's first frame");

    let second_id = second_mob(&host, definition("second")).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    // The default mob named by its id is the default mob: its event is the
    // host's own, stored with no mob.
    record(&host, Some(&default_id), "alice", "default-2").await;
    let of_the_default = frames_showing(&host, None, "default-2").await;
    assert_eq!(texts(&of_the_default), ["default-1", "default-2"]);
    assert!(
        of_the_default.iter().all(|frame| frame.mob_id.is_none()),
        "the default mob's frames name no mob"
    );
    assert!(
        frames_now(&host, Some(&second_id)).await.is_empty(),
        "a silent mob has no frame"
    );
    unmoved("with a second mob created, resolved and silent");

    shut_down(&host, &[&second_id]).await;
    unmoved("after the runtime shut down");
}

/// The console log is told the host's default mob when the runtime's
/// console projection is built: the mob of the launch, known without
/// loading any mob. A store that was told refuses another mob as its
/// default mob, so the answer to a second telling says which mob it was
/// told.
#[tokio::test]
async fn the_console_log_is_told_the_launchs_default_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let default_id = host.mob_handle().mob_id().clone();
    let store = host.console_log_store();
    let another = store.bind_default_mob(&DefaultMob::new(MobId::from("another-mob")));
    assert!(
        another
            .as_ref()
            .err()
            .is_some_and(|refused| refused.to_string().contains(default_id.as_str())),
        "the store was told the launch's mob before: {:?}",
        another.as_ref().err().map(ToString::to_string)
    );
    assert!(
        store
            .bind_default_mob(&DefaultMob::new(default_id.clone()))
            .is_ok(),
        "the launch's mob again is accepted"
    );
    shut_down(&host, &[]).await;
}

/// One request on the stdio dispatch of `host`, answered as JSON.
async fn stdio_call(host: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer =
        crate::rpc::handle_unified_rpc_json(host, &request, Duration::from_secs(1), None, None)
            .await;
    serde_json::from_str(&answer).expect("the dispatch answers JSON")
}

/// The unified dispatch runs on the workspace's default test-thread stack
/// (8 MiB, `.cargo/config.toml`) in an unoptimised build: a plain
/// capabilities call, and a call that names a mob the host serves beside
/// its default mob, which resolves that mob.
///
/// What this bounds: the stack one poll of the dispatch takes, the frame
/// of its own body first. Every arm of the dispatch that names a mob
/// awaits the mob's resolution in place, and a future awaited in place on
/// that path is part of the body's frame once for each such arm. The end
/// of a mob's console events, awaited in place, took the body from about
/// 6 MB to over 11 MB. Meta's own tests and a gateway's workers run on
/// 16 MiB, where that still fits; these calls run on a thread of 8 MiB
/// whatever stack the test itself was given, so such a growth is seen
/// where the tests run on 16 MiB too. A stack that is too small ends the
/// test's process: a stack overflow is no panic. Run with a process for
/// each row, that takes this row alone; where the rows share one process
/// (a plain run of the test binary) it takes every later row with it.
#[test]
fn the_unified_dispatch_runs_on_the_workspaces_default_stack() {
    const DEFAULT_TEST_STACK: usize = 8 * 1024 * 1024;
    let calls = std::thread::Builder::new()
        .name("dispatch-on-the-default-stack".to_string())
        .stack_size(DEFAULT_TEST_STACK)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime on this thread");
            // The calls' own state is on the heap: the thread's stack holds
            // the frames of a poll, which is what the row bounds.
            runtime.block_on(Box::pin(async {
                let temp = tempfile::tempdir().expect("tempdir");
                let host = console_host(&temp, None).await;
                let second_id = second_mob(&host, definition("second")).await;
                let plain = stdio_call(&host, "meta/capabilities", json!({})).await;
                let named = stdio_call(
                    &host,
                    "meta/status",
                    json!({ "mob_id": second_id.as_str() }),
                )
                .await;
                shut_down(&host, &[&second_id]).await;
                (plain, named)
            }))
        })
        .expect("the thread starts")
        .join();
    let (plain, named) = calls.expect("the calls ran to their end on the 8 MiB stack");
    assert!(
        plain["result"]["feature_capabilities"].is_array(),
        "the plain call is answered: {plain:#}"
    );
    assert!(
        named["error"].is_null() && named["result"].is_object(),
        "the call that names the second mob is answered for it: {named:#}"
    );
}

/// One frame with the text `text`, of `identity` in the mob `mob`, as a
/// projection that ran late would store it.
fn late_frame(mob: &MobId, identity: &str, text: &str) -> NewConsoleFrame {
    NewConsoleFrame {
        mob_id: Some(mob.clone()),
        id: None,
        dedupe_key: format!("late:{text}"),
        timestamp_ms: 1,
        runtime_key: format!("mob/{}", mob.as_str()),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: None,
        kind: "identity_respawned".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "text": text }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: Some(format!("late:{text}")),
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// A mob created under the id of a destroyed mob shows no console frame of
/// the earlier mob: the create itself removes what the host's console log
/// still keeps under that id (frames a destroy left, a frame written under
/// the id after it), before the mob is built. The default mob's frames
/// stay, and the new mob's own events are stored under the id.
#[tokio::test]
async fn a_mob_created_under_a_destroyed_mobs_id_shows_no_console_frame_of_the_earlier_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    record(&host, None, "alice", "of-the-default-mob").await;
    record(&host, Some(&second_id), "alice", "of-the-earlier-mob").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-earlier-mob").await),
        ["of-the-earlier-mob"]
    );
    assert_eq!(
        texts(&frames_showing(&host, None, "of-the-default-mob").await),
        ["of-the-default-mob"]
    );
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    // Whatever the destroy left, a frame is under the id after it.
    host.console_log_store()
        .append_if_absent(late_frame(&second_id, "alice", "written-after-the-destroy"))
        .await
        .expect("a frame under the destroyed mob's id");
    assert!(
        texts(&frames_now(&host, Some(&second_id)).await).contains(&"written-after-the-destroy"),
        "the host's log keeps a frame under the destroyed mob's id"
    );

    assert_eq!(
        second_mob(&host, second_definition).await,
        second_id,
        "a mob under the same id"
    );
    assert_eq!(
        texts(&frames_now(&host, Some(&second_id)).await),
        Vec::<&str>::new(),
        "the mob created under the id shows no frame of the earlier mob"
    );
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the mob created under the id resolves"
    );
    record(&host, Some(&second_id), "alice", "of-the-new-mob").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-new-mob").await),
        ["of-the-new-mob"],
        "the new mob's events are stored under the id"
    );
    assert_eq!(
        texts(&frames_now(&host, None).await),
        ["of-the-default-mob"],
        "the default mob's frames stay"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A console log that has no removal of a mob's frames: it keeps the store
/// trait's default, which refuses.
struct WithoutRemoval(InMemoryConsoleLogStore);

#[async_trait::async_trait]
impl ConsoleLogStore for WithoutRemoval {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.0.append_if_absent(frame).await
    }
    async fn update_frame_status(
        &self,
        id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.0.update_frame_status(id, status).await
    }
    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        self.0.query_frames(query).await
    }
    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.0.query_windowed_frames(query).await
    }
    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.0.frame_by_dedupe_key(key).await
    }
    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.0.latest_cursor().await
    }
    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.0.clear_frames().await
    }
    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.0.record_source_watermark(runtime, kind, cursor).await
    }
    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.0.source_watermark(runtime, kind).await
    }
}

/// A create is refused when the console frames an earlier mob of the id
/// left cannot be removed: the mob would show another mob's frames. Shown
/// with a console log that has no removal of a mob's frames, the one
/// release of this launch that fails; the create is answered the one
/// sentence of a failed release, with nothing of the console log's own
/// words, and leaves no mob.
#[tokio::test]
async fn a_create_is_refused_when_an_earlier_mobs_console_frames_cannot_be_removed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _sessions) = launch(&temp, definition("host")).await;
    host.set_console_log_store(Arc::new(WithoutRemoval(InMemoryConsoleLogStore::new())));
    host.initialize_console_projection_with_policy(Arc::new(
        HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ))
    .expect("the runtime's console projection");
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    assert!(
        matches!(
            &created,
            Err(error) if error.to_string().contains(crate::private_owners::CREATE_RELEASE_FAILED)
        ),
        "the create is refused with the one sentence of a failed release: {:?}",
        created.as_ref().map(|_| "created")
    );
    // The refusal says what could not be done, and nothing of the console
    // log's own failure: those words stay in the host's log.
    let answer = created
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        !answer.contains("console frames") && !answer.contains("remove_frames_of_mob"),
        "the refusal carries nothing of the console log's own failure: {answer}"
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    shut_down(&host, &[]).await;
}

/// A launch like [`console_host`]'s, built by the LIBRARY BUILDER with a
/// console log of its own (`with_console_log_store`), as an application
/// that embeds the runtime builds it: its mob is declared to its mob set,
/// and nothing tells the runtime its console log after the build.
async fn builder_host(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    console_log: Arc<dyn ConsoleLogStore>,
) -> UnifiedRuntime {
    use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
    use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};

    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(Arc::new(
        ObjectStoreBlobStore::memory(),
    )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(temp.path())
            .builtins(false)
            .comms(true),
        meerkat::Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let namespace = definition.id.to_string();
    let spec = MobBootstrapSpec::declared(
        definition,
        crate::mob_composition_manifest::DeclaredMobStore::in_memory(),
        service,
    )
    .with_session_runtime_adapter(Arc::clone(&machine))
    .expect("the launch adopts the runtime machine")
    .with_runtime_delivery(crate::MobRuntimeDelivery::new(store, jobs))
    .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
    .expect("the agent mob tools")
    .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    });
    let host = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(crate::MetaConfig {
                modules: Vec::new(),
                discovery: crate::DiscoverySpec {
                    namespace,
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .with_console_log_store(console_log)
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("a runtime built by the library builder");
    host.initialize_console_projection_with_policy(Arc::new(
        HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ))
    .expect("the runtime's console projection");
    host
}

/// A runtime built by the library builder with a console log of its own
/// removes from THAT log what it keeps under an id before a mob is created
/// under the id: a frame stored under the id is gone once the mob set
/// created a mob there. The runtime holds one console log, and the release
/// before a create asks the same one, however the runtime was built.
#[tokio::test]
async fn a_runtime_built_by_the_library_builder_removes_an_earlier_mobs_console_frames_at_a_create()
{
    let temp = tempfile::tempdir().expect("tempdir");
    let log: Arc<dyn ConsoleLogStore> = Arc::new(InMemoryConsoleLogStore::new());
    let host = builder_host(&temp, definition("host"), Arc::clone(&log)).await;
    assert!(
        std::ptr::addr_eq(Arc::as_ptr(&host.console_log_store()), Arc::as_ptr(&log)),
        "the runtime holds the console log its builder was given"
    );
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    log.append_if_absent(late_frame(&id, "alice", "of-an-earlier-mob"))
        .await
        .expect("a frame under the id");
    assert_eq!(
        texts(&frames_now(&host, Some(&id)).await),
        ["of-an-earlier-mob"],
        "the log keeps a frame under the id"
    );

    assert_eq!(
        second_mob(&host, of_the_second).await,
        id,
        "a mob under the id"
    );
    assert_eq!(
        texts(&frames_now(&host, Some(&id)).await),
        Vec::<&str>::new(),
        "the mob created under the id shows no frame of the earlier mob"
    );
    shut_down(&host, &[&id]).await;
}

/// A runtime built by the library builder with a console log that has no
/// removal of a mob's frames refuses a create, as a launch that was told
/// its console log after its start does: the release before a create asks
/// the log the runtime holds, and that log cannot remove. The create leaves
/// no mob. (What the refusal says is the create hook's to word; this row
/// holds that it is one.)
#[tokio::test]
async fn a_runtime_built_by_the_library_builder_refuses_a_create_its_console_log_cannot_release_for()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let log: Arc<dyn ConsoleLogStore> = Arc::new(WithoutRemoval(InMemoryConsoleLogStore::new()));
    let host = builder_host(&temp, definition("host"), log).await;
    let of_the_second = definition("second");
    let id = of_the_second.id.clone();
    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    assert!(
        created.is_err(),
        "the create is refused: {:?}",
        created.as_ref().map(|_| "created")
    );
    // The refusal says what could not be done, and nothing of the console
    // log's own failure: those words stay in the host's log.
    let answer = created
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        !answer.contains("console frames") && !answer.contains("remove_frames_of_mob"),
        "the refusal carries nothing of the console log's own failure: {answer}"
    );
    assert!(
        set_of(&host).handle_for(&id).await.is_err(),
        "and the set holds no mob under the id"
    );
    shut_down(&host, &[]).await;
}

/// A host of one mob keeps a version 3 console log across a restart and a
/// Console send: a start, a console event, a Console send to an agent of the
/// launch mob, a clean shutdown, a second start of the same launch on the
/// same log, another event and a shutdown leave the SQLite log at schema
/// version 3 without the mob column at every point.
#[tokio::test]
async fn a_host_of_one_mob_keeps_a_version_3_console_log_across_a_restart_and_a_send() {
    let (first, second, state) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let path = state.path().join("console-log-rows.sqlite");
    let of_the_launch_mob = definition("host");
    let unmoved = |when: &str| {
        assert_eq!(ledger_version(&path), Some(3), "{when}");
        assert!(!has_the_mob_column(&path), "{when}");
    };

    let host = console_host_of(&first, of_the_launch_mob.clone(), Some(&path)).await;
    record(&host, None, "alice", "before-the-restart").await;
    assert_eq!(
        texts(&frames_showing(&host, None, "before-the-restart").await),
        ["before-the-restart"]
    );
    unmoved("after the first start's event");

    let seated = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
    assert!(
        seated.is_ok(),
        "an agent of the launch mob: {:?}",
        seated.as_ref().err()
    );
    let sent = match host.console_projection.get() {
        Some(projection) => Box::pin(projection.send(send_to("desk"))).await.map(|_| ()),
        None => Err(ConsoleSendError::State(
            "the runtime has no console projection",
        )),
    };
    assert!(
        sent.is_ok(),
        "the Console send to the launch mob's agent is accepted: {:?}",
        sent.as_ref().err().map(ToString::to_string)
    );
    let of_the_send = host
        .console_log_store()
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            identity: Some("desk".to_string()),
            ..Default::default()
        })
        .await
        .expect("the agent's frames")
        .frames
        .into_iter()
        .filter(|frame| frame.kind == "user_input")
        .count();
    assert_eq!(of_the_send, 1, "the send's record is a frame of the log");
    unmoved("after a Console send");
    shut_down(&host, &[]).await;
    unmoved("after the first shutdown");

    let host = console_host_of(&second, of_the_launch_mob, Some(&path)).await;
    unmoved("after the second start");
    record(&host, None, "alice", "after-the-restart").await;
    // The agent's run after the send left frames with a text of their own:
    // the row's events are `alice`'s.
    let of_alice: Vec<ConsoleFrame> = frames_showing(&host, None, "after-the-restart")
        .await
        .into_iter()
        .filter(|frame| frame.identity == "alice")
        .collect();
    assert_eq!(
        texts(&of_alice),
        ["before-the-restart", "after-the-restart"],
        "the log the second start opened holds the first start's frame"
    );
    unmoved("after the second start's event");
    shut_down(&host, &[]).await;
    unmoved("after the second shutdown");
}

/// A Console send of the text `hello` to `identity`, queued.
fn send_to(identity: &str) -> ConsoleSendRequest {
    ConsoleSendRequest {
        identity: identity.to_string(),
        content: json!("hello"),
        origin: "console".to_string(),
        idempotency_key: format!("a-send-to-{identity}"),
        handling_mode: Some("queue".to_string()),
        origin_kind: None,
        skill_refs: Vec::new(),
    }
}

/// The members of a mob the host serves beside its default mob are no
/// delegate members of the launch mob: a console view that lists the
/// launch's delegate members does not list them, and a Console send made
/// for the launch mob to a name only the second mob holds reaches no agent
/// and stores no frame.
#[tokio::test]
async fn a_second_mobs_members_are_no_delegate_members_of_the_launch_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let second_id = second_mob(&host, definition("second")).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", "courier"))).await;
        assert!(
            seated.is_ok(),
            "an agent of the second mob: {:?}",
            seated.as_ref().err()
        );
    }
    let seated = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
    assert!(
        seated.is_ok(),
        "an agent of the launch mob: {:?}",
        seated.as_ref().err()
    );

    let Some(projection) = host.console_projection.get() else {
        shut_down(&host, &[&second_id]).await;
        return;
    };
    // A view whose policy lists the launch's delegate members.
    let view = projection.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
    let listed: Vec<String> = Box::pin(view.list_identities())
        .await
        .expect("the view's identities")
        .into_iter()
        .map(|record| record.identity)
        .collect();
    assert!(
        listed.iter().any(|identity| identity == "desk"),
        "the launch mob's agent is listed: {listed:?}"
    );
    assert!(
        !listed.iter().any(|identity| identity == "courier"),
        "the second mob's agent is not listed as the launch's: {listed:?}"
    );

    let sent = Box::pin(view.send(send_to("courier"))).await;
    assert!(
        matches!(sent, Err(ConsoleSendError::UnknownIdentity(_))),
        "a send for the launch mob knows no agent of the second mob: {:?}",
        sent.as_ref()
            .map(|_| "accepted")
            .map_err(ToString::to_string)
    );
    let of_the_name = host
        .console_log_store()
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            identity: Some("courier".to_string()),
            ..Default::default()
        })
        .await
        .expect("the default mob's frames of the name")
        .frames;
    assert!(
        of_the_name.is_empty(),
        "the send stored no frame under the default mob: {} frames",
        of_the_name.len()
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob the host's mob set builds for the host has console events of its
/// own with the host from the moment the set asks the host for the mob's
/// member wiring (at a create and at a load), before the mob has an entry.
/// The launch mob's delegate scan leaves out every mob that has console
/// events of its own with the host, so no agent of a mob of the host is
/// read as an agent of a mob a member made in the time between the mob's
/// creation and its entry.
#[tokio::test]
async fn a_mob_being_built_for_the_host_has_console_events_of_its_own_before_its_entry() {
    use crate::mob_handle_runtime::HostMobEntryWiring;

    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let events = host.console_events();
    // The set holds no mob of this id, so no pass of the table gives it an
    // entry: what names its console events here is the wiring alone.
    let being_built = MobId::from("a-mob-being-built");
    assert!(
        !events.mobs_with_a_store().contains(&being_built),
        "a mob nothing named has no console events with the host"
    );
    let wiring = host.other_mobs().member_wiring(&being_built).await;
    assert!(
        wiring.is_ok(),
        "the host answers the wiring of a mob the set builds for it: {:?}",
        wiring.as_ref().err().map(ToString::to_string)
    );
    assert!(
        events.mobs_with_a_store().contains(&being_built),
        "a mob whose member wiring the set asked for has console events of its own with the \
         host before it has an entry: {:?}",
        events.mobs_with_a_store()
    );
    shut_down(&host, &[]).await;
}

/// A member of a mob the host serves beside its default mob is no stale
/// generation of the default mob's identity of the same name. The launch
/// mob and a second mob of the host each hold a member under the generated
/// name `rt:desk:1`. Asked, for the launch mob's registration, for the
/// members that are an older generation than `rt:desk:2`, the Console's
/// walk names the launch mob's member alone: what it names is retired with
/// the identity.
#[tokio::test]
async fn a_second_mobs_member_is_no_stale_generation_of_the_launch_mobs_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let second_id = second_mob(&host, definition("second")).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    let launch = host.mob_runtime().handle();
    let launch_id = launch.mob_id().to_string();
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the mob set serves the second mob");
    for (named, handle) in [("the launch mob", &launch), ("the second mob", &second)] {
        let mut spec = SpawnMemberSpec::new("worker", "desk");
        spec.identity = crate::member_comms_id::mob_member_id("rt:desk:1");
        let seated = Box::pin(handle.spawn_spec(spec)).await;
        assert!(
            seated.is_ok(),
            "{named} seats a member under the generated name: {:?}",
            seated.as_ref().err().map(ToString::to_string)
        );
    }
    let named = match host.console_projection.get() {
        Some(projection) => {
            crate::console_aggregator::stale_generated_members_for_test(
                projection,
                "rt:desk:2",
                false,
            )
            .await
        }
        None => Vec::new(),
    };
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        named,
        [(launch_id, "rt:desk:1".to_string())],
        "the walk for the launch mob's identity names the launch mob's own member of an older \
         generation, and no member of another mob the host serves"
    );
}

/// How many Console sends to `identity` the host's console log holds under
/// the mob `mob`.
async fn sends_under(host: &UnifiedRuntime, mob: &MobId, identity: &str) -> usize {
    host.console_log_store()
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(mob.clone()),
            identity: Some(identity.to_string()),
            ..Default::default()
        })
        .await
        .expect("the member's frames")
        .frames
        .into_iter()
        .filter(|frame| frame.kind == "user_input")
        .count()
}

/// A mob created under the id of a destroyed mob shows no Console send of
/// the earlier mob: the record of a send to the earlier mob's agent goes
/// with that mob's other frames at the create, so the same send made to
/// the new mob's agent of the name is a new send, not the earlier one
/// found again.
#[tokio::test]
async fn a_mob_created_under_a_destroyed_mobs_id_shows_no_send_of_the_earlier_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    let Some(projection) = host.console_projection.get() else {
        shut_down(&host, &[&second_id]).await;
        return;
    };
    let scope = ConsoleSendScope::Human {
        issuer: "https://issuer.example".to_string(),
        subject: "u1".to_string(),
    };
    let to_the_earlier = Box::pin(projection.reserve_identity_first_interaction_for_mob(
        send_to("alice"),
        Some(&second_id),
        None,
        &scope,
        None,
    ))
    .await;
    assert!(
        matches!(to_the_earlier, Ok(ScopedReservation::Fresh(..))),
        "the send to the earlier mob's agent is recorded: {to_the_earlier:?}"
    );
    // The reservation holds the send's dispatch claim, as a caller that is
    // dispatching it does: it ends here, as a dispatch does.
    drop(to_the_earlier);
    assert_eq!(sends_under(&host, &second_id, "alice").await, 1);

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        second_mob(&host, second_definition).await,
        second_id,
        "a mob under the same id"
    );
    assert_eq!(
        sends_under(&host, &second_id, "alice").await,
        0,
        "the mob created under the id shows no send of the earlier mob"
    );
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the mob created under the id resolves"
    );
    let to_the_new = Box::pin(projection.reserve_identity_first_interaction_for_mob(
        send_to("alice"),
        Some(&second_id),
        None,
        &scope,
        None,
    ))
    .await;
    assert!(
        matches!(to_the_new, Ok(ScopedReservation::Fresh(..))),
        "the same send to the new mob's agent is a new send: {to_the_new:?}"
    );
    assert_eq!(sends_under(&host, &second_id, "alice").await, 1);
    shut_down(&host, &[&second_id]).await;
}

/// A policy that hides the identity it names, in every mob.
struct HideIdentity(&'static str);

impl ConsoleVisibilityPolicy for HideIdentity {
    fn identity_visible(&self, record: &ConsoleIdentityRecord) -> bool {
        record.identity != self.0
    }
}

/// The texts of the row's frames in the timeline of the mob `mob` (none:
/// the default mob) as `console` reads it, for every member or for
/// `identity`. The console's identities are read first, as a request that
/// lists the agents does before one reads their timeline. The row's frames
/// are the console events it recorded, whose texts begin `of-the-`: an
/// agent's own run leaves frames with a text too, at a time of its own.
async fn timeline_texts(
    console: &MetaConsoleAggregator,
    mob: Option<&MobId>,
    identity: Option<&str>,
) -> Vec<String> {
    let _ = Box::pin(console.list_identities()).await;
    Box::pin(console.query_mob_timeline_windowed_for(
        ConsoleTimelineWindowQuery {
            mob_id: mob.cloned(),
            identity: identity.map(str::to_string),
            ..Default::default()
        },
        None,
    ))
    .await
    .map(|page| {
        page.frames
            .iter()
            .filter_map(text_of)
            .filter(|text| text.starts_with("of-the-"))
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_else(|error| vec![format!("the timeline read failed: {error:?}")])
}

/// A host of two mobs with an agent `desk` in each, and one console event
/// of each agent stored: `of-the-launch-desk` and `of-the-second-desk`.
/// With it the second mob's id.
async fn two_desks_with_a_frame_each(host: &UnifiedRuntime) -> MobId {
    let second_id = second_mob(host, definition("second")).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
        assert!(
            seated.is_ok(),
            "the second mob's agent: {:?}",
            seated.as_ref().err()
        );
    }
    let seated = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
    assert!(
        seated.is_ok(),
        "the launch mob's agent: {:?}",
        seated.as_ref().err()
    );
    record(host, None, "desk", "of-the-launch-desk").await;
    record(host, Some(&second_id), "desk", "of-the-second-desk").await;
    assert!(
        texts(&frames_showing(host, Some(&second_id), "of-the-second-desk").await)
            .contains(&"of-the-second-desk"),
        "the second mob's agent's event is stored under its mob"
    );
    assert!(
        texts(&frames_showing(host, None, "of-the-launch-desk").await)
            .contains(&"of-the-launch-desk"),
        "the launch mob's agent's event is stored"
    );
    second_id
}

/// The frames of a second mob's agent show in that mob's timeline, and
/// only there, through the runtime's console projection and through a
/// console view, also one made before the host served the mob (a gateway
/// makes its views with its router): for every agent of the mob and for the
/// one a read names. A view whose policy hides the agent shows none of its
/// frames, and the default mob's list of agents lists no agent of the
/// second mob.
#[tokio::test]
async fn a_second_mobs_agents_frames_show_in_that_mobs_timeline_through_a_console_view() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let Some(projection) = host.console_projection.get() else {
        shut_down(&host, &[]).await;
        return;
    };
    let made_before =
        projection.policy_view(Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy));
    let second_id = two_desks_with_a_frame_each(&host).await;
    let made_after = projection.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));

    for (who, console) in [
        ("the runtime's projection", projection),
        ("a view made before the mob", &made_before),
        ("a view made after the mob", &made_after),
    ] {
        assert_eq!(
            timeline_texts(console, Some(&second_id), None).await,
            ["of-the-second-desk"],
            "{who}: the second mob's timeline shows its agent's frame"
        );
        assert_eq!(
            timeline_texts(console, Some(&second_id), Some("desk")).await,
            ["of-the-second-desk"],
            "{who}: the second mob's agent's own timeline"
        );
        assert_eq!(
            timeline_texts(console, None, None).await,
            ["of-the-launch-desk"],
            "{who}: the launch mob's timeline shows its own agent's frame alone"
        );
        assert_eq!(
            timeline_texts(console, None, Some("desk")).await,
            ["of-the-launch-desk"],
            "{who}: the launch mob's agent's own timeline"
        );
    }

    let hiding = projection.policy_view(Arc::new(HideIdentity("desk")));
    assert_eq!(
        timeline_texts(&hiding, Some(&second_id), None).await,
        Vec::<String>::new(),
        "a view that hides the agent shows none of its frames"
    );
    assert_eq!(
        timeline_texts(&hiding, Some(&second_id), Some("desk")).await,
        Vec::<String>::new(),
        "a view that hides the agent shows none of its frames to a read that names it"
    );

    let listed = Box::pin(made_after.list_identities())
        .await
        .expect("the view's identities");
    let desks = listed
        .iter()
        .filter(|record| record.identity == "desk")
        .count();
    assert_eq!(
        desks, 1,
        "the default mob's list of agents lists its own agent and no agent of the second mob"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A second mob's agent's frame, as the timeline's subscribers are sent it,
/// is visible to a subscriber of a console view and of the runtime's
/// projection, and not to one of a view whose policy hides the agent.
#[tokio::test]
async fn a_second_mobs_agents_live_frame_is_visible_to_a_console_views_subscriber() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let Some(projection) = host.console_projection.get() else {
        shut_down(&host, &[]).await;
        return;
    };
    let made_before =
        projection.policy_view(Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy));
    let second_id = two_desks_with_a_frame_each(&host).await;
    let hiding = projection.policy_view(Arc::new(HideIdentity("desk")));
    let of_the_second = frames_showing(&host, Some(&second_id), "of-the-second-desk")
        .await
        .into_iter()
        .find(|frame| text_of(frame) == Some("of-the-second-desk"));
    let Some(frame) = of_the_second else {
        shut_down(&host, &[&second_id]).await;
        return;
    };
    let event = ConsoleTimelineEvent::ConsoleFrame { frame };

    for (who, console, visible) in [
        ("the runtime's projection", projection, true),
        ("a view made before the mob", &made_before, true),
        ("a view that hides the agent", &hiding, false),
    ] {
        let _ = Box::pin(console.list_identities()).await;
        assert_eq!(
            Box::pin(console.timeline_event_visible_for_subscriber(&event, None)).await,
            visible,
            "{who}: a subscriber to every agent"
        );
        assert_eq!(
            Box::pin(console.timeline_event_visible_for_subscriber(&event, Some("desk"))).await,
            visible,
            "{who}: a subscriber to the agent"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

pub(super) const FEED_ROW_SIGNING_SECRET: &str = "console-feed-row-signing";
/// The console's administrator in the feed row.
pub(super) const FEED_ROW_ADMIN: &str = "admin@example.test";
/// A person whose one allow rule names no mob: it grants in the launch mob.
const FEED_ROW_OF_THE_LAUNCH_MOB: &str = "launch@example.test";

/// A sign-in token of `email` for the feed row's console.
fn feed_row_token(email: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": "http://127.0.0.1/meta-gateway",
            "aud": "persistent-gateway",
            "sub": email,
            "email": email,
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(FEED_ROW_SIGNING_SECRET.as_bytes()),
    )
    .expect("token")
}

/// The frames `app` answers `request` with, found at the JSON pointer
/// `frames_at` of its body, and the answer's whole body. An answer that
/// holds no page of frames there is an error with its status and body.
async fn frames_answered(
    app: &axum::Router,
    request: axum::http::Request<axum::body::Body>,
    frames_at: &str,
) -> Result<(Vec<Value>, String), String> {
    use tower::ServiceExt;

    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the door answers in time")
        .expect("the door answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("the answer's body");
    let body = String::from_utf8_lossy(&body).into_owned();
    let frames = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|answer| answer.pointer(frames_at).and_then(Value::as_array).cloned());
    match frames {
        Some(frames) if status == axum::http::StatusCode::OK => Ok((frames, body)),
        _ => Err(format!("{} {body}", status.as_u16())),
    }
}

/// The frames `GET /console/timeline` answers on `app` for the agent
/// `identity`, asked with no mob and with a sign-in of `email`, and the
/// answer's whole body. An answer that is no page of frames is an error
/// with its status and body.
pub(super) async fn timeline_read_as(
    app: &axum::Router,
    identity: &str,
    email: &str,
) -> Result<(Vec<Value>, String), String> {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/console/timeline?identity={identity}"))
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", feed_row_token(email)),
        )
        .body(axum::body::Body::empty())
        .expect("GET request");
    frames_answered(app, request, "/frames").await
}

/// The frames the Console's request door answers on `app` for
/// `meta/console/query_timeline` of the agent `identity` of the mob `mob`,
/// asked with a sign-in of `email`, and the answer's whole body. An answer
/// that is no page of frames (a refusal of the mob's id among them) is an
/// error with its status and body.
pub(super) async fn timeline_request_as(
    app: &axum::Router,
    identity: &str,
    mob: &MobId,
    email: &str,
) -> Result<(Vec<Value>, String), String> {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", feed_row_token(email)),
        )
        .body(axum::body::Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": "row",
                "method": "meta/console/query_timeline",
                "params": { "identity": identity, "mob_id": mob.as_str() },
            })
            .to_string(),
        ))
        .expect("console request");
    frames_answered(app, request, "/result/frames").await
}

/// One turn of the agent `desk` of `mob`'s mob with the task `task`, run to
/// its end.
pub(super) async fn run_a_turn_of_desk(mob: &meerkat_mob::MobHandle, task: &str) {
    let member = mob
        .member(&meerkat_mob::AgentIdentity::from("desk"))
        .await
        .expect("the agent's handle");
    let _receipt = tokio::time::timeout(Duration::from_secs(30), member.internal_turn(task))
        .await
        .expect("the turn ended")
        .expect("the turn completed");
}

/// Whether the host's console log holds, under `mob` (none: the default
/// mob), a frame that names the session `session`: read after a drain of
/// the agents' events, again until it does or the wait for it ran out.
pub(super) async fn drained_until_a_frame_names(
    host: &UnifiedRuntime,
    mob: Option<&MobId>,
    session: &str,
) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let drained = host.drain_mob_agent_events().await;
        assert!(
            drained.is_ok(),
            "the drain of the agents' events: {:?}",
            drained.as_ref().err().map(ToString::to_string)
        );
        let held = host
            .console_log_store()
            .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                mob_id: mob.cloned(),
                ..Default::default()
            })
            .await
            .expect("the read of one mob")
            .frames
            .iter()
            .any(|frame| frame.session_id.as_deref() == Some(session));
        if held || tokio::time::Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// An agent's run in a mob the host serves beside its default mob lands in
/// that mob's console events and in no other mob's. A console that
/// requires a sign-in is over a launch whose mob set made a second mob;
/// each mob has an agent `desk`. The launch mob's `desk` runs a turn of
/// its own, then the second mob's `desk` runs the turn the row is about. A
/// run's frames name the session of the agent that ran.
///
/// (a) A person whose one rule names no mob (it grants in the launch mob)
///     reads `desk`'s timeline with no mob, on `GET /console/timeline`: the
///     launch mob's agent's own run, no frame that names the second mob's
///     agent's session, and nothing of that agent's task. The same holds
///     for the console's administrator, who reads every frame stored for
///     the launch mob's `desk`.
/// (b) The administrator's read with the second mob's id holds the run:
///     `meta/console/query_timeline` on the Console's request door, signed
///     in, names the second mob and its `desk`. The same read made on the
///     runtime's console projection, under a policy that hides nothing,
///     holds it too.
/// (c) The launch mob's console event store holds no event of the run.
#[tokio::test]
async fn a_second_mobs_agents_run_lands_in_that_mobs_console_events_alone() {
    use crate::access::{AccessControlConfig, AccessController, AccessRule};
    /// The task of the second mob's agent: a text no other agent is given.
    const TASK: &str = "a-task-only-the-second-mobs-desk-was-given";

    let temp = tempfile::tempdir().expect("tempdir");
    let mut host = console_host(&temp, None).await;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![FEED_ROW_ADMIN.to_string()],
        rules: vec![AccessRule {
            id: "sees-the-launch-mobs-agents".to_string(),
            subjects: vec![FEED_ROW_OF_THE_LAUNCH_MOB.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            ..AccessRule::default()
        }],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    host.set_access_controller(controller);
    let second_id = two_desks_with_a_frame_each(&host).await;
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": FEED_ROW_SIGNING_SECRET,
        "email_allowlist": [FEED_ROW_ADMIN, FEED_ROW_OF_THE_LAUNCH_MOB],
    }))
    .expect("console sign-in configuration");
    let app = host.build_console_json_router(decisions);

    let desk = meerkat_mob::AgentIdentity::from("desk");
    let launch = host.mob_runtime().handle();
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the mob set serves the second mob");
    let launch_session = launch
        .resolve_bridge_session_id(&desk)
        .await
        .expect("the launch mob's agent's session")
        .to_string();
    let second_session = second
        .resolve_bridge_session_id(&desk)
        .await
        .expect("the second mob's agent's session")
        .to_string();
    assert_ne!(launch_session, second_session, "two agents, two sessions");

    let mut wrong = Vec::new();
    run_a_turn_of_desk(&launch, "a-task-of-the-launch-mobs-desk").await;
    if !drained_until_a_frame_names(&host, None, &launch_session).await {
        wrong.push(
            "the launch mob's agent's own run is not stored under the launch mob".to_string(),
        );
    }
    // The run the row is about.
    run_a_turn_of_desk(&second, TASK).await;
    let stored_under_the_second =
        drained_until_a_frame_names(&host, Some(&second_id), &second_session).await;
    // An event of the launch mob's agent after the run: the launch mob's
    // console events are stored in their order, so whatever the run left
    // among them is stored once this one is.
    record(&host, None, "desk", "of-the-launch-desk-after").await;
    let _ = frames_showing(&host, None, "of-the-launch-desk-after").await;

    for (who, email) in [
        ("a person of the launch mob", FEED_ROW_OF_THE_LAUNCH_MOB),
        ("the administrator", FEED_ROW_ADMIN),
    ] {
        match timeline_read_as(&app, "desk", email).await {
            Ok((frames, body)) => {
                let named = |session: &str| -> Vec<String> {
                    frames
                        .iter()
                        .filter(|frame| frame["session_id"].as_str() == Some(session))
                        .filter_map(|frame| frame["kind"].as_str())
                        .map(str::to_string)
                        .collect()
                };
                if named(&launch_session).is_empty() {
                    wrong.push(format!(
                        "(a) {who} does not read the launch mob's agent's own run in `desk`'s \
                         timeline with no mob: {} frames",
                        frames.len()
                    ));
                }
                let of_the_second = named(&second_session);
                if !of_the_second.is_empty()
                    || body.contains(TASK)
                    || body.contains(&second_session)
                {
                    wrong.push(format!(
                        "(a) {who} reads the second mob's agent's run in `desk`'s timeline with \
                         no mob: {of_the_second:?}"
                    ));
                }
            }
            Err(answer) => wrong.push(format!(
                "(a) the read of `desk`'s timeline with no mob by {who} is not answered: {answer}"
            )),
        }
    }

    match timeline_request_as(&app, "desk", &second_id, FEED_ROW_ADMIN).await {
        Ok((frames, _body)) => {
            if !frames
                .iter()
                .any(|frame| frame["session_id"].as_str() == Some(second_session.as_str()))
            {
                wrong.push(format!(
                    "(b) the administrator's read with the second mob's id, on the request \
                     door, does not hold the run of its agent (stored under the second mob: \
                     {stored_under_the_second}): {:?}",
                    frames
                        .iter()
                        .filter_map(|frame| frame["kind"].as_str())
                        .collect::<Vec<_>>()
                ));
            }
        }
        Err(answer) => wrong.push(format!(
            "(b) the administrator's read with the second mob's id, on the request door, is \
             not answered: {answer}"
        )),
    }
    let of_the_second: Vec<ConsoleFrame> = match host.console_projection.get() {
        Some(projection) => {
            let administrator = projection.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
            let _ = Box::pin(administrator.list_identities()).await;
            Box::pin(administrator.query_mob_timeline_windowed_for(
                ConsoleTimelineWindowQuery {
                    mob_id: Some(second_id.clone()),
                    identity: Some("desk".to_string()),
                    ..Default::default()
                },
                None,
            ))
            .await
            .map(|page| page.frames)
            .unwrap_or_default()
        }
        None => Vec::new(),
    };
    if !of_the_second
        .iter()
        .any(|frame| frame.session_id.as_deref() == Some(second_session.as_str()))
    {
        wrong.push(format!(
            "(b) the read named with the second mob's id, on the runtime's console projection, \
             does not hold the run of its agent \
             (stored under the second mob: {stored_under_the_second}): {:?}",
            of_the_second
                .iter()
                .map(|frame| frame.kind.as_str())
                .collect::<Vec<_>>()
        ));
    }

    let of_the_run: Vec<String> = host
        .console_events()
        .replay_all(None)
        .await
        .expect("the launch mob's console events")
        .into_iter()
        .filter(|event| {
            let data = event.data.to_string();
            data.contains(&second_session) || data.contains(TASK)
        })
        .map(|event| format!("{} {}", event.identity, event.event_type))
        .collect();
    if !of_the_run.is_empty() {
        wrong.push(format!(
            "(c) the launch mob's console event store holds events of the second mob's agent's \
             run: {of_the_run:?}"
        ));
    }

    drop(app);
    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a second mob's agent's run does not land in that mob's console events alone:\n{}",
        wrong.join("\n")
    );
}

/// The frames the host's console log holds under `mob` (none: the default
/// mob) that come from a session's history and carry `text`: read after a
/// drain of the agents' events, again until there is one or the wait for it
/// ran out.
async fn drained_until_history_carries(
    host: &UnifiedRuntime,
    mob: Option<&MobId>,
    text: &str,
) -> Vec<ConsoleFrame> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let drained = host.drain_mob_agent_events().await;
        assert!(
            drained.is_ok(),
            "the drain of the agents' events: {:?}",
            drained.as_ref().err().map(ToString::to_string)
        );
        let carrying: Vec<ConsoleFrame> = host
            .console_log_store()
            .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                mob_id: mob.cloned(),
                ..Default::default()
            })
            .await
            .expect("the read of one mob")
            .frames
            .into_iter()
            .filter(|frame| {
                frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                    && frame.payload.to_string().contains(text)
            })
            .collect();
        if !carrying.is_empty() || tokio::time::Instant::now() >= deadline {
            return carrying;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The session history of an agent of a mob the host serves beside its
/// default mob is stored in the host's console log under that mob. Two mobs
/// each have an agent `desk`, and each `desk` runs one turn with a task of
/// its own. A session's history holds the task its agent was given.
///
/// - The default mob's frames hold the launch mob's agent's task, from its
///   session's history, as on a host of one mob.
/// - The second mob's frames hold its agent's task, from that agent's
///   session's history: each such frame names the second mob and that
///   agent's session.
/// - Neither mob's frames hold the other agent's task.
/// - A console view's read named with the second mob's id shows the task.
#[tokio::test]
async fn a_second_mobs_agents_session_history_is_stored_under_that_mob() {
    const TASK_OF_THE_LAUNCH_DESK: &str = "a-task-only-the-launch-mobs-desk-was-given";
    const TASK_OF_THE_SECOND_DESK: &str = "a-task-only-the-second-mobs-desk-was-given";

    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let second_id = two_desks_with_a_frame_each(&host).await;
    let desk = meerkat_mob::AgentIdentity::from("desk");
    let launch = host.mob_runtime().handle();
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the mob set serves the second mob");
    let second_session = second
        .resolve_bridge_session_id(&desk)
        .await
        .expect("the second mob's agent's session")
        .to_string();

    run_a_turn_of_desk(&launch, TASK_OF_THE_LAUNCH_DESK).await;
    run_a_turn_of_desk(&second, TASK_OF_THE_SECOND_DESK).await;

    let mut wrong = Vec::new();
    let of_the_launch = drained_until_history_carries(&host, None, TASK_OF_THE_LAUNCH_DESK).await;
    if of_the_launch.is_empty() {
        wrong.push(
            "the default mob's frames hold no frame of the launch mob's agent's session history \
             that carries its task"
                .to_string(),
        );
    }
    let of_the_second =
        drained_until_history_carries(&host, Some(&second_id), TASK_OF_THE_SECOND_DESK).await;
    if of_the_second.is_empty() {
        wrong.push(
            "the second mob's frames hold no frame of its agent's session history that carries \
             its task"
                .to_string(),
        );
    }
    for frame in &of_the_second {
        if frame.mob_id.as_ref() != Some(&second_id)
            || frame.session_id.as_deref() != Some(second_session.as_str())
            || frame.identity != "desk"
        {
            wrong.push(format!(
                "a frame of the second mob's agent's session history does not name its mob, its \
                 session and its agent: mob {:?}, session {:?}, agent {}",
                frame.mob_id, frame.session_id, frame.identity
            ));
        }
    }
    for (named, mob, other_task) in [
        ("the default mob's", None, TASK_OF_THE_SECOND_DESK),
        (
            "the second mob's",
            Some(&second_id),
            TASK_OF_THE_LAUNCH_DESK,
        ),
    ] {
        let others: Vec<String> = host
            .console_log_store()
            .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                mob_id: mob.cloned(),
                ..Default::default()
            })
            .await
            .expect("the read of one mob")
            .frames
            .into_iter()
            .filter(|frame| frame.payload.to_string().contains(other_task))
            .map(|frame| frame.kind)
            .collect();
        if !others.is_empty() {
            wrong.push(format!(
                "{named} frames hold the other mob's agent's task: {others:?}"
            ));
        }
    }
    let shown = match host.console_projection.get() {
        Some(projection) => {
            let view = projection.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
            let _ = Box::pin(view.list_identities()).await;
            Box::pin(view.query_mob_timeline_windowed_for(
                ConsoleTimelineWindowQuery {
                    mob_id: Some(second_id.clone()),
                    identity: Some("desk".to_string()),
                    ..Default::default()
                },
                None,
            ))
            .await
            .map(|page| {
                page.frames
                    .iter()
                    .any(|frame| frame.payload.to_string().contains(TASK_OF_THE_SECOND_DESK))
            })
            .unwrap_or(false)
        }
        None => false,
    };
    if !shown {
        wrong.push(
            "a console view's read named with the second mob's id does not show its agent's task"
                .to_string(),
        );
    }

    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a second mob's agent's session history is not stored under that mob:\n{}",
        wrong.join("\n")
    );
}

/// A mob the host serves beside its default mob whose agents record
/// nothing and hold no session history leaves the console log where it
/// was. A second mob is created and resolved, an agent that waits for its
/// first turn is seated in it, and the agent's timeline is read by the
/// mob's id, which asks for its session's history. The SQLite log stays at
/// schema version 3 without the mob column and holds no frame of the mob,
/// also after the runtime shut down.
#[tokio::test]
async fn a_second_mobs_agent_that_never_ran_leaves_the_console_log_at_version_3() {
    let (temp, state) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let path = state.path().join("console-log-rows.sqlite");
    let host = console_host(&temp, Some(&path)).await;
    let second_id = second_mob(&host, definition("second")).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        // A turn-driven agent runs nothing until it is given a turn.
        let mut spec = SpawnMemberSpec::new("worker", "desk");
        spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        let seated = Box::pin(mob.spawn(spec)).await;
        assert!(
            seated.is_ok(),
            "an agent of the second mob that waits for its first turn: {:?}",
            seated.as_ref().err()
        );
    }
    let drained = host.drain_mob_agent_events().await;
    assert!(
        drained.is_ok(),
        "the drain of the agents' events: {:?}",
        drained.as_ref().err().map(ToString::to_string)
    );

    let mut wrong = Vec::new();
    match host.console_projection.get() {
        Some(projection) => {
            let view = projection.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
            let _ = Box::pin(view.list_identities()).await;
            // The read finds no history of the agent and asks for it.
            let read = Box::pin(view.query_mob_timeline_windowed_for(
                ConsoleTimelineWindowQuery {
                    mob_id: Some(second_id.clone()),
                    identity: Some("desk".to_string()),
                    ..Default::default()
                },
                None,
            ))
            .await
            .map(|page| page.frames.len());
            if !matches!(read, Ok(0)) {
                wrong.push(format!(
                    "the timeline of an agent that never ran is not empty: {:?}",
                    read.as_ref().map_err(|error| format!("{error:?}"))
                ));
            }
            if tokio::time::timeout(
                Duration::from_secs(10),
                projection.history_backfill_converged(),
            )
            .await
            .is_err()
            {
                wrong.push("the passes over the agent's session history did not end".to_string());
            }
        }
        None => wrong.push("the runtime has no console projection".to_string()),
    }
    let of_the_mob = host
        .console_log_store()
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(second_id.clone()),
            ..Default::default()
        })
        .await
        .expect("the read of one mob")
        .frames;
    if !of_the_mob.is_empty() {
        wrong.push(format!(
            "the console log holds frames of the mob: {:?}",
            of_the_mob
                .iter()
                .map(|frame| frame.kind.as_str())
                .collect::<Vec<_>>()
        ));
    }
    if ledger_version(&path) != Some(3) || has_the_mob_column(&path) {
        wrong.push(format!(
            "the console log moved: version {:?}, mob column {}",
            ledger_version(&path),
            has_the_mob_column(&path)
        ));
    }
    shut_down(&host, &[&second_id]).await;
    if ledger_version(&path) != Some(3) || has_the_mob_column(&path) {
        wrong.push(format!(
            "the console log moved at the shutdown: version {:?}, mob column {}",
            ledger_version(&path),
            has_the_mob_column(&path)
        ));
    }
    assert!(
        wrong.is_empty(),
        "a second mob's agent that never ran moved the console log:\n{}",
        wrong.join("\n")
    );
}

/// The console's administrator in the prime rows.
const PRIME_ROWS_ADMIN: &str = "admin@example.test";
/// A person whose one rule lets it view the agents of the default mob that
/// carry the label `team` = `second`. The agents of the mob the host
/// created carry it; no agent of the default mob does.
const PRIME_ROWS_PERSON: &str = "second-team@example.test";

/// A spec of the agent `name` that carries the label `team` = `team`.
fn of_the_team(name: &str, team: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", name);
    spec.labels = Some(std::collections::BTreeMap::from([(
        "team".to_string(),
        team.to_string(),
    )]));
    spec
}

/// The host of the prime rows, its access controller (a clone shares the
/// host's attribute cache) and the id of a second mob the host created.
/// The launch mob holds `desk` of the team `launch`; the second mob holds
/// `desk` and `courier` of the team `second`.
async fn host_for_the_prime_rows(
    temp: &tempfile::TempDir,
) -> (UnifiedRuntime, crate::access::AccessController, MobId) {
    use crate::access::{AccessControlConfig, AccessController, AccessRule};

    let mut host = console_host(temp, None).await;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![PRIME_ROWS_ADMIN.to_string()],
        rules: vec![AccessRule {
            id: "sees-the-agents-of-the-second-team".to_string(),
            subjects: vec![PRIME_ROWS_PERSON.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            match_labels: std::collections::BTreeMap::from([(
                "team".to_string(),
                "second".to_string(),
            )]),
            ..AccessRule::default()
        }],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    host.set_access_controller(controller.clone());
    let second_id = second_mob(&host, definition("second")).await;
    {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        for name in ["desk", "courier"] {
            Box::pin(second.spawn(of_the_team(name, "second")))
                .await
                .expect("an agent of the second mob");
        }
    }
    Box::pin(host.spawn(of_the_team("desk", "launch")))
        .await
        .expect("an agent of the launch mob");
    (host, controller, second_id)
}

/// What the attribute cache answers [`PRIME_ROWS_PERSON`] in the default
/// mob, written to `wrong` where it is not so: each name of `known` is an
/// agent the cache knows, no name of `not_known` is, and the person may
/// not view the launch mob's `desk` (it could only by the label the
/// second mob's `desk` carries).
fn faults_in_the_default_mobs_cache(
    controller: &crate::access::AccessController,
    when: &str,
    known: &[&str],
    not_known: &[&str],
    wrong: &mut Vec<String>,
) {
    let view = controller.view_for_subject(Some(PRIME_ROWS_PERSON));
    for name in known {
        if !view.knows_agent(name) {
            wrong.push(format!(
                "{when}: `{name}`, an agent of the launch mob or of a mob its agent made, is \
                 not known"
            ));
        }
    }
    for name in not_known {
        if view.knows_agent(name) {
            wrong.push(format!(
                "{when}: `{name}`, an agent of the mob the host created, is known as an agent \
                 of the default mob"
            ));
        }
    }
    if view.can_view_agent("desk") {
        wrong.push(format!(
            "{when}: the person may view the launch mob's `desk` by the label of the second \
             mob's `desk`"
        ));
    }
}

/// The console's read of the live roster into its attribute cache (the
/// prime every console route and stream makes) records no agent of a mob
/// the host created as an agent of the launch mob: not at the first read,
/// and not when the roster is read again after both mobs gained an agent.
/// A mob the launch mob's agent made is read as before: its agent is
/// known. The same read made for the host-created mob records that mob's
/// agents as its own, and a further read of the launch mob's roster leaves
/// them so.
#[tokio::test]
async fn the_consoles_prime_records_no_agent_of_a_host_created_mob_as_the_launch_mobs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    // For comparison, a mob the launch mob's `desk` made, with an agent.
    let owner_session = host
        .mob_runtime()
        .handle()
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("desk"))
        .await
        .expect("the launch mob's agent's session");
    let below_id = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(
            definition("below"),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the launch mob's agent made");
    let below = set_of(&host)
        .handle_for(&below_id)
        .await
        .expect("the mob set serves the agent's mob");
    Box::pin(below.spawn_spec(of_the_team("helper", "second")))
        .await
        .expect("an agent of the agent's mob");
    // The agent's mob's agent is named by that mob and its name.
    let helper = format!("{below_id}/helper");

    let mut wrong = Vec::new();
    crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;
    faults_in_the_default_mobs_cache(
        &controller,
        "after the prime",
        &["desk", helper.as_str()],
        &["courier"],
        &mut wrong,
    );

    // Both mobs gain an agent, and the roster is read again.
    let second_runtime = {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        Box::pin(second.spawn(of_the_team("porter", "second")))
            .await
            .expect("one more agent of the second mob");
        second.mob_runtime().clone()
    };
    Box::pin(host.spawn(of_the_team("clerk", "launch")))
        .await
        .expect("one more agent of the launch mob");
    crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;
    faults_in_the_default_mobs_cache(
        &controller,
        "after the roster was read again",
        &["desk", helper.as_str(), "clerk"],
        &["courier", "porter"],
        &mut wrong,
    );

    // The same read for the mob the host created: its agents are its own.
    // The launch mob's roster is then read once more: the read of one mob
    // leaves what was read of the other as it is.
    crate::http_console::prime_access_cache_from_runtime_of(
        &second_runtime,
        &controller,
        Some(&second_id),
    )
    .await;
    for when in [
        "after the read for the host-created mob",
        "after the launch mob's roster was read once more",
    ] {
        let in_the_second = controller
            .view_for_subject(Some(PRIME_ROWS_PERSON))
            .in_stored_mob(Some(&second_id));
        for name in ["desk", "courier", "porter"] {
            if !in_the_second.knows_agent(name) {
                wrong.push(format!(
                    "{when}: the host-created mob's agent `{name}` is not known under that mob"
                ));
            }
        }
        faults_in_the_default_mobs_cache(
            &controller,
            when,
            &["desk", helper.as_str(), "clerk"],
            &["courier", "porter"],
            &mut wrong,
        );
        crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;
    }

    shut_down(&host, &[&below_id, &second_id]).await;
    assert!(
        wrong.is_empty(),
        "the console's prime took an agent of a host-created mob for the launch mob's:\n{}",
        wrong.join("\n")
    );
}

/// The console's read of the live roster for a mob the host created records
/// that mob's agents under it and no agent of another mob: an agent only
/// the launch mob has is not known under the host-created mob. The row
/// first reads that the launch's record of its members holds that agent
/// and none of the host-created mob's: without it the read has nothing of
/// another mob to record.
#[tokio::test]
async fn the_consoles_prime_for_a_host_created_mob_records_no_agent_of_the_launch_mob_under_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    Box::pin(host.spawn(of_the_team("clerk", "launch")))
        .await
        .expect("an agent only the launch mob has");
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .mob_runtime()
        .clone();

    let mut wrong = Vec::new();
    let owners = host.mob_runtime().private_owners();
    owners
        .first_attempts_settled(crate::private_owners::FIRST_READ_WAIT)
        .await;
    let recorded = owners.memory_members();
    if !recorded.contains_key("clerk") {
        wrong.push(format!(
            "the launch's record of its members does not hold `clerk` here, so the row shows \
             nothing of the read: it holds {:?}",
            recorded.keys().collect::<Vec<_>>()
        ));
    }
    if recorded.contains_key("courier") {
        wrong.push(
            "the launch's record of its members holds `courier`, an agent of the host-created mob"
                .to_string(),
        );
    }

    crate::http_console::prime_access_cache_from_runtime_of(
        &second_runtime,
        &controller,
        Some(&second_id),
    )
    .await;
    let in_the_second = controller
        .view_for_subject(Some(PRIME_ROWS_PERSON))
        .in_stored_mob(Some(&second_id));
    for name in ["desk", "courier"] {
        if !in_the_second.knows_agent(name) {
            wrong.push(format!(
                "the host-created mob's agent `{name}` is not known under that mob"
            ));
        }
    }
    if in_the_second.knows_agent("clerk") {
        wrong.push(
            "the launch mob's `clerk`, which the host-created mob has no agent named like, is \
             known under the host-created mob"
                .to_string(),
        );
    }

    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "the console's prime for a host-created mob recorded another mob's agent under it:\n{}",
        wrong.join("\n")
    );
}

/// A mob created under the id of a destroyed mob is judged by none of the
/// attribute entries the console read of the earlier mob. A destroy keeps
/// them. The create forgets them,
/// before the mob is built: a name only the earlier mob held is then not
/// known under the id, and a caller the console keeps apart from other
/// people is withheld it, as it is any member the cache does not know;
/// a name the new mob holds too is not known either until the new mob's
/// own roster is read, and is then judged by the new agent's own label,
/// which here is not one the person may view where the earlier agent's
/// was.
/// Another mob's entries, and the launch mob's, stay as they are.
///
/// The second of the two things the announcements' audience asks of a
/// member of another mob is read here on the view itself
/// ([`crate::access::AccessView::withholds_unknown_member`] of the view of
/// that mob); no announcement is made in this row.
#[tokio::test]
async fn a_mob_created_under_a_destroyed_mobs_id_is_judged_by_none_of_the_earlier_mobs_attribute_entries()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    // In this row the person's rule selects the agents of the team
    // `second` in every mob: the fixture's rule names no mob, and so
    // selects the default mob's agents alone.
    controller
        .replace_config(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![PRIME_ROWS_ADMIN.to_string()],
            rules: vec![crate::access::AccessRule {
                id: "sees-the-agents-of-the-second-team-in-every-mob".to_string(),
                subjects: vec![PRIME_ROWS_PERSON.to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                match_labels: std::collections::BTreeMap::from([(
                    "team".to_string(),
                    "second".to_string(),
                )]),
                mobs: vec!["*".to_string()],
                ..crate::access::AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration");
    let second_definition = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .mob_runtime()
        .handle()
        .definition()
        .clone();
    // Another mob of the host with an agent of its own.
    let third_id = second_mob(&host, definition("third")).await;
    {
        let third = host
            .resolve_mob(Some(&third_id))
            .await
            .expect("the third mob of the host resolves");
        Box::pin(third.spawn(of_the_team("scout", "second")))
            .await
            .expect("an agent of the third mob");
        crate::http_console::prime_access_cache_from_runtime_of(
            third.mob_runtime(),
            &controller,
            Some(&third_id),
        )
        .await;
    }
    {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        crate::http_console::prime_access_cache_from_runtime_of(
            second.mob_runtime(),
            &controller,
            Some(&second_id),
        )
        .await;
    }
    crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;

    // The person's view of a mob, on a console that identifies its people:
    // such a caller is kept apart from other people.
    let person_in = |mob_id: &MobId| {
        controller
            .view_for_subject(Some(PRIME_ROWS_PERSON))
            .on_console(true)
            .in_stored_mob(Some(mob_id))
    };
    // What the person's view of `mob_id` answers for `name`: whether the
    // cache knows the agent, whether the person may view it, and whether
    // the person is withheld it as a member the cache does not know.
    let answers = |mob_id: &MobId, name: &str| {
        let view = person_in(mob_id);
        (
            view.knows_agent(name),
            view.can_view_agent(name),
            view.withholds_unknown_member(name),
        )
    };
    // (known, may view, withheld as unknown): an agent of the team the
    // person may view; an agent of another team; a name the cache does not
    // know.
    let known_and_seen = (true, true, false);
    let known_and_not_seen = (true, false, false);
    let not_known = (false, false, true);
    let mut wrong = Vec::new();
    let mut read =
        |when: &str, what: &str, found: (bool, bool, bool), wanted: (bool, bool, bool)| {
            if found != wanted {
                wrong.push(format!(
                    "{when}: {what}: (known, may view, withheld as unknown) is {found:?}, not \
                 {wanted:?}"
                ));
            }
        };

    // The scene, read before anything is destroyed: without these entries
    // the row shows nothing of what a create forgets.
    for name in ["desk", "courier"] {
        read(
            "before the destroy",
            &format!("the second mob's `{name}`"),
            answers(&second_id, name),
            known_and_seen,
        );
    }
    read(
        "before the destroy",
        "the third mob's `scout`",
        answers(&third_id, "scout"),
        known_and_seen,
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    // The readings after the destroy.
    for name in ["desk", "courier"] {
        read(
            "after the destroy",
            &format!("the destroyed mob's `{name}`"),
            answers(&second_id, name),
            known_and_seen,
        );
    }

    let created = Box::pin(set_of(&host).mob_create_definition(second_definition))
        .await
        .map_err(|error| error.to_string());
    assert_eq!(created, Ok(second_id.clone()), "a mob under the same id");
    // The new mob holds a `desk` of another team and no `courier`. Its
    // roster is not read yet.
    let new_runtime = {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the mob created under the id resolves");
        Box::pin(second.spawn(of_the_team("desk", "new")))
            .await
            .expect("an agent of the new mob");
        second.mob_runtime().clone()
    };
    read(
        "after the create, before the new mob's roster is read",
        "`courier`, a name only the earlier mob held",
        answers(&second_id, "courier"),
        not_known,
    );
    read(
        "after the create, before the new mob's roster is read",
        "`desk`, a name the new mob holds too",
        answers(&second_id, "desk"),
        not_known,
    );

    // The new mob's roster is read: its own agent is known, by its own
    // label, which is not the earlier `desk`'s; the earlier mob's `courier`
    // is still no agent of it.
    crate::http_console::prime_access_cache_from_runtime_of(
        &new_runtime,
        &controller,
        Some(&second_id),
    )
    .await;
    read(
        "after the new mob's roster was read",
        "the new mob's `desk`, of a team the person may not view",
        answers(&second_id, "desk"),
        known_and_not_seen,
    );
    read(
        "after the new mob's roster was read",
        "`courier`, a name only the earlier mob held",
        answers(&second_id, "courier"),
        not_known,
    );
    // The other mobs' entries stay.
    read(
        "after the create",
        "the third mob's `scout`",
        answers(&third_id, "scout"),
        known_and_seen,
    );
    if !controller
        .view_for_subject(Some(PRIME_ROWS_ADMIN))
        .knows_agent("desk")
    {
        wrong.push("after the create: the launch mob's `desk` is not known".to_string());
    }

    drop(new_runtime);
    shut_down(&host, &[&second_id, &third_id]).await;
    assert!(
        wrong.is_empty(),
        "a mob created under a destroyed mob's id met the earlier mob's attribute entries:\n{}",
        wrong.join("\n")
    );
}

/// A create that a release refuses leaves what the access controller holds
/// of the id as it was, the attribute entries and the current runtime
/// aliases: the release that refuses here is bound to the launch's record
/// after the launch's own, and so is asked after them. The same create,
/// once that release passes, forgets them.
#[tokio::test]
async fn a_create_a_later_release_refuses_leaves_the_access_controllers_entries_and_aliases_of_the_id()
 {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A release that refuses while it is told to.
    struct Refusing(AtomicBool);
    #[async_trait::async_trait]
    impl crate::private_owners::MobCreateRelease for Refusing {
        fn what(&self) -> &'static str {
            "what the row refuses"
        }
        async fn release_before_create(&self, _mob_id: &MobId) -> Result<(), String> {
            if self.0.load(Ordering::SeqCst) {
                Err("the row refuses this create".to_string())
            } else {
                Ok(())
            }
        }
    }
    const ALIAS: &str = "rt:desk:1";

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    let second_definition = {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        crate::http_console::prime_access_cache_from_runtime_of(
            second.mob_runtime(),
            &controller,
            Some(&second_id),
        )
        .await;
        second.mob_runtime().handle().definition().clone()
    };
    controller.record_current_runtime_aliases_in(
        &second_id,
        [("desk".to_string(), Some(ALIAS.to_string()))],
    );
    // What the controller holds of the second mob's id: whether `desk` and
    // `courier` are known there, and how many links the alias names (none
    // unless it is the current alias of an agent recorded there).
    let held = || {
        let view = controller
            .view_for_subject(Some(PRIME_ROWS_ADMIN))
            .in_stored_mob(Some(&second_id));
        (
            view.knows_agent("desk"),
            view.knows_agent("courier"),
            view.lineage_links(ALIAS),
        )
    };
    let scene = held();

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id))
        .await
        .map_err(|error| error.to_string());
    let refusing = Arc::new(Refusing(AtomicBool::new(true)));
    host.mob_runtime()
        .private_owners()
        .bind_create_release(Arc::clone(&refusing) as _);
    let refused = Box::pin(set_of(&host).mob_create_definition(second_definition.clone()))
        .await
        .map_err(|error| error.to_string());
    let after_the_refusal = held();

    refusing.0.store(false, Ordering::SeqCst);
    let created = Box::pin(set_of(&host).mob_create_definition(second_definition))
        .await
        .map_err(|error| error.to_string());
    let after_the_create = held();
    shut_down(&host, &[&second_id]).await;

    let mut wrong = Vec::new();
    let scene_stands = scene.0 && scene.1 && scene.2 > 0;
    if !scene_stands {
        wrong.push(format!(
            "the scene: (`desk` known, `courier` known, links of the alias) is {scene:?}: the \
             row shows nothing without the two entries and the alias"
        ));
    }
    if let Err(error) = &destroyed {
        wrong.push(format!("the set destroys the second mob: {error}"));
    }
    if !matches!(
        &refused,
        Err(error) if error.contains(crate::private_owners::CREATE_RELEASE_FAILED)
    ) {
        wrong.push(format!(
            "the create the release refuses is refused as a release that failed: {refused:?}"
        ));
    }
    if after_the_refusal != scene {
        wrong.push(format!(
            "after the refused create: (`desk` known, `courier` known, links of the alias) is \
             {after_the_refusal:?}, not {scene:?} as before it"
        ));
    }
    if created.as_ref() != Ok(&second_id) {
        wrong.push(format!(
            "the create goes through once the release passes: {created:?}"
        ));
    }
    if after_the_create != (false, false, 0) {
        wrong.push(format!(
            "after the admitted create: (`desk` known, `courier` known, links of the alias) is \
             {after_the_create:?}, not forgotten"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a create a later release refuses, and the same create admitted:\n{}",
        wrong.join("\n")
    );
}

/// The console's read of the live roster for a mob the host created records
/// the agents of that mob that left its roster: an agent of the
/// host-created mob that was retired is still known under that mob after
/// the read, as an ended agent of the launch mob is known in the launch
/// mob's read, and it is not known as an agent of the default mob.
#[tokio::test]
async fn the_consoles_prime_for_a_host_created_mob_records_its_own_ended_agent_under_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .mob_runtime()
        .clone();
    // The second mob's `courier` ends before the roster is read at all.
    let courier = meerkat_mob::AgentIdentity::from("courier");
    second_runtime
        .handle()
        .retire(courier.clone())
        .await
        .expect("the second mob's courier is retired");
    let mut left = false;
    for _ in 0..100 {
        if second_runtime
            .handle()
            .get_member(&courier)
            .await
            .expect("the second mob's roster is read")
            .is_none()
        {
            left = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(left, "the second mob's courier left its roster");

    let mut wrong = Vec::new();
    crate::http_console::prime_access_cache_from_runtime_of(
        &second_runtime,
        &controller,
        Some(&second_id),
    )
    .await;
    crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;
    let person = controller.view_for_subject(Some(PRIME_ROWS_PERSON));
    let in_the_second = person.in_stored_mob(Some(&second_id));
    if !in_the_second.knows_agent("desk") {
        wrong.push(
            "the host-created mob's live agent `desk` is not known under that mob".to_string(),
        );
    }
    if !in_the_second.knows_agent("courier") {
        wrong.push(
            "the host-created mob's ended agent `courier` is not known under that mob".to_string(),
        );
    }
    if person.knows_agent("courier") {
        wrong.push(
            "the host-created mob's ended agent `courier` is known as an agent of the default mob"
                .to_string(),
        );
    }

    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "the console's prime for a host-created mob did not record that mob's ended agent:\n{}",
        wrong.join("\n")
    );
}

/// The console's read of the live roster for a mob the host created takes
/// what the spawn registry holds for that mob: a name a spawn by the
/// launch mob's agent registered is not recorded under the host-created
/// mob, and the labels registered with it are not given to the
/// host-created mob's agent of the same name. The launch mob's own read
/// records the registered name as before.
#[tokio::test]
async fn the_consoles_prime_for_a_host_created_mob_takes_no_name_the_launch_mobs_spawns_registered()
{
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    // In this row the person's rule selects the agents of the team
    // `second` in every mob: the fixture's rule names no mob, and so
    // selects the default mob's agents alone.
    controller
        .replace_config(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![PRIME_ROWS_ADMIN.to_string()],
            rules: vec![crate::access::AccessRule {
                id: "sees-the-agents-of-the-second-team-in-every-mob".to_string(),
                subjects: vec![PRIME_ROWS_PERSON.to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                match_labels: std::collections::BTreeMap::from([(
                    "team".to_string(),
                    "second".to_string(),
                )]),
                mobs: vec!["*".to_string()],
                ..crate::access::AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration");
    let second_runtime = {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        // An agent of the second mob that carries no team: the person's
        // rule does not let it view this one.
        Box::pin(second.spawn(SpawnMemberSpec::new("worker", "porter")))
            .await
            .expect("an agent of the second mob without a team");
        second.mob_runtime().clone()
    };
    // What a spawn by the launch mob's `desk` registers: `scout`, a name no
    // roster holds, and `porter`, a name the second mob's agent has, each
    // with the team the person may view.
    let registry = host
        .mob_runtime()
        .console_spawn_sink()
        .expect("the launch's spawn registry");
    for name in ["scout", "porter"] {
        registry
            .project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
                mob_id: Some(host.mob_runtime().handle().mob_id().to_string()),
                member_id: name.to_string(),
                identity: name.to_string(),
                initial_message: None,
                labels: std::collections::BTreeMap::from([(
                    "team".to_string(),
                    "second".to_string(),
                )]),
                spawned_by: Some("desk".to_string()),
                spawned_by_session: None,
                session_id: None,
                via_tool: "mob_spawn_member".to_string(),
            })
            .await;
    }

    let mut wrong = Vec::new();
    crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;
    if !controller
        .view_for_subject(Some(PRIME_ROWS_PERSON))
        .knows_agent("scout")
    {
        wrong.push(
            "the launch mob's read does not know `scout`, a name its agent's spawn registered, \
             so the row shows nothing of the registry"
                .to_string(),
        );
    }
    crate::http_console::prime_access_cache_from_runtime_of(
        &second_runtime,
        &controller,
        Some(&second_id),
    )
    .await;
    let in_the_second = controller
        .view_for_subject(Some(PRIME_ROWS_PERSON))
        .in_stored_mob(Some(&second_id));
    if !in_the_second.can_view_agent("courier") {
        wrong.push(
            "the person may not view the host-created mob's `courier`, an agent of its team"
                .to_string(),
        );
    }
    if in_the_second.knows_agent("scout") {
        wrong.push(
            "`scout`, a name a spawn of the launch mob's agent registered, is known under the \
             host-created mob"
                .to_string(),
        );
    }
    if !in_the_second.knows_agent("porter") {
        wrong.push("the host-created mob's agent `porter` is not known under that mob".to_string());
    }
    if in_the_second.can_view_agent("porter") {
        wrong.push(
            "the person may view the host-created mob's `porter` by the labels a spawn of the \
             launch mob's agent registered for that name"
                .to_string(),
        );
    }

    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "the console's prime for a host-created mob took what another mob's spawns \
         registered:\n{}",
        wrong.join("\n")
    );
}

/// A dispatcher that stands in for meerkat's mob tools: every call answers
/// that it seated the member it names.
struct SeatsAMember(&'static str);

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for SeatsAMember {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        Vec::<Arc<meerkat_core::types::ToolDef>>::new().into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            meerkat_core::types::ToolResult::new(
                call.id.to_string(),
                json!({ "agent_identity": self.0, "member_ref": "a-member-ref" }).to_string(),
                false,
            ),
        ))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

/// One `mob_spawn_member` call by the agent of the member `spawner` of the
/// mob `of`, through the tool surface the launch builds for that member:
/// the call seats `member` with the label `team` = `team`, and is
/// registered with the console as that agent's spawn is.
async fn an_agent_spawns(
    host: &UnifiedRuntime,
    of: &meerkat_mob::MobHandle,
    spawner: &str,
    member: &'static str,
    team: &str,
) {
    an_agent_spawns_through(host, of, spawner, member, team, true).await;
}

/// [`an_agent_spawns`]; with `its_mob_can_be_read` false the spawning
/// agent's tool surface has lost its mob tool state, so nothing of its
/// member's mob can be read when the spawn is registered.
async fn an_agent_spawns_through(
    host: &UnifiedRuntime,
    of: &meerkat_mob::MobHandle,
    spawner: &str,
    member: &'static str,
    team: &str,
    its_mob_can_be_read: bool,
) {
    let session = of
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(spawner))
        .await
        .expect("the spawning agent's session");
    let mob_id = of.mob_id().to_string();
    let tools = crate::mob_handle_runtime::agent_mob_tool_wrapper_with_console_for_test(
        Arc::new(SeatsAMember(member)),
        host.mob_runtime()
            .implicit_delegate_retirement_overrides()
            .expect("the launch installs the agent mob tools"),
        host.mob_runtime(),
        session.clone(),
        &format!("{mob_id}/worker/{spawner}"),
        its_mob_can_be_read,
    );
    let args = serde_json::value::RawValue::from_string(
        json!({
            "mob_id": mob_id,
            "profile": "worker",
            "member_id": member,
            "labels": { "team": team },
        })
        .to_string(),
    )
    .expect("the arguments of a spawn");
    tools
        .dispatch_with_context(
            meerkat_core::types::ToolCallView {
                id: "a-spawn-call",
                name: "mob_spawn_member",
                args: &args,
            },
            &meerkat_core::ToolDispatchContext::default().with_runtime_identity(session, None),
        )
        .await
        .expect("the spawn is answered");
}

/// What a console holds for a name: the labels spawns registered for it,
/// and the console identity its runtime member id is mapped to.
type HeldForAName = (
    Option<std::collections::BTreeMap<String, String>>,
    Option<String>,
);

/// What a console event store holds for the name `name`: the labels spawns
/// registered for it, and the console identity its runtime member id is
/// mapped to. The spawn registry keeps an entry under the mob its spawn
/// seated the member in; in these rows a store holds a name under one mob
/// at most, and a store that holds it under two fails the row here.
async fn registered_for(
    store: &crate::unified_runtime::ConsoleEventStore,
    name: &str,
) -> HeldForAName {
    let registry = store.identity_labels_snapshot().await;
    let held: Vec<(String, std::collections::BTreeMap<String, String>)> = registry
        .mobs()
        .into_iter()
        .filter_map(|mob_id| {
            let labels = registry.labels_of_row(&mob_id, name).cloned()?;
            Some((mob_id, labels))
        })
        .collect();
    assert!(
        held.len() <= 1,
        "one console holds `{name}` as registered by spawns into more than one mob: {held:?}"
    );
    (
        held.into_iter().next().map(|(_, labels)| labels),
        store.registered_runtime_identity(name).await,
    )
}

/// The name a console holds a member of a mob a member made under: its mob
/// and its name. A member of the launch mob, or of a mob the host created,
/// keeps its bare name in its own mob's console.
fn named_in(mob_id: &MobId, name: &str) -> String {
    crate::console_member_ref::nested_member_identity(mob_id.as_str(), name)
        .expect("a mob id and a name make the console name of a member of a member-made mob")
}

/// What a console event store holds of the member `name` under ANY
/// spelling: every name it holds a spawn's registration under that is the
/// member's bare name or a mob and that name, and the console identity the
/// member's runtime member id is mapped to.
///
/// A console holds a spawned member under its bare name where the member's
/// mob is the console's own, and under its mob and its name where it is any
/// other mob. So a lookup for ABSENCE asks here: a console that wrongly held
/// the member is found whichever of the two it used, and the row does not
/// rest on a name that console holds nothing under.
async fn held_under_any_spelling(
    store: &crate::unified_runtime::ConsoleEventStore,
    name: &str,
) -> (Vec<String>, Option<String>) {
    let registry = store.identity_labels_snapshot().await;
    let names = registry
        .identities()
        .filter(|held| {
            *held == name
                || crate::console_member_ref::parse_nested_member_identity(held)
                    .is_some_and(|(_, member)| member == name)
        })
        .map(str::to_string)
        .collect();
    (names, store.registered_runtime_identity(name).await)
}

/// The person of the prime rows gets a rule that selects the agents of the
/// team `second` in every mob: the fixture's rule names no mob, and so
/// selects the default mob's agents alone.
fn the_person_views_the_second_team_in_every_mob(controller: &crate::access::AccessController) {
    controller
        .replace_config(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![PRIME_ROWS_ADMIN.to_string()],
            rules: vec![crate::access::AccessRule {
                id: "sees-the-agents-of-the-second-team-in-every-mob".to_string(),
                subjects: vec![PRIME_ROWS_PERSON.to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                match_labels: std::collections::BTreeMap::from([(
                    "team".to_string(),
                    "second".to_string(),
                )]),
                mobs: vec!["*".to_string()],
                ..crate::access::AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration");
}

/// A spawn by an agent of a mob the host created is registered with that
/// mob's console and with no other mob's. The launch mob's console holds a
/// name its own agent's spawn registered, and a console identity for that
/// name's runtime member id; an agent of the host-created mob then spawns a
/// member of the same name. The launch mob's labels, lineage and mapping
/// for the name are as before, and so is what a person may view of it in
/// the launch mob; the host-created mob's console holds what its own
/// agent's spawn registered, and that mob's read records the name.
#[tokio::test]
async fn a_spawn_by_a_host_created_mobs_agent_is_registered_with_that_mobs_console_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    the_person_views_the_second_team_in_every_mob(&controller);
    let second_runtime = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .mob_runtime()
        .clone();
    let of_the_launch = host.console_events();
    let of_the_second = of_the_launch.of_mob(Some(&second_id));

    let mut wrong = Vec::new();
    an_agent_spawns(
        &host,
        &host.mob_runtime().handle(),
        "desk",
        "scout",
        "launch",
    )
    .await;
    of_the_launch
        .register_runtime_identity("scout", "chief")
        .await;
    let before = registered_for(&of_the_launch, "scout").await;
    // And under every spelling: the host-created mob's member of the name
    // would be held here under its mob and its name.
    let spelled_before = held_under_any_spelling(&of_the_launch, "scout").await.0;
    if before.0.is_none() {
        wrong.push(
            "the launch mob's console holds nothing for `scout` after its own agent's spawn, so \
             the row shows nothing"
                .to_string(),
        );
    }
    crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;
    let answer = |controller: &crate::access::AccessController| {
        let person = controller.view_for_subject(Some(PRIME_ROWS_PERSON));
        (person.knows_agent("scout"), person.can_view_agent("scout"))
    };
    let answered_before = answer(&controller);

    // An agent of the host-created mob spawns a member of that name, of
    // the team the person may view. The roster of the host-created mob
    // holds that member, so that mob's read decides it by its row and by
    // what its spawn registered: a registered name no roster holds is read
    // as an ended member, by none of the registered labels.
    {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        Box::pin(second.spawn(SpawnMemberSpec::new("worker", "scout")))
            .await
            .expect("the member the second mob's agent spawns");
    }
    an_agent_spawns(
        &host,
        &second_runtime.handle(),
        "courier",
        "scout",
        "second",
    )
    .await;

    let after = registered_for(&of_the_launch, "scout").await;
    if after != before {
        wrong.push(format!(
            "the launch mob's console holds other labels or another mapping for `scout` after \
             a spawn by the host-created mob's agent: before {before:?}, after {after:?}"
        ));
    }
    let spelled_after = held_under_any_spelling(&of_the_launch, "scout").await.0;
    if spelled_after != spelled_before {
        wrong.push(format!(
            "the launch mob's console holds `scout` under another name after a spawn by the \
             host-created mob's agent: before {spelled_before:?}, after {spelled_after:?}"
        ));
    }
    crate::http_console::prime_access_cache_from_runtime(host.mob_runtime(), &controller).await;
    let answered_after = answer(&controller);
    if answered_after != answered_before {
        wrong.push(format!(
            "what the person knows and may view of the launch mob's `scout` changed with a \
             spawn by the host-created mob's agent: before {answered_before:?}, after \
             {answered_after:?}"
        ));
    }
    let own = registered_for(&of_the_second, "scout").await.0;
    let own_is_the_spawns = own.as_ref().is_some_and(|labels| {
        labels.get("team").map(String::as_str) == Some("second")
            && labels
                .get(crate::console_spawn::SPAWNED_BY_LABEL)
                .map(String::as_str)
                == Some("courier")
    });
    if !own_is_the_spawns {
        wrong.push(format!(
            "the host-created mob's console does not hold what its own agent's spawn \
             registered for `scout`: {own:?}"
        ));
    }
    crate::http_console::prime_access_cache_from_runtime_of(
        &second_runtime,
        &controller,
        Some(&second_id),
    )
    .await;
    let in_the_second = controller
        .view_for_subject(Some(PRIME_ROWS_PERSON))
        .in_stored_mob(Some(&second_id));
    if !in_the_second.knows_agent("scout") || !in_the_second.can_view_agent("scout") {
        wrong.push(
            "the host-created mob's read does not record the name its own agent's spawn \
             registered, with its labels"
                .to_string(),
        );
    }

    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a spawn by an agent of a host-created mob was not registered with that mob's console \
         alone:\n{}",
        wrong.join("\n")
    );
}

/// A spawn by an agent of a mob that a member of a host-created mob made is
/// registered with the host-created mob's console, where that mob's read
/// looks for it, and not with the launch mob's.
#[tokio::test]
async fn a_spawn_by_an_agent_of_a_mob_below_a_host_created_mob_is_registered_with_that_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _controller, second_id) = host_for_the_prime_rows(&temp).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the mob set serves the second mob");
    let owner_session = second
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("courier"))
        .await
        .expect("the second mob's agent's session");
    let below_id = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(
            definition("below-the-second"),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the second mob's agent made");
    let below = set_of(&host)
        .handle_for(&below_id)
        .await
        .expect("the mob set serves the agent's mob");
    Box::pin(below.spawn_spec(of_the_team("runner", "second")))
        .await
        .expect("an agent of the agent's mob");
    let of_the_launch = host.console_events();
    let of_the_second = of_the_launch.of_mob(Some(&second_id));

    an_agent_spawns(&host, &below, "runner", "page", "second").await;

    // The spawned member sits in a mob a member made: a console holds it
    // under its mob and its name.
    let page = named_in(&below_id, "page");
    let mut wrong = Vec::new();
    if let Some(labels) = registered_for(&of_the_launch, &page).await.0 {
        wrong.push(format!(
            "the launch mob's console holds what a spawn below the host-created mob \
             registered: {labels:?}"
        ));
    }
    if registered_for(&of_the_second, &page).await.0.is_none() {
        wrong.push(
            "the host-created mob's console does not hold what a spawn below it registered"
                .to_string(),
        );
    }

    shut_down(&host, &[&below_id, &second_id]).await;
    assert!(
        wrong.is_empty(),
        "a spawn below a host-created mob was not registered with that mob's console:\n{}",
        wrong.join("\n")
    );
}

/// A spawn by an agent of the launch mob, and one by an agent of a mob the
/// launch mob's agent made, are registered with the launch mob's console as
/// before, and with no other mob's.
#[tokio::test]
async fn a_spawn_by_the_launch_mobs_agents_is_registered_with_the_launch_mobs_console_as_before() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _controller, second_id) = host_for_the_prime_rows(&temp).await;
    let owner_session = host
        .mob_runtime()
        .handle()
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("desk"))
        .await
        .expect("the launch mob's agent's session");
    let below_id = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(
            definition("below"),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the launch mob's agent made");
    let below = set_of(&host)
        .handle_for(&below_id)
        .await
        .expect("the mob set serves the agent's mob");
    Box::pin(below.spawn_spec(of_the_team("helper", "launch")))
        .await
        .expect("an agent of the agent's mob");
    let of_the_launch = host.console_events();
    let of_the_second = of_the_launch.of_mob(Some(&second_id));

    an_agent_spawns(
        &host,
        &host.mob_runtime().handle(),
        "desk",
        "scout",
        "launch",
    )
    .await;
    an_agent_spawns(&host, &below, "helper", "page", "launch").await;

    // `scout` sits in the launch mob and keeps its bare name. `page` sits in
    // a mob the launch mob's agent made: the console holds it under its mob
    // and its name, spawned by `helper` of that mob, and maps no runtime
    // member id to it (its runtime id is its bare name, which a member of
    // the launch mob may hold).
    let page = named_in(&below_id, "page");
    let helper = named_in(&below_id, "helper");
    let mut wrong = Vec::new();
    for (name, spawner, its_runtime_id_is_mapped) in [
        ("scout", "desk", true),
        (page.as_str(), helper.as_str(), false),
    ] {
        let held = registered_for(&of_the_launch, name).await;
        let is_the_spawns = held.0.as_ref().is_some_and(|labels| {
            labels.get("team").map(String::as_str) == Some("launch")
                && labels
                    .get(crate::console_spawn::SPAWNED_BY_LABEL)
                    .map(String::as_str)
                    == Some(spawner)
        });
        if !is_the_spawns || (its_runtime_id_is_mapped && held.1.as_deref() != Some(name)) {
            wrong.push(format!(
                "the launch mob's console does not hold the spawn of `{name}` by `{spawner}` \
                 as before: {held:?}"
            ));
        }
    }
    // The host-created mob's console holds neither member under any
    // spelling: it would hold a member of another mob under that mob and
    // its name, not under the bare name.
    for member in ["scout", "page"] {
        let held = held_under_any_spelling(&of_the_second, member).await.0;
        if !held.is_empty() {
            wrong.push(format!(
                "the host-created mob's console holds the launch mob's spawn of `{member}`: \
                 {held:?}"
            ));
        }
    }

    shut_down(&host, &[&below_id, &second_id]).await;
    assert!(
        wrong.is_empty(),
        "a spawn by the launch mob's agents was not registered as before:\n{}",
        wrong.join("\n")
    );
}

/// A mob the host created is read as a mob beside the launch mob from its
/// own records, whether or not this process has named it since it started:
/// with no mob named (as after a start over the same stores, before any
/// mob beside the launch mob was built), a spawn by an agent of the
/// host-created mob, and one by an agent of a mob its member made, are
/// registered with the host-created mob's console and not with the launch
/// mob's.
#[tokio::test]
async fn a_spawn_in_or_below_a_host_created_mob_no_one_named_is_registered_with_that_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _controller, second_id) = host_for_the_prime_rows(&temp).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the mob set serves the second mob");
    let owner_session = second
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("courier"))
        .await
        .expect("the second mob's agent's session");
    let below_id = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(
            definition("below-the-second"),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the second mob's agent made");
    let below = set_of(&host)
        .handle_for(&below_id)
        .await
        .expect("the mob set serves the agent's mob");
    Box::pin(below.spawn_spec(of_the_team("runner", "second")))
        .await
        .expect("an agent of the agent's mob");
    let of_the_launch = host.console_events();
    let of_the_second = of_the_launch.of_mob(Some(&second_id));

    // No mob is named beside the launch mob any more.
    host.mob_runtime()
        .private_owners()
        .hosted_mobs()
        .name_none_in_tests();
    an_agent_spawns(&host, &second, "courier", "scout", "second").await;
    an_agent_spawns(&host, &below, "runner", "page", "second").await;

    // `scout` sits in the host-created mob and keeps its bare name in that
    // mob's console; `page` sits in a mob that mob's agent made, and a
    // console holds it under its mob and its name. The launch mob's console
    // would hold either under its mob and its name, so it is asked for the
    // member under every spelling.
    let page = named_in(&below_id, "page");
    let mut wrong = Vec::new();
    for (member, name) in [("scout", "scout"), ("page", page.as_str())] {
        let held = held_under_any_spelling(&of_the_launch, member).await.0;
        if !held.is_empty() {
            wrong.push(format!(
                "the launch mob's console holds the spawn of `{member}`, made in or below the \
                 host-created mob: {held:?}"
            ));
        }
        if registered_for(&of_the_second, name).await.0.is_none() {
            wrong.push(format!(
                "the host-created mob's console does not hold the spawn of `{name}`"
            ));
        }
    }

    shut_down(&host, &[&below_id, &second_id]).await;
    assert!(
        wrong.is_empty(),
        "a spawn in or below a host-created mob that no one named was not registered with \
         that mob's console:\n{}",
        wrong.join("\n")
    );
}

/// On a host with no mob beside its launch mob a spawn is registered with
/// the launch mob's console also where nothing of the spawning member's
/// mob can be read: such a host keeps every registration it kept before
/// spawns were registered by mob.
#[tokio::test]
async fn on_a_host_of_one_mob_a_spawn_whose_mob_cannot_be_read_is_registered_as_before() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    Box::pin(host.spawn(of_the_team("desk", "launch")))
        .await
        .expect("an agent of the launch mob");
    let of_the_launch = host.console_events();

    an_agent_spawns_through(
        &host,
        &host.mob_runtime().handle(),
        "desk",
        "scout",
        "launch",
        false,
    )
    .await;

    let held = registered_for(&of_the_launch, "scout").await;
    shut_down(&host, &[]).await;
    let is_the_spawns = held.0.as_ref().is_some_and(|labels| {
        labels.get("team").map(String::as_str) == Some("launch")
            && labels
                .get(crate::console_spawn::SPAWNED_BY_LABEL)
                .map(String::as_str)
                == Some("desk")
    });
    assert!(
        is_the_spawns && held.1.as_deref() == Some("scout"),
        "on a host with no mob beside its launch mob, a spawn whose member's mob cannot be \
         read is not registered with the launch mob's console as before: {held:?}"
    );
}

/// On a host that serves a mob beside its launch mob, a spawn whose
/// spawning member's mob cannot be read is registered with no console:
/// neither the launch mob's nor the other mob's holds it.
#[tokio::test]
async fn on_a_host_of_two_mobs_a_spawn_whose_mob_cannot_be_read_is_registered_with_no_console() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _controller, second_id) = host_for_the_prime_rows(&temp).await;
    let of_the_launch = host.console_events();
    let of_the_second = of_the_launch.of_mob(Some(&second_id));

    an_agent_spawns_through(
        &host,
        &host.mob_runtime().handle(),
        "desk",
        "scout",
        "launch",
        false,
    )
    .await;

    // Asked under every spelling: a console that held the launch mob's
    // member would hold it under the bare name where it is the launch
    // mob's own, and under the launch mob and the name in the other.
    let held = (
        held_under_any_spelling(&of_the_launch, "scout").await,
        held_under_any_spelling(&of_the_second, "scout").await,
    );
    shut_down(&host, &[&second_id]).await;
    assert_eq!(
        held,
        ((Vec::new(), None), (Vec::new(), None)),
        "a spawn whose member's mob cannot be read was registered with a console of a host \
         that serves two mobs"
    );
}

/// A launch over a state directory, as a gateway's is, with a mob the host
/// created, `second` (its agent `courier`), and a mob that agent made,
/// `made-below` (its agent `runner`): the launch and the ids of the two
/// mobs. The mobs are stored under the directory, so the launch's mob set
/// can unload them, and a later launch over the directory finds them.
async fn a_stored_host_with_a_mob_below_a_second_mob(
    stored: &StoredHost,
) -> (UnifiedRuntime, MobId, MobId) {
    let host = stored.launch().await;
    let second_id = second_mob(&host, definition("second")).await;
    let below_id = {
        let second = set_of(&host)
            .handle_for(&second_id)
            .await
            .expect("the mob set serves the second mob");
        Box::pin(second.spawn_spec(of_the_team("courier", "second")))
            .await
            .expect("an agent of the second mob");
        let owner_session = second
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("courier"))
            .await
            .expect("the second mob's agent's session");
        let below_id = Box::pin(
            set_of(&host).mob_create_definition_with_owner_bridge_session(
                definition("made-below"),
                owner_session,
                true,
                false,
            ),
        )
        .await
        .expect("a mob the second mob's agent made");
        let below = set_of(&host)
            .handle_for(&below_id)
            .await
            .expect("the mob set serves the agent's mob");
        Box::pin(below.spawn_spec(of_the_team("runner", "second")))
            .await
            .expect("an agent of the agent's mob");
        below_id
    };
    (host, second_id, below_id)
}

/// What the launch's console and the console of the mob `second_id` hold
/// for the name `name`, after the agent `runner` of the mob `below_id`
/// spawned a member of that name; and whether the mob `second_id` is
/// loaded once the spawn is registered. The mob `below_id` is loaded by the
/// lookup here, as the first thing that addresses it loads it.
async fn held_after_a_spawn_below(
    host: &UnifiedRuntime,
    second_id: &MobId,
    below_id: &MobId,
    name: &'static str,
) -> (HeldForAName, HeldForAName, bool) {
    let below = set_of(host)
        .handle_for(below_id)
        .await
        .expect("the lookup loads the mob the second mob's agent made");
    let of_the_launch = host.console_events();
    let of_the_second = of_the_launch.of_mob(Some(second_id));
    an_agent_spawns(host, &below, "runner", name, "second").await;
    // The spawned member sits in a mob a member made: a console holds it
    // under its mob and its name.
    let console_name = named_in(below_id, name);
    (
        registered_for(&of_the_launch, &console_name).await,
        registered_for(&of_the_second, &console_name).await,
        known(host).await.loaded.contains(second_id),
    )
}

/// Whether `labels` are what the spawn of a member of the team `second` by
/// `runner` of the mob `below_id`, a mob a member made, registered: the
/// spawner is named by its mob and its name.
fn is_the_spawn_by_the_runner(
    labels: Option<&std::collections::BTreeMap<String, String>>,
    below_id: &MobId,
) -> bool {
    let runner = named_in(below_id, "runner");
    labels.is_some_and(|labels| {
        labels.get("team").map(String::as_str) == Some("second")
            && labels
                .get(crate::console_spawn::SPAWNED_BY_LABEL)
                .map(String::as_str)
                == Some(runner.as_str())
    })
}

/// A spawn below a mob the host created is registered with that mob's
/// console while that mob is not loaded: the mob set unloaded the
/// host-created mob, this process names no mob beside the launch mob, and
/// the mob its agent made is loaded. Who created the host-created mob is
/// read from what the mob set keeps of it; the mob stays not loaded.
#[tokio::test]
async fn a_spawn_below_a_host_created_mob_that_was_unloaded_is_registered_with_that_mob() {
    let stored = StoredHost::new();
    let (host, second_id, below_id) = a_stored_host_with_a_mob_below_a_second_mob(&stored).await;
    // The mob set unloads the host-created mob; the mob its agent made
    // stays loaded, or is loaded again by the lookup below.
    asleep(&host, &[&second_id]).await;
    host.mob_runtime()
        .private_owners()
        .hosted_mobs()
        .name_none_in_tests();

    let (launch_holds, second_holds, second_is_loaded) =
        held_after_a_spawn_below(&host, &second_id, &below_id, "page").await;
    shut_down(&host, &[]).await;

    let mut wrong = Vec::new();
    if launch_holds != (None, None) {
        wrong.push(format!(
            "the launch mob's console holds the spawn: {launch_holds:?}"
        ));
    }
    if !is_the_spawn_by_the_runner(second_holds.0.as_ref(), &below_id) {
        wrong.push(format!(
            "the host-created mob's console does not hold the spawn: {second_holds:?}"
        ));
    }
    if second_is_loaded {
        wrong.push("registering the spawn loaded the host-created mob".to_string());
    }
    assert!(
        wrong.is_empty(),
        "a spawn below a host-created mob that was unloaded was not registered with that \
         mob's console:\n{}",
        wrong.join("\n")
    );
}

/// A spawn below a mob the host created is registered with that mob's
/// console after a start over the same stores, before anything addressed
/// the host-created mob: it is not loaded, this process has named no mob
/// beside the launch mob, and the mob its agent made is loaded. Who created
/// the host-created mob is read from its store's creation record, whether
/// or not the mob set has read that store yet; the mob stays not loaded.
#[tokio::test]
async fn a_spawn_below_a_host_created_mob_not_loaded_since_the_start_is_registered_with_that_mob() {
    let stored = StoredHost::new();
    let (first, second_id, below_id) = a_stored_host_with_a_mob_below_a_second_mob(&stored).await;
    shut_down(&first, &[]).await;
    drop(first);

    let host = stored.launch().await;
    let (launch_holds, second_holds, second_is_loaded) =
        held_after_a_spawn_below(&host, &second_id, &below_id, "page").await;
    shut_down(&host, &[]).await;

    let mut wrong = Vec::new();
    if launch_holds != (None, None) {
        wrong.push(format!(
            "the launch mob's console holds the spawn: {launch_holds:?}"
        ));
    }
    if !is_the_spawn_by_the_runner(second_holds.0.as_ref(), &below_id) {
        wrong.push(format!(
            "the host-created mob's console does not hold the spawn: {second_holds:?}"
        ));
    }
    if second_is_loaded {
        wrong.push("registering the spawn loaded the host-created mob".to_string());
    }
    assert!(
        wrong.is_empty(),
        "a spawn below a host-created mob that is not loaded since the start was not \
         registered with that mob's console:\n{}",
        wrong.join("\n")
    );
}

/// Damage the store of the stored mob `mob_id` of `stored` on disk: what a
/// store that cannot be read is.
fn damage_the_store_of(stored: &StoredHost, mob_id: &MobId) {
    let file = crate::child_mob_storage::child_mob_databases(stored.state_dir())
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(mob_id.as_str()))
        })
        .expect("the stored mob's file under the state directory");
    std::fs::write(&file, b"not a sqlite database").expect("damage the stored mob's file");
}

/// On a host whose mob set lists a mob beside the launch mob, a spawn below
/// a mob whose creation cannot be read is registered with no console: after
/// a start over the same stores the store of the mob the host created
/// cannot be read, and the mob its agent made is loaded. Neither the launch
/// mob's console nor the other mob's holds the spawn, although this process
/// has named no mob beside the launch mob.
#[tokio::test]
async fn a_spawn_below_a_mob_whose_store_cannot_be_read_is_registered_with_no_console() {
    let stored = StoredHost::new();
    let (first, second_id, below_id) = a_stored_host_with_a_mob_below_a_second_mob(&stored).await;
    shut_down(&first, &[]).await;
    drop(first);
    damage_the_store_of(&stored, &second_id);

    let host = stored.launch().await;
    let (launch_holds, second_holds, _) =
        held_after_a_spawn_below(&host, &second_id, &below_id, "page").await;
    shut_down(&host, &[]).await;

    assert_eq!(
        (launch_holds, second_holds),
        ((None, None), (None, None)),
        "a spawn below a mob whose store cannot be read was registered with a console of a \
         host whose mob set lists a mob beside the launch mob"
    );
}

/// The event stream's read of the member an event is of takes what a spawn
/// registered with the console of that member's mob. An agent of a mob the
/// host created spawns a member with a label; the stream's read of that
/// member, made with that mob's runtime, carries the registered label and
/// who spawned it, as its read of a member the launch mob's agent spawned
/// does with the launch's runtime.
///
/// A read that misses the registration decides the member by fewer labels
/// than its spawn gave it: a rule on such a label then withholds the
/// member's events from a person it lets view the member. The miss shows no
/// one anything.
#[tokio::test]
async fn the_streams_read_of_a_member_takes_what_a_spawn_registered_with_its_mobs_console() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _controller, second_id) = host_for_the_prime_rows(&temp).await;
    let second_runtime = {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        Box::pin(second.spawn(SpawnMemberSpec::new("worker", "scout")))
            .await
            .expect("the member the second mob's agent spawns");
        second.mob_runtime().clone()
    };
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", "page")))
        .await
        .expect("the member the launch mob's agent spawns");
    an_agent_spawns(
        &host,
        &second_runtime.handle(),
        "courier",
        "scout",
        "second",
    )
    .await;
    an_agent_spawns(
        &host,
        &host.mob_runtime().handle(),
        "desk",
        "page",
        "launch",
    )
    .await;

    let policy = HideImplicitDelegateMembersConsoleVisibilityPolicy;
    let mut wrong = Vec::new();
    for (runtime, of, name, team, spawner) in [
        (
            host.mob_runtime(),
            "the launch mob",
            "page",
            "launch",
            "desk",
        ),
        (
            &second_runtime,
            "the host-created mob",
            "scout",
            "second",
            "courier",
        ),
    ] {
        let handle = runtime.handle();
        let member = handle
            .get_member(&meerkat_mob::AgentIdentity::from(name))
            .await
            .expect("the roster is read")
            .expect("the member is seated");
        let read = crate::http_console::sse_member_authorization_projection(
            &handle,
            Some(runtime),
            &policy,
            name,
            &member.agent_runtime_id,
            member.fence_token,
        )
        .await;
        let labels = read.as_ref().map(|read| &read.attributes.labels);
        let is_the_spawns = labels.is_some_and(|labels| {
            labels.get("team").map(String::as_str) == Some(team)
                && labels
                    .get(crate::console_spawn::SPAWNED_BY_LABEL)
                    .map(String::as_str)
                    == Some(spawner)
        });
        if !is_the_spawns {
            wrong.push(format!(
                "the stream's read of `{name}` of {of} does not carry what the spawn by \
                 `{spawner}` registered: {labels:?}"
            ));
        }
    }

    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "the event stream's read of a member did not take what a spawn registered with the \
         console of the member's mob:\n{}",
        wrong.join("\n")
    );
}

/// An open event stream of the console: the bytes still to come and the
/// bytes read and not yet cut into events.
struct OpenConsoleStream {
    body: axum::body::BodyDataStream,
    buffered: String,
}

impl OpenConsoleStream {
    /// The stream `uri` opens on `app` for a sign-in of `email`; an answer
    /// other than 200 is an error with its status.
    async fn open_as(app: &axum::Router, uri: &str, email: &str) -> Result<Self, String> {
        use tower::ServiceExt;

        let request = axum::http::Request::builder()
            .method("GET")
            .uri(uri)
            .header(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", feed_row_token(email)),
            )
            .body(axum::body::Body::empty())
            .expect("GET request");
        let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
            .await
            .expect("the route answers in time")
            .expect("the route answers");
        if response.status() != axum::http::StatusCode::OK {
            return Err(format!("GET {uri} is answered {}", response.status()));
        }
        Ok(Self {
            body: response.into_body().into_data_stream(),
            buffered: String::new(),
        })
    }

    /// The name of the next event when one comes within `within`
    /// (keep-alive comments are passed over); `None` when none came or the
    /// stream ended.
    async fn event_within(&mut self, within: Duration) -> Option<String> {
        use futures::StreamExt;

        let next = async {
            loop {
                if let Some(end) = self.buffered.find("\n\n") {
                    let event: String = self.buffered.drain(..end + 2).collect();
                    if let Some(name) = event.lines().find_map(|line| line.strip_prefix("event:")) {
                        return Some(name.trim().to_string());
                    }
                } else {
                    let chunk = self.body.next().await?.ok()?;
                    self.buffered.push_str(&String::from_utf8_lossy(&chunk));
                }
            }
        };
        tokio::time::timeout(within, next).await.ok().flatten()
    }

    /// Whether an event named `name` comes within `within`; the events
    /// before it are read and passed over.
    async fn reads_event(&mut self, name: &str, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            match self.event_within(left).await {
                Some(event) if event == name => return true,
                Some(_) => {}
                None => return false,
            }
        }
    }
}

/// The console's timeline stream reads the live roster at its open, again
/// after a committed access change, and again before the first event of an
/// agent it does not know. On a host that created a second mob, none of
/// these reads records an agent of that mob as an agent of the launch mob.
/// Each read is driven on a stream of a signed-in person, alone: a stream
/// of the launch mob's `desk` is opened, both mobs gain an agent and an
/// access change is committed; a stream of `usher` is opened before the
/// launch mob has such an agent, both mobs gain an agent and an event of
/// `usher` arrives. After each read the attribute cache knows the launch
/// mob's agents, the one just seated among them, and none of the second
/// mob's, and the person may not view the launch mob's `desk`.
#[tokio::test]
async fn the_consoles_stream_reads_no_agent_of_a_host_created_mob_as_the_launch_mobs() {
    use crate::access::AccessRule;

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, controller, second_id) = host_for_the_prime_rows(&temp).await;
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": FEED_ROW_SIGNING_SECRET,
        "email_allowlist": [PRIME_ROWS_ADMIN, PRIME_ROWS_PERSON],
    }))
    .expect("console sign-in configuration");
    let app = host.build_console_json_router(decisions);

    let knows = |name: &str| {
        controller
            .view_for_subject(Some(PRIME_ROWS_PERSON))
            .knows_agent(name)
    };
    let mut wrong = Vec::new();

    // ONE: the open, then a committed access change. The stream follows the
    // launch mob's `desk` alone, so no event of another agent makes it read
    // the roster; it runs only while this row reads it.
    match OpenConsoleStream::open_as(
        &app,
        "/console/timeline/stream?identity=desk",
        PRIME_ROWS_PERSON,
    )
    .await
    {
        Err(refused) => wrong.push(format!("the person's stream of `desk`: {refused}")),
        Ok(mut stream) => {
            faults_in_the_default_mobs_cache(
                &controller,
                "at the stream's open",
                &["desk"],
                &["courier"],
                &mut wrong,
            );
            if !stream
                .reads_event("access_changed", Duration::from_secs(10))
                .await
            {
                wrong.push(
                    "the stream of `desk` does not go live (no access revision is named)"
                        .to_string(),
                );
            }
            // Both mobs gain an agent while the stream is open.
            {
                let second = host
                    .resolve_mob(Some(&second_id))
                    .await
                    .expect("the second mob of the host resolves");
                Box::pin(second.spawn(of_the_team("porter", "second")))
                    .await
                    .expect("one more agent of the second mob");
            }
            Box::pin(host.spawn(of_the_team("clerk", "launch")))
                .await
                .expect("one more agent of the launch mob");
            if knows("clerk") {
                wrong.push(
                    "before the access change: `clerk` is known already, so something other \
                     than the stream's read after the change read the roster"
                        .to_string(),
                );
            }
            // An access change is committed: the stream reads the roster
            // again before it names the new revision.
            match controller.upsert_rule(AccessRule {
                id: "sees-the-clerk".to_string(),
                subjects: vec![PRIME_ROWS_PERSON.to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                agents: vec!["clerk".to_string()],
                ..AccessRule::default()
            }) {
                Err(refused) => wrong.push(format!("the access change: {refused:?}")),
                Ok(_revision) => {
                    if !stream
                        .reads_event("access_changed", Duration::from_secs(10))
                        .await
                    {
                        wrong.push(
                            "the stream of `desk` does not name the committed access revision"
                                .to_string(),
                        );
                    }
                }
            }
            faults_in_the_default_mobs_cache(
                &controller,
                "after a committed access change",
                &["desk", "clerk"],
                &["courier", "porter"],
                &mut wrong,
            );
        }
    }

    // TWO: the first event of an agent the stream does not know. The stream
    // is opened for `usher` before the launch mob has such an agent; both
    // mobs then gain an agent, and the first event of `usher` makes the
    // stream read the roster again. The event itself is not sent to this
    // person.
    {
        let second = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        Box::pin(second.spawn(of_the_team("page", "second")))
            .await
            .expect("a third agent of the second mob");
    }
    match OpenConsoleStream::open_as(
        &app,
        "/console/timeline/stream?identity=usher",
        PRIME_ROWS_PERSON,
    )
    .await
    {
        Err(refused) => wrong.push(format!("the person's stream of `usher`: {refused}")),
        Ok(mut stream) => {
            if !stream
                .reads_event("access_changed", Duration::from_secs(10))
                .await
            {
                wrong.push(
                    "the stream of `usher` does not go live (no access revision is named)"
                        .to_string(),
                );
            }
            Box::pin(host.spawn(of_the_team("usher", "launch")))
                .await
                .expect("a third agent of the launch mob");
            if knows("usher") {
                wrong.push(
                    "before its first event is read: `usher` is known already, so something \
                     other than the stream's read for an agent it does not know read the roster"
                        .to_string(),
                );
            }
            if let Some(projection) = host.console_projection.get() {
                let _ = Box::pin(projection.list_identities()).await;
            }
            record(&host, None, "usher", "of-the-launch-usher").await;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while !knows("usher") && tokio::time::Instant::now() < deadline {
                let _ = stream.event_within(Duration::from_millis(100)).await;
            }
            faults_in_the_default_mobs_cache(
                &controller,
                "after the first event of an agent the stream did not know",
                &["desk", "clerk", "usher"],
                &["courier", "porter", "page"],
                &mut wrong,
            );
        }
    }

    drop(app);
    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "the console's stream took an agent of a host-created mob for the launch mob's:\n{}",
        wrong.join("\n")
    );
}

/// Every mob of a launch has the launch's ONE upload store, and a blob
/// read of a mob says only whether that mob is served. On a launch with an
/// upload store, an agent of a mob the host serves beside its launch mob
/// uploads a file:
///
/// - the upload store the console finds for that agent, in its own mob, is
///   the launch's store itself;
/// - the file is found by the blob read of the agent's mob and by the
///   launch mob's, by its address (its content's): the launch's store
///   serves both reads;
/// - the blob read of a mob whose console events are not registered, and
///   of a blank id, walks no store and finds nothing, as for a blob nobody
///   stored.
#[tokio::test]
async fn a_hosted_mobs_uploaded_file_is_in_the_launchs_one_upload_store() {
    use crate::blob_store::{BinaryBlobStore, ObjectStoreBlobStore};

    let temp = tempfile::tempdir().expect("tempdir");
    let uploads: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
    let of_the_launch = Arc::clone(&uploads);
    let (host, _sessions, _machine, ()) = launch_composed(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        move |mut spec| {
            spec.binary_blob_store = Some(of_the_launch);
            spec
        },
    )
    .await;
    host.initialize_console_projection_with_policy(Arc::new(
        HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ))
    .expect("the runtime's console projection");
    let second_id = two_desks_with_a_frame_each(&host).await;
    let not_registered = MobId::from("no-such-mob-of-this-host");
    let blank = MobId::from("");
    let projection = host
        .console_projection
        .get()
        .expect("the launch has a console projection");

    // The hosted mob's agent's upload store, once its mob lists the agent.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let found = loop {
        let found = Box::pin(projection.binary_blob_store_for_member("desk", Some(&second_id)))
            .await
            .map_err(|error| error.to_string());
        if matches!(found, Ok(Some(_))) || tokio::time::Instant::now() >= deadline {
            break found;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let mut wrong = Vec::new();
    let mut reads = Vec::new();
    match found {
        Ok(Some(store)) => {
            if !std::ptr::addr_eq(Arc::as_ptr(&store), Arc::as_ptr(&uploads)) {
                wrong.push(
                    "the hosted mob's agent's upload store is not the launch's own store"
                        .to_string(),
                );
            }
            match store
                .put_bytes(
                    "text/plain",
                    bytes::Bytes::from_static(b"a file of the hosted mob's desk"),
                )
                .await
            {
                Ok(file) => {
                    for mob in [Some(&second_id), None, Some(&not_registered), Some(&blank)] {
                        let stores = projection.binary_blob_stores_of_mob(mob);
                        let mut read = format!("{} stores", stores.len());
                        for store in stores {
                            match store.get_bytes(&file.blob_id).await {
                                Ok(_) => read.push_str(", found"),
                                Err(meerkat_core::BlobStoreError::NotFound(_)) => {}
                                Err(error) => read = format!("{read}, refused: {error}"),
                            }
                        }
                        reads.push(read);
                    }
                }
                Err(error) => wrong.push(format!("the file is not stored: {error}")),
            }
        }
        other => wrong.push(format!(
            "the hosted mob's agent has no upload store: {:?}",
            other.map(|store| store.is_some())
        )),
    }
    shut_down(&host, &[&second_id]).await;

    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(
        reads,
        ["1 stores, found", "1 stores, found", "0 stores", "0 stores"],
        "the blob reads of the file, in order: by the agent's own mob; by the launch mob (no \
         mob named): the launch's store serves both reads; by a mob whose console events are \
         not registered; by a blank id"
    );
}

// ---------------------------------------------------------------------------
// The writes of an earlier mob under an id a mob is created under again.
//
// A stored frame names its mob's id and no more: a reader of a mob created
// under a destroyed mob's id is kept from the earlier mob's frames by the
// create's removal of them alone. These rows hold what that removal needs:
// no write of the earlier mob lands after it. Each row holds ONE write of the
// earlier mob inside the console log (a log whose append the row can hold),
// has the mob destroyed and a mob created under its id, and lets the held
// write go once the earlier mob's entry has ended. No row waits a time for
// that: the held write is let go on what the row reads of the host.
// ---------------------------------------------------------------------------

/// A console log (in memory) whose append of ONE frame a row can hold: the
/// first append of a frame the row's rule picks waits inside the log until
/// the row lets it go. Every other call goes to the log at once. The log
/// keeps the order of three things: `held` (the picked append came in),
/// `stored` (it was let go and the log answered it), `removed` (a mob's
/// frames were removed).
struct HeldLog {
    inner: InMemoryConsoleLogStore,
    picks: std::sync::Mutex<Option<PicksFrame>>,
    came_in: tokio::sync::Notify,
    let_go: tokio::sync::Notify,
    answered: tokio::sync::Notify,
    order: std::sync::Mutex<Vec<&'static str>>,
}

type PicksFrame = Box<dyn Fn(&NewConsoleFrame) -> bool + Send + Sync>;

impl HeldLog {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryConsoleLogStore::new(),
            picks: std::sync::Mutex::new(None),
            came_in: tokio::sync::Notify::new(),
            let_go: tokio::sync::Notify::new(),
            answered: tokio::sync::Notify::new(),
            order: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Hold the first append of a frame `picks` says yes to. The order the
    /// log keeps begins here: what it saw before (the removal a mob's own
    /// create makes) is no part of what a row holds.
    fn hold_the_first(&self, picks: impl Fn(&NewConsoleFrame) -> bool + Send + Sync + 'static) {
        self.order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        *self
            .picks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(picks));
    }

    /// Hold the first append of a console event frame of `mob` whose text
    /// is `text`.
    fn hold_the_first_event(&self, mob: &MobId, text: &'static str) {
        let mob = mob.clone();
        self.hold_the_first(move |frame| {
            frame.mob_id.as_ref() == Some(&mob)
                && frame.payload.get("text").and_then(Value::as_str) == Some(text)
        });
    }

    fn note(&self, what: &'static str) {
        self.order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(what);
    }

    /// What happened in the log, in order.
    fn order(&self) -> Vec<&'static str> {
        self.order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Once the picked append came in and is held.
    async fn held(&self) {
        let came_in = tokio::time::timeout(Duration::from_secs(20), self.came_in.notified()).await;
        assert!(came_in.is_ok(), "the write the row holds reached the log");
    }

    /// Let the held append go.
    fn let_go(&self) {
        self.let_go.notify_one();
    }

    /// Once the held append was let go and the log answered it.
    async fn answered(&self) {
        let answered =
            tokio::time::timeout(Duration::from_secs(20), self.answered.notified()).await;
        assert!(answered.is_ok(), "the held write was answered by the log");
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for HeldLog {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        let held = {
            let mut picks = self
                .picks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if picks.as_ref().is_some_and(|picks| picks(&frame)) {
                *picks = None;
                true
            } else {
                false
            }
        };
        if held {
            self.note("held");
            self.came_in.notify_one();
            self.let_go.notified().await;
        }
        let outcome = self.inner.append_if_absent(frame).await;
        if held {
            self.note("stored");
            self.answered.notify_one();
        }
        outcome
    }
    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(frame_id, status).await
    }
    async fn transition_send_frame(
        &self,
        frame_id: &str,
        transition: crate::console_aggregator::SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .transition_send_frame(frame_id, transition, note)
            .await
    }
    fn supports_send_transitions(&self) -> bool {
        self.inner.supports_send_transitions()
    }
    async fn take_send_frame_hold(&self, frame_id: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.take_send_frame_hold(frame_id).await
    }
    async fn update_frame_status_of_mob(
        &self,
        mob_id: Option<&MobId>,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .update_frame_status_of_mob(mob_id, frame_id, status)
            .await
    }
    async fn transition_send_frame_of_mob(
        &self,
        mob_id: Option<&MobId>,
        frame_id: &str,
        transition: crate::console_aggregator::SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .transition_send_frame_of_mob(mob_id, frame_id, transition, note)
            .await
    }
    async fn take_send_frame_hold_of_mob(
        &self,
        mob_id: Option<&MobId>,
        frame_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .take_send_frame_hold_of_mob(mob_id, frame_id)
            .await
    }
    async fn frame_by_dedupe_key_of_mob(
        &self,
        mob_id: Option<&MobId>,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .frame_by_dedupe_key_of_mob(mob_id, dedupe_key)
            .await
    }
    async fn send_frame_by_interaction_of_mob(
        &self,
        mob_id: Option<&MobId>,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .send_frame_by_interaction_of_mob(mob_id, interaction_id)
            .await
    }
    async fn unsettled_send_frames_of_mob(
        &self,
        mob_id: Option<&MobId>,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        self.inner
            .unsettled_send_frames_of_mob(mob_id, window, limit)
            .await
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
    async fn query_windowed_frames_of_mob(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.inner.query_windowed_frames_of_mob(query).await
    }
    fn reads_one_mob(&self) -> bool {
        self.inner.reads_one_mob()
    }
    async fn frame_by_dedupe_key(
        &self,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(dedupe_key).await
    }
    async fn send_frame_by_interaction(
        &self,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.send_frame_by_interaction(interaction_id).await
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
    ) -> ConsoleLogResult<std::collections::HashMap<String, u64>> {
        self.inner.latest_activity_times(identities).await
    }
    async fn latest_activity_times_in_mob(
        &self,
        mob_id: &str,
        identities: &[String],
    ) -> ConsoleLogResult<std::collections::HashMap<String, u64>> {
        self.inner
            .latest_activity_times_in_mob(mob_id, identities)
            .await
    }
    fn send_dedupe_durability(&self) -> crate::console_aggregator::ConsoleSendDedupeDurability {
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
        compaction: &crate::console_aggregator::ConsoleDeltaCompaction,
    ) -> ConsoleLogResult<usize> {
        self.inner.compact_message_deltas(compaction).await
    }
    async fn remove_frames_of_mob(&self, mob_id: &MobId) -> ConsoleLogResult<u64> {
        self.note("removed");
        self.inner.remove_frames_of_mob(mob_id).await
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
    fn bind_default_mob(&self, default_mob: &DefaultMob) -> ConsoleLogResult<()> {
        self.inner.bind_default_mob(default_mob)
    }
}

/// A launch whose console log is `log`, serving the mobs its mob set builds
/// for the host. `projected`: whether its console projection is built now
/// (a row that wants events to stand in a mob's store before anything
/// projects them builds it later).
async fn held_host(
    temp: &tempfile::TempDir,
    log: &Arc<HeldLog>,
    projected: bool,
) -> UnifiedRuntime {
    let (mut host, _sessions) = launch(temp, definition("host")).await;
    host.set_console_log_store(Arc::clone(log) as Arc<dyn ConsoleLogStore>);
    if projected {
        project(&host);
    }
    host
}

/// Build `host`'s console projection.
fn project(host: &UnifiedRuntime) {
    host.initialize_console_projection_with_policy(Arc::new(
        HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ))
    .expect("the runtime's console projection");
}

/// Once the entry of the mob that had `id` has ended: the console events
/// the host holds under the id are no longer `earlier`, the destroyed
/// mob's (they go with its entry, and the mob created under the id gets
/// its own). Read from the host again and again, letting its tasks run in
/// between; no time is waited.
async fn the_earlier_entry_ended(
    host: &UnifiedRuntime,
    id: &MobId,
    earlier: &super::ConsoleEventStore,
) {
    let ended = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let events = host.console_events();
            let stands = events.mobs_with_a_store().contains(id)
                && events.of_mob(Some(id)).is_the_store(earlier);
            if !stands {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(ended.is_ok(), "the destroyed mob's entry ended");
}

/// Let the tasks that are ready run to where they wait.
async fn let_the_ready_tasks_run() {
    for _ in 0..256 {
        tokio::task::yield_now().await;
    }
}

/// Destroy the mob `id`, have a mob created under the id again from
/// `definition` while one write of the destroyed mob is held in `log`, let
/// the held write go once the destroyed mob's entry has ended, and answer
/// what the create answered once the held write was answered too.
async fn recreated_over_a_held_write(
    host: &UnifiedRuntime,
    log: &HeldLog,
    id: &MobId,
    definition: MobDefinition,
) -> Result<MobId, String> {
    let earlier = host.console_events().of_mob(Some(id));
    let destroyed = Box::pin(set_of(host).mob_destroy(id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the mob: {:?}",
        destroyed.as_ref().err()
    );
    let set = set_of(host);
    let create_ended = std::sync::atomic::AtomicBool::new(false);
    // The create and the row's own steps run side by side on this task.
    let create = async {
        let created = Box::pin(set.mob_create_definition(definition))
            .await
            .map_err(|error| error.to_string());
        create_ended.store(true, std::sync::atomic::Ordering::SeqCst);
        created
    };
    let steps = async {
        the_earlier_entry_ended(host, id, &earlier).await;
        // A create that does not wait for the held write goes on by itself.
        for _ in 0..4096 {
            if create_ended.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        log.let_go();
    };
    let (created, ()) = tokio::time::timeout(Duration::from_mins(1), async {
        tokio::join!(create, steps)
    })
    .await
    .expect("the create ended");
    log.answered().await;
    let_the_ready_tasks_run().await;
    created
}

/// A console event of a destroyed mob that is being written when a mob is
/// created under its id is not a frame of the new mob. The destroyed mob
/// recorded an event whose write the log holds; the mob is destroyed and a
/// mob created under its id; the held write is let go once the destroyed
/// mob's entry has ended. The write was the earlier mob's, so it is stored
/// BEFORE the frames under the id are removed, and the mob created under
/// the id shows no frame of the earlier mob.
#[tokio::test]
async fn a_late_event_of_a_destroyed_mob_is_not_stored_after_a_mob_is_created_under_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = HeldLog::new();
    let host = held_host(&temp, &log, true).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    record(&host, Some(&second_id), "alice", "of-the-earlier-mob").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-earlier-mob").await),
        ["of-the-earlier-mob"]
    );
    log.hold_the_first_event(&second_id, "of-the-earlier-mob-in-flight");
    record(
        &host,
        Some(&second_id),
        "alice",
        "of-the-earlier-mob-in-flight",
    )
    .await;
    log.held().await;

    let created = recreated_over_a_held_write(&host, &log, &second_id, second_definition).await;
    assert_eq!(created, Ok(second_id.clone()), "a mob under the same id");
    let of_the_new_mob = frames_now(&host, Some(&second_id)).await;
    assert_eq!(
        (log.order(), texts(&of_the_new_mob)),
        (vec!["held", "stored", "removed"], Vec::<&str>::new()),
        "the earlier mob's write in flight was stored before the frames under the id were \
         removed, and the mob created under the id shows no frame of the earlier mob"
    );
    record(&host, Some(&second_id), "alice", "of-the-new-mob").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-new-mob").await),
        ["of-the-new-mob"],
        "the new mob's own events are stored under the id"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The replay of a destroyed mob's console events writes nothing into a
/// mob created under its id, however much of it was left. A mob records
/// three events before the host's console projection is built; the
/// projection's replay of them is held at the first; the mob is destroyed
/// and a mob created under its id; the held write is let go. None of the
/// three is a frame of the new mob: not the one that was in flight, and not
/// the rest of the replay, which would be written for whatever mob holds
/// the id by then.
#[tokio::test]
async fn a_destroyed_mobs_long_replay_writes_nothing_into_a_mob_created_under_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = HeldLog::new();
    let host = held_host(&temp, &log, false).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    for text in [
        "of-the-earlier-mob-1",
        "of-the-earlier-mob-2",
        "of-the-earlier-mob-3",
    ] {
        record(&host, Some(&second_id), "alice", text).await;
    }
    log.hold_the_first_event(&second_id, "of-the-earlier-mob-1");
    project(&host);
    log.held().await;

    let created = recreated_over_a_held_write(&host, &log, &second_id, second_definition).await;
    assert_eq!(created, Ok(second_id.clone()), "a mob under the same id");
    let of_the_new_mob = frames_now(&host, Some(&second_id)).await;
    assert_eq!(
        (log.order(), texts(&of_the_new_mob)),
        (vec!["held", "stored", "removed"], Vec::<&str>::new()),
        "the replay's write in flight was stored before the frames under the id were removed, \
         and the mob created under the id shows nothing of the earlier mob's replay"
    );
    record(&host, Some(&second_id), "alice", "of-the-new-mob").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-new-mob").await),
        ["of-the-new-mob"],
        "the new mob's own events, and nothing else, are stored under the id"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A write of a console projection that was replaced is still the earlier
/// mob's write when a mob is created under its id later. A mob's event is
/// held in the log; the runtime is told its console log again (the same
/// log), which replaces its console projection, and a new one is built;
/// the mob is destroyed and a mob created under its id; the held write of
/// the replaced projection is let go. It is stored before the frames under
/// the id are removed, and the new mob shows no frame of the earlier mob.
#[tokio::test]
async fn a_replaced_projections_write_in_flight_is_stored_before_a_create_removes_the_frames() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = HeldLog::new();
    let mut host = held_host(&temp, &log, true).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    log.hold_the_first_event(&second_id, "of-the-earlier-mob-in-flight");
    record(
        &host,
        Some(&second_id),
        "alice",
        "of-the-earlier-mob-in-flight",
    )
    .await;
    log.held().await;
    host.set_console_log_store(Arc::clone(&log) as Arc<dyn ConsoleLogStore>);
    project(&host);

    let created = recreated_over_a_held_write(&host, &log, &second_id, second_definition).await;
    assert_eq!(created, Ok(second_id.clone()), "a mob under the same id");
    let of_the_new_mob = frames_now(&host, Some(&second_id)).await;
    assert_eq!(
        (log.order(), texts(&of_the_new_mob)),
        (vec!["held", "stored", "removed"], Vec::<&str>::new()),
        "the replaced projection's write was stored before the frames under the id were \
         removed, and the mob created under the id shows no frame of the earlier mob"
    );
    record(&host, Some(&second_id), "alice", "of-the-new-mob").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-new-mob").await),
        ["of-the-new-mob"],
        "the new mob's own events, and nothing else, are stored under the id"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A session history pass of a destroyed mob's agent writes nothing under
/// a mob created under its id. The second mob's agent `desk` runs a turn;
/// the first frame its session's history gives the console log is held; the
/// mob is destroyed and a mob created under its id; the held write is let
/// go. The held frame was the earlier mob's and is stored before the
/// frames under the id are removed; no frame of the earlier agent's
/// session, held or later in that pass, is a frame of the new mob.
#[tokio::test]
async fn a_history_pass_of_a_destroyed_mobs_session_writes_nothing_under_a_mob_created_under_its_id()
 {
    const TASK: &str = "a-task-only-the-earlier-mobs-desk-was-given";

    let temp = tempfile::tempdir().expect("tempdir");
    let log = HeldLog::new();
    let host = held_host(&temp, &log, true).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
        assert!(
            seated.is_ok(),
            "the second mob's agent: {:?}",
            seated.as_ref().err()
        );
    }
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the mob set serves the second mob");
    let earlier_session = second
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("desk"))
        .await
        .expect("the second mob's agent's session")
        .to_string();
    {
        let mob = second_id.clone();
        log.hold_the_first(move |frame| {
            frame.mob_id.as_ref() == Some(&mob)
                && frame.source.kind == ConsoleFrameSourceKind::SessionHistory
        });
    }
    run_a_turn_of_desk(&second, TASK).await;
    // The run's events reach the mob's console events by a drain, and the
    // pass over the session's history follows them: the row drains and
    // reads again until the pass's first frame is held, as the rows of a
    // session's history above wait for their frames. What this row holds
    // is ordered by the held write from here on, not by this wait.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let drained = host.drain_mob_agent_events().await;
        assert!(
            drained.is_ok(),
            "the drain of the agents' events: {:?}",
            drained.as_ref().err().map(ToString::to_string)
        );
        if log.order().contains(&"held") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a frame of the agent's session history reached the log"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let created = recreated_over_a_held_write(&host, &log, &second_id, second_definition).await;
    assert_eq!(created, Ok(second_id.clone()), "a mob under the same id");
    let of_the_earlier_session: Vec<String> = host
        .console_log_store()
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(second_id.clone()),
            ..Default::default()
        })
        .await
        .expect("the read of one mob")
        .frames
        .iter()
        .filter(|frame| {
            frame.session_id.as_deref() == Some(earlier_session.as_str())
                || frame.payload.to_string().contains(TASK)
        })
        .map(|frame| frame.kind.clone())
        .collect();
    assert_eq!(
        (log.order(), of_the_earlier_session),
        (vec!["held", "stored", "removed"], Vec::<String>::new()),
        "the earlier agent's history frame in flight was stored before the frames under the id \
         were removed, and the mob created under the id holds no frame of the earlier agent's \
         session"
    );
    record(&host, Some(&second_id), "alice", "of-the-new-mob").await;
    assert_eq!(
        texts(&frames_showing(&host, Some(&second_id), "of-the-new-mob").await),
        ["of-the-new-mob"],
        "the new mob's own events, and nothing else, are stored under the id"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob is created again under its id right after its destroy, time and
/// again, and each life shows its own event alone. The create waits until
/// nothing writes into the console log for an earlier mob of the id, and
/// that wait refuses at once where a registration under the id has not
/// ended: a create that follows a destroy does not meet that refusal (the
/// id is free for a create only once the destroy is recorded, after the
/// mob's actor closed, and the create ends the earlier entry before it
/// waits).
#[tokio::test]
async fn a_mob_is_created_again_right_after_its_destroy_time_and_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = HeldLog::new();
    let host = held_host(&temp, &log, true).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    for life in 0..5 {
        record(&host, Some(&second_id), "alice", "of-this-life").await;
        assert_eq!(
            texts(&frames_showing(&host, Some(&second_id), "of-this-life").await),
            ["of-this-life"],
            "life {life}: the mob shows its own event and nothing of the life before it"
        );
        let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
        assert!(
            destroyed.is_ok(),
            "life {life}: the set destroys the mob: {:?}",
            destroyed.as_ref().err()
        );
        let created = Box::pin(set_of(&host).mob_create_definition(second_definition.clone()))
            .await
            .map_err(|error| error.to_string());
        assert_eq!(
            created,
            Ok(second_id.clone()),
            "life {life}: a create right after the destroy"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// A request that resolved a mob records its console event for THAT mob,
/// also where the mob was destroyed and a mob created under its id before
/// the request's handler took the mob's console events. The row resolves
/// the second mob as a request does, destroys the mob and has one created
/// under its id, and only then takes the console events of the resolved
/// mob, as the dispatch's handlers take them, and records an event. The
/// event is not in the store the later mob's events are read from, and the
/// later mob shows its own event alone.
#[tokio::test]
async fn a_request_resolved_for_a_destroyed_mob_records_nothing_in_the_store_of_a_mob_created_under_its_id()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = HeldLog::new();
    let host = held_host(&temp, &log, true).await;
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    let resolved = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the request resolves the second mob");

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the mob: {:?}",
        destroyed.as_ref().err()
    );
    let created = Box::pin(set_of(&host).mob_create_definition(second_definition))
        .await
        .map_err(|error| error.to_string());
    assert_eq!(created, Ok(second_id.clone()), "a mob under the same id");

    // The handler of the request that resolved the earlier mob takes that
    // mob's console events now, and records.
    let of_the_request = crate::rpc::console_events_of_a_served_mob_for_tests(&resolved);
    of_the_request
        .record_lifecycle(
            "alice",
            "identity_respawned",
            json!({ "text": "of-the-earlier-request" }),
        )
        .await;
    let of_the_later_mob = host.console_events().of_mob(Some(&second_id));
    record(&host, Some(&second_id), "alice", "of-the-new-mob").await;
    let shown = frames_showing(&host, Some(&second_id), "of-the-new-mob").await;
    assert_eq!(
        (
            of_the_request.is_the_store(&of_the_later_mob),
            texts(&shown)
        ),
        (false, vec!["of-the-new-mob"]),
        "the earlier request's event is not recorded in the later mob's console events, and \
         the later mob shows its own event alone"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The owned form of a resolved mob hands the console events its ENTRY
/// holds: for a mob the host serves beside its default mob, the store the
/// entry was built with, which is the store the host names for the id
/// while that entry stands and the store the borrowed form answers; for
/// the default mob, none (its console events are the host's own store).
#[tokio::test]
async fn the_owned_form_of_a_hosted_mob_holds_its_entrys_console_events() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = HeldLog::new();
    let host = held_host(&temp, &log, true).await;
    let second_id = second_mob(&host, definition("second")).await;
    let resolved = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves");
    let of_the_entry = resolved
        .handles()
        .console_events_of_its_entry()
        .expect("the owned form of a hosted mob holds its entry's console events");
    assert!(
        of_the_entry.is_the_store(&host.console_events().of_mob(Some(&second_id))),
        "the entry's store is the one the host names for the id while the entry stands"
    );
    assert!(
        of_the_entry.is_the_store(&resolved.console_events()),
        "the owned and the borrowed form answer the same store"
    );
    let default_mob = host
        .resolve_mob(None)
        .await
        .expect("the default mob resolves");
    assert!(
        default_mob
            .handles()
            .console_events_of_its_entry()
            .is_none(),
        "the default mob's owned form holds none: its console events are the host's own store"
    );
    assert!(
        default_mob
            .console_events()
            .is_the_store(&host.console_events()),
        "the borrowed form answers the host's own store for the default mob"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A policy of a console view that redacts the payload of every frame a
/// row's event made: a frame whose payload carries a `text`.
struct RedactsRowTexts;

impl ConsoleVisibilityPolicy for RedactsRowTexts {
    fn redact_payload(&self, frame: &NewConsoleFrame) -> Option<Value> {
        frame
            .payload
            .get("text")
            .is_some()
            .then(|| json!({ "redacted_by": "the view" }))
    }
}

/// How `frame` is shown, when it is not shown as a view under
/// [`RedactsRowTexts`] shows a row's frame: with the policy's payload, no
/// text and the status `redacted`.
fn not_redacted_by_the_view(frame: &ConsoleFrame) -> Option<String> {
    let redacted = frame.payload.get("redacted_by").and_then(Value::as_str) == Some("the view")
        && frame.payload.get("text").is_none()
        && frame.status == ConsoleFrameStatus::Redacted;
    (!redacted).then(|| format!("status {:?}, payload {}", frame.status, frame.payload))
}

/// A console view's own payload policy redacts a frame of a mob the host
/// serves beside its default mob as it redacts a frame of the default mob.
/// The host's projection is built under one policy, which redacts nothing;
/// a view of it is under a second policy, which redacts the payload of the
/// row's frames. Each mob has an agent `desk` with one stored frame. The
/// view answers each mob's frame in that mob's timeline, and projects each
/// as a live event, with the policy's payload and the status `redacted`.
/// The default mob's frame is the control. Both frames are stored with
/// their text: a view redacts what it answers, never what is stored.
#[tokio::test]
async fn a_views_payload_policy_redacts_a_second_mobs_frame_as_the_default_mobs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = console_host(&temp, None).await;
    let projection = host
        .console_projection
        .get()
        .expect("the runtime's console projection");
    let second_id = two_desks_with_a_frame_each(&host).await;
    let view = projection.policy_view(Arc::new(RedactsRowTexts));
    // The console's identities are read first, as a request that lists the
    // agents does before one reads their timeline.
    let _ = Box::pin(view.list_identities()).await;

    let mut wrong = Vec::new();
    for (whose, mob, text) in [
        ("the default mob's", None, "of-the-launch-desk"),
        ("the second mob's", Some(&second_id), "of-the-second-desk"),
    ] {
        let stored = frames_showing(&host, mob, text)
            .await
            .into_iter()
            .find(|frame| text_of(frame) == Some(text));
        let Some(stored) = stored else {
            wrong.push(format!("{whose} frame is not stored with its text"));
            continue;
        };
        match Box::pin(view.query_mob_timeline_windowed_for(
            ConsoleTimelineWindowQuery {
                mob_id: mob.cloned(),
                ..Default::default()
            },
            None,
        ))
        .await
        {
            Ok(page) => match page.frames.iter().find(|frame| frame.id == stored.id) {
                Some(read) => {
                    if let Some(shown) = not_redacted_by_the_view(read) {
                        wrong.push(format!(
                            "{whose} frame, read in its mob's timeline, is not redacted by the \
                             view's policy: {shown}"
                        ));
                    }
                }
                None => wrong.push(format!(
                    "{whose} frame is not answered in its mob's timeline"
                )),
            },
            Err(error) => wrong.push(format!("{whose} timeline is not read: {error:?}")),
        }
        match view.project_event_for_view(ConsoleTimelineEvent::ConsoleFrame {
            frame: stored.clone(),
        }) {
            ConsoleTimelineEvent::ConsoleFrame { frame } => {
                if let Some(shown) = not_redacted_by_the_view(&frame) {
                    wrong.push(format!(
                        "{whose} frame, projected as a live event, is not redacted by the \
                         view's policy: {shown}"
                    ));
                }
            }
            _ => wrong.push(format!(
                "{whose} frame, projected as a live event, is no frame event"
            )),
        }
    }
    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a view's payload policy does not redact a second mob's frame as the default \
         mob's:\n{}",
        wrong.join("\n")
    );
}

/// The status `GET /console/identities` answers on `app` with a sign-in of
/// `email`: the console's list of agents, read as a console reads it before
/// it reads a timeline.
async fn identities_read_as(app: &axum::Router, email: &str) -> u16 {
    use tower::ServiceExt;

    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/console/identities")
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", feed_row_token(email)),
        )
        .body(axum::body::Body::empty())
        .expect("GET request");
    tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the door answers in time")
        .expect("the door answers")
        .status()
        .as_u16()
}

/// One page of a timeline read over every mob
/// (`meta/console/query_timeline` with `"mobs": "*"`) on `app`, after the
/// cursor `after` (none: from the start), asked with a sign-in of `email`:
/// the texts of its frames that carry one, in order, and the cursor the
/// page hands out. An answer that is no page of frames is an error with
/// its status and body.
async fn every_mob_page_as(
    app: &axum::Router,
    after: Option<&str>,
    email: &str,
) -> Result<(Vec<String>, Option<String>), String> {
    let mut params = json!({ "mobs": "*" });
    if let Some(after) = after {
        params["after"] = json!(after);
    }
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", feed_row_token(email)),
        )
        .body(axum::body::Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": "row",
                "method": "meta/console/query_timeline",
                "params": params,
            })
            .to_string(),
        ))
        .expect("console request");
    let (frames, body) = frames_answered(app, request, "/result/frames").await?;
    let texts = frames
        .iter()
        .filter_map(|frame| frame.pointer("/payload/text").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let cursor = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|answer| {
            answer
                .pointer("/result/next_cursor")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Ok((texts, cursor))
}

/// What a reader over every mob is answered when it reads on from the
/// cursor `cursor` of its last page, page after page until a page holds no
/// frame (four pages at most): the texts it is answered, in order, and the
/// cursor of its last page. A page that is not answered ends the reading
/// and is told in the texts.
async fn read_on_as(
    app: &axum::Router,
    mut cursor: Option<String>,
    email: &str,
) -> (Vec<String>, Option<String>) {
    let mut texts = Vec::new();
    for _ in 0..4 {
        match every_mob_page_as(app, cursor.as_deref(), email).await {
            Ok((page, next)) => {
                let empty = page.is_empty();
                texts.extend(page);
                if next.is_some() {
                    cursor = next;
                }
                if empty {
                    break;
                }
            }
            Err(answer) => {
                texts.push(format!("the page is not answered: {answer}"));
                break;
            }
        }
    }
    (texts, cursor)
}

/// The texts of the frames `meta/console/query_timeline` answers on `app`
/// for the mob `mob`, asked from no cursor, for every agent of the mob, with
/// a sign-in of `email`. An answer that is no page of frames is an error
/// with its status and body.
async fn mob_page_as(app: &axum::Router, mob: &MobId, email: &str) -> Result<Vec<String>, String> {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", feed_row_token(email)),
        )
        .body(axum::body::Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": "row",
                "method": "meta/console/query_timeline",
                "params": { "mob_id": mob.as_str() },
            })
            .to_string(),
        ))
        .expect("console request");
    let (frames, _body) = frames_answered(app, request, "/result/frames").await?;
    Ok(frames
        .iter()
        .filter_map(|frame| frame.pointer("/payload/text").and_then(Value::as_str))
        .map(str::to_string)
        .collect())
}

/// A timeline read over every mob answers no frame of a mob that is not
/// loaded, and the cursor it hands out moves past that mob's frames: a
/// caller that follows the cursor is not answered them once the mob is
/// loaded either. A read of that mob's own timeline from no cursor answers
/// them once the mob is loaded, which is how a client reads a mob it opens.
///
/// A console that requires a sign-in is over a launch on a state
/// directory whose mob set made a second mob; each mob has an agent `desk`
/// with one stored frame. The mob set unloads the second mob: it is known
/// and not loaded, and the runtime holds no entry of it. The console's
/// administrator reads the timeline over every mob from the start
/// (`meta/console/query_timeline` with `"mobs": "*"`) and reads on from the
/// cursor of each page.
///
/// The launch mob is the control: the caller is answered its frame at the
/// first read, and a later frame of it when it reads on while the second
/// mob is still not loaded, which no read of the row loads. No frame of
/// the second mob is answered by either read.
///
/// The second mob is then loaded, as a request that names it loads it. Its
/// frame is answered from no cursor: by the read of the mob's own timeline,
/// by the read of its agent's own timeline, and by a read over every mob.
/// The caller that reads on from its own cursor is not answered that frame;
/// it is answered a frame the second mob's agent leaves after the load.
#[tokio::test]
async fn a_read_over_every_mob_answers_no_frame_of_a_mob_that_is_not_loaded() {
    use crate::access::{AccessControlConfig, AccessController};
    const OF_THE_LAUNCH: &str = "of-the-launch-desk";
    const OF_THE_LAUNCH_LATER: &str = "of-the-launch-desk-later";
    const OF_THE_SECOND: &str = "of-the-second-desk";
    const OF_THE_SECOND_LATER: &str = "of-the-second-desk-later";

    let stored = StoredHost::new();
    let mut host = stored.launch().await;
    host.set_access_controller(
        AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec![FEED_ROW_ADMIN.to_string()],
            ..AccessControlConfig::default()
        })
        .expect("access configuration"),
    );
    host.initialize_console_projection_with_policy(Arc::new(
        HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ))
    .expect("the runtime's console projection");
    let second_id = two_desks_with_a_frame_each(&host).await;
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": FEED_ROW_SIGNING_SECRET,
        "email_allowlist": [FEED_ROW_ADMIN],
    }))
    .expect("console sign-in configuration");
    let app = host.build_console_json_router(decisions);
    let holds = |texts: &[String], text: &str| texts.iter().any(|held| held == text);

    // The second mob is unloaded, and its entry with the runtime ends.
    asleep(&host, &[&second_id]).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while host.mob_entries_in_tests().contains(&second_id) && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut wrong = Vec::new();
    if host.mob_entries_in_tests().contains(&second_id) {
        wrong.push("the scene: the runtime still holds an entry of the unloaded mob".to_string());
    }

    // The caller's first read, from the start.
    let listed = identities_read_as(&app, FEED_ROW_ADMIN).await;
    if listed != 200 {
        wrong.push(format!(
            "the scene: the console's list of agents is not answered: {listed}"
        ));
    }
    let (at_first, cursor) = match every_mob_page_as(&app, None, FEED_ROW_ADMIN).await {
        Ok(page) => page,
        Err(answer) => {
            wrong.push(format!("the caller's first read is not answered: {answer}"));
            (Vec::new(), None)
        }
    };
    if !holds(&at_first, OF_THE_LAUNCH) {
        wrong.push(format!(
            "the control: the caller's first read does not answer the launch mob's frame: \
             {at_first:?}"
        ));
    }
    if cursor.is_none() {
        wrong.push("the caller's first read hands out no cursor".to_string());
    }

    // The control while the second mob is not loaded: a later frame of the
    // launch mob reaches the caller that reads on.
    record(&host, None, "desk", OF_THE_LAUNCH_LATER).await;
    let _ = frames_showing(&host, None, OF_THE_LAUNCH_LATER).await;
    let (while_not_loaded, cursor) = read_on_as(&app, cursor, FEED_ROW_ADMIN).await;
    if !holds(&while_not_loaded, OF_THE_LAUNCH_LATER) {
        wrong.push(format!(
            "the control: the caller that reads on while the second mob is not loaded is not \
             answered the launch mob's later frame: {while_not_loaded:?}"
        ));
    }
    if !known(&host).await.not_loaded.contains(&second_id) {
        wrong.push("the scene: a read over every mob loaded the second mob".to_string());
    }
    // No read over every mob answers a frame of the mob that is not loaded.
    if holds(&at_first, OF_THE_SECOND) || holds(&while_not_loaded, OF_THE_SECOND) {
        wrong.push(format!(
            "a read over every mob answers the frame of the mob that is not loaded: at the \
             first read {at_first:?}, reading on {while_not_loaded:?}"
        ));
    }

    // The second mob is loaded, as a request that names it loads it.
    if let Err(error) = host.resolve_mob(Some(&second_id)).await {
        wrong.push(format!(
            "the scene: the second mob does not load: {error:?}"
        ));
    }
    // Its frame is answered from no cursor once it is loaded: by a read
    // over every mob, by the read of the mob's own timeline and by the read
    // of its agent's own timeline.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let from_no_cursor = loop {
        let _ = identities_read_as(&app, FEED_ROW_ADMIN).await;
        let page = every_mob_page_as(&app, None, FEED_ROW_ADMIN)
            .await
            .map(|(texts, _)| texts)
            .unwrap_or_else(|answer| vec![format!("the page is not answered: {answer}")]);
        if holds(&page, OF_THE_SECOND) || tokio::time::Instant::now() >= deadline {
            break page;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    if !holds(&from_no_cursor, OF_THE_SECOND) {
        wrong.push(format!(
            "a read over every mob from no cursor, made once the second mob is loaded, does \
             not answer its frame: {from_no_cursor:?}"
        ));
    }
    match mob_page_as(&app, &second_id, FEED_ROW_ADMIN).await {
        Ok(of_the_mob) => {
            if !holds(&of_the_mob, OF_THE_SECOND) {
                wrong.push(format!(
                    "the read of the second mob's own timeline from no cursor, made once the \
                     mob is loaded, does not answer its frame: {of_the_mob:?}"
                ));
            }
        }
        Err(answer) => wrong.push(format!(
            "the read of the second mob's own timeline is not answered: {answer}"
        )),
    }
    match timeline_request_as(&app, "desk", &second_id, FEED_ROW_ADMIN).await {
        Ok((frames, _body)) => {
            if !frames.iter().any(|frame| {
                frame.pointer("/payload/text").and_then(Value::as_str) == Some(OF_THE_SECOND)
            }) {
                wrong.push(
                    "the read of the second mob's agent's own timeline from no cursor, made \
                     once the mob is loaded, does not answer its frame"
                        .to_string(),
                );
            }
        }
        Err(answer) => wrong.push(format!(
            "the read of the second mob's agent's own timeline is not answered: {answer}"
        )),
    }

    // The caller reads on from its own cursor: the frame it read past is
    // not answered, a frame the second mob's agent leaves now is.
    record(&host, Some(&second_id), "desk", OF_THE_SECOND_LATER).await;
    let _ = frames_showing(&host, Some(&second_id), OF_THE_SECOND_LATER).await;
    let (after_the_load, _cursor) = read_on_as(&app, cursor, FEED_ROW_ADMIN).await;
    if holds(&after_the_load, OF_THE_SECOND) {
        wrong.push(format!(
            "the caller that reads on from its cursor is answered the frame it read past \
             while the mob was not loaded: {after_the_load:?}"
        ));
    }
    if !holds(&after_the_load, OF_THE_SECOND_LATER) {
        wrong.push(format!(
            "the caller that reads on from its cursor is not answered the frame the second \
             mob's agent left once the mob was loaded: {after_the_load:?}"
        ));
    }

    drop(app);
    shut_down(&host, &[&second_id]).await;
    assert!(
        wrong.is_empty(),
        "a read over every mob and a mob that is not loaded:\n{}",
        wrong.join("\n")
    );
}
