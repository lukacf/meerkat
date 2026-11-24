//! The console's timeline read of one mob, and where that mob's frames end.
//!
//! A read that carries a cursor is refused when the cursor lies past the
//! end of what the read can answer. That end is the newest frame of the mob
//! the read is of. Another mob's frames, later or earlier, do not move it,
//! and a refusal names the newest cursor of the read's own mob and no
//! other. The same end bounds the search of a member's stored frames for
//! the member captured with them.
//!
//! The rows run over both stock stores, on a console with no runtime
//! registered (every stored frame is then served), with frames stored at
//! the store's own door.
//!
//! The last rows are of the session history pass such a read asks for:
//! which member a pass takes (that row registers a runtime for the default
//! mob and the console events of a second mob, each with a member of one
//! name), and which pass a read asks for.

#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::member_key::DefaultMob;

#[derive(Clone, Copy, Debug)]
enum Log {
    Memory,
    Sqlite,
}

/// A console over a new log, with the directory that holds a SQLite log's
/// file.
fn console(log: Log) -> (MetaConsoleAggregator, Option<tempfile::TempDir>) {
    match log {
        Log::Memory => (MetaConsoleAggregator::in_memory(), None),
        Log::Sqlite => {
            let dir = tempfile::tempdir().expect("a directory for the log");
            let store = SqliteConsoleLogStore::open(dir.path().join("console.sqlite3"))
                .expect("the log opens");
            (MetaConsoleAggregator::new(Arc::new(store)), Some(dir))
        }
    }
}

fn frame(key: &str, identity: &str, mob: Option<&meerkat_mob::MobId>) -> NewConsoleFrame {
    NewConsoleFrame {
        mob_id: mob.cloned(),
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms: 1,
        runtime_key: "runtime-a".to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(format!("{identity}-session")),
        kind: "text_complete".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "text": key }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// Store one frame at the store's own door and answer its cursor.
async fn stored(
    aggregator: &MetaConsoleAggregator,
    key: &str,
    identity: &str,
    mob: Option<&meerkat_mob::MobId>,
) -> ConsoleCursor {
    aggregator
        .inner
        .store
        .append_if_absent(frame(key, identity, mob))
        .await
        .expect("the frame is stored")
        .frame
        .cursor
}

/// The console's read of one mob's timeline after `cursor`.
async fn read_after(
    aggregator: &MetaConsoleAggregator,
    mob: Option<&meerkat_mob::MobId>,
    cursor: &ConsoleCursor,
) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
    aggregator
        .query_mob_timeline_windowed_for(
            ConsoleTimelineWindowQuery {
                mob_id: mob.cloned(),
                after: Some(cursor.clone()),
                ..Default::default()
            },
            None,
        )
        .await
}

/// The members of the frames a read answered, with their mobs.
fn members(page: ConsoleTimelineWindowPage) -> Vec<(String, Option<meerkat_mob::MobId>)> {
    page.frames
        .into_iter()
        .map(|frame| (frame.identity, frame.mob_id))
        .collect()
}

/// The newest cursor a refused read names, when the read was refused as one
/// whose cursor lies past the end. Any other answer comes back as its own
/// words.
fn named_end(
    refused: &ConsoleTimelineQueryResult<ConsoleTimelineWindowPage>,
) -> Result<Option<ConsoleCursor>, String> {
    match refused {
        Err(ConsoleTimelineQueryError::ReplayUnavailable { latest_cursor, .. }) => {
            Ok(latest_cursor.clone())
        }
        other => Err(format!("{other:?}")),
    }
}

/// A read of a mob after a cursor of that mob's own frames is answered,
/// also where every frame of that mob is later than the newest frame of the
/// host's default mob.
async fn a_read_after_a_cursor_of_its_own_mob_is_answered(log: Log) {
    let (aggregator, _dir) = console(log);
    let second = meerkat_mob::MobId::from("second");
    let of_the_default_mob = stored(&aggregator, "launch-1", "alice", None).await;
    let first = stored(&aggregator, "second-1", "alice", Some(&second)).await;
    stored(&aggregator, "second-2", "bob", Some(&second)).await;

    let page = read_after(&aggregator, Some(&second), &first).await;
    assert_eq!(
        page.map(members).map_err(|refusal| refusal.to_string()),
        Ok(vec![("bob".to_string(), Some(second.clone()))]),
        "{log:?}: the second mob's read after its own first frame answers its later frame"
    );
    let page = read_after(&aggregator, None, &of_the_default_mob).await;
    assert_eq!(
        page.map(members).map_err(|refusal| refusal.to_string()),
        Ok(Vec::new()),
        "{log:?}: the default mob's read after its newest frame answers no frame"
    );
}

