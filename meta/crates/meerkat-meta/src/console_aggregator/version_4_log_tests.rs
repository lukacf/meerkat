//! A console log at schema version 4 under this release.
//!
//! A later release stores, in the same `console_frames` table, the frames of
//! mobs a gateway hosts beside the one it was launched with, and moves the
//! log to schema version 4 when it stores the first of them: one added
//! column, `mob_id` (empty for the launch mob's frames, so no stored row is
//! rewritten), an index by mob and identity in place of the index by
//! identity, and an index by mob and cursor. This release stores no such
//! frame and never moves a log. It opens a log that was moved, and reads
//! and changes the launch mob's frames alone: a moved log reads as the same
//! log without the other mobs' frames.
//!
//! The version 4 logs of these rows are written here, by SQL, from that
//! description: a log this release wrote, the four statements of the move,
//! the ledger row, and the frames of another mob.

use super::*;

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

/// The move to schema version 4, as the later release makes it, in that
/// release's own statements: this release checks a moved log's tables and
/// indexes against them when it opens the log, down to the statement text
/// (runs of white space aside).
const MOVE_TO_VERSION_4: &str = "
    ALTER TABLE console_frames ADD COLUMN mob_id TEXT NOT NULL DEFAULT '';
    DROP INDEX idx_console_frames_identity_cursor;
    CREATE INDEX idx_console_frames_mob_identity_cursor
        ON console_frames(mob_id, identity, cursor_seq);
    CREATE INDEX idx_console_frames_mob_cursor
        ON console_frames(mob_id, cursor_seq);
    UPDATE meerkat_schema SET version = 4 WHERE domain = 'meta-console';
";

/// A frame of another mob for each frame of the launch mob the log holds:
/// the same member name, runtime key, conversation, kind, status, send and
/// message, under the mob `crew`, newer than every frame of the launch mob,
/// its text marked `HOSTED` where the launch mob's says `LAUNCH`.
const ANOTHER_MOBS_FRAMES: &str = "
    INSERT INTO console_frames (
        id, dedupe_key, timestamp_ms, runtime_key, identity, conversation_id, session_id,
        kind, status, frame_version, updated_at_ms, payload_json, source_kind, source_cursor,
        source_event_id, interaction_id, parent_frame_id, caused_by_frame_id, turn_id, run_id,
        member_provenance_json, member_provenance_id, update_marker_shares_provenance, mob_id
    )
    SELECT
        'hosted-' || id, 'hosted-' || dedupe_key, timestamp_ms + 1000, runtime_key, identity,
        conversation_id, session_id, kind, status, frame_version, updated_at_ms,
        replace(payload_json, 'LAUNCH', 'HOSTED'), source_kind, source_cursor,
        source_event_id, interaction_id, parent_frame_id, caused_by_frame_id, turn_id, run_id,
        member_provenance_json, member_provenance_id, update_marker_shares_provenance, 'crew'
    FROM console_frames WHERE mob_id = '' ORDER BY cursor_seq;
";

const RUNTIME: &str = "runtime-a";
const MESSAGE: &str = "message-1";
const INTERACTION: &str = "interaction-1";

