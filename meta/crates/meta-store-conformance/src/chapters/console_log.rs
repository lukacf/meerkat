//! `ConsoleLogStore` profile: append idempotency, the mob in a member's
//! frames, the one stored key of the default mob, the per-handle watermark
//! cache contract, and windowed-query pagination under concurrent append.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_meta::console_aggregator::{
    AppendDisposition, ConsoleCursor, ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleLogStore,
    ConsoleTimelineMode, ConsoleTimelineQuery, ConsoleTimelineWindowQuery,
};
use meerkat_meta::member_key::DefaultMob;
use meerkat_store_conformance::ConformanceFailure;

use crate::factory::ConsoleLogStoreFactory;
use crate::fixtures;
use crate::steps::Steps;

const CHAPTER: &str = "console_log";

/// Writers in the concurrent-append step.
const CONCURRENT_WRITERS: usize = 4;
/// Frames appended per writer.
const FRAMES_PER_WRITER: usize = 8;
/// Page size used to force multi-page pagination.
const WINDOW_PAGE_LIMIT: usize = 5;
/// Upper bound on pagination rounds before declaring the walk broken.
const MAX_PAGINATION_ROUNDS: usize = 64;

pub async fn console_log(factory: &dyn ConsoleLogStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    append_if_absent_idempotent(&steps, store.as_ref()).await?;
    member_frames_keyed_by_mob(&steps, store.as_ref()).await?;
    default_mob_named_by_its_id(&steps, store.as_ref()).await?;
    member_provenance_roundtrip(&steps, factory, store.as_ref()).await?;
    watermark_contract(&steps, factory, store.as_ref()).await?;
    windowed_query_under_concurrent_append(&steps, Arc::clone(&store)).await?;
    Ok(())
}

async fn append_if_absent_idempotent(
    steps: &Steps,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "append_if_absent_idempotent";
    let frame = fixtures::console_frame("conformance-dedupe-1", "console:probe", 1_000);

    let first = steps.wrap(STEP, store.append_if_absent(frame.clone()).await)?;
    steps.ensure(
        STEP,
        first.disposition == AppendDisposition::Inserted,
        "the first append of a dedupe key must report Inserted",
    )?;

    // Same handle, same dedupe key: the append must be idempotent and serve
    // back the original frame (same id, same cursor), never a duplicate row.
    let replay = steps.wrap(STEP, store.append_if_absent(frame).await)?;
    steps.ensure(
        STEP,
        replay.disposition == AppendDisposition::Existing,
        "replaying a dedupe key on the same handle must report Existing",
    )?;
    steps.ensure(
        STEP,
        replay.frame.id == first.frame.id && replay.frame.cursor == first.frame.cursor,
        "the replayed append must serve the original frame (same id and cursor)",
    )?;

    let by_key = steps
        .wrap(
            STEP,
            store.frame_by_dedupe_key("conformance-dedupe-1").await,
        )?
        .ok_or_else(|| steps.fail(STEP, "frame_by_dedupe_key must find the appended frame"))?;
    steps.ensure(
        STEP,
        by_key.id == first.frame.id,
        "frame_by_dedupe_key must serve the appended frame",
    )?;
    Ok(())
}

