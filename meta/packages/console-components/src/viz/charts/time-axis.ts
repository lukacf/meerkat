// The shared time scale (console v2 spec A3.8, B23 K1): the one mapping from
// time to x that DotStrip, ActivityBars and the panel charts (Swimlanes,
// SeverityStrip, Agenda, the Dreams timeline) share, with nice local-clock
// ticks, the "now" marker and the brush range. Pure functions, no React.

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export interface TimeScale {
  readonly start: number;
  readonly end: number;
  readonly x0: number;
  readonly x1: number;
  /** x for a time (not clamped). */
  x(t: number): number;
  /** The time at an x (not clamped). */
  t(x: number): number;
  /** Ticks across the domain, with their x. */
  ticks(options?: TimeTickOptions): Array<TimeTick & { x: number }>;
}

/** A linear time scale from [start, end] (epoch ms) onto [x0, x1]. An empty
 * domain maps every time to x0. */
export function createTimeScale({ start, end, range }: { start: number; end: number; range: readonly [number, number] }): TimeScale {
  const [x0, x1] = range;
  const span = end - start;
  const scale: TimeScale = {
    start,
    end,
    x0,
    x1,
    x: (t: number) => (span > 0 ? x0 + ((t - start) / span) * (x1 - x0) : x0),
    t: (x: number) => (x1 !== x0 ? start + ((x - x0) / (x1 - x0)) * span : start),
    ticks: (options?: TimeTickOptions) => timeTicks(start, end, options).map((tick) => ({ ...tick, x: scale.x(tick.t) })),
  };
  return scale;
}

export type TimeTickFormat = "auto" | "hour" | "clock" | "day";

export interface TimeTickOptions {
  /** About how many ticks (default 5). */
  count?: number;
  /** A fixed step in ms (rounded to the nearest nice step otherwise). */
  step?: number;
  /** "hour" reads "06", "clock" "06:00", "day" "Mon 2" (default: by step;
   * an automatic clock axis names the day at midnight). */
  format?: TimeTickFormat;
}

export interface TimeTick {
  t: number;
  label: string;
  /** Local midnight (a day boundary). */
  major: boolean;
}

const STEPS = [
  MINUTE,
  2 * MINUTE,
  5 * MINUTE,
  10 * MINUTE,
  15 * MINUTE,
  30 * MINUTE,
  HOUR,
  2 * HOUR,
  3 * HOUR,
  6 * HOUR,
  12 * HOUR,
  DAY,
  2 * DAY,
  7 * DAY,
];

/** The nice step closest to span / count. */
export function niceTimeStep(span: number, count = 5): number {
  if (!(span > 0)) return HOUR;
  const raw = span / Math.max(1, count);
  let best = STEPS[0];
  for (const step of STEPS) {
    if (Math.abs(Math.log(step / raw)) < Math.abs(Math.log(best / raw))) best = step;
  }
  return best;
}

const pad = (n: number) => String(n).padStart(2, "0");
const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** A tick label in local time: "06" (hour), "06:00" (clock), "Mon 2" (day). */
export function formatTimeTick(t: number, format: Exclude<TimeTickFormat, "auto">): string {
  const date = new Date(t);
  if (format === "hour") return pad(date.getHours());
  if (format === "day") return `${WEEKDAYS[date.getDay()]} ${date.getDate()}`;
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** The first multiple of `step` at or after `t`, aligned to the local clock
 * (local midnight for day steps, the local hour for hour steps). */
function alignUp(t: number, step: number): number {
  const date = new Date(t);
  if (step >= DAY) {
    const midnight = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
    return midnight >= t ? midnight : new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1).getTime();
  }
  // Steps under a day align to the local day's start, so 6 h ticks fall on
  // 00, 06, 12 and 18 local whatever the zone offset.
  const dayStart = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const k = Math.ceil((t - dayStart) / step);
  return dayStart + k * step;
}

/** Ticks on the local clock across [start, end], about `count` of them. */
export function timeTicks(start: number, end: number, options: TimeTickOptions = {}): TimeTick[] {
  if (!(end > start)) return [];
  const step = options.step ?? niceTimeStep(end - start, options.count ?? 5);
  const auto = !options.format || options.format === "auto";
  const format: Exclude<TimeTickFormat, "auto"> = auto ? (step >= DAY ? "day" : "clock") : (options.format as Exclude<TimeTickFormat, "auto">);
  const ticks: TimeTick[] = [];
  for (let t = alignUp(start, step); t <= end && ticks.length < 500; ) {
    const date = new Date(t);
    const major = date.getHours() === 0 && date.getMinutes() === 0;
    // An automatic clock axis names the day at midnight ("Sun 1", not
    // "00:00"), so a span across midnight says which day it reaches.
    ticks.push({ t, label: formatTimeTick(t, auto && major ? "day" : format), major });
    // Day steps follow the calendar (a daylight-saving day is 23 or 25 h).
    t = step >= DAY ? new Date(date.getFullYear(), date.getMonth(), date.getDate() + Math.round(step / DAY)).getTime() : t + step;
  }
  return ticks;
}

