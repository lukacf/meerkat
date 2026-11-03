import type { ConsoleAgent, ConsoleFrame } from "../types";
import { WORKGRAPH_LOCAL_SOURCE_KIND } from "./adapters";

/// The member status poll is the server's authoritative idle signal: the
/// console projection reports no response phase and Core reports no open
/// run. A frame fold can still hold an owner open when the frames that
/// opened it never get a matching terminal (a runtime-started continuation
/// that finishes a paused interaction under another lineage, or a frame
/// with no ids at all), and the chat then reads "working" and queues every
/// send. Two idle polls in a row, with no frame for the member between or
/// during them, settle every frame the console had seen.
export function agentReportsIdle(agent: Pick<ConsoleAgent, "response_phase" | "progress">): boolean {
  return agent.response_phase === null && agent.progress?.run_state === "idle";
}

/// The least time between the idle poll that arms a settlement and the one
/// that settles it. Polls also run shortly after any member's lifecycle
/// event, so two polls can read the same cached walk a few hundred
/// milliseconds apart; the regular poll runs every 15 s.
export const IDLE_SETTLE_MIN_GAP_MS = 10_000;

/// The clock the settlement reads: the page's monotonic clock, so setting
/// the computer's clock neither settles a member early nor holds a settled
/// one open. Tests replace `now`.
export const idleSettleClock = { now: (): number => performance.now() };

/// What one status poll does to one identity's fold:
/// - "reset": the poll does not count toward settling (the server reports
///   the member busy, a frame arrived while the poll was in flight, this
///   console's own send is in flight, or nothing is held open);
/// - "arm": the first idle poll since the last frame; nothing changes yet;
/// - "wait": another idle poll too soon after the arming one; it keeps the
///   arming poll;
/// - "settle": an idle poll at least `IDLE_SETTLE_MIN_GAP_MS` after the
///   arming one, with no frame for the member in between.
///
/// One idle poll is not enough: a run the runtime starts itself shows no
/// response phase until its first output, and the run state the poll reads
/// comes from a cached walk that can predate the run.
export type IdleSettlement = "reset" | "arm" | "wait" | "settle";

export interface IdleArm {
  version: number;
  atMs: number;
}

export function decideIdleSettlement(args: {
  agent: Pick<ConsoleAgent, "response_phase" | "progress">;
  logVersionAtRequest: number | undefined;
  logVersionNow: number;
  sendInFlight: boolean;
  foldBusy: boolean;
  foldPhase: string | null;
  armed: IdleArm | undefined;
  nowMs: number;
}): IdleSettlement {
  if (!agentReportsIdle(args.agent)) return "reset";
  if (args.logVersionAtRequest === undefined || args.logVersionAtRequest !== args.logVersionNow) return "reset";
  if (args.sendInFlight) return "reset";
  if (!args.foldBusy && args.foldPhase === null) return "reset";
  if (!args.armed || args.armed.version !== args.logVersionNow) return "arm";
  return args.nowMs - args.armed.atMs >= IDLE_SETTLE_MIN_GAP_MS ? "settle" : "wait";
}

/// The identity key of a frame in the console's per-identity log: the
/// event id both pipelines carry, then the cursor, then a synthetic key.
export function consoleFrameKey(frame: ConsoleFrame): string {
  if (frame.id) return frame.id;
  if (frame.cursor) return frame.cursor;
  return `${frame.event}:${frame.identity || ""}:${frame.interactionId || ""}:${frame.timestampMs || 0}`;
}

/// What the console had seen when the member was settled: every frame key
/// in the log, and the newest SERVER timestamp among them. Frames the
/// console mints itself (`console-local`) carry the browser's clock, which
/// can run ahead of the server's, so they never set the timestamp.
export interface IdleWatermark {
  timestampMs: number;
  settledKeys: ReadonlySet<string>;
}

export function idleWatermarkFor(frames: readonly ConsoleFrame[]): IdleWatermark | undefined {
  if (frames.length === 0) return undefined;
  let newest = Number.NEGATIVE_INFINITY;
  const settledKeys = new Set<string>();
  for (const frame of frames) {
    settledKeys.add(consoleFrameKey(frame));
    if (frame.sourceKind !== WORKGRAPH_LOCAL_SOURCE_KIND
      && typeof frame.timestampMs === "number" && frame.timestampMs > newest) {
      newest = frame.timestampMs;
    }
  }
  return { timestampMs: newest, settledKeys };
}

/// Whether a frame is newer than an idle watermark. A frame the console had
/// when the member was settled is settled, and so is a late frame stamped
/// strictly before the watermark. Any other frame is read, including one
/// stamped in the watermark's own millisecond.
export function isAfterIdleWatermark(frame: ConsoleFrame, watermark: IdleWatermark | undefined): boolean {
  if (!watermark) return true;
  if (watermark.settledKeys.has(consoleFrameKey(frame))) return false;
  return !(typeof frame.timestampMs === "number" && frame.timestampMs < watermark.timestampMs);
}

/// The frames a fold should still read after an idle watermark. Without a
/// watermark the same array is returned, so memoised derivations keep their
/// identity.
export function framesAfterIdleWatermark(
  frames: ConsoleFrame[],
  watermark: IdleWatermark | undefined,
): ConsoleFrame[] {
  if (!watermark) return frames;
  return frames.filter((frame) => isAfterIdleWatermark(frame, watermark));
}
