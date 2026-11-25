//! A timeline read over every mob its caller may address.
//!
//! The console's log reads the frames of one mob. A timeline read with
//! every mob selected reads the launch mob and each other mob of the host
//! its caller may address, each with the log's read of one mob, and merges
//! their pages into one page. The log has one sequence of cursors across
//! its mobs, so the merged page is ordered by it, and the page bound a
//! request names is the bound of the merged page.
//!
//! One cursor goes over the wire, a position in the whole log. The read of
//! one mob holds a cursor against the newest frame of that mob, and such a
//! cursor may lie past the end of another mob's frames. So the merged read
//! asks where each mob's frames end before it reads a page:
//!
//! - a cursor at or past the end of one mob's frames means that mob has
//!   nothing new, and that mob is not read;
//! - a cursor past the newest frame of every mob read is a cursor the
//!   merged read cannot answer from, refused as the read of one mob
//!   refuses it, naming the newest frame of the mobs read;
//! - a refusal or a fault of one mob's read is the answer of the merged
//!   read: a merged page is never short of a mob without saying so.
//!
//! The ends are asked before the pages are read, and the merged page ends
//! at the newest of them: a frame stored while the pages were being read
//! is left for the next read, so a page of one mob read earlier never
//! hides a frame behind the cursor the merged page hands out.
//!
//! The refusal fires after the log was cleared (every end is gone, so any
//! cursor is past it). It can also fire once for a client whose set of
//! mobs shrank, when a mob it read was removed with its frames or its
//! right to address one was withdrawn: a cursor it holds may then be a
//! frame of that mob past the newest end of what is left. It is answered
//! the newest end of the mobs it still reads and goes on from there.
//!
//! A stream lists its mobs once, when it opens. A mob its caller comes to
//! address later sends its live frames, each judged as it arrives, and
//! its earlier frames are not replayed; a mob the caller may no longer
//! address is still read by the snapshot's pages, and each of its frames
//! is dropped by the frame's own judgement.
//!
//! A mob that is not loaded has no frame in a read over every mob: a
//! frame is decided under its mob's registration with the console's log,
//! which a mob has while it is loaded, and the read loads none. The end of
//! such a mob's frames still counts toward the cursor the merged page
//! hands out, so a caller that follows the cursor is not answered those
//! frames once the mob is loaded either: a cursor held back for them
//! would hold back every other mob's newer frames. A client that opens
//! such a mob (the request that loads it) reads that mob's timeline, or
//! a member's, from no cursor.
//!
//! One mob is read by the log's read of one mob too: a request without
//! the selector, and one served for a mob beside the launch mob, whose
//! cursor is then held against that mob's own newest frame. A store that
//! does not read one mob's frames itself holds the launch mob's frames
//! alone: with every mob selected nothing beside the launch mob is read
//! from it.
//!
//! The read loads no mob and resolves none: the mobs are the ones the
//! door listed from the host's records for its caller, and every read here
//! is a read of the console's log.

use meerkat_mob::MobId;

use crate::console_aggregator::{
    ConsoleCursor, ConsoleFrame, ConsoleTimelineMode, ConsoleTimelineQueryError,
    ConsoleTimelineQueryResult, ConsoleTimelineWindowPage, ConsoleTimelineWindowQuery,
    MetaConsoleAggregator, TIMELINE_RAW_SCAN_PAGE_LIMIT, TimelineFrameFilter,
};

/// How the frames of a page are answered: projected for the view, or as
/// stored, for a stream that decides each frame again before it sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PageFrames {
    ForView,
    Stored,
}

/// A cursor's position in the log. A cursor that is no position sorts
/// after every position.
fn position(cursor: &ConsoleCursor) -> u64 {
    cursor.seq().unwrap_or(u64::MAX)
}

/// The newest of `cursors`.
fn newest(cursors: impl Iterator<Item = ConsoleCursor>) -> Option<ConsoleCursor> {
    cursors.max_by_key(position)
}