/// A read whose cursor lies past the newest frame of its own mob is
/// refused, and the refusal names that mob's newest cursor: not a later
/// frame's of another mob, and not an earlier one's.
async fn a_refused_read_names_the_newest_cursor_of_its_own_mob(log: Log) {
    let (aggregator, _dir) = console(log);
    let second = meerkat_mob::MobId::from("second");
    stored(&aggregator, "launch-1", "alice", None).await;
    stored(&aggregator, "second-1", "alice", Some(&second)).await;
    let newest_of_the_second = stored(&aggregator, "second-2", "bob", Some(&second)).await;
    let newest_of_the_default = stored(&aggregator, "launch-2", "carol", None).await;
    let past_every_frame = ConsoleCursor::from_seq(99);

    // The default mob holds the log's last frame.
    let refused = read_after(&aggregator, Some(&second), &past_every_frame).await;
    assert_eq!(
        named_end(&refused),
        Ok(Some(newest_of_the_second.clone())),
        "{log:?}: the second mob's refused read names the second mob's newest cursor: \
         {refused:?}"
    );
    // A cursor of the default mob's later frame is past the second mob's end.
    let refused = read_after(&aggregator, Some(&second), &newest_of_the_default).await;
    assert_eq!(
        named_end(&refused),
        Ok(Some(newest_of_the_second.clone())),
        "{log:?}: a cursor past the second mob's newest frame is refused for the second \
         mob: {refused:?}"
    );

    // The second mob holds the log's last frame.
    stored(&aggregator, "second-3", "bob", Some(&second)).await;
    let refused = read_after(&aggregator, None, &past_every_frame).await;
    assert_eq!(
        named_end(&refused),
        Ok(Some(newest_of_the_default)),
        "{log:?}: the default mob's refused read names the default mob's newest cursor: \
         {refused:?}"
    );
}

#[tokio::test]
async fn a_second_mobs_read_after_its_own_cursor_is_answered_on_sqlite() {
    a_read_after_a_cursor_of_its_own_mob_is_answered(Log::Sqlite).await;
}

#[tokio::test]
async fn a_second_mobs_read_after_its_own_cursor_is_answered_in_memory() {
    a_read_after_a_cursor_of_its_own_mob_is_answered(Log::Memory).await;
}

#[tokio::test]
async fn a_refused_read_names_the_newest_cursor_of_its_own_mob_on_sqlite() {
    a_refused_read_names_the_newest_cursor_of_its_own_mob(Log::Sqlite).await;
}

#[tokio::test]
async fn a_refused_read_names_the_newest_cursor_of_its_own_mob_in_memory() {
    a_refused_read_names_the_newest_cursor_of_its_own_mob(Log::Memory).await;
}

/// Register the console events of a second mob with `aggregator`, as a host
/// does for a mob it serves beside its default mob: that mob's runtime
/// (which the row shuts down), its id and the runtime key of its
/// registration.
async fn second_mob_of(
    aggregator: &MetaConsoleAggregator,
) -> (UnifiedRuntime, meerkat_mob::MobId, String) {
    let second = meerkat_mob::MobId::from(super::tests::unique_console_mob_id("read-bound-second"));
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    aggregator.tell_default_mob(DefaultMob::new(meerkat_mob::MobId::from(
        "read-bound-launch",
    )));
    aggregator
        .register_mob_console_events(
            &second,
            runtime.mob_runtime().clone(),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        )
        .expect("the second mob's console events register");
    let runtime_key = format!("mob/{}", second.as_str());
    (runtime, second, runtime_key)
}

