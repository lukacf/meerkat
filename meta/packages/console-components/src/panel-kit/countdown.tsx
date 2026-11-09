import { useEffect, useState } from "react";

import { cx } from "./cx";
import { formatAbsoluteTime, toEpochMs, type TimeInput } from "./format";
import { KitGlyph } from "./glyphs";
import { useNow } from "./relative-time";
import type { Tone } from "./tone";

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function span(ms: number): string {
  if (ms < MINUTE) return `${Math.max(1, Math.floor(ms / SECOND))}s`;
  if (ms < 2 * MINUTE) {
    const seconds = Math.floor((ms % MINUTE) / SECOND);
    return seconds ? `1m ${seconds}s` : "1m";
  }
  if (ms < HOUR) return `${Math.floor(ms / MINUTE)}m`;
  if (ms < DAY) {
    const hours = Math.floor(ms / HOUR);
    const minutes = Math.floor((ms % HOUR) / MINUTE);
    return minutes ? `${hours}h ${minutes}m` : `${hours}h`;
  }
  const days = Math.floor(ms / DAY);
  const hours = Math.floor((ms % DAY) / HOUR);
  return hours ? `${days}d ${hours}h` : `${days}d`;
}

/** Time to a deadline in words: "in 45s", "in 1m 30s", "in 25m",
 * "in 2h 5m", "in 2d 4h"; past it, "5m overdue". */
export function formatCountdown(remainingMs: number): string {
  if (!Number.isFinite(remainingMs)) return "";
  if (remainingMs <= 0) return `${span(-remainingMs || SECOND)} overdue`;
  return `in ${span(remainingMs)}`;
}

/** The tone of a deadline by its distance (A3.5): more than 1 h neutral,
 * 10 min to 1 h info, under 10 min warn, past due crit. */
export function countdownTone(remainingMs: number): Tone {
  if (remainingMs <= 0) return "crit";
  if (remainingMs < 10 * MINUTE) return "warn";
  if (remainingMs <= HOUR) return "info";
  return "neutral";
}

/** A one-second clock while `active` and the page is visible. */
function useSecondClock(active: boolean): number | null {
  const [now, setNow] = useState<number | null>(null);
  useEffect(() => {
    if (!active || typeof window === "undefined") {
      setNow(null);
      return undefined;
    }
    let timer: ReturnType<typeof setInterval> | null = null;
    const start = () => {
      if (timer === null && !document.hidden) {
        setNow(Date.now());
        timer = setInterval(() => setNow(Date.now()), SECOND);
      }
    };
    const stop = () => {
      if (timer !== null) clearInterval(timer);
      timer = null;
    };
    const onVisibility = () => (document.hidden ? stop() : start());
    start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [active]);
  return now;
}

/**
 * A deadline, next run or pause end (spec A3.5): "in 5m", "in 2h",
 * "5m overdue", toned by distance (neutral, info, warn, crit). It follows
 * the shared 15 s clock, and under two minutes ticks every second on its
 * own timer, which stops when it unmounts or the page is hidden. `chip`
 * draws the 20 px pill with the clock glyph.
 */
export function Countdown({
  to,
  now,
  chip,
  className,
  testId,
}: {
  to: TimeInput;
  /** A fixed clock (tests, snapshots). */
  now?: number;
  chip?: boolean;
  className?: string;
  testId?: string;
}) {
  const shared = useNow();
  const target = toEpochMs(to);
  const coarse = now ?? shared;
  const near = target !== null && Math.abs(target - coarse) < 2 * MINUTE;
  const fine = useSecondClock(now === undefined && near);
  if (target === null) return null;
  const current = now ?? fine ?? shared;
  const remaining = target - current;
  const tone = countdownTone(remaining);
  return (
    <span className={cx("cc-countdown", chip && "cc-countdown--chip", className)} data-tone={tone} data-testid={testId}>
      {chip ? <KitGlyph name="clock" className="cc-countdown__glyph" /> : null}
      <time dateTime={new Date(target).toISOString()} title={formatAbsoluteTime(target)}>
        {formatCountdown(remaining)}
      </time>
    </span>
  );
}
