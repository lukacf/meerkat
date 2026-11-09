//! `ConsoleLogStore` profile: append idempotency, the per-handle watermark
//! cache contract, and windowed-query pagination under concurrent append.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_meta::console_aggregator::{
    AppendDisposition, ConsoleCursor, ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleLogStore,
    ConsoleTimelineMode, ConsoleTimelineQuery, ConsoleTimelineWindowQuery,
};
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
