import type { ConsoleFrame, ConsoleTimelinePage } from "./runtime-types";
import type { ConsoleTimelineSubscribeInput, MetaConsoleTransport } from "./headless";
import { acceptConsoleFrame, ConsoleConsumerError } from "./sse-reader";
import { timelineCursor, type ConsoleTransportState } from "./timeline-subscription";

const MIN_TIMELINE_DEDUP_KEYS = 1_000;

/**
 * The frame keys a timeline consumer has accepted. A consumer that keeps one
 * for its scope and passes it to every subscription of that scope gets each
 * frame once: a re-subscription's recent page (the stream re-seeds whenever
 * the consumer re-subscribes) delivers only frames not accepted before.
 * Bounded: the oldest keys go first.
 */
export interface ConsoleTimelineDedupe {
  has(key: string): boolean;
  add(key: string): void;
  readonly size: number;
}

export function createConsoleTimelineDedupe(maxKeys = MIN_TIMELINE_DEDUP_KEYS * 4): ConsoleTimelineDedupe {
  const keys = new Set<string>();
  const cap = Math.max(1, Math.floor(maxKeys));
  return {
    has: (key) => keys.has(key),
    add(key) {
      keys.add(key);
      while (keys.size > cap) keys.delete(keys.values().next().value!);
    },
    get size() { return keys.size; },
  };
}

export interface ConsoleTimelineRecoveryInput extends ConsoleTimelineSubscribeInput {
  /** The scope's accepted keys, kept by the consumer across its
   * subscriptions. Without one, each subscription starts with none. It is
   * never passed to the transport. */
  dedupe?: ConsoleTimelineDedupe;
}