/// One page of a timeline read for a request that reads, beside the launch
/// mob, the mobs `beside_the_launch_mob`: the mobs of the host its caller
/// may address when the request selects every mob, and none otherwise.
///
/// With no such mob the page is the page of the one mob the query names
/// (the launch mob when it names none), read by the log's read of one mob.
/// With some, it is the merged page of the module: each mob's page is
/// read with `frame_filter`, which decides a frame in the frame's own
/// mob. Over a store that does not read one mob's frames itself no mob
/// beside the launch mob has frames, and none is read.
pub(super) async fn page(
    aggregator: &MetaConsoleAggregator,
    beside_the_launch_mob: &[MobId],
    query: ConsoleTimelineWindowQuery,
    frame_filter: Option<&TimelineFrameFilter<'_>>,
    frames: PageFrames,
) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
    if beside_the_launch_mob.is_empty() || !aggregator.reads_one_mob() {
        return Box::pin(page_of_one_mob(aggregator, query, frame_filter, frames)).await;
    }
    // Where each mob's frames end, asked before any page is read: the
    // launch mob's frames name no mob.
    let mut ends = Vec::with_capacity(beside_the_launch_mob.len() + 1);
    for mob in std::iter::once(None).chain(beside_the_launch_mob.iter().cloned().map(Some)) {
        let end = aggregator.newest_cursor_of_mob_frames(mob.as_ref()).await?;
        ends.push((mob, end));
    }
    let end = newest(ends.iter().filter_map(|(_, end)| end.clone()));
    let after = query.after.as_ref().and_then(ConsoleCursor::seq);
    if let Some(after) = after
        && after > end.as_ref().and_then(ConsoleCursor::seq).unwrap_or(0)
    {
        return Err(ConsoleTimelineQueryError::ReplayUnavailable {
            requested_cursor: query.after.clone(),
            latest_cursor: end,
        });
    }
    let mut pages = Vec::with_capacity(ends.len());
    for (mob, mob_end) in ends {
        // A mob with no frame, or with none after the cursor, has nothing
        // for this page and is not read.
        let Some(mob_end) = mob_end else {
            continue;
        };
        if after.is_some_and(|after| mob_end.seq().is_some_and(|mob_end| after >= mob_end)) {
            continue;
        }
        let of_the_mob = ConsoleTimelineWindowQuery {
            mob_id: mob,
            ..query.clone()
        };
        pages.push(
            Box::pin(page_of_one_mob(
                aggregator,
                of_the_mob,
                frame_filter,
                frames,
            ))
            .await?,
        );
    }
    Ok(merged(
        query.mode,
        query.after.as_ref(),
        query.limit.clamp(1, TIMELINE_RAW_SCAN_PAGE_LIMIT),
        end,
        pages,
    ))
}

/// The page of the one mob `query` names (the launch mob when it names
/// none), by the log's read of one mob, which holds the query's cursor
/// against that mob's own newest frame.
async fn page_of_one_mob(
    aggregator: &MetaConsoleAggregator,
    query: ConsoleTimelineWindowQuery,
    frame_filter: Option<&TimelineFrameFilter<'_>>,
    frames: PageFrames,
) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
    match frames {
        PageFrames::ForView => {
            Box::pin(aggregator.query_mob_timeline_windowed_for(query, frame_filter)).await
        }
        PageFrames::Stored => {
            Box::pin(aggregator.query_mob_timeline_windowed_stored_for(query, frame_filter)).await
        }
    }
}

