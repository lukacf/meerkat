// Upcoming runs of console schedules, computed in the browser (console v2
// spec B10): the gateway lists each schedule with its next due time only, and
// the Schedules panel draws every planned run in a window. The rules are the
// schedule service's own (crates/meerkat-schedule/src/trigger.rs):
// - once: the due time, when it is after the cursor;
// - interval: the strictly future multiples of the step from the start, none
//   past the end;
// - calendar: every UTC minute after the cursor whose local time in the
//   schedule's zone matches each field, so a daylight-saving gap never fires
//   and a repeated hour fires twice.
// Pure module (no React), so node test bundles can import it.

import type {
  ConsoleSchedule,
  ConsoleScheduleCalendarField,
  ConsoleScheduleTrigger,
} from "./schedules";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
/** Every current zone offset and transition sits on this UTC grid. */
const OFFSET_BLOCK_MS = 15 * MINUTE;

/** The most runs one schedule plans in a window. */
export const SCHEDULE_RUN_CAP = 64;
/** The default window: a week. */
export const SCHEDULE_RUN_WINDOW_MS = 7 * DAY;

export type ScheduleKind = "once" | "interval" | "calendar" | "unknown";

/** How a schedule fires: once, on a repeat, on a calendar. */
export function scheduleKind(trigger: ConsoleScheduleTrigger | null | undefined): ScheduleKind {
  if (!trigger) return "unknown";
  return trigger.type;
}

/** The kind in words, for legends and tooltips. */
export function scheduleKindWords(kind: ScheduleKind): string {
  switch (kind) {
    case "once":
      return "Once";
    case "interval":
      return "Repeating";
    case "calendar":
      return "On a calendar";
    default:
      return "Unknown timing";
  }
}

export interface UpcomingRuns {
  /** Each planned run (epoch ms), earliest first. */
  runs: number[];
  /** The schedule plans more runs in the window than the cap allowed. */
  truncated: boolean;
  /** The calendar's time zone is not one this browser knows: no runs. */
  invalidZone: boolean;
}

export interface UpcomingRunsOptions {
  /** The cursor: runs strictly after it (epoch ms). */
  from: number;
  /** The end of the window, inclusive (epoch ms); a week after `from` by default. */
  until?: number;
  /** At most this many runs (64 by default). */
  max?: number;
}

const NO_RUNS: UpcomingRuns = { runs: [], truncated: false, invalidZone: false };

function epoch(iso: string | undefined | null): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isFinite(ms) ? ms : null;
}

/**
 * The runs `trigger` plans after `from` up to `until`, at most `max` of
 * them, exactly as the schedule service plans them.
 */
export function upcomingRuns(
  trigger: ConsoleScheduleTrigger | null | undefined,
  options: UpcomingRunsOptions,
): UpcomingRuns {
  if (!trigger) return NO_RUNS;
  const from = options.from;
  const until = options.until ?? from + SCHEDULE_RUN_WINDOW_MS;
  const max = Math.max(0, Math.floor(options.max ?? SCHEDULE_RUN_CAP));
  if (!(until >= from) || max === 0) return NO_RUNS;
  if (trigger.type === "once") {
    const due = epoch(trigger.due_at_utc);
    return due !== null && due > from && due <= until ? { runs: [due], truncated: false, invalidZone: false } : NO_RUNS;
  }
  if (trigger.type === "interval") return intervalRuns(trigger, from, until, max);
  return calendarRuns(trigger, from, until, max);
}

function intervalRuns(
  trigger: Extract<ConsoleScheduleTrigger, { type: "interval" }>,
  from: number,
  until: number,
  max: number,
): UpcomingRuns {
  const start = epoch(trigger.start_at_utc);
  const step = trigger.every_seconds * 1000;
  if (start === null || !(step > 0)) return NO_RUNS;
  const end = epoch(trigger.end_at_utc ?? null);
  const stop = end === null ? until : Math.min(until, end);
  let k = from < start ? 0 : Math.floor((from - start) / step) + 1;
  let candidate = start + k * step;
  while (candidate <= from) {
    k += 1;
    candidate = start + k * step;
  }
  const runs: number[] = [];
  while (candidate <= stop && runs.length < max) {
    runs.push(candidate);
    k += 1;
    candidate = start + k * step;
  }
  return { runs, truncated: candidate <= stop, invalidZone: false };
}

