import type { ConsoleFrame } from "../types";

/// The per-identity frame store behind the console's chat panes. `events`
/// keeps arrival order (the key index points into it); `sorted` is the
/// transcript-order view, maintained incrementally on append and on
/// in-place updates that keep a frame's position, and rebuilt lazily by
/// `sortedEvents` after a mutation that could move a frame. `version` is
/// bumped on every mutation so render-time derivations can memoise on it.
export interface IdentityLogCore {
  events: ConsoleFrame[];
  byKey: Map<string, number>;
  version: number;
  sorted: ConsoleFrame[] | null;
}

export function createIdentityLogCore(): IdentityLogCore {
  return { events: [], byKey: new Map(), version: 0, sorted: null };
}

export function cursorSeq(cursor: string | undefined): number | null {
  if (!cursor) return null;
  const match = /^console:(\d+)$/.exec(cursor);
  if (!match) return null;
  const parsed = Number(match[1]);
  return Number.isFinite(parsed) ? parsed : null;
}

/// Transcript order: timestamp, then aggregate cursor sequence, then
/// arrival index. Aggregate cursor order alone can disagree with
/// conversational order when delayed peer-message or session-history frames
/// are backfilled after newer frames, so it is only a tie-break.
export function transcriptOrder(
  a: ConsoleFrame,
  aIndex: number,
  b: ConsoleFrame,
  bIndex: number,
): number {
  const ta = typeof a.timestampMs === "number" ? a.timestampMs : Number.MAX_SAFE_INTEGER;
  const tb = typeof b.timestampMs === "number" ? b.timestampMs : Number.MAX_SAFE_INTEGER;
  if (ta !== tb) return ta - tb;
  const ca = cursorSeq(a.cursor);
  const cb = cursorSeq(b.cursor);
  if (ca !== null && cb !== null && ca !== cb) return ca - cb;
  return aIndex - bIndex;
}

/// Insert `frame` into `sorted`, which holds frames in transcript order.
/// Every frame already in `sorted` arrived earlier, and arrival is the final
/// tie-break, so a frame that compares equal to an existing one goes after
/// it; the common case (newest timestamp) is a plain push.
export function insertSorted(sorted: ConsoleFrame[], frame: ConsoleFrame): void {
  let lo = 0;
  let hi = sorted.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (transcriptOrder(sorted[mid], 0, frame, 1) <= 0) lo = mid + 1;
    else hi = mid;
  }
  if (lo === sorted.length) sorted.push(frame);
  else sorted.splice(lo, 0, frame);
}

export function sortedEvents(log: IdentityLogCore): ConsoleFrame[] {
  if (log.sorted) return log.sorted;
  const view = log.events
    .map((frame, index) => ({ frame, index }))
    .sort((a, b) => transcriptOrder(a.frame, a.index, b.frame, b.index))
    .map((entry) => entry.frame);
  log.sorted = view;
  return view;
}

/// Append a new frame under `key`. Returns false when the key is already
/// present (RPC and SSE deliver the same logical event).
export function pushFrame(log: IdentityLogCore, key: string, frame: ConsoleFrame): boolean {
  if (log.byKey.has(key)) return false;
  log.byKey.set(key, log.events.length);
  log.events.push(frame);
  log.version += 1;
  if (log.sorted) insertSorted(log.sorted, frame);
  return true;
}

export interface FrameUpdateResult {
  previous: ConsoleFrame;
  next: ConsoleFrame;
  /// True when the update changed the frame's transcript position
  /// (timestamp or cursor), which forces a re-sort. Status-only updates,
  /// the common case for tool calls going pending to done, keep the sorted
  /// view and splice the merged frame in place.
  moved: boolean;
}

/// Merge a `frame_updated` payload into the frame it names. Returns null
/// when the frame is unknown or the update is older than what is held.
export function mergeFrameUpdate(
  log: IdentityLogCore,
  updated: ConsoleFrame,
): FrameUpdateResult | null {
  if (!updated.id) return null;
  const index = log.byKey.get(updated.id);
  if (index === undefined) return null;
  const previous = log.events[index];
  if (!previous) return null;
  const existingVersion = previous.frameVersion ?? 0;
  const updatedVersion = updated.frameVersion ?? existingVersion;
  if (updatedVersion < existingVersion) return null;
  const next: ConsoleFrame = { ...previous, ...updated };
  log.events[index] = next;
  log.version += 1;
  const moved =
    previous.timestampMs !== next.timestampMs || cursorSeq(previous.cursor) !== cursorSeq(next.cursor);
  if (moved) {
    log.sorted = null;
  } else if (log.sorted) {
    const at = log.sorted.indexOf(previous);
    if (at >= 0) log.sorted[at] = next;
    else log.sorted = null;
  }
  return { previous, next, moved };
}

/// Drop the oldest frames in transcript order until at most `max` remain,
/// rebuilding the key index. Returns the retained frames in arrival order,
/// or null when nothing was dropped.
export function trimIdentityLogCore(
  log: IdentityLogCore,
  max: number,
  keyOf: (frame: ConsoleFrame) => string,
): ConsoleFrame[] | null {
  const sorted = sortedEvents(log);
  const drop = sorted.length - max;
  if (drop <= 0) return null;
  const dropped = new Set<ConsoleFrame>(sorted.slice(0, drop));
  const retained = log.events.filter((frame) => !dropped.has(frame));
  log.events = retained;
  log.sorted = sorted.slice(drop);
  log.byKey.clear();
  retained.forEach((frame, index) => log.byKey.set(keyOf(frame), index));
  log.version += 1;
  return retained;
}

export function resetIdentityLogCore(log: IdentityLogCore): void {
  log.events = [];
  log.byKey.clear();
  log.sorted = null;
  log.version += 1;
}