/// A frame belongs to a member: a mob and an identity in it. Two mobs of
/// one host may each have a member of the same name, so a store keeps a
/// frame's `mob_id`, answers it, and reads a query that names an identity as
/// the frames of that mob's member: the query's `mob_id`, the default mob
/// when it names none.
///
/// The step reads another mob's stored frame by the read that names that
/// mob. It does not ask `frame_by_dedupe_key`, which names no mob, for it:
/// the two stores of `meerkat-meta` answer that differently (the SQLite
/// store answers the default mob's frame alone, the store in memory a frame
/// of any mob), and no door of a host serves the difference.
async fn member_frames_keyed_by_mob(
    steps: &Steps,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "member_frames_keyed_by_mob";
    const IDENTITY: &str = "console:member-key-probe";
    const OTHER_MOB: &str = "conformance-other-mob";
    let of_default_mob = fixtures::console_frame("conformance-member-default", IDENTITY, 1_500);
    let mut of_other_mob = fixtures::console_frame("conformance-member-other", IDENTITY, 1_501);
    of_other_mob.mob_id = Some(OTHER_MOB.into());
    steps.wrap(STEP, store.append_if_absent(of_default_mob).await)?;
    let stored = steps.wrap(STEP, store.append_if_absent(of_other_mob).await)?;
    steps.ensure(
        STEP,
        stored.frame.mob_id == Some(OTHER_MOB.into()),
        "an appended frame must answer the mob_id it was appended with",
    )?;

    for (mob_id, expected) in [
        (None, "conformance-member-default"),
        (Some(OTHER_MOB), "conformance-member-other"),
    ] {
        let window = steps.wrap(
            STEP,
            store
                .query_windowed_frames(ConsoleTimelineWindowQuery {
                    identity: Some(IDENTITY.to_string()),
                    mob_id: mob_id.map(Into::into),
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await,
        )?;
        let page = steps.wrap(
            STEP,
            store
                .query_frames(ConsoleTimelineQuery {
                    identity: Some(IDENTITY.to_string()),
                    mob_id: mob_id.map(Into::into),
                    ..ConsoleTimelineQuery::default()
                })
                .await,
        )?;
        for frames in [&window.frames, &page.frames] {
            let keys = frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>();
            steps.ensure(
                STEP,
                keys == [expected],
                format!(
                    "a query that names an identity must answer the frames of that mob's \
                     member only (mob_id {mob_id:?}): got {keys:?}"
                ),
            )?;
            steps.ensure(
                STEP,
                frames
                    .iter()
                    .all(|frame| frame.mob_id == mob_id.map(Into::into)),
                format!("a stored frame must keep its mob_id (mob_id {mob_id:?})"),
            )?;
        }
    }
    Ok(())
}

/// A host's default mob has one stored key. A store is told the default mob
/// once (`bind_default_mob`); from then on a frame appended under that
/// mob's id is a frame of the default mob, answered and stored as a frame
/// that names no mob, and a read that names the default mob by its id reads
/// the default mob's member. Telling the same mob again is accepted;
/// another mob and a blank id are refused. The step is required: a store
/// that keeps the trait's own `bind_default_mob`, which answers an error,
/// fails it until it implements the method. The step asks for the appended
/// frame and the two queries, which every store has. The trait asks the
/// same of `latest_activity_times_in_mob` and `compact_message_deltas`: a
/// store that implements either reads the default mob's id as no mob there
/// too.
async fn default_mob_named_by_its_id(
    steps: &Steps,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "default_mob_named_by_its_id";
    const IDENTITY: &str = "console:default-mob-probe";
    const DEFAULT_MOB: &str = "conformance-default-mob";
    // Asked first, of a store that was not told yet: a store that took the
    // blank id as its default mob would refuse the real one below.
    steps.ensure(
        STEP,
        store.bind_default_mob(&DefaultMob::new("".into())).is_err(),
        "a blank id names no mob: a store must refuse it as its default mob",
    )?;
    let default_mob = DefaultMob::new(DEFAULT_MOB.into());
    steps.wrap(STEP, store.bind_default_mob(&default_mob))?;
    steps.wrap(STEP, store.bind_default_mob(&default_mob))?;
    steps.ensure(
        STEP,
        store
            .bind_default_mob(&DefaultMob::new("conformance-other-mob".into()))
            .is_err(),
        "a store that was told its default mob must refuse another",
    )?;

    let named_none = fixtures::console_frame("conformance-default-none", IDENTITY, 1_600);
    let mut named_by_id = fixtures::console_frame("conformance-default-by-id", IDENTITY, 1_601);
    named_by_id.mob_id = Some(DEFAULT_MOB.into());
    steps.wrap(STEP, store.append_if_absent(named_none).await)?;
    let stored = steps.wrap(STEP, store.append_if_absent(named_by_id).await)?;
    steps.ensure(
        STEP,
        stored.frame.mob_id.is_none(),
        "a frame appended under the default mob's id must be answered as a frame of the default \
         mob (no mob_id)",
    )?;
    let reread = steps
        .wrap(
            STEP,
            store.frame_by_dedupe_key("conformance-default-by-id").await,
        )?
        .ok_or_else(|| steps.fail(STEP, "frame_by_dedupe_key must find the appended frame"))?;
    steps.ensure(
        STEP,
        reread.mob_id.is_none(),
        "a frame appended under the default mob's id must be stored as a frame of the default \
         mob (no mob_id)",
    )?;

    for mob_id in [None, Some(DEFAULT_MOB)] {
        let window = steps.wrap(
            STEP,
            store
                .query_windowed_frames(ConsoleTimelineWindowQuery {
                    identity: Some(IDENTITY.to_string()),
                    mob_id: mob_id.map(Into::into),
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await,
        )?;
        let page = steps.wrap(
            STEP,
            store
                .query_frames(ConsoleTimelineQuery {
                    identity: Some(IDENTITY.to_string()),
                    mob_id: mob_id.map(Into::into),
                    ..ConsoleTimelineQuery::default()
                })
                .await,
        )?;
        for frames in [&window.frames, &page.frames] {
            let keys = frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>();
            steps.ensure(
                STEP,
                keys == ["conformance-default-none", "conformance-default-by-id"],
                format!(
                    "a read of the default mob's member must answer its frames however the \
                     default mob is named (mob_id {mob_id:?}): got {keys:?}"
                ),
            )?;
        }
    }
    Ok(())
}

async fn member_provenance_roundtrip(
    steps: &Steps,
    factory: &dyn ConsoleLogStoreFactory,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "member_provenance_roundtrip";
    let frame = fixtures::console_frame_with_member_provenance();
    let expected = frame.source.member_provenance.clone();
    let appended = steps.wrap(STEP, store.append_if_absent(frame.clone()).await)?;
    steps.ensure(
        STEP,
        appended.frame.source.member_provenance == expected,
        "append must retain the owner member-policy witness",
    )?;
    let replay = steps.wrap(STEP, store.append_if_absent(frame).await)?;
    steps.ensure(
        STEP,
        replay.frame == appended.frame,
        "idempotent append must return the same witness-bearing frame",
    )?;
    let updated = steps
        .wrap(
            STEP,
            store
                .update_frame_status(&appended.frame.id, ConsoleFrameStatus::Delivered)
                .await,
        )?
        .ok_or_else(|| steps.fail(STEP, "status update must find the stored frame"))?;
    steps.ensure(
        STEP,
        updated.source.member_provenance == expected,
        "status update must preserve the member-policy witness",
    )?;
    let reopened = factory.open().await?;
    for current in [store, reopened.as_ref()] {
        let by_key = steps
            .wrap(STEP, current.frame_by_dedupe_key(&updated.dedupe_key).await)?
            .ok_or_else(|| steps.fail(STEP, "lookup must survive store reopen"))?;
        steps.ensure(
            STEP,
            by_key == updated,
            "lookup and reopen must retain the complete updated frame",
        )?;
        let query = ConsoleTimelineQuery {
            identity: Some(updated.identity.clone()),
            ..Default::default()
        };
        let page = steps.wrap(STEP, current.query_frames(query).await)?;
        steps.ensure(
            STEP,
            page.frames == [updated.clone()],
            "query must preserve the member-policy witness",
        )?;
        for mode in [ConsoleTimelineMode::Recent, ConsoleTimelineMode::Since] {
            let page = steps.wrap(
                STEP,
                current
                    .query_windowed_frames(ConsoleTimelineWindowQuery {
                        identity: Some(updated.identity.clone()),
                        mode,
                        ..Default::default()
                    })
                    .await,
            )?;
            steps.ensure(
                STEP,
                page.frames == [updated.clone()],
                "recent/since queries must retain the witness after reopen",
            )?;
        }
    }
    Ok(())
}

async fn watermark_contract(
    steps: &Steps,
    factory: &dyn ConsoleLogStoreFactory,
    store: &dyn ConsoleLogStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "watermark_round_trip";
    steps.wrap(
        STEP,
        store
            .record_source_watermark(
                "conformance-runtime",
                ConsoleFrameSourceKind::Synthetic,
                "cursor-0001",
            )
            .await,
    )?;
    let same_handle = steps.wrap(
        STEP,
        store
            .source_watermark("conformance-runtime", ConsoleFrameSourceKind::Synthetic)
            .await,
    )?;
    steps.ensure(
        STEP,
        same_handle.as_deref() == Some("cursor-0001"),
        "the recording handle must read back its own watermark",
    )?;

    // PINNED CONTRACT (per-handle cache): `source_watermark` reads an
    // in-memory cache hydrated at open. A handle opened AFTER a durable write
    // sees that watermark; concurrent cross-handle visibility after both
    // handles are open is NOT contractual and is deliberately not asserted
    // here — do not tighten this into cross-process cache coherence.
    const REOPEN_STEP: &str = "watermark_visible_after_reopen";
    let reopened = factory.open().await?;
    let after_reopen = steps.wrap(
        REOPEN_STEP,
        reopened
            .source_watermark("conformance-runtime", ConsoleFrameSourceKind::Synthetic)
            .await,
    )?;
    steps.ensure(
        REOPEN_STEP,
        after_reopen.as_deref() == Some("cursor-0001"),
        "a handle opened after a durable watermark write must see that watermark (hydration at \
         open is the durable half of the per-handle cache contract)",
    )?;
    Ok(())
}

async fn windowed_query_under_concurrent_append(
    steps: &Steps,
    store: Arc<dyn ConsoleLogStore>,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "windowed_query_concurrent_append";
    const IDENTITY: &str = "console:windowed";

    let mut joins = Vec::with_capacity(CONCURRENT_WRITERS);
    for writer in 0..CONCURRENT_WRITERS {
        let store = Arc::clone(&store);
        joins.push(tokio::spawn(async move {
            for index in 0..FRAMES_PER_WRITER {
                let key = format!("windowed-{writer}-{index}");
                let frame = fixtures::console_frame(&key, IDENTITY, 2_000 + index as u64);
                store
                    .append_if_absent(frame)
                    .await
                    .map_err(|error| format!("concurrent append failed: {error}"))?;
            }
            Ok::<(), String>(())
        }));
    }
    for join in joins {
        match join.await {
            Ok(Ok(())) => {}
            Ok(Err(detail)) => return Err(steps.fail(STEP, detail)),
            Err(join_error) => {
                return Err(steps.fail(STEP, format!("writer task panicked: {join_error}")));
            }
        }
    }

    // Walk the windowed query in pages. Every appended frame must appear
    // exactly once across the walk; a frame appearing twice or never means
    // the cursor ordering broke under concurrent append.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut after: Option<ConsoleCursor> = None;
    for _round in 0..MAX_PAGINATION_ROUNDS {
        let page = steps.wrap(
            STEP,
            store
                .query_windowed_frames(ConsoleTimelineWindowQuery {
                    mob_id: None,
                    identity: Some(IDENTITY.to_string()),
                    conversation_id: None,
                    after: after.clone(),
                    before: None,
                    mode: ConsoleTimelineMode::Since,
                    limit: WINDOW_PAGE_LIMIT,
                })
                .await,
        )?;
        if page.frames.is_empty() {
            break;
        }
        for frame in &page.frames {
            steps.ensure(
                STEP,
                seen.insert(frame.dedupe_key.clone()),
                format!(
                    "frame {} appeared twice across the paginated walk — windowed-query \
                     ordering must be stable under concurrent append",
                    frame.dedupe_key
                ),
            )?;
        }
        match page.next_cursor {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }

    let expected = CONCURRENT_WRITERS * FRAMES_PER_WRITER;
    steps.ensure(
        STEP,
        seen.len() == expected,
        format!(
            "the paginated walk must surface every appended frame exactly once (expected \
             {expected}, saw {})",
            seen.len()
        ),
    )?;
    Ok(())
}