// ── Time zones ───────────────────────────────────────────────────────────

const ZONE_FORMATTERS = new Map<string, Intl.DateTimeFormat | null>();

/** A formatter that reads an instant's wall clock in `timeZone`, or null for
 * a zone this browser does not know. */
function zoneFormatter(timeZone: string): Intl.DateTimeFormat | null {
  const cached = ZONE_FORMATTERS.get(timeZone);
  if (cached !== undefined) return cached;
  let formatter: Intl.DateTimeFormat | null = null;
  try {
    formatter = new Intl.DateTimeFormat("en-US", {
      timeZone,
      hourCycle: "h23",
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "numeric",
      minute: "numeric",
      second: "numeric",
    });
  } catch {
    formatter = null;
  }
  ZONE_FORMATTERS.set(timeZone, formatter);
  return formatter;
}

/** Whether this browser knows the IANA time zone. */
export function isKnownTimeZone(timeZone: string): boolean {
  return Boolean(timeZone && zoneFormatter(timeZone));
}

/** The zone's offset from UTC at an instant, in ms (local = utc + offset). */
function zoneOffsetMs(formatter: Intl.DateTimeFormat, at: number): number {
  const seconds = Math.floor(at / 1000) * 1000;
  let year = 1970;
  let month = 1;
  let day = 1;
  let hour = 0;
  let minute = 0;
  let second = 0;
  for (const part of formatter.formatToParts(new Date(seconds))) {
    switch (part.type) {
      case "year":
        year = Number(part.value);
        break;
      case "month":
        month = Number(part.value);
        break;
      case "day":
        day = Number(part.value);
        break;
      case "hour":
        // An engine that ignores hourCycle writes midnight as 24.
        hour = Number(part.value) % 24;
        break;
      case "minute":
        minute = Number(part.value);
        break;
      case "second":
        second = Number(part.value);
        break;
      default:
        break;
    }
  }
  return Date.UTC(year, month - 1, day, hour, minute, second) - seconds;
}

/** Zone offsets already read, per zone and instant: the planner reads the
 * same quarter hours for every schedule of a zone. */
const ZONE_OFFSETS = new Map<string, Map<number, number>>();
const ZONE_OFFSET_CACHE_LIMIT = 50_000;

/** The zone's offset at an instant, read once per instant across scans. */
function offsetReader(timeZone: string, formatter: Intl.DateTimeFormat): (at: number) => number {
  let known = ZONE_OFFSETS.get(timeZone);
  if (!known) {
    known = new Map<number, number>();
    ZONE_OFFSETS.set(timeZone, known);
  }
  const cache = known;
  return (at: number) => {
    let value = cache.get(at);
    if (value === undefined) {
      if (cache.size >= ZONE_OFFSET_CACHE_LIMIT) cache.clear();
      value = zoneOffsetMs(formatter, at);
      cache.set(at, value);
    }
    return value;
  };
}

/** The wall clock of an instant in a zone. */
export interface ZonedFields {
  year: number;
  /** 1-12. */
  month: number;
  /** 1-31. */
  day: number;
  /** 0-23. */
  hour: number;
  /** 0-59. */
  minute: number;
  /** 0 is Sunday. */
  weekday: number;
}

/** The wall clock of `at` in `timeZone`, or null for an unknown zone. */
export function zonedFields(at: number, timeZone: string): ZonedFields | null {
  const formatter = zoneFormatter(timeZone);
  if (!formatter) return null;
  return fieldsOf(at + zoneOffsetMs(formatter, at));
}

function fieldsOf(local: number): ZonedFields {
  const date = new Date(local);
  return {
    year: date.getUTCFullYear(),
    month: date.getUTCMonth() + 1,
    day: date.getUTCDate(),
    hour: date.getUTCHours(),
    minute: date.getUTCMinutes(),
    weekday: date.getUTCDay(),
  };
}