/// The member captured with a member's stored frames is found again when
/// the member is no longer live and the process keeps nothing of it, also
/// where those frames are later than the newest frame of the host's default
/// mob. The state the row needs: a mob the host serves beside its default
/// mob, a member of it that has left, and a process that started after the
/// member's frames were stored (here: a member the runtime never held, and
/// a console that captured nothing). The next frame of that member's
/// session is then stored with the member captured for it, and every read
/// decides it as that member's.
async fn a_departed_members_captured_member_is_found_among_its_stored_frames(log: Log) {
    let (aggregator, _dir) = console(log);
    let (runtime, second, runtime_key) = second_mob_of(&aggregator).await;
    let entry =
        registered_entry(&aggregator.inner, &runtime_key).expect("the second mob's registration");
    let captured: ConsoleFrameMemberProvenance = serde_json::from_value(json!({
        "identity": {"identity": "agent-a", "display_name": "Agent", "runtime_key": runtime_key,
            "runtime_member_id": "agent-a", "session_id": "session-a",
            "visibility": "retired_readable", "addressable": false, "health": "retired",
            "labels": {"tier": "kept"}},
        "member": {"agent_identity": "agent-a", "role": "worker", "state": "retired",
            "session_id": "session-a", "wired_to": [], "labels": {"tier": "kept"}},
        "primary_mob_id": second.as_str(), "source_mob_id": second.as_str()
    }))
    .expect("the captured member");

    // The default mob's newest frame, and after it the member's frame with
    // the member captured for it.
    stored(&aggregator, "launch-1", "alice", None).await;
    let mut of_the_member = frame("second-agent-a-1", "agent-a", Some(&second));
    of_the_member.runtime_key.clone_from(&runtime_key);
    of_the_member.session_id = Some("session-a".to_string());
    of_the_member.source.member_provenance = Some(captured.clone());
    aggregator
        .inner
        .store
        .append_if_absent(of_the_member)
        .await
        .expect("the member's frame is stored");

    let found = member_provenance_for_identity(
        &aggregator.inner,
        &entry,
        "agent-a",
        Some("session-a"),
        false,
    )
    .await;
    runtime.shutdown().await;
    assert_eq!(
        found,
        Some(captured),
        "{log:?}: the member captured with the second mob's member's stored frame is found"
    );
}

#[tokio::test]
async fn a_second_mobs_departed_members_captured_member_is_found_on_sqlite() {
    a_departed_members_captured_member_is_found_among_its_stored_frames(Log::Sqlite).await;
}

#[tokio::test]
async fn a_second_mobs_departed_members_captured_member_is_found_in_memory() {
    a_departed_members_captured_member_is_found_among_its_stored_frames(Log::Memory).await;
}

/// What a session history pass takes for already stored, for a member of a
/// mob the host serves beside its default mob, holds that member's stored
/// frames also where they are later than the newest frame of the host's
/// default mob: the pass reads them as stored, and what it writes next says
/// so.
async fn a_members_stored_history_is_seen_past_the_default_mobs_newest_frame(log: Log) {
    let (aggregator, _dir) = console(log);
    let (runtime, second, runtime_key) = second_mob_of(&aggregator).await;

    // The default mob's newest frame, and after it a notice of the member's
    // session as its history holds it.
    stored(&aggregator, "launch-1", "alice", None).await;
    let mut notice = frame("second-agent-a-notice", "agent-a", Some(&second));
    notice.runtime_key.clone_from(&runtime_key);
    notice.session_id = Some("session-a".to_string());
    notice.kind = "system_notice".to_string();
    notice.payload = json!({ "message": { "runtime_origin": {} } });
    let of_the_notice = aggregator
        .inner
        .store
        .append_if_absent(notice)
        .await
        .expect("the member's notice is stored")
        .frame
        .cursor;

    let observation =
        observe_runtime_notice_attempts(&aggregator.inner, &runtime_key, "agent-a", "session-a")
            .await;
    runtime.shutdown().await;
    let observation = observation.expect("the member's stored history is read");
    assert!(
        observation.has_notice_history,
        "{log:?}: the member's stored notice is seen"
    );
    assert!(
        of_the_notice
            .seq()
            .is_some_and(|seq| observation.observed_through >= seq),
        "{log:?}: the read reaches the member's newest stored frame ({of_the_notice}): it \
         ended at {}",
        observation.observed_through
    );
}

