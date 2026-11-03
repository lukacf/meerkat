//! Compaction of the streamed delta rows a durable complete row supersedes.
//!
//! A member's streamed text, and each of its reasoning blocks, reaches the
//! console log as one row per provider chunk (`text_delta`,
//! `reasoning_delta`) and then whole, in one complete row (`text_complete`,
//! `reasoning_complete`). Once the complete row is stored the delta rows are
//! superseded, and [`ConsoleLogStore::compact_message_deltas`] removes them
//! but the first, which keeps the block's place in the timeline.
//!
//! [`StreamedBlocks`] is what the projection needs for that: per runtime,
//! where each block it is following began (the first delta row it observed;
//! only the rows after it go), and which blocks it compacted (so a replay of
//! the console event store, which delivers the same events again, does not
//! store their removed deltas a second time). It is a projection dedupe cache
//! over the durable rows; it owns no lifecycle and decides nothing about the
//! member.
//!
//! [`ConsoleLogStore::compact_message_deltas`]: super::ConsoleLogStore::compact_message_deltas

use std::collections::HashMap;

use super::types::{
    ConsoleDeltaKind, ConsoleFrame, ConsoleFrameSourceKind, NewConsoleFrame, streamed_block_payload,
};
use crate::unified_runtime::console_events::CONSOLE_EVENT_REDELIVERY_HORIZON;

/// A block silent this long has stopped streaming: provider streams fail far
/// sooner. A block that resumes later (a retry after a long backoff) keeps
/// its delta rows.
pub(super) const STALE_BLOCK_MS: u64 = 10 * 60 * 1_000;

/// Streamed-row projections of one runtime after which a compacted block's
/// deltas can no longer be delivered again: every delta projection and every
/// stored complete row counts once (a replayed complete row the log already
/// holds does not, which only keeps blocks longer), and any event delivered
/// again arrives within [`CONSOLE_EVENT_REDELIVERY_HORIZON`] events of its
/// first delivery.
pub(super) fn redelivery_horizon() -> u64 {
    u64::try_from(CONSOLE_EVENT_REDELIVERY_HORIZON).unwrap_or(u64::MAX)
}

/// Blocks one runtime tracks before compaction stops for it. Every block
/// compacted within the redelivery horizon stays tracked, so the bound is
/// the horizon plus room for the blocks still streaming.
pub(super) fn max_tracked_blocks() -> usize {
    CONSOLE_EVENT_REDELIVERY_HORIZON.saturating_add(16_384)
}

/// Tracked blocks below which no sweep runs.
const SWEEP_FLOOR: usize = 1_024;

/// One streamed block of a member's message: its text, or one of its
/// reasoning blocks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct BlockKey {
    pub(super) identity: String,
    pub(super) kind: ConsoleDeltaKind,
    pub(super) assistant_message_id: String,
}

/// A console event frame's part in a streamed block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BlockRow {
    Delta { key: BlockKey, source_sequence: u64 },
    Complete { key: BlockKey, source_sequence: u64 },
}

impl BlockRow {
    /// The frame's block row, read from its source payload: only console
    /// event frames whose payload names the assistant message and carries
    /// the member stream's sequence take part. Every other frame keeps its
    /// rows as before.
    pub(super) fn of(frame: &NewConsoleFrame) -> Option<Self> {
        if frame.source.kind != ConsoleFrameSourceKind::ConsoleEvent {
            return None;
        }
        let (assistant_message_id, source_sequence) = streamed_block_payload(&frame.payload)?;
        let key = |kind| BlockKey {
            identity: frame.identity.clone(),
            kind,
            assistant_message_id: assistant_message_id.to_string(),
        };
        if let Some(kind) = ConsoleDeltaKind::of_delta_frame_kind(&frame.kind) {
            return Some(Self::Delta {
                key: key(kind),
                source_sequence,
            });
        }
        ConsoleDeltaKind::of_complete_frame_kind(&frame.kind).map(|kind| Self::Complete {
            key: key(kind),
            source_sequence,
        })
    }
}

/// Whether a stored row is a streamed delta row compaction can remove: the
/// same rows [`BlockRow::of`] tracks and the stores remove. Such a row is a
/// poor anchor for a cached read.
pub(super) fn is_compactable_delta_row(frame: &ConsoleFrame) -> bool {
    frame.source.kind == ConsoleFrameSourceKind::ConsoleEvent
        && ConsoleDeltaKind::of_delta_frame_kind(&frame.kind).is_some()
        && streamed_block_payload(&frame.payload).is_some()
}