function fieldMatcher(field: ConsoleScheduleCalendarField | undefined): (value: number) => boolean {
  if (!field || field.kind === "any") return () => true;
  const values = new Set(field.values);
  return (value) => values.has(value);
}

function calendarRuns(
  trigger: Extract<ConsoleScheduleTrigger, { type: "calendar" }>,
  from: number,
  until: number,
  max: number,
): UpcomingRuns {
  const formatter = zoneFormatter(trigger.timezone);
  if (!formatter) return { runs: [], truncated: false, invalidZone: true };
  const minuteOk = fieldMatcher(trigger.minute);
  const hourOk = fieldMatcher(trigger.hour);
  const dayOk = fieldMatcher(trigger.day_of_month);
  const monthOk = fieldMatcher(trigger.month);
  const weekdayOk = fieldMatcher(trigger.day_of_week);
  const yearOk = fieldMatcher(trigger.year);
  const offsetAt = offsetReader(trigger.timezone, formatter);
  const dateOk = (local: Date) =>
    hourOk(local.getUTCHours())
    && dayOk(local.getUTCDate())
    && monthOk(local.getUTCMonth() + 1)
    && weekdayOk(local.getUTCDay())
    && yearOk(local.getUTCFullYear());
  const runs: number[] = [];
  // The next minute boundary strictly after the cursor, then every minute
  // up to the window's end, one 15-minute UTC block at a time.
  let cursor = Math.floor(from / MINUTE) * MINUTE + MINUTE;
  const stop = Math.floor(until / MINUTE) * MINUTE + MINUTE;
  while (cursor < stop) {
    const block = Math.floor(cursor / OFFSET_BLOCK_MS) * OFFSET_BLOCK_MS;
    const blockEnd = Math.min(block + OFFSET_BLOCK_MS, stop);
    const head = offsetAt(block);
    if (head === offsetAt(block + OFFSET_BLOCK_MS) && head % OFFSET_BLOCK_MS === 0) {
      // One offset on the quarter-hour grid: the block's minutes share one
      // local hour and day, so those fields are checked once.
      const local = new Date(cursor + head);
      if (dateOk(local)) {
        const firstMinute = local.getUTCMinutes();
        for (let at = cursor; at < blockEnd; at += MINUTE) {
          if (!minuteOk(firstMinute + (at - cursor) / MINUTE)) continue;
          if (runs.length >= max) return { runs, truncated: true, invalidZone: false };
          runs.push(at);
        }
      }
    } else {
      // A transition (or an off-grid offset) in the block: minute by minute.
      for (let at = cursor; at < blockEnd; at += MINUTE) {
        const local = new Date(at + offsetAt(at));
        if (!dateOk(local) || !minuteOk(local.getUTCMinutes())) continue;
        if (runs.length >= max) return { runs, truncated: true, invalidZone: false };
        runs.push(at);
      }
    }
    cursor = blockEnd;
  }
  return { runs, truncated: false, invalidZone: false };
}

// ── Schedules ────────────────────────────────────────────────────────────

/** One schedule's planned runs in a window. */
export interface ScheduleRunPlan {
  schedule: ConsoleSchedule;
  kind: ScheduleKind;
  /** Planned runs in the window, earliest first (a paused schedule's would-be runs). */
  runs: number[];
  truncated: boolean;
  invalidZone: boolean;
  /** The next run: the gateway's next due time while it is current, else the
   * first planned run; null for a paused schedule or one with no run left. */
  next: number | null;
}

/** The gateway's next due time is current when it is not more than this in the past. */
const NEXT_DUE_GRACE_MS = MINUTE;

/**
 * The planned runs of `schedule` from `now` to `now + windowMs`. Paused
 * schedules plan their would-be runs (drawn hollow) and have no next run.
 */