fn text(key: &str, identity: &str, timestamp_ms: u64) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms,
        runtime_key: RUNTIME.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some("session-1".to_string()),
        kind: "text_delta".to_string(),
        status: ConsoleFrameStatus::Delivered,
        payload: json!({ "delta": format!("LAUNCH {key}") }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
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

/// A console send to `identity`, accepted and not settled.
fn send(key: &str, identity: &str, timestamp_ms: u64) -> NewConsoleFrame {
    NewConsoleFrame {
        kind: "user_input".to_string(),
        status: ConsoleFrameStatus::Accepted,
        payload: json!({ "content": format!("LAUNCH {key}"), "delivery_hold": { "by": "LAUNCH" } }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::Send,
            source_cursor: Some("fingerprint".to_string()),
        },
        source_event_id: None,
        interaction_id: Some(INTERACTION.to_string()),
        ..text(key, identity, timestamp_ms)
    }
}

/// One streamed row of the text block of [`MESSAGE`], or its complete row.
fn block(key: &str, kind: &str, sequence: u64) -> NewConsoleFrame {
    NewConsoleFrame {
        kind: kind.to_string(),
        payload: json!({
            "delta": format!("LAUNCH {key}"),
            "assistant_message_id": MESSAGE,
            "source_sequence": sequence,
        }),
        ..text(key, "alice", 100 + sequence)
    }
}

/// The member a frame was captured for, as the launch mob's frames carry it.
fn witness() -> crate::ConsoleFrameMemberProvenance {
    serde_json::from_value(json!({
        "identity": {"identity":"alice", "display_name":"Alice", "runtime_key":RUNTIME,
            "runtime_member_id":"alice", "session_id":"session-1", "visibility":"addressable",
            "addressable":true, "health":"ready", "labels":{}},
        "member": {"agent_identity":"alice", "role":"worker", "state":"running",
            "session_id":"session-1", "wired_to":[], "labels":{}},
        "primary_mob_id":"primary", "source_mob_id":"primary"
    }))
    .expect("witness")
}

/// The launch mob's frames of the rows' log, in the order they are stored.
fn launch_frames() -> Vec<NewConsoleFrame> {
    let mut witnessed = text("alice-witnessed", "alice", 10);
    witnessed.source.member_provenance = Some(witness());
    vec![
        witnessed,
        text("alice-1", "alice", 20),
        text("bob-1", "bob", 30),
        send("alice-send", "alice", 40),
        text("alice-2", "alice", 50),
    ]
}

/// A log this release wrote, holding [`launch_frames`]: schema version 3.
async fn version_3_log(path: &std::path::Path) -> Result<Vec<ConsoleFrame>, Failure> {
    let store = SqliteConsoleLogStore::open(path)?;
    let mut stored = Vec::new();
    for frame in launch_frames() {
        stored.push(store.append_if_absent(frame).await?.frame);
    }
    Ok(stored)
}

/// [`version_3_log`], moved to schema version 4 and holding a frame of
/// another mob for each frame of the launch mob.
async fn version_4_log(path: &std::path::Path) -> Result<Vec<ConsoleFrame>, Failure> {
    let stored = version_3_log(path).await?;
    let conn = rusqlite::Connection::open(path)?;
    conn.execute_batch(MOVE_TO_VERSION_4)?;
    conn.execute_batch(ANOTHER_MOBS_FRAMES)?;
    Ok(stored)
}

/// A copy of the log at `from` without the frames of another mob.
fn without_another_mobs_frames(
    from: &std::path::Path,
    to: &std::path::Path,
) -> Result<(), Failure> {
    let conn = rusqlite::Connection::open(from)?;
    conn.execute("VACUUM INTO ?1", [to.to_string_lossy().as_ref()])?;
    drop(conn);
    let conn = rusqlite::Connection::open(to)?;
    conn.execute("DELETE FROM console_frames WHERE mob_id <> ''", [])?;
    Ok(())
}

/// The schema version the log's ledger holds.
fn ledger_version(path: &std::path::Path) -> Result<Option<i64>, Failure> {
    let conn = rusqlite::Connection::open(path)?;
    Ok(meerkat_sqlite::domain_version(&conn, "meta-console")?)
}

/// The log's tables and indexes, as the file declares them.
fn catalog(path: &std::path::Path) -> Result<Vec<String>, Failure> {
    let conn = rusqlite::Connection::open(path)?;
    let mut statement = conn.prepare(
        "SELECT type || ' ' || name || ': ' || COALESCE(sql, '') FROM sqlite_master \
         WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every stored row the SQL `filter` selects, each column as text, in
/// cursor order.
fn rows(path: &std::path::Path, filter: &str) -> Result<Vec<String>, Failure> {
    let conn = rusqlite::Connection::open(path)?;
    let mut statement = conn.prepare(&format!(
        "SELECT cursor_seq || '|' || id || '|' || dedupe_key || '|' || identity || '|' || kind \
         || '|' || status || '|' || frame_version || '|' || COALESCE(updated_at_ms, '') || '|' \
         || payload_json || '|' || COALESCE(member_provenance_id, '') \
         FROM console_frames WHERE {filter} ORDER BY cursor_seq"
    ))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn another_mobs_rows(path: &std::path::Path) -> Result<Vec<String>, Failure> {
    rows(path, "mob_id <> ''")
}

fn window(
    identity: Option<&str>,
    conversation_id: Option<&str>,
    mode: ConsoleTimelineMode,
    limit: usize,
) -> ConsoleTimelineWindowQuery {
    ConsoleTimelineWindowQuery {
        identity: identity.map(str::to_string),
        conversation_id: conversation_id.map(str::to_string),
        mode,
        limit,
        ..ConsoleTimelineWindowQuery::default()
    }
}

/// Every read of the store, over the launch mob's member names and over the
/// keys and ids of the launch mob's frames and of another mob's.
async fn every_read(
    store: &SqliteConsoleLogStore,
    stored: &[ConsoleFrame],
) -> Result<Value, Failure> {
    let mut reads = serde_json::Map::new();
    for (name, query) in [
        ("since", window(None, None, ConsoleTimelineMode::Since, 200)),
        ("recent", window(None, None, ConsoleTimelineMode::Recent, 3)),
        (
            "since of alice",
            window(Some("alice"), None, ConsoleTimelineMode::Since, 200),
        ),
        (
            "recent of alice",
            window(Some("alice"), None, ConsoleTimelineMode::Recent, 2),
        ),
        (
            "recent of bob's conversation",
            window(None, Some("bob"), ConsoleTimelineMode::Recent, 2),
        ),
    ] {
        let page = store.query_windowed_frames(query).await?;
        reads.insert(name.to_string(), serde_json::to_value(page)?);
    }
    let page = store.query_frames(ConsoleTimelineQuery::default()).await?;
    reads.insert("frames".to_string(), serde_json::to_value(page)?);
    reads.insert(
        "latest cursor".to_string(),
        serde_json::to_value(store.latest_cursor().await?)?,
    );
    for frame in stored {
        reads.insert(
            format!("by key {}", frame.dedupe_key),
            serde_json::to_value(store.frame_by_dedupe_key(&frame.dedupe_key).await?)?,
        );
        // The key of the other mob's frame of the same member and text.
        reads.insert(
            format!("by the other mob's key of {}", frame.dedupe_key),
            serde_json::to_value(
                store
                    .frame_by_dedupe_key(&format!("hosted-{}", frame.dedupe_key))
                    .await?,
            )?,
        );
    }
    reads.insert(
        "send by interaction".to_string(),
        serde_json::to_value(store.send_frame_by_interaction(INTERACTION).await?)?,
    );
    reads.insert(
        "unsettled sends".to_string(),
        serde_json::to_value(store.unsettled_send_frames(1000, 100).await?)?,
    );
    let mut activity: Vec<(String, u64)> = store
        .latest_activity_times(&["alice".to_string(), "bob".to_string()])
        .await?
        .into_iter()
        .collect();
    activity.sort();
    reads.insert("last activity".to_string(), serde_json::to_value(activity)?);
    Ok(Value::Object(reads))
}

/// NO MIXING. A version 4 log that holds, for every frame of the launch
/// mob, a newer frame of another mob under the same member name, runtime
/// key, conversation, send and message answers every read exactly as the
/// same log without the other mob's frames: no page, cursor, frame by key,
/// send or last activity is another mob's. The aggregator's timeline over
/// that log shows a viewer the launch mob's frames alone.
#[tokio::test]
async fn a_version_4_log_reads_as_the_same_log_without_another_mobs_frames() -> TestResult {
    let dir = tempfile::tempdir()?;
    let moved = dir.path().join("moved.sqlite3");
    let stored = version_4_log(&moved).await?;
    let alone = dir.path().join("alone.sqlite3");
    without_another_mobs_frames(&moved, &alone)?;
    assert_eq!(
        another_mobs_rows(&moved)?.len(),
        stored.len(),
        "the log holds a frame of another mob for each of the launch mob's"
    );

    let store = SqliteConsoleLogStore::open(&moved)?;
    let reads = every_read(&store, &stored).await?;
    let expected = every_read(&SqliteConsoleLogStore::open(&alone)?, &stored).await?;
    assert_eq!(reads, expected, "{reads:#}");
    assert!(
        !reads.to_string().contains("HOSTED") && !reads.to_string().contains("hosted-"),
        "no read answers a frame of another mob: {reads:#}"
    );
    assert_eq!(
        reads["since"]["frames"].as_array().map(Vec::len),
        Some(stored.len()),
        "the launch mob's frames are all read: {reads:#}"
    );

    let recent = || window(None, None, ConsoleTimelineMode::Recent, 200);
    let shown = MetaConsoleAggregator::new(Arc::new(store))
        .query_timeline_windowed(recent())
        .await?;
    let shown_alone = MetaConsoleAggregator::new(Arc::new(SqliteConsoleLogStore::open(&alone)?))
        .query_timeline_windowed(recent())
        .await?;
    let shown = serde_json::to_string(&shown)?;
    assert_eq!(shown, serde_json::to_string(&shown_alone)?);
    assert!(
        !shown.contains("HOSTED") && !shown.contains("hosted-"),
        "a viewer of the launch mob is shown its frames alone: {shown}"
    );
    Ok(())
}

/// Opening a version 4 log, reading it and closing it leaves it at version
/// 4 with its tables, its indexes and the other mob's rows as they were.
#[tokio::test]
async fn a_version_4_log_is_opened_and_not_moved() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    let stored = version_4_log(&path).await?;
    let declared = catalog(&path)?;
    let another_mobs = another_mobs_rows(&path)?;

    let store = SqliteConsoleLogStore::open(&path)?;
    every_read(&store, &stored).await?;
    drop(store);

    assert_eq!(ledger_version(&path)?, Some(4));
    assert_eq!(catalog(&path)?, declared);
    assert_eq!(another_mobs_rows(&path)?, another_mobs);
    Ok(())
}

/// A log that is moved to schema version 4 while this release holds it open
/// (another connection makes the move and stores another mob's frames) is
/// served as a version 4 log from the next statement on, with no reopening:
/// every read answers what it answered before the move, which is what the
/// same log without the other mob's frames answers; a change by the id of
/// another mob's frame finds nothing; an append stores the launch mob's
/// frame. The log stays at version 4 with the other mob's rows as they were.
#[tokio::test]
async fn a_log_moved_while_it_is_open_is_served_as_a_version_4_log() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    let stored = version_3_log(&path).await?;
    let store = SqliteConsoleLogStore::open(&path)?;
    let before = every_read(&store, &stored).await?;

    let conn = rusqlite::Connection::open(&path)?;
    conn.execute_batch(MOVE_TO_VERSION_4)?;
    conn.execute_batch(ANOTHER_MOBS_FRAMES)?;
    drop(conn);
    let another_mobs = another_mobs_rows(&path)?;
    assert_eq!(another_mobs.len(), stored.len());
    let alone = dir.path().join("alone.sqlite3");
    without_another_mobs_frames(&path, &alone)?;

    let reads = every_read(&store, &stored).await?;
    let expected = every_read(&SqliteConsoleLogStore::open(&alone)?, &stored).await?;
    assert_eq!(reads, expected, "{reads:#}");
    assert_eq!(
        reads, before,
        "the reads are what they were before the move"
    );
    assert!(
        !reads.to_string().contains("HOSTED") && !reads.to_string().contains("hosted-"),
        "no read answers a frame of another mob: {reads:#}"
    );

    let launch_send = stored
        .iter()
        .find(|frame| frame.dedupe_key == "alice-send")
        .expect("the launch mob's send");
    assert_eq!(
        store
            .update_frame_status(
                &format!("hosted-{}", launch_send.id),
                ConsoleFrameStatus::DeliveryFailed
            )
            .await?,
        None
    );
    let appended = store.append_if_absent(text("alice-3", "alice", 60)).await?;
    assert_eq!(appended.disposition, AppendDisposition::Inserted);
    assert_eq!(
        rows(&path, "mob_id = '' AND dedupe_key = 'alice-3'")?.len(),
        1,
        "the appended frame is stored with the empty mob"
    );
    assert_eq!(another_mobs_rows(&path)?, another_mobs);
    assert_eq!(ledger_version(&path)?, Some(4));
    Ok(())
}

/// A status change, a send transition and a taken hold that name a frame
/// of another mob by its id find no frame, and its row stays as it was.
/// The same changes on the launch mob's own send apply.
#[tokio::test]
async fn a_change_by_id_leaves_another_mobs_frame_as_it_was() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    let stored = version_4_log(&path).await?;
    let another_mobs = another_mobs_rows(&path)?;
    let launch_send = stored
        .iter()
        .find(|frame| frame.dedupe_key == "alice-send")
        .expect("the launch mob's send");
    let hosted_send = format!("hosted-{}", launch_send.id);
    let store = SqliteConsoleLogStore::open(&path)?;

    assert_eq!(
        store
            .update_frame_status(&hosted_send, ConsoleFrameStatus::DeliveryFailed)
            .await?,
        None
    );
    assert_eq!(
        store
            .transition_send_frame(&hosted_send, SendTransition::StartDispatch, None)
            .await?,
        None
    );
    assert_eq!(store.take_send_frame_hold(&hosted_send).await?, None);
    assert_eq!(another_mobs_rows(&path)?, another_mobs);

    let taken = store
        .take_send_frame_hold(&launch_send.id)
        .await?
        .expect("the launch mob's hold is taken");
    assert!(taken.payload.get("delivery_hold").is_none());
    let dispatching = store
        .transition_send_frame(&launch_send.id, SendTransition::StartDispatch, None)
        .await?
        .expect("the launch mob's send moves");
    assert_eq!(dispatching.status, ConsoleFrameStatus::Dispatching);
    assert_eq!(another_mobs_rows(&path)?, another_mobs);
    Ok(())
}

/// An append to a version 4 log stores the launch mob's frame (the empty
/// mob) past every stored row, and a replay of its key answers that frame.
/// A key that a frame of another mob holds is refused: the append answers
/// neither that frame nor a stored frame of its own.
#[tokio::test]
async fn an_append_to_a_version_4_log_stores_the_launch_mobs_frame() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    let stored = version_4_log(&path).await?;
    let another_mobs = another_mobs_rows(&path)?;
    let store = SqliteConsoleLogStore::open(&path)?;

    let appended = store.append_if_absent(text("alice-3", "alice", 60)).await?;
    assert_eq!(appended.disposition, AppendDisposition::Inserted);
    let replay = store.append_if_absent(text("alice-3", "alice", 60)).await?;
    assert_eq!(replay.disposition, AppendDisposition::Existing);
    assert_eq!(replay.frame, appended.frame);
    assert_eq!(
        rows(&path, "mob_id = '' AND dedupe_key = 'alice-3'")?.len(),
        1,
        "the appended frame is stored with the empty mob"
    );
    assert_eq!(
        rows(&path, "1 = 1")?.len(),
        2 * stored.len() + 1,
        "one row was added"
    );
    let newest = rows(&path, "1 = 1")?.pop().unwrap_or_default();
    assert!(
        newest.contains("|alice-3|"),
        "it is the log's newest row: {newest}"
    );

    let refused = store
        .append_if_absent(text("hosted-alice-1", "alice", 70))
        .await;
    assert!(
        refused.is_err(),
        "a key another mob's frame holds is refused: {refused:?}"
    );
    assert_eq!(another_mobs_rows(&path)?, another_mobs);
    assert_eq!(rows(&path, "1 = 1")?.len(), 2 * stored.len() + 1);
    Ok(())
}

