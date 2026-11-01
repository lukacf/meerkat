import { useSyncExternalStore } from "react";

import { cx } from "./cx";
import { formatAbsoluteTime, formatRelativeTime, toEpochMs, type TimeInput } from "./format";
import { KitGlyph } from "./glyphs";

// One shared clock for every RelativeTime on the page: a single interval runs
// while at least one is mounted, so a panel with hundreds of rows re-renders
// its times together instead of each row owning a timer.
const TICK_MS = 15_000;
let tickNow = Date.now();
let timer: ReturnType<typeof setInterval> | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (timer === null) {
    tickNow = Date.now();
    timer = setInterval(() => {
      tickNow = Date.now();
      for (const notify of listeners) notify();
    }, TICK_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  };
}

function getSnapshot(): number {
  // With no subscriber the interval is stopped; refresh a stale reading so the
  // first render after a quiet period does not show an old clock.
  if (timer === null) {
    const now = Date.now();
    if (now - tickNow >= TICK_MS) tickNow = now;
  }
  return tickNow;
}

/** The shared clock in epoch milliseconds, updated every 15 s while used. */
export function useNow(): number {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * A compact relative time ("12m ago") in a <time> element whose title holds
 * the absolute local time. Renders nothing for unreadable values.
 */
export function RelativeTime({
  value,
  now,
  title,
  prefix,
  className,
  testId,
}: {
  value: TimeInput;
  /** Fixed clock (tests, snapshots); the shared clock otherwise. */
  now?: number;
  /** Tooltip override; the absolute local time otherwise. */
  title?: string;
  /** Words before the time, inside the element: "Updated", "Last run". */
  prefix?: string;
  className?: string;
  testId?: string;
}) {
  const shared = useNow();
  const ms = toEpochMs(value);
  if (ms === null) return null;
  // The shared clock lags the real one by up to a tick: a time read since
  // its last tick ("Updated" after a refresh) must never read as the future.
  // So compare against the later of the two, and read a time less than one
  // tick ahead (a clock skew between hosts) as "just now".
  const current = now ?? Math.max(shared, Date.now());
  const text = ms > current && ms - current < TICK_MS ? "just now" : formatRelativeTime(ms, current);
  return (
    <time
      className={cx("cc-time", className)}
      dateTime={new Date(ms).toISOString()}
      title={title ?? formatAbsoluteTime(ms)}
      data-testid={testId}
    >
      {prefix ? `${prefix} ${text}` : text}
    </time>
  );
}

/** RelativeTime in a 20 px pill on the soft surface with the clock glyph
 * (spec A3.5): for a time that stands alone in a card or a strip. */
export function RelativeTimeChip({
  value,
  now,
  title,
  prefix,
  className,
  testId,
}: {
  value: TimeInput;
  now?: number;
  title?: string;
  prefix?: string;
  className?: string;
  testId?: string;
}) {
  if (toEpochMs(value) === null) return null;
  return (
    <span className={cx("cc-time-chip", className)} data-testid={testId}>
      <KitGlyph name="clock" className="cc-time-chip__glyph" />
      <RelativeTime value={value} now={now} title={title} prefix={prefix} />
    </span>
  );
}