export function planScheduleRuns(
  schedule: ConsoleSchedule,
  now: number,
  windowMs: number = SCHEDULE_RUN_WINDOW_MS,
  max: number = SCHEDULE_RUN_CAP,
): ScheduleRunPlan {
  const planned = upcomingRuns(schedule.trigger, { from: now, until: now + windowMs, max });
  const active = schedule.phase === "active";
  const due = epoch(schedule.nextDueAt);
  let next: number | null = null;
  if (active) {
    if (due !== null && due >= now - NEXT_DUE_GRACE_MS) next = due;
    else next = planned.runs[0] ?? null;
  }
  return {
    schedule,
    kind: scheduleKind(schedule.trigger),
    runs: planned.runs,
    truncated: planned.truncated,
    invalidZone: planned.invalidZone,
    next,
  };
}

/** The earliest next run among `plans`, with its plan. */
export function nextPlannedRun(plans: readonly ScheduleRunPlan[]): { plan: ScheduleRunPlan; at: number } | null {
  let best: { plan: ScheduleRunPlan; at: number } | null = null;
  for (const plan of plans) {
    if (plan.next === null) continue;
    if (!best || plan.next < best.at) best = { plan, at: plan.next };
  }
  return best;
}

/** How many runs of active schedules fall in [from, until]. */
export function countRunsBetween(plans: readonly ScheduleRunPlan[], from: number, until: number): number {
  let count = 0;
  for (const plan of plans) {
    if (plan.schedule.phase !== "active") continue;
    for (const run of plan.runs) if (run >= from && run <= until) count += 1;
  }
  return count;
}

/** Local midnight (the viewer's clock) at or before `at`. */
export function localDayStart(at: number): number {
  const date = new Date(at);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** One row of the Week grid: a local day and its runs per local hour. */
export interface WeekRow {
  /** Local midnight of the day. */
  start: number;
  /** "Mon 3". */
  label: string;
  /** Runs per hour, 24 values (a daylight-saving day still shows 24 hours;
   * the repeated hour adds to its hour). */
  hours: number[];
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/**
 * The Week grid (spec B10): the next `days` local days from today, each with
 * the count of active schedules' planned runs per local hour.
 */
export function runsPerHour(plans: readonly ScheduleRunPlan[], now: number, days = 7): WeekRow[] {
  const rows: WeekRow[] = [];
  const first = new Date(localDayStart(now));
  for (let index = 0; index < days; index += 1) {
    const day = new Date(first.getFullYear(), first.getMonth(), first.getDate() + index);
    rows.push({ start: day.getTime(), label: `${WEEKDAYS[day.getDay()]} ${day.getDate()}`, hours: new Array<number>(24).fill(0) });
  }
  const end = new Date(first.getFullYear(), first.getMonth(), first.getDate() + days).getTime();
  for (const plan of plans) {
    if (plan.schedule.phase !== "active") continue;
    for (const run of plan.runs) {
      if (run < rows[0].start || run >= end) continue;
      const date = new Date(run);
      const dayStart = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
      const row = rows.find((entry) => entry.start === dayStart);
      if (row) row.hours[date.getHours()] += 1;
    }
  }
  return rows;
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

/**
 * A run's time in words for its mark: "Mon 07:30 Europe/Paris" for a
 * calendar schedule (its own zone), "Mon 07:30" on the viewer's clock for
 * the other kinds.
 */
export function formatRunTime(at: number, schedule: Pick<ConsoleSchedule, "trigger">): string {
  const trigger = schedule.trigger;
  if (trigger?.type === "calendar") {
    const fields = zonedFields(at, trigger.timezone);
    if (fields) return `${WEEKDAYS[fields.weekday]} ${pad(fields.hour)}:${pad(fields.minute)} ${trigger.timezone}`;
  }
  const date = new Date(at);
  return `${WEEKDAYS[date.getDay()]} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** "every 5 min", "every 2 h", "every day": a repeat's step in short words. */
export function shortIntervalWords(everySeconds: number): string {
  if (everySeconds % 86_400 === 0) {
    const days = everySeconds / 86_400;
    return days === 1 ? "every day" : `every ${days} days`;
  }
  if (everySeconds % 3_600 === 0) return `every ${everySeconds / 3_600} h`;
  if (everySeconds % 60 === 0) return `every ${everySeconds / 60} min`;
  return `every ${everySeconds} s`;
}