#[tokio::test]
async fn a_second_mobs_members_stored_history_is_seen_on_sqlite() {
    a_members_stored_history_is_seen_past_the_default_mobs_newest_frame(Log::Sqlite).await;
}

#[tokio::test]
async fn a_second_mobs_members_stored_history_is_seen_in_memory() {
    a_members_stored_history_is_seen_past_the_default_mobs_newest_frame(Log::Memory).await;
}

/// One of the twin frames of the row below: the same member name,
/// conversation, session, interaction, turn and run in every mob.
fn twin(key: &str, id: &str, mob: Option<&meerkat_mob::MobId>) -> NewConsoleFrame {
    NewConsoleFrame {
        id: Some(id.to_string()),
        session_id: Some("session-1".to_string()),
        interaction_id: Some("interaction-1".to_string()),
        turn_id: Some("turn-1".to_string()),
        run_id: Some("run-1".to_string()),
        ..frame(key, "alice", mob)
    }
}

/// Two mobs' frames that share every name the log lets two frames share
/// (the member's name, its conversation, session, interaction, turn and
/// run) are written to a SQLite log, the log is opened again by a new
/// store, and each mob's read answers that mob's frames and none of the
/// other's. A frame key or a frame id is one frame's in the whole log: the
/// second mob's frame under the default mob's key, or under its id, is
/// refused and nothing of it is stored. A read that names no mob answers
/// the default mob's frames alone, by page and by key.
#[tokio::test]
async fn two_mobs_frames_read_apart_after_the_log_is_opened_again() {
    let dir = tempfile::tempdir().expect("a directory for the log");
    let path = dir.path().join("console.sqlite3");
    let second = meerkat_mob::MobId::from("second");
    {
        let store = SqliteConsoleLogStore::open(&path).expect("the log opens");
        for (key, id, mob) in [
            ("launch-1", "frame-launch-1", None),
            ("second-1", "frame-second-1", Some(&second)),
            ("launch-2", "frame-launch-2", None),
            ("second-2", "frame-second-2", Some(&second)),
        ] {
            store
                .append_if_absent(twin(key, id, mob))
                .await
                .expect("the frame is stored");
        }
        let under_the_same_key = store
            .append_if_absent(twin("launch-1", "frame-second-3", Some(&second)))
            .await;
        assert!(
            under_the_same_key.is_err(),
            "the second mob's frame under the default mob's key is refused: \
             {under_the_same_key:?}"
        );
        let under_the_same_id = store
            .append_if_absent(twin("second-4", "frame-launch-1", Some(&second)))
            .await;
        assert!(
            under_the_same_id.is_err(),
            "the second mob's frame under the default mob's frame id is refused: \
             {under_the_same_id:?}"
        );
    }

    let store = Arc::new(SqliteConsoleLogStore::open(&path).expect("the log opens again"));
    let keys = |page: ConsoleTimelineWindowPage| {
        page.frames
            .into_iter()
            .map(|frame| (frame.dedupe_key, frame.id, frame.mob_id))
            .collect::<Vec<_>>()
    };
    let of_the_default_mob = [
        (
            "launch-1".to_string(),
            "frame-launch-1".to_string(),
            None::<meerkat_mob::MobId>,
        ),
        ("launch-2".to_string(), "frame-launch-2".to_string(), None),
    ];
    let of_the_second_mob = [
        (
            "second-1".to_string(),
            "frame-second-1".to_string(),
            Some(second.clone()),
        ),
        (
            "second-2".to_string(),
            "frame-second-2".to_string(),
            Some(second.clone()),
        ),
    ];
    let of_mob = |mob: Option<&meerkat_mob::MobId>| ConsoleTimelineWindowQuery {
        mob_id: mob.cloned(),
        ..Default::default()
    };
    assert_eq!(
        keys(
            store
                .query_windowed_frames_of_mob(of_mob(None))
                .await
                .expect("the default mob's frames")
        ),
        of_the_default_mob,
        "the default mob's read answers its two frames and none of the second mob's"
    );
    assert_eq!(
        keys(
            store
                .query_windowed_frames_of_mob(of_mob(Some(&second)))
                .await
                .expect("the second mob's frames")
        ),
        of_the_second_mob,
        "the second mob's read answers its two frames and none of the default mob's"
    );
    assert_eq!(
        keys(
            store
                .query_windowed_frames(ConsoleTimelineWindowQuery::default())
                .await
                .expect("the page that names no mob")
        ),
        of_the_default_mob,
        "a page that names no mob answers the default mob's frames alone"
    );
    assert_eq!(
        store
            .frame_by_dedupe_key("launch-1")
            .await
            .expect("the read by the default mob's key")
            .map(|frame| frame.id),
        Some("frame-launch-1".to_string()),
        "a read by key that names no mob answers the default mob's frame"
    );
    assert_eq!(
        store
            .frame_by_dedupe_key("second-1")
            .await
            .expect("the read by the second mob's key"),
        None,
        "a read by key that names no mob answers no frame of the second mob"
    );

    let aggregator = MetaConsoleAggregator::new(store);
    for (mob, expected) in [
        (None, vec![("alice".to_string(), None); 2]),
        (
            Some(&second),
            vec![("alice".to_string(), Some(second.clone())); 2],
        ),
    ] {
        assert_eq!(
            aggregator
                .query_mob_timeline_windowed_for(of_mob(mob), None)
                .await
                .map(members)
                .map_err(|refusal| refusal.to_string()),
            Ok(expected),
            "the console's read of {mob:?} answers that mob's frames alone"
        );
    }
}