#[derive(Debug, Default)]
struct BlockState {
    /// The block's first delta row, while the projection has followed the
    /// block from it.
    open_first_delta: Option<u64>,
    /// The projection cannot follow the block from its first delta row, so
    /// the block keeps its delta rows.
    dropped: bool,
    /// The source sequence of the complete row the block was last compacted
    /// through: a delta at or below it was removed, and a replay of it is
    /// not stored again. A later block of the same message streams above it.
    compacted_through: Option<u64>,
    /// The runtime's streamed-row projections when the block last settled
    /// (compacted, completed or dropped).
    settled_at: u64,
    /// When the block last showed a row.
    last_seen_ms: u64,
}

#[derive(Debug)]
struct RuntimeBlocks {
    projections: u64,
    blocks: HashMap<BlockKey, BlockState>,
    next_sweep_at: usize,
    max_blocks: usize,
    /// Compaction stopped for the runtime: it follows no block, and keeps
    /// the compacted ones against replays.
    stopped: bool,
}

impl RuntimeBlocks {
    fn new(max_blocks: usize) -> Self {
        Self {
            projections: 0,
            blocks: HashMap::new(),
            next_sweep_at: SWEEP_FLOOR,
            max_blocks,
            stopped: false,
        }
    }

    fn follow(&mut self, key: BlockKey, cursor: u64, now_ms: u64) {
        // Sweep at the latest when the cap is reached, so tracking stops only
        // when the sweep frees nothing.
        if self.blocks.len() >= self.next_sweep_at.min(self.max_blocks) {
            self.sweep(now_ms);
            self.next_sweep_at = self.blocks.len().saturating_mul(2).max(SWEEP_FLOOR);
        }
        if self.blocks.len() >= self.max_blocks {
            self.stop();
            return;
        }
        self.blocks.insert(
            key,
            BlockState {
                open_first_delta: Some(cursor),
                last_seen_ms: now_ms,
                ..BlockState::default()
            },
        );
    }

    /// Drop the blocks that stopped streaming without their complete row,
    /// and forget the settled blocks no replay can deliver again.
    fn sweep(&mut self, now_ms: u64) {
        let projections = self.projections;
        let horizon = redelivery_horizon();
        self.blocks.retain(|_, state| {
            let silent = now_ms.saturating_sub(state.last_seen_ms) >= STALE_BLOCK_MS;
            if state.open_first_delta.is_some() {
                if !silent {
                    return true;
                }
                state.open_first_delta = None;
                state.dropped = true;
                state.settled_at = projections;
            }
            let past_horizon = projections.saturating_sub(state.settled_at) >= horizon;
            // Keep a block a replay can still deliver, and a dropped block
            // that may still stream, so it is never followed again from a
            // later delta row.
            !past_horizon || (state.dropped && !silent)
        });
    }

    /// Every tracked block is still streaming or within the redelivery
    /// horizon even after a sweep, and there is no room to follow another one
    /// from its first delta row: stop compacting for the runtime. The blocks
    /// it was following keep their delta rows.
    fn stop(&mut self) {
        if !self.stopped {
            tracing::warn!(
                tracked = self.blocks.len(),
                "console log stopped compacting streamed delta rows: too many open blocks"
            );
        }
        self.stopped = true;
        for state in self.blocks.values_mut() {
            if state.open_first_delta.take().is_some() {
                state.dropped = true;
            }
        }
    }
}

/// Streamed-block tracking for delta compaction, per runtime (see the
/// module documentation).
#[derive(Debug)]
pub(super) struct StreamedBlocks {
    runtimes: HashMap<String, RuntimeBlocks>,
    max_blocks: usize,
}

impl Default for StreamedBlocks {
    fn default() -> Self {
        Self::with_max_blocks(max_tracked_blocks())
    }
}

impl StreamedBlocks {
    pub(super) fn with_max_blocks(max_blocks: usize) -> Self {
        Self {
            runtimes: HashMap::new(),
            max_blocks,
        }
    }

    fn runtime(&mut self, runtime_key: &str) -> &mut RuntimeBlocks {
        let max_blocks = self.max_blocks;
        self.runtimes
            .entry(runtime_key.to_string())
            .or_insert_with(|| RuntimeBlocks::new(max_blocks))
    }

    /// Before a delta row is appended: whether it is a replay of a row
    /// compaction removed, which must not be stored again.
    pub(super) fn replays_removed_delta(
        &mut self,
        runtime_key: &str,
        key: &BlockKey,
        source_sequence: u64,
        now_ms: u64,
    ) -> bool {
        let runtime = self.runtime(runtime_key);
        runtime.projections = runtime.projections.saturating_add(1);
        let Some(state) = runtime.blocks.get_mut(key) else {
            return false;
        };
        state.last_seen_ms = now_ms;
        state
            .compacted_through
            .is_some_and(|through| source_sequence <= through)
    }