/// A delta compaction on a version 4 log removes the launch mob's streamed
/// rows of the block and no row of another mob, also where another mob's
/// rows of the same member name, runtime key and message lie inside the
/// block's span.
#[tokio::test]
async fn a_compaction_on_a_version_4_log_removes_no_frame_of_another_mob() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    {
        let store = SqliteConsoleLogStore::open(&path)?;
        for frame in [
            block("text-1", "text_delta", 1),
            block("text-2", "text_delta", 2),
        ] {
            store.append_if_absent(frame).await?;
        }
    }
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute_batch(MOVE_TO_VERSION_4)?;
    conn.execute_batch(ANOTHER_MOBS_FRAMES)?;
    drop(conn);
    let another_mobs = another_mobs_rows(&path)?;
    assert_eq!(another_mobs.len(), 2);

    let store = SqliteConsoleLogStore::open(&path)?;
    let first = store
        .frame_by_dedupe_key("text-1")
        .await?
        .expect("the block's first row");
    store
        .append_if_absent(block("text-3", "text_delta", 3))
        .await?;
    let complete = store
        .append_if_absent(block("complete", "text_complete", 4))
        .await?
        .frame;
    let removed = store
        .compact_message_deltas(&ConsoleDeltaCompaction::new(
            RUNTIME,
            "alice",
            ConsoleDeltaKind::Text,
            MESSAGE,
            first.cursor.clone(),
            complete.cursor.clone(),
        ))
        .await?;

    assert_eq!(removed, 2, "the launch mob's two later streamed rows go");
    assert_eq!(another_mobs_rows(&path)?, another_mobs);
    let left: Vec<String> = store
        .query_windowed_frames(window(None, None, ConsoleTimelineMode::Since, 200))
        .await?
        .frames
        .into_iter()
        .map(|frame| frame.dedupe_key)
        .collect();
    assert_eq!(left, ["text-1", "complete"]);
    Ok(())
}