/** One lifetime owns seed, live reader and every gap repair for this scope. */
export async function subscribeTimelineWithRecovery(
  transport: Pick<MetaConsoleTransport, "queryTimeline" | "subscribeTimeline">,
  recoveryInput: ConsoleTimelineRecoveryInput,
  onFrame: (frame: ConsoleFrame) => void,
  onReplayGap?: () => void,
): Promise<() => void> {
  const { dedupe, ...input } = recoveryInput;
  const lifetime = new AbortController();
  const keys = dedupe ?? createConsoleTimelineDedupe(Math.max(MIN_TIMELINE_DEDUP_KEYS, (input.limit || 400) * 4));
  let streamGeneration = 0;
  let unsubscribe: (() => void) | undefined;
  let repairing = false;
  let stopped = false;
  let consecutiveGaps = 0;
  let liveSince: number | undefined;
  let acceptedCursor = input.after;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let wake: (() => void) | undefined;
  const state = (phase: ConsoleTransportState["phase"], extra: Partial<ConsoleTransportState> = {}) => {
    acceptConsoleFrame(input.onTransportState, {
      phase, stale: phase !== "live", freshness: "unknown", cursor: acceptedCursor, ...extra,
    });
  };
  const stopStream = () => {
    ++streamGeneration;
    const previous = unsubscribe;
    unsubscribe = undefined;
    previous?.();
  };
  const stop = () => {
    if (stopped) return;
    stopped = true;
    lifetime.abort();
    stopStream();
    if (timer !== undefined) clearTimeout(timer);
    wake?.();
    input.signal?.removeEventListener("abort", stop);
    try { state("stopped"); } catch { /* Disposed observer. */ }
  };
  input.signal?.addEventListener("abort", stop, { once: true });
  if (input.signal?.aborted) { stop(); return stop; }
  const deliver = (frame: ConsoleFrame) => {
    if (stopped) return;
    const key = timelineDedupKey(frame);
    if (key && keys.has(key)) return;
    acceptConsoleFrame(onFrame, frame);
    // Commit dedup and cursor only after consumer acceptance.
    if (key) keys.add(key);
    acceptedCursor = timelineCursor(frame) || acceptedCursor;
  };
  const reportFailure = (error: unknown): boolean => {
    if (stopped) return true;
    const httpStatus = (error as { httpStatus?: number } | null)?.httpStatus;
    if (error instanceof ConsoleConsumerError) {
      try { state("consumer-failed", { error }); } catch { /* Failed observer. */ }
      stopped = true;
    } else if (httpStatus === 401 || httpStatus === 403) {
      state(httpStatus === 401 ? "authentication-required" : "forbidden", { error, httpStatus });
      stopped = true;
    }
    if (stopped) {
      lifetime.abort();
      stopStream();
      input.signal?.removeEventListener("abort", stop);
    }
    return stopped;
  };
  const delay = (ms: number) => new Promise<void>((resolve) => {
    wake = resolve;
    timer = setTimeout(() => { timer = undefined; wake = undefined; resolve(); }, ms);
  });
  const waitUntilOnline = async () => {
    if (typeof navigator === "undefined" || navigator.onLine !== false) return;
    state("offline");
    await new Promise<void>((resolve) => {
      const done = () => {
        if (typeof window !== "undefined") window.removeEventListener("online", check);
        if (typeof document !== "undefined") document.removeEventListener("visibilitychange", check);
        lifetime.signal.removeEventListener("abort", done);
        resolve();
      };
      const check = () => { if (navigator.onLine !== false) done(); };
      if (typeof window !== "undefined") window.addEventListener("online", check);
      if (typeof document !== "undefined") document.addEventListener("visibilitychange", check);
      lifetime.signal.addEventListener("abort", done, { once: true });
      if (lifetime.signal.aborted) done();
    });
  };
  const query = async (): Promise<ConsoleTimelinePage | undefined> => {
    // Even a legacy transport ignoring AbortSignal cannot keep disposal pending
    // or deliver a late page into a replacement scope.
    let abort: (() => void) | undefined;
    const cancelled = new Promise<undefined>((resolve) => {
      abort = () => resolve(undefined);
      lifetime.signal.addEventListener("abort", abort, { once: true });
    });
    try {
      return await Promise.race([
        transport.queryTimeline({ ...input, after: undefined, mode: "recent", signal: lifetime.signal }),
        cancelled,
      ]);
    } finally {
      if (abort) lifetime.signal.removeEventListener("abort", abort);
    }
  };
  const startStream = () => {
    if (stopped) return;
    const generation = ++streamGeneration;
    const handle = transport.subscribeTimeline({ ...input, after: acceptedCursor, signal: lifetime.signal }, (frame) => {
      if (stopped || generation !== streamGeneration) return;
      if (frame.event === "replay_unavailable") {
        if (liveSince !== undefined && Date.now() - liveSince >= 30_000) consecutiveGaps = 0;
        liveSince = undefined;
        stopStream();
        void repair(true);
        return;
      }
      try { deliver(frame); } catch (error) {
        reportFailure(error);
        throw error;
      }
      if (frame.event === "snapshot_complete") liveSince = Date.now();
    }, {
      signal: lifetime.signal,
      onTransportState: (value) => {
        if (!stopped && generation === streamGeneration) acceptConsoleFrame(input.onTransportState, value);
      },
    });
    // Custom transports may deliver synchronously from subscribeTimeline.
    if (stopped || generation !== streamGeneration) handle();
    else unsubscribe = handle;
  };
  const repair = async (gap: boolean) => {
    if (stopped || repairing) return;
    repairing = true;
    try {
      if (gap && ++consecutiveGaps > 5) {
        state("stopped", { error: new Error("Repeated timeline gaps require an explicit retry") });
        stopped = true;
        lifetime.abort();
        return;
      }
      if (gap && consecutiveGaps > 1) {
        const retryInMs = Math.min(250 * 2 ** (consecutiveGaps - 2), 4000);
        state("retrying", { retryInMs });
        await delay(retryInMs);
      }
      for (let attempt = 0; !stopped; attempt++) {
        try {
          if (typeof navigator !== "undefined" && navigator.onLine === false) await waitUntilOnline();
          if (stopped) return;
          state("connecting");
          const page = await query();
          if (stopped || !page) return;
          if (page.available === false) throw new Error("Timeline history is unavailable");
          for (const frame of page.frames) {
            deliver(frame);
            if (stopped) return;
          }
          // The page's frontier commits atomically with acceptance of its frames.
          // For a recent page latestCursor covers the queried snapshot, including
          // filtered events. Never accept a cursor from a rejected gap response.
          acceptedCursor = page.latestCursor || page.nextCursor || acceptedCursor;
          if (gap) acceptConsoleFrame(onReplayGap, undefined);
          break;
        } catch (error) {
          if (reportFailure(error)) return;
          if (attempt >= 3) {
            state("stopped", { error });
            stopped = true;
            lifetime.abort();
            return;
          }
          const retryInMs = 250 * 2 ** attempt;
          state("retrying", { error, retryInMs });
          await delay(retryInMs);
        }
      }
    } catch (error) {
      reportFailure(error);
    } finally {
      repairing = false;
      if (stopped) input.signal?.removeEventListener("abort", stop);
    }
    if (!stopped) {
      try { startStream(); } catch (error) {
        if (!reportFailure(error)) {
          try { state("stopped", { error }); } catch { /* Failed observer. */ }
          stop();
        }
      }
    } else input.signal?.removeEventListener("abort", stop);
  };
  await repair(false);
  return stop;
}