    /// A delta row of the block is stored at `cursor`, appended now or found
    /// from an earlier projection: follow the block from its first row.
    pub(super) fn delta_stored(
        &mut self,
        runtime_key: &str,
        key: BlockKey,
        cursor: u64,
        now_ms: u64,
    ) {
        let runtime = self.runtime(runtime_key);
        if runtime.stopped {
            return;
        }
        if let Some(state) = runtime.blocks.get_mut(&key) {
            state.last_seen_ms = now_ms;
            if !state.dropped {
                state.open_first_delta = Some(
                    state
                        .open_first_delta
                        .map_or(cursor, |first| first.min(cursor)),
                );
            }
            return;
        }
        runtime.follow(key, cursor, now_ms);
    }

    /// The block's complete row was appended: the first delta row to compact
    /// from, when the projection followed the block from it.
    pub(super) fn complete_stored(
        &mut self,
        runtime_key: &str,
        key: &BlockKey,
        now_ms: u64,
    ) -> Option<u64> {
        let runtime = self.runtime(runtime_key);
        runtime.projections = runtime.projections.saturating_add(1);
        let projections = runtime.projections;
        let stopped = runtime.stopped;
        let state = runtime.blocks.get_mut(key)?;
        state.last_seen_ms = now_ms;
        state.settled_at = projections;
        if stopped || state.dropped {
            return None;
        }
        state.open_first_delta.take()
    }