/// Clearing a version 4 log removes the launch mob's frames and leaves
/// every row of another mob, with the member it was captured for. A frame
/// appended afterwards is stored past every row the log has held.
#[tokio::test]
async fn clearing_a_version_4_log_keeps_another_mobs_frames() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    let stored = version_4_log(&path).await?;
    let another_mobs = another_mobs_rows(&path)?;
    let store = SqliteConsoleLogStore::open(&path)?;

    store.clear_frames().await?;

    assert!(
        store
            .query_windowed_frames(window(None, None, ConsoleTimelineMode::Since, 200))
            .await?
            .frames
            .is_empty(),
        "the launch mob's frames are cleared"
    );
    assert_eq!(store.latest_cursor().await?, None);
    assert!(rows(&path, "mob_id = ''")?.is_empty());
    assert_eq!(another_mobs_rows(&path)?, another_mobs);
    let conn = rusqlite::Connection::open(&path)?;
    let witnesses: i64 = conn.query_row(
        "SELECT COUNT(*) FROM console_frames AS frame \
         JOIN console_member_provenance AS witness ON witness.id = frame.member_provenance_id \
         WHERE frame.mob_id <> ''",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        witnesses, 1,
        "another mob's frame keeps its captured member"
    );
    drop(conn);

    let appended = store
        .append_if_absent(text("alice-after", "alice", 90))
        .await?
        .frame;
    let newest_before = 2 * stored.len() as u64;
    assert!(
        appended.cursor.seq().is_some_and(|seq| seq > newest_before),
        "the new frame is past every row the log has held: {:?}",
        appended.cursor
    );
    Ok(())
}