function timelineDedupKey(frame: ConsoleFrame): string | null {
  const id = frame.id?.trim();
  if (id) return `id:${id}:${frame.event || ""}:${frame.frameVersion ?? ""}:${frame.updatedAtMs ?? ""}`;
  const cursor = frame.cursor?.trim();
  if (cursor) return `cursor:${cursor}`;
  const timestamp = frame.timestampMs;
  if (typeof timestamp === "number") {
    return `timestamp:${frame.event || ""}:${frame.identity || ""}:${timestamp}:${stableDedupText(frame.data)}`;
  }
  return null;
}
function stableDedupText(value: unknown): string {
  try {
    return JSON.stringify(value, (_key, nested) => {
      if (!nested || typeof nested !== "object" || Array.isArray(nested)) return nested;
      return Object.fromEntries(Object.entries(nested as Record<string, unknown>).sort(([left], [right]) => left.localeCompare(right)));
    });
  } catch { return String(value); }
}

const LOCAL_FRAME_KEYS = new WeakMap<ConsoleFrame, string>();
let localFrameSerial = 0;

/**
 * The key a list row of one frame version takes: `id:frameVersion` (version
 * 0 when the frame names none). A frame without an id gets a key of its own
 * object, stable while that object lives. With a buffer kept by
 * {@link insertConsoleFrame} the keys of a list are unique.
 */
export function consoleFrameRowKey(frame: ConsoleFrame): string {
  if (frame.id) return `${frame.id}:${frame.frameVersion ?? 0}`;
  let key = LOCAL_FRAME_KEYS.get(frame);
  if (!key) {
    key = `local:${++localFrameSerial}`;
    LOCAL_FRAME_KEYS.set(frame, key);
  }
  return key;
}

function isNewerFrameVersion(candidate: ConsoleFrame, held: ConsoleFrame): boolean {
  const candidateVersion = candidate.frameVersion ?? 0;
  const heldVersion = held.frameVersion ?? 0;
  if (candidateVersion !== heldVersion) return candidateVersion > heldVersion;
  return (candidate.updatedAtMs ?? 0) > (held.updatedAtMs ?? 0);
}

/**
 * Put one frame at the head of a newest-first frame buffer that keeps at
 * most `cap` frames and one copy of each frame. A frame the buffer already
 * holds (the same id and event) is replaced in place by a newer version
 * (a higher `frameVersion`, or the same one updated later) and ignored
 * otherwise, so a frame delivered twice never lists twice. A frame without
 * an id is always added. Returns the next buffer, or null when it is
 * unchanged.
 */
export function insertConsoleFrame(
  buffer: readonly ConsoleFrame[],
  frame: ConsoleFrame,
  cap: number,
): ConsoleFrame[] | null {
  if (frame.id) {
    const index = buffer.findIndex((held) => held.id === frame.id && held.event === frame.event);
    if (index >= 0) {
      if (!isNewerFrameVersion(frame, buffer[index]!)) return null;
      const next = buffer.slice();
      next[index] = frame;
      return next;
    }
  }
  const next = [frame, ...buffer];
  if (next.length > cap) next.length = Math.max(0, cap);
  return next;
}