    /// The store removed the block's delta rows up to its complete row,
    /// which sits at `through_sequence` in the member's stream.
    pub(super) fn compacted(&mut self, runtime_key: &str, key: &BlockKey, through_sequence: u64) {
        let runtime = self.runtime(runtime_key);
        let projections = runtime.projections;
        if let Some(state) = runtime.blocks.get_mut(key) {
            state.compacted_through = Some(
                state
                    .compacted_through
                    .map_or(through_sequence, |through| through.max(through_sequence)),
            );
            state.settled_at = projections;
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const RUNTIME: &str = "runtime-a";

    fn key(kind: ConsoleDeltaKind, message: &str) -> BlockKey {
        BlockKey {
            identity: "worker".to_string(),
            kind,
            assistant_message_id: message.to_string(),
        }
    }

    /// Project one delta of `key` stored at `cursor`, as the projection does.
    fn delta(
        blocks: &mut StreamedBlocks,
        key: &BlockKey,
        sequence: u64,
        cursor: u64,
        now_ms: u64,
    ) -> bool {
        if blocks.replays_removed_delta(RUNTIME, key, sequence, now_ms) {
            return false;
        }
        blocks.delta_stored(RUNTIME, key.clone(), cursor, now_ms);
        true
    }

    #[test]
    fn the_redelivery_horizon_covers_every_replay_and_every_lag_channel() {
        use crate::unified_runtime::console_events::{
            ALL_EVENTS_REPLAY_CAP, event_channel_capacity_for_members,
        };
        // A replay reads at most the buffered window; a lagging receiver
        // holds at most its channel, whatever the member count.
        for members in [0, 1, 256, 10_000, usize::MAX] {
            let largest =
                ALL_EVENTS_REPLAY_CAP.saturating_add(event_channel_capacity_for_members(members));
            assert!(
                CONSOLE_EVENT_REDELIVERY_HORIZON >= largest,
                "{members} members: horizon {CONSOLE_EVENT_REDELIVERY_HORIZON} < {largest}"
            );
        }
        assert_eq!(
            redelivery_horizon(),
            u64::try_from(CONSOLE_EVENT_REDELIVERY_HORIZON).expect("fits")
        );
        assert!(max_tracked_blocks() > CONSOLE_EVENT_REDELIVERY_HORIZON);
    }

    #[test]
    fn a_followed_block_compacts_from_its_first_delta() {
        let mut blocks = StreamedBlocks::default();
        let text = key(ConsoleDeltaKind::Text, "message-a");
        for (sequence, cursor) in [(1, 10), (2, 12), (3, 15)] {
            assert!(delta(&mut blocks, &text, sequence, cursor, 0));
        }
        assert_eq!(blocks.complete_stored(RUNTIME, &text, 0), Some(10));
        // Taken once: a second complete row of the block compacts nothing.
        assert_eq!(blocks.complete_stored(RUNTIME, &text, 0), None);
    }

    #[test]
    fn replays_of_removed_deltas_are_skipped_until_the_horizon_passes() {
        let mut blocks = StreamedBlocks::default();
        let text = key(ConsoleDeltaKind::Text, "message-a");
        assert!(delta(&mut blocks, &text, 1, 10, 0));
        assert!(delta(&mut blocks, &text, 2, 11, 0));
        assert_eq!(blocks.complete_stored(RUNTIME, &text, 0), Some(10));
        blocks.compacted(RUNTIME, &text, 3);
        // Registration replay, lag recovery and the shutdown drain deliver
        // the same events again.
        assert!(!delta(&mut blocks, &text, 1, 99, 0));
        assert!(!delta(&mut blocks, &text, 2, 99, 0));
        // Past the horizon nothing can deliver them again, and a sweep
        // forgets the block.
        let other = key(ConsoleDeltaKind::Text, "message-b");
        let horizon = redelivery_horizon();
        let runtime = blocks.runtime(RUNTIME);
        runtime.projections = runtime.projections.saturating_add(horizon);
        runtime.sweep(0);
        assert!(!blocks.runtime(RUNTIME).blocks.contains_key(&text));
        assert!(delta(&mut blocks, &other, 1, 200, 0));
    }

    #[test]
    fn a_second_reasoning_block_of_the_message_streams_and_compacts() {
        let mut blocks = StreamedBlocks::default();
        let reasoning = key(ConsoleDeltaKind::Reasoning, "message-a");
        assert!(delta(&mut blocks, &reasoning, 1, 10, 0));
        assert!(delta(&mut blocks, &reasoning, 2, 11, 0));
        assert_eq!(blocks.complete_stored(RUNTIME, &reasoning, 0), Some(10));
        blocks.compacted(RUNTIME, &reasoning, 3);
        // The second block streams above the first one's complete row.
        assert!(delta(&mut blocks, &reasoning, 4, 20, 0));
        assert!(delta(&mut blocks, &reasoning, 5, 21, 0));
        // A replay of the first block is still skipped.
        assert!(!delta(&mut blocks, &reasoning, 2, 99, 0));
        assert_eq!(blocks.complete_stored(RUNTIME, &reasoning, 0), Some(20));
        blocks.compacted(RUNTIME, &reasoning, 6);
        assert!(!delta(&mut blocks, &reasoning, 5, 99, 0));
    }

    #[test]
    fn an_interrupted_block_is_dropped_and_keeps_its_deltas() {
        let mut blocks = StreamedBlocks::default();
        let text = key(ConsoleDeltaKind::Text, "message-a");
        assert!(delta(&mut blocks, &text, 1, 10, 0));
        // Silent past the stale bound: the next sweep drops it.
        let runtime = blocks.runtime(RUNTIME);
        runtime.sweep(STALE_BLOCK_MS);
        let state = runtime.blocks.get(&text).expect("dropped block remembered");
        assert!(state.dropped && state.open_first_delta.is_none());
        // It resumes (a retry after a long wait): followed no more.
        assert!(delta(&mut blocks, &text, 2, 30, STALE_BLOCK_MS));
        assert_eq!(blocks.complete_stored(RUNTIME, &text, STALE_BLOCK_MS), None);
    }

    #[test]
    fn on_overflow_the_open_blocks_keep_their_deltas() {
        let mut blocks = StreamedBlocks::with_max_blocks(3);
        let open: Vec<BlockKey> = (0..3)
            .map(|index| key(ConsoleDeltaKind::Text, &format!("open-{index}")))
            .collect();
        for (index, block) in open.iter().enumerate() {
            let cursor = u64::try_from(index).expect("cursor") + 10;
            assert!(delta(&mut blocks, block, 1, cursor, 0));
        }
        // No room to follow a fourth block from its first delta row.
        let fourth = key(ConsoleDeltaKind::Text, "fourth");
        assert!(delta(&mut blocks, &fourth, 1, 20, 0));
        for block in open.iter().chain([&fourth]) {
            assert!(delta(&mut blocks, block, 2, 30, 0));
            assert_eq!(blocks.complete_stored(RUNTIME, block, 0), None);
        }
        // A later block is not followed either.
        let later = key(ConsoleDeltaKind::Text, "later");
        assert!(delta(&mut blocks, &later, 1, 40, 0));
        assert_eq!(blocks.complete_stored(RUNTIME, &later, 0), None);
    }

    #[test]
    fn runtimes_are_tracked_apart() {
        let mut blocks = StreamedBlocks::default();
        let text = key(ConsoleDeltaKind::Text, "message-a");
        assert!(delta(&mut blocks, &text, 1, 10, 0));
        assert_eq!(blocks.complete_stored(RUNTIME, &text, 0), Some(10));
        blocks.compacted(RUNTIME, &text, 2);
        // Another runtime's projection of the same block is its own.
        assert!(!blocks.replays_removed_delta("runtime-b", &text, 1, 0));
    }
}