/// A version 3 log is opened and written as before: it stays at version 3
/// with the tables and indexes it had, the index by identity among them and
/// no mob column, and a frame appended to it is read back.
#[tokio::test]
async fn a_version_3_log_is_opened_and_written_as_before() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    let stored = version_3_log(&path).await?;
    let declared = catalog(&path)?;
    assert!(
        declared
            .iter()
            .any(|object| object.starts_with("index idx_console_frames_identity_cursor:")),
        "{declared:#?}"
    );
    assert!(
        !declared.iter().any(|object| object.contains("mob_id")),
        "{declared:#?}"
    );

    let store = SqliteConsoleLogStore::open(&path)?;
    let appended = store.append_if_absent(text("alice-3", "alice", 60)).await?;
    assert_eq!(appended.disposition, AppendDisposition::Inserted);
    let page = store
        .query_windowed_frames(window(Some("alice"), None, ConsoleTimelineMode::Since, 200))
        .await?;
    assert_eq!(
        page.frames.len(),
        stored
            .iter()
            .filter(|frame| frame.identity == "alice")
            .count()
            + 1
    );
    assert_eq!(page.latest_cursor, Some(appended.frame.cursor.clone()));
    assert_eq!(store.latest_cursor().await?, Some(appended.frame.cursor));
    store.clear_frames().await?;
    assert_eq!(store.latest_cursor().await?, None);
    drop(store);

    assert_eq!(ledger_version(&path)?, Some(3));
    assert_eq!(catalog(&path)?, declared);
    Ok(())
}