/** A tick label placed on the axis: its centre, kept inside the chart. */
export interface PlacedTickLabel extends TimeTick {
  /** The tick's own x. */
  x: number;
  /** Where the label centres: the tick's x, slid inward at an edge. */
  labelX: number;
}

/** Where an axis label lies: its centre and its estimated edges. */
export interface AxisLabelSpan {
  labelX: number;
  left: number;
  right: number;
}

/** The estimated half width of an axis label in the tick font (mono: 0.6 em
 * a character in IBM Plex Mono, 0.66 em allowed for a fallback face, and a
 * pixel each side), so nothing needs measuring. */
function labelHalf(label: string, fontSize: number): number {
  return (label.length * fontSize * 0.66) / 2 + 1;
}

/** An axis label centred at `x` ("now" at its marker), slid inward to stay
 * inside a chart `width` wide. */
export function placeAxisLabel(x: number, label: string, width: number, fontSize = 9.5): AxisLabelSpan {
  const half = labelHalf(label, fontSize);
  const labelX = Math.min(Math.max(x, half), Math.max(half, width - half));
  return { labelX, left: labelX - half, right: labelX + half };
}

/**
 * Tick labels centred on their ticks but kept inside a chart `width` wide: a
 * label that would cross the left edge, or the right edge less `endReserve`,
 * slides inward, and one that would then meet the label before it, or a
 * label in `avoid` (the "now" label, wherever its marker is), is left out.
 */
export function placeTickLabels(
  ticks: ReadonlyArray<TimeTick & { x: number }>,
  width: number,
  {
    fontSize = 9.5,
    endReserve = 0,
    gap = 6,
    avoid = [],
  }: { fontSize?: number; endReserve?: number; gap?: number; avoid?: ReadonlyArray<AxisLabelSpan> } = {},
): PlacedTickLabel[] {
  const placed: PlacedTickLabel[] = [];
  let lastEnd = -Infinity;
  for (const tick of ticks) {
    const half = labelHalf(tick.label, fontSize);
    const right = width - endReserve;
    if (right - half < half) continue;
    const labelX = Math.min(Math.max(tick.x, half), right - half);
    const left = labelX - half;
    const end = labelX + half;
    if (left < lastEnd + gap) continue;
    if (avoid.some((span) => left < span.right + gap && end > span.left - gap)) continue;
    placed.push({ ...tick, labelX });
    lastEnd = end;
  }
  return placed;
}

/** The "now" marker: its x and label when now is inside the domain. */
export function nowMarker(scale: TimeScale, now: number = Date.now()): { x: number; t: number; label: string } | null {
  if (now < scale.start || now > scale.end) return null;
  return { x: scale.x(now), t: now, label: "now" };
}

/** The start time of each of `bins` equal bins across [start, end]. */
export function binTimes(start: number, end: number, bins: number): number[] {
  const count = Math.max(0, Math.floor(bins));
  const width = count > 0 ? (end - start) / count : 0;
  return Array.from({ length: count }, (_, index) => start + index * width);
}

/** The bin a time falls in, or -1 outside [start, end). The end itself
 * counts in the last bin. */
export function binIndex(t: number, start: number, end: number, bins: number): number {
  if (!(end > start) || bins <= 0 || t < start || t > end) return -1;
  return Math.min(bins - 1, Math.floor(((t - start) / (end - start)) * bins));
}

/** Counts of the times in each of `bins` bins across [start, end]. */
export function binCounts(times: Iterable<number>, start: number, end: number, bins: number): number[] {
  const counts = new Array<number>(Math.max(0, Math.floor(bins))).fill(0);
  for (const t of times) {
    const index = binIndex(t, start, end, counts.length);
    if (index >= 0) counts[index] += 1;
  }
  return counts;
}

/** The time range between two x positions (a drag), ordered and clamped to
 * the domain. */
export function brushRange(scale: TimeScale, xa: number, xb: number): { start: number; end: number } {
  const lo = Math.min(xa, xb);
  const hi = Math.max(xa, xb);
  const clamp = (t: number) => Math.min(scale.end, Math.max(scale.start, t));
  return { start: clamp(scale.t(lo)), end: clamp(scale.t(hi)) };
}

/** "17:02-17:09" for a brushed range (the filter chip's words). */
export function formatTimeRange(range: { start: number; end: number }): string {
  const same = new Date(range.start).toDateString() === new Date(range.end).toDateString();
  const from = formatTimeTick(range.start, "clock");
  const to = formatTimeTick(range.end, "clock");
  return same ? `${from}-${to}` : `${formatTimeTick(range.start, "day")} ${from}-${formatTimeTick(range.end, "day")} ${to}`;
}