/// What a session history pass takes: the mob of the registration it reads
/// its member from (none: a runtime registered for the default mob), and
/// the member's session.
type Taken = std::collections::BTreeSet<(Option<String>, String)>;

/// What a session history pass asked for the member `alice` of `mob` (none:
/// the default mob) takes.
async fn taken(aggregator: &MetaConsoleAggregator, mob: Option<&meerkat_mob::MobId>) -> Taken {
    Box::pin(session_backfill_targets_for_identity_of(
        &aggregator.inner,
        mob,
        "alice",
    ))
    .await
    .into_iter()
    .map(|target| {
        (
            target.entry.mob.map(|mob| mob.as_str().to_string()),
            target.session_id,
        )
    })
    .collect()
}

/// A session history pass asked for one mob's member is that mob's
/// member's alone, also where another mob has a member of the same name.
/// The host's default mob and a mob it serves beside it each have a member
/// `alice`, each with a session of its own. The pass asked for the second
/// mob's `alice` takes that mob's member's session and no other. The pass
/// asked for `alice` with no mob named takes the default mob's member's
/// session and no other. A mob the console holds no events of has no
/// member to take. Each half is the other's control: a pass that went by
/// the name alone would take one session for both, or both sessions for
/// one.
///
/// The console makes no pass of its own here (its session history passes
/// are off), so the row reads what a pass would take and nothing else.
#[tokio::test]
async fn a_session_history_pass_for_one_mobs_member_takes_no_member_of_that_name_in_another_mob() {
    let launch = super::tests::build_empty_runtime(&super::tests::unique_console_mob_id(
        "read-bound-launch-of-two",
    ))
    .await;
    let second = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(
        "read-bound-second-of-two",
    ));
    let hosted = super::tests::build_empty_runtime(second.as_str()).await;
    for (runtime, whose) in [
        (&launch, "the default mob's"),
        (&hosted, "the second mob's"),
    ] {
        let seated =
            Box::pin(runtime.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "alice"))).await;
        assert!(seated.is_ok(), "{whose} alice: {:?}", seated.as_ref().err());
    }
    let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
        session_history_backfill_enabled: false,
        ..Default::default()
    });
    aggregator.tell_default_mob(DefaultMob::new(launch.mob_handle().mob_id().clone()));
    aggregator.register_runtime_handles_with_policy(
        "runtime-a",
        "",
        launch.mob_runtime().clone(),
        None,
        launch.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    aggregator
        .register_mob_console_events(
            &second,
            hosted.mob_runtime().clone(),
            hosted.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        )
        .expect("the second mob's console events register");

    // A member's session is bound a moment after it is seated.
    let no_such_mob = meerkat_mob::MobId::from("read-bound-no-such-mob");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let (of_the_default, of_the_second) = loop {
        let of_the_default = taken(&aggregator, None).await;
        let of_the_second = taken(&aggregator, Some(&second)).await;
        if (!of_the_default.is_empty() && !of_the_second.is_empty())
            || tokio::time::Instant::now() >= deadline
        {
            break (of_the_default, of_the_second);
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let of_no_such_mob = taken(&aggregator, Some(&no_such_mob)).await;
    let alice = meerkat_mob::AgentIdentity::from("alice");
    let session_of_the_default = launch
        .mob_handle()
        .resolve_bridge_session_id(&alice)
        .await
        .map(|session| session.to_string());
    let session_of_the_second = hosted
        .mob_handle()
        .resolve_bridge_session_id(&alice)
        .await
        .map(|session| session.to_string());
    launch.shutdown().await;
    hosted.shutdown().await;

    let (Some(session_of_the_default), Some(session_of_the_second)) =
        (session_of_the_default, session_of_the_second)
    else {
        panic!("each mob's alice has a session");
    };
    assert_ne!(
        session_of_the_default, session_of_the_second,
        "the two members of one name have a session each"
    );
    assert_eq!(
        of_the_second,
        Taken::from([(Some(second.as_str().to_string()), session_of_the_second)]),
        "the pass asked for the second mob's member takes that mob's member's session alone"
    );
    assert_eq!(
        of_the_default,
        Taken::from([(None, session_of_the_default)]),
        "the pass asked with no mob named takes the default mob's member's session alone"
    );
    assert_eq!(
        of_no_such_mob,
        Taken::new(),
        "a mob the console holds no events of has no member to take"
    );
}

/// A timeline read of one mob's member that finds no stored frame of it
/// asks a session history pass for THAT mob's member: the pass is asked by
/// the member's name with the read's own mob, so it never finds the default
/// mob's member of that name. A read that names the host's default mob by
/// its id asks the pass for the default mob's member, as a read that names
/// no mob does: under that id the pass would find no registration and
/// start nothing.
///
/// The row reads what was asked ([`HistoryPassAsked`]); no runtime is
/// registered, so nothing else asks a pass.
async fn a_read_of_one_mobs_member_asks_the_history_pass_of_that_mobs_member(log: Log) {
    let (aggregator, _dir) = console(log);
    let launch = meerkat_mob::MobId::from("read-bound-launch");
    let second = meerkat_mob::MobId::from("read-bound-second");
    aggregator.tell_default_mob(DefaultMob::new(launch.clone()));
    let mut reads = Vec::new();
    for mob in [Some(&second), Some(&launch), None] {
        reads.push(
            aggregator
                .query_mob_timeline_windowed_for(
                    ConsoleTimelineWindowQuery {
                        mob_id: mob.cloned(),
                        identity: Some("alice".to_string()),
                        ..Default::default()
                    },
                    None,
                )
                .await
                .map(|page| page.frames.len())
                .map_err(|refusal| refusal.to_string()),
        );
    }
    let of = |mob: Option<&meerkat_mob::MobId>| HistoryPassAsked::OfMember {
        mob: mob.cloned(),
        identity: "alice".to_string(),
    };

    assert_eq!(
        reads,
        [Ok(0), Ok(0), Ok(0)],
        "{log:?}: each read is answered, with no frame"
    );
    assert_eq!(
        aggregator.history_passes_asked(),
        [of(Some(&second)), of(None), of(None)],
        "{log:?}: the passes asked, in order: by the read of the second mob's member, of the \
         default mob named by its id, and of no mob"
    );
}

#[tokio::test]
async fn a_read_of_one_mobs_member_asks_the_history_pass_of_that_mobs_member_on_sqlite() {
    a_read_of_one_mobs_member_asks_the_history_pass_of_that_mobs_member(Log::Sqlite).await;
}

#[tokio::test]
async fn a_read_of_one_mobs_member_asks_the_history_pass_of_that_mobs_member_in_memory() {
    a_read_of_one_mobs_member_asks_the_history_pass_of_that_mobs_member(Log::Memory).await;
}
