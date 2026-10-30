// Display formatting shared by every panel: relative and absolute times,
// counted nouns and enum words. Pure functions with no imports, so panels
// whose test bundles cannot resolve React can still use them.

export type TimeInput = number | string | Date | null | undefined;

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;

/** Epoch milliseconds for an epoch-milliseconds number, an ISO string or a
 * Date. Anything unreadable (including zero and negative numbers) is null. */
export function toEpochMs(value: TimeInput): number | null {
  if (value == null || value === "") return null;
  if (typeof value === "number") {
    return Number.isFinite(value) && value > 0 ? value : null;
  }
  const ms = value instanceof Date ? value.getTime() : Date.parse(value);
  return Number.isFinite(ms) && ms > 0 ? ms : null;
}

function sameYear(a: number, b: number): boolean {
  return new Date(a).getFullYear() === new Date(b).getFullYear();
}

/** A calendar date for times outside the relative window: "Oct 23", or
 * "Oct 23, 2025" when the year differs from now. */
export function formatShortDate(ms: number, now: number = Date.now()): string {
  const options: Intl.DateTimeFormatOptions = sameYear(ms, now)
    ? { month: "short", day: "numeric" }
    : { month: "short", day: "numeric", year: "numeric" };
  return new Date(ms).toLocaleDateString(undefined, options);
}

/**
 * Compact relative time: "just now", "40s ago", "12m ago", "3h ago",
 * "2d ago", then a date ("Oct 23"). Future times read "in 5m", "in 3h",
 * "in 2d". A clock skew of a few seconds reads as "just now". Unreadable
 * input returns an empty string so callers can omit the field.
 */
export function formatRelativeTime(value: TimeInput, now: number = Date.now()): string {
  const ms = toEpochMs(value);
  if (ms === null) return "";
  const diff = now - ms;
  const distance = Math.abs(diff);
  if (distance < 10 * SECOND) return "just now";
  if (distance >= WEEK) return formatShortDate(ms, now);
  let amount: string;
  if (distance < MINUTE) amount = `${Math.floor(distance / SECOND)}s`;
  else if (distance < HOUR) amount = `${Math.floor(distance / MINUTE)}m`;
  else if (distance < DAY) amount = `${Math.floor(distance / HOUR)}h`;
  else amount = `${Math.floor(distance / DAY)}d`;
  return diff >= 0 ? `${amount} ago` : `in ${amount}`;
}

/** Full local date and time for tooltips: "Oct 30, 2026, 14:52:07". */
export function formatAbsoluteTime(value: TimeInput): string {
  const ms = toEpochMs(value);
  if (ms === null) return "";
  return new Date(ms).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** Local clock time for dense timelines: "14:52". */
export function formatClockTime(value: TimeInput): string {
  const ms = toEpochMs(value);
  if (ms === null) return "";
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** A short duration: "4 min", "45 s", "2 h 5 min". */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return "";
  // Seconds round down, so 59.6 s reads "59 s" and never "60 s".
  if (ms < MINUTE) return `${Math.max(0, Math.floor(ms / SECOND))} s`;
  if (ms < HOUR) return `${Math.round(ms / MINUTE)} min`;
  const hours = Math.floor(ms / HOUR);
  const minutes = Math.round((ms % HOUR) / MINUTE);
  return minutes ? `${hours} h ${minutes} min` : `${hours} h`;
}

/** A counted noun with a real plural: "1 record", "2 records", "3 policies". */
export function plural(count: number, singular: string, pluralForm?: string): string {
  const many = pluralForm ?? `${singular}s`;
  return `${count} ${count === 1 ? singular : many}`;
}

/** Sentence-case words for a raw enum: "decision_recorded" reads "Decision
 * recorded", "in-progress" reads "In progress". Explicit vocabularies in the
 * panels take precedence; this is the fallback for unknown values. */
export function humanizeEnum(value: string | null | undefined): string {
  if (!value) return "";
  const words = value
    .trim()
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_\-.]+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "";
}

/** Join display parts with the app's middle-dot separator, skipping empties. */
export function joinMeta(...parts: Array<string | number | null | undefined | false>): string {
  return parts
    .filter((part): part is string | number => part !== null && part !== undefined && part !== false && part !== "")
    .map(String)
    .join(" · ");
}