/// The one page of the pages `pages`, each the page of one mob read for
/// the same query (its mode `mode`, its cursor `after`, its bound `limit`),
/// in a log whose mobs' frames ended at `end`, the newest of them, before
/// the pages were read. No frame after `end` is in the merged page.
///
/// A `since` page holds the oldest frames after the cursor, in the log's
/// order. A page of a mob that was not read to its end has more frames of
/// that mob after its own cursor, so the merged page holds no frame after
/// the earliest such cursor. Its cursor is the last frame it holds when
/// the bound cut it, else that earliest cursor, else `end`: everything up
/// to it was read. It is exhausted only when each mob was read to its end
/// and nothing was cut.
///
/// A `recent` page holds the newest frames. A page of a mob that left
/// older frames out holds none before its oldest frame, so the merged page
/// holds no frame before the newest such oldest frame: it stays one
/// unbroken tail of the merged log, and a read before its oldest frame
/// continues it. It is exhausted only when no mob left a frame out and
/// nothing was cut.
fn merged(
    mode: ConsoleTimelineMode,
    after: Option<&ConsoleCursor>,
    limit: usize,
    end: Option<ConsoleCursor>,
    pages: Vec<ConsoleTimelineWindowPage>,
) -> ConsoleTimelineWindowPage {
    let end_position = end.as_ref().map_or(0, position);
    let each_read_to_its_end = pages.iter().all(|page| page.exhausted);
    match mode {
        ConsoleTimelineMode::Since => {
            let earliest_stop = pages
                .iter()
                .filter(|page| !page.exhausted)
                .filter_map(|page| page.next_cursor.clone())
                .min_by_key(position);
            let through = earliest_stop
                .as_ref()
                .map_or(end_position, |stop| position(stop).min(end_position));
            let mut frames: Vec<ConsoleFrame> = pages
                .into_iter()
                .flat_map(|page| page.frames)
                .filter(|frame| position(&frame.cursor) <= through)
                .collect();
            frames.sort_by_key(|frame| position(&frame.cursor));
            let (next_cursor, exhausted) = if frames.len() > limit {
                frames.truncate(limit);
                (frames.last().map(|frame| frame.cursor.clone()), false)
            } else if let Some(stop) = earliest_stop.filter(|stop| position(stop) <= end_position) {
                (Some(stop), false)
            } else {
                let read_through = match (after, &end) {
                    (Some(after), Some(end)) if position(after) >= position(end) => {
                        Some(after.clone())
                    }
                    (_, Some(end)) => Some(end.clone()),
                    (after, None) => after.cloned(),
                };
                (read_through, each_read_to_its_end)
            };
            ConsoleTimelineWindowPage {
                frames,
                next_cursor,
                latest_cursor: end,
                exhausted,
            }
        }
        ConsoleTimelineMode::Recent => {
            // The oldest frame of each page that left older frames out (the
            // cursor it stopped at when it holds none): the newest of them.
            let floor = newest(
                pages
                    .iter()
                    .filter(|page| !page.exhausted)
                    .filter_map(|page| {
                        page.frames
                            .first()
                            .map(|frame| frame.cursor.clone())
                            .or_else(|| page.next_cursor.clone())
                    }),
            );
            let no_later_than_the_end = |cursor: ConsoleCursor| {
                if position(&cursor) > end_position {
                    end.clone()
                } else {
                    Some(cursor)
                }
            };
            let next_cursor = newest(pages.iter().filter_map(|page| page.next_cursor.clone()))
                .and_then(no_later_than_the_end);
            let latest_cursor = newest(pages.iter().filter_map(|page| page.latest_cursor.clone()))
                .and_then(no_later_than_the_end)
                .or_else(|| end.clone());
            let mut frames: Vec<ConsoleFrame> = pages
                .into_iter()
                .flat_map(|page| page.frames)
                .filter(|frame| position(&frame.cursor) <= end_position)
                .collect();
            let read = frames.len();
            let floor_position = floor.as_ref().map_or(0, position);
            frames.retain(|frame| position(&frame.cursor) >= floor_position);
            let left_older_out = frames.len() < read;
            frames.sort_by_key(|frame| position(&frame.cursor));
            let cut = frames.len() > limit;
            if cut {
                frames = frames.split_off(frames.len() - limit);
            }
            ConsoleTimelineWindowPage {
                // A page with no frame that left older frames out hands the
                // cursor to read before, as the read of one mob does.
                next_cursor: if frames.is_empty() && floor.is_some() {
                    floor
                } else {
                    next_cursor
                },
                frames,
                latest_cursor,
                exhausted: each_read_to_its_end && !left_older_out && !cut,
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A stored frame at the position `seq`.
    fn frame(seq: u64) -> ConsoleFrame {
        serde_json::from_value(json!({
            "id": format!("frame-{seq}"),
            "cursor": ConsoleCursor::from_seq(seq).to_string(),
            "dedupe_key": format!("frame-{seq}"),
            "timestamp_ms": 1,
            "runtime_key": "runtime",
            "identity": "alice",
            "kind": "text_complete",
            "status": "completed",
            "frame_version": 1,
            "payload": {},
            "source": { "kind": "session_history" },
        }))
        .expect("a stored frame")
    }

    fn at(seq: u64) -> Option<ConsoleCursor> {
        Some(ConsoleCursor::from_seq(seq))
    }

    /// The page of one mob: its frames at `seqs`, its cursor at
    /// `next_cursor`, and whether the mob was read to its end.
    fn page_of(
        seqs: &[u64],
        next_cursor: Option<u64>,
        exhausted: bool,
    ) -> ConsoleTimelineWindowPage {
        ConsoleTimelineWindowPage {
            frames: seqs.iter().copied().map(frame).collect(),
            next_cursor: next_cursor.map(ConsoleCursor::from_seq),
            latest_cursor: seqs.last().copied().map(ConsoleCursor::from_seq),
            exhausted,
        }
    }

    fn positions(page: &ConsoleTimelineWindowPage) -> Vec<u64> {
        page.frames
            .iter()
            .map(|frame| position(&frame.cursor))
            .collect()
    }

    /// Two mobs read to their ends give one page in the log's order, whose
    /// cursor is the newest frame of the two and which is exhausted.
    #[test]
    fn a_since_page_of_two_mobs_is_in_the_logs_order() {
        let page = merged(
            ConsoleTimelineMode::Since,
            None,
            10,
            at(6),
            vec![
                page_of(&[1, 4, 6], Some(6), true),
                page_of(&[2, 3, 5], Some(5), true),
            ],
        );
        assert_eq!(positions(&page), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(page.next_cursor, at(6));
        assert_eq!(page.latest_cursor, at(6));
        assert!(page.exhausted);
    }

    /// The bound a request names is the bound of the merged page: a page
    /// it cuts ends at the last frame it holds and is not exhausted.
    #[test]
    fn the_bound_cuts_the_merged_since_page() {
        let page = merged(
            ConsoleTimelineMode::Since,
            None,
            3,
            at(6),
            vec![
                page_of(&[1, 4, 6], Some(6), true),
                page_of(&[2, 3, 5], Some(5), true),
            ],
        );
        assert_eq!(positions(&page), vec![1, 2, 3]);
        assert_eq!(page.next_cursor, at(3));
        assert!(!page.exhausted);
    }

    /// A mob that was not read to its end has more frames after its own
    /// cursor: the merged page holds no frame of another mob after that
    /// cursor, hands that cursor out and is not exhausted.
    #[test]
    fn a_mob_not_read_to_its_end_stops_the_merged_since_page_at_its_cursor() {
        let page = merged(
            ConsoleTimelineMode::Since,
            None,
            10,
            at(9),
            vec![
                page_of(&[1, 2], Some(3), false),
                page_of(&[4, 9], Some(9), true),
            ],
        );
        assert_eq!(positions(&page), vec![1, 2]);
        assert_eq!(page.next_cursor, at(3));
        assert!(!page.exhausted);
    }

    /// A frame stored after the ends were asked is left for the next read:
    /// the merged page ends at the newest end, and its cursor hides no
    /// frame of a mob whose page was read before that frame was stored.
    #[test]
    fn a_frame_stored_after_the_ends_were_asked_is_left_for_the_next_read() {
        let page = merged(
            ConsoleTimelineMode::Since,
            at(2).as_ref(),
            10,
            at(5),
            vec![
                page_of(&[3], Some(3), true),
                page_of(&[5, 7], Some(7), true),
            ],
        );
        assert_eq!(positions(&page), vec![3, 5]);
        assert_eq!(page.next_cursor, at(5));
        assert_eq!(page.latest_cursor, at(5));
        assert!(page.exhausted);
    }

    /// A read from a cursor no mob has a frame after is an empty page at
    /// that cursor, exhausted, that names the newest frame of the mobs.
    #[test]
    fn a_since_read_with_nothing_new_stays_at_its_cursor() {
        let page = merged(
            ConsoleTimelineMode::Since,
            at(8).as_ref(),
            10,
            at(8),
            Vec::new(),
        );
        assert!(page.frames.is_empty());
        assert_eq!(page.next_cursor, at(8));
        assert_eq!(page.latest_cursor, at(8));
        assert!(page.exhausted);
    }

    /// A recent page is the newest frames of the mobs up to the bound, in
    /// the log's order; a page the bound cut is not exhausted.
    #[test]
    fn a_recent_page_of_two_mobs_is_the_newest_frames_of_both() {
        let page = merged(
            ConsoleTimelineMode::Recent,
            None,
            3,
            at(6),
            vec![
                page_of(&[1, 4, 6], Some(6), true),
                page_of(&[2, 3, 5], Some(5), true),
            ],
        );
        assert_eq!(positions(&page), vec![4, 5, 6]);
        assert_eq!(page.latest_cursor, at(6));
        assert!(!page.exhausted);
        let whole = merged(
            ConsoleTimelineMode::Recent,
            None,
            10,
            at(6),
            vec![
                page_of(&[1, 4, 6], Some(6), true),
                page_of(&[2, 3, 5], Some(5), true),
            ],
        );
        assert_eq!(positions(&whole), vec![1, 2, 3, 4, 5, 6]);
        assert!(whole.exhausted);
    }

    /// A mob that left older frames out holds none before its oldest
    /// frame: the merged page holds no frame of another mob before it, so
    /// it is one unbroken tail of the merged log, and it is not exhausted.
    #[test]
    fn a_mob_that_left_older_frames_out_floors_the_merged_recent_page() {
        let page = merged(
            ConsoleTimelineMode::Recent,
            None,
            10,
            at(9),
            vec![
                page_of(&[7, 9], Some(9), false),
                page_of(&[2, 6, 8], Some(8), true),
            ],
        );
        assert_eq!(positions(&page), vec![7, 8, 9]);
        assert!(!page.exhausted);
    }

    /// A recent page ends at the newest end too: a frame stored after the
    /// ends were asked is not in it, and the cursors it names are not past
    /// that end.
    #[test]
    fn a_recent_page_ends_at_the_newest_end() {
        let page = merged(
            ConsoleTimelineMode::Recent,
            None,
            10,
            at(5),
            vec![
                page_of(&[3], Some(3), true),
                page_of(&[5, 7], Some(7), true),
            ],
        );
        assert_eq!(positions(&page), vec![3, 5]);
        assert_eq!(page.next_cursor, at(5));
        assert_eq!(page.latest_cursor, at(5));
        assert!(page.exhausted);
    }
}
