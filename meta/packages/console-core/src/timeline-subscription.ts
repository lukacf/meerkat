import type { ConsoleFrame } from "./runtime-types";
import { ConsoleConsumerError, acceptConsoleFrame } from "./sse-reader";

export type ConsoleTransportPhase =
  | "connecting" | "connected-replaying" | "live" | "offline" | "retrying"
  | "authentication-required" | "forbidden" | "consumer-failed" | "stopped";
export interface ConsoleTransportState {
  phase: ConsoleTransportPhase;
  stale: boolean;
  freshness: "unknown" | "replaying" | "current";
  cursor?: string;
  retryInMs?: number;
  httpStatus?: number;
  error?: unknown;
}
export interface ConsoleTimelineSubscriptionOptions {
  signal?: AbortSignal;
  onTransportState?: (state: ConsoleTransportState) => void;
}
export interface ConsoleTimelineSubscription {
  (): void;
  /** Wake a transient retry. Authentication and denial require a new subscription. */
  retry(): void;
  /** Last synchronously accepted cursor; never an unaccepted server frontier. */
  cursor(): string | undefined;
}
export interface ConsoleStreamFailure extends Error {
  httpStatus?: number;
  replayFrame?: ConsoleFrame;
}

export function timelineCursor(frame: ConsoleFrame): string | undefined {
  if (frame.cursor?.trim()) return frame.cursor.trim();
  if (frame.event === "snapshot_complete") {
    if (frame.id?.startsWith("console:")) return frame.id;
    const data = frame.data as { cursor?: unknown } | null;
    if (typeof data?.cursor === "string") return data.cursor;
  }
  return undefined;
}

export function createTimelineSubscription(input: {
  after?: string;
  options?: ConsoleTimelineSubscriptionOptions;
  open(signal: AbortSignal, after: string | undefined, connected: () => void, deliver: (frame: ConsoleFrame) => void): Promise<void>;
  onFrame(frame: ConsoleFrame): void;
}): ConsoleTimelineSubscription {
  const options = input.options || {};
  let stopped = false;
  let after = input.after?.trim() || undefined;
  let active: AbortController | undefined;
  let wake: (() => void) | undefined;
  let retryAttempt = 0;
  let freshness: ConsoleTransportState["freshness"] = "unknown";
  const state = (phase: ConsoleTransportPhase, extra: Partial<ConsoleTransportState> = {}) => {
    acceptConsoleFrame(options.onTransportState, {
      phase, stale: phase !== "live", freshness, cursor: after, ...extra,
    });
  };
  const online = () => typeof navigator === "undefined" || navigator.onLine !== false;
  const onWake = () => { if (online()) wake?.(); };
  const onForeground = () => {
    if (typeof document === "undefined" || document.visibilityState !== "hidden") onWake();
  };
  const cleanup = () => {
    options.signal?.removeEventListener("abort", stop);
    if (typeof window !== "undefined") window.removeEventListener("online", onWake);
    if (typeof document !== "undefined") document.removeEventListener("visibilitychange", onForeground);
  };
  const stop = (() => {
    if (stopped) return;
    stopped = true;
    active?.abort();
    wake?.();
    cleanup();
    try { state("stopped"); } catch { /* A disposed observer cannot restart work. */ }
  }) as ConsoleTimelineSubscription;
  stop.retry = onWake;
  stop.cursor = () => after;
  options.signal?.addEventListener("abort", stop, { once: true });
  if (typeof window !== "undefined") window.addEventListener("online", onWake);
  if (typeof document !== "undefined") document.addEventListener("visibilitychange", onForeground);
  const wait = (ms?: number) => new Promise<void>((resolve) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = () => {
      if (timer !== undefined) clearTimeout(timer);
      if (wake === finish) wake = undefined;
      resolve();
    };
    wake = finish;
    if (ms !== undefined) timer = setTimeout(finish, ms);
    if (stopped) finish();
  });
  const deliver = (frame: ConsoleFrame) => {
    if (stopped) return;
    acceptConsoleFrame(input.onFrame, frame);
    // Delivery is the commit boundary. A throwing consumer leaves after alone.
    after = timelineCursor(frame) || after;
    if (!stopped && frame.event === "snapshot_complete") {
      freshness = "current";
      retryAttempt = 0;
      state("live");
    }
  };
  void (async () => {
    if (options.signal?.aborted) { stop(); return; }
    try {
      while (!stopped) {
        if (!online()) {
          freshness = "unknown";
          state("offline");
          await wait();
          continue;
        }
        active = new AbortController();
        freshness = "unknown";
        state("connecting");
        let error: unknown;
        try {
          await input.open(active.signal, after, () => {
            if (stopped) return;
            freshness = "replaying";
            state("connected-replaying");
          }, deliver);
        } catch (caught) { error = caught; }
        if (stopped || active.signal.aborted) break;
        if (error instanceof ConsoleConsumerError) {
          state("consumer-failed", { error });
          break;
        }
        const fault = error as ConsoleStreamFailure | undefined;
        if (fault?.replayFrame) {
          // Only owner-typed faults become protocol frames. The headless owner
          // cancels this subscription, repairs history and chooses the cursor.
          deliver(fault.replayFrame);
          if (!stopped) state("stopped", { error });
          break;
        }
        if (fault?.httpStatus === 401 || fault?.httpStatus === 403) {
          state(fault.httpStatus === 401 ? "authentication-required" : "forbidden", {
            error, httpStatus: fault.httpStatus,
          });
          break;
        }
        if (fault?.httpStatus && fault.httpStatus < 500 && fault.httpStatus !== 408 && fault.httpStatus !== 429) {
          state("stopped", { error, httpStatus: fault.httpStatus });
          break;
        }
        freshness = "unknown";
        if (!online()) { state("offline", { error }); continue; }
        const retryInMs = Math.round(Math.min(250 * 2 ** Math.min(retryAttempt++, 5), 8000) * (0.8 + Math.random() * 0.4));
        state("retrying", { error, retryInMs, httpStatus: fault?.httpStatus });
        await wait(retryInMs);
      }
    } catch (error) {
      // This also catches observers throwing from state notifications. Never
      // allow their exception to escape as an unhandled async rejection.
      try { state("consumer-failed", { error }); } catch { /* Failed observer. */ }
    } finally {
      stopped = true;
      active?.abort();
      wake?.();
      cleanup();
    }
  })();
  return stop;
}