/// A log stamped version 4 whose tables and indexes are not the ones of
/// version 4 is refused at open, and the refusal names what differs and
/// changes nothing in the file: here the log lacks the index by mob and
/// cursor.
#[tokio::test]
async fn a_version_4_log_in_another_form_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    version_4_log(&path).await?;
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute_batch("DROP INDEX idx_console_frames_mob_cursor")?;
    drop(conn);
    let declared = catalog(&path)?;
    let stored = rows(&path, "1 = 1")?;

    let refused = SqliteConsoleLogStore::open(&path)
        .err()
        .map(|error| error.to_string());

    let refusal = refused.expect("a version 4 log in another form is refused");
    assert!(
        refusal.contains("meta-console") && refusal.contains("idx_console_frames_mob_cursor"),
        "the refusal names the log and what differs: {refusal}"
    );
    assert_eq!(ledger_version(&path)?, Some(4));
    assert_eq!(catalog(&path)?, declared);
    assert_eq!(rows(&path, "1 = 1")?, stored);
    Ok(())
}

/// A log at schema version 5 is refused at open, and the refusal changes
/// nothing in the file.
#[tokio::test]
async fn a_version_5_log_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("console.sqlite3");
    version_4_log(&path).await?;
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute(
        "UPDATE meerkat_schema SET version = 5 WHERE domain = 'meta-console'",
        [],
    )?;
    drop(conn);
    let declared = catalog(&path)?;
    let stored = rows(&path, "1 = 1")?;

    let refused = SqliteConsoleLogStore::open(&path)
        .err()
        .map(|error| error.to_string());

    let refusal = refused.expect("a version 5 log is refused");
    assert!(
        refusal.contains("`meta-console`")
            && refusal.contains("file has version 5")
            && refusal.contains("supports up to 4"),
        "the refusal names the log's version and the newest this release reads: {refusal}"
    );
    assert_eq!(ledger_version(&path)?, Some(5));
    assert_eq!(catalog(&path)?, declared);
    assert_eq!(rows(&path, "1 = 1")?, stored);
    Ok(())
}
