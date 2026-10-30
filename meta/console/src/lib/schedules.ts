import type { ConsoleAgent } from "../types";
import { CONSOLE_RPC_METHODS } from "./contract";

/**
 * Console schedules: deferred sends to one agent, kept by the gateway's
 * schedule service and listed in the agent's details pane
 * (`meta/schedule/list`, `create`, `update`, `pause`, `resume`, `delete`).
 * Each fire is a console send by the caller who last defined the schedule,
 * through the same checks as a send now. These helpers keep the pane's
 * decisions pure: what the gateway offers, what the viewer may change, the
 * wire forms in and out, and the refusal copy keyed by `data.kind`.
 */

/** The console commands of the schedule methods, by what they do. */
export const CONSOLE_SCHEDULE_COMMANDS = {
  list: "listSchedules",
  create: "createSchedule",
  update: "updateSchedule",
  pause: "pauseSchedule",
  resume: "resumeSchedule",
  delete: "deleteSchedule",
} as const;

export type ConsoleScheduleCommand =
  typeof CONSOLE_SCHEDULE_COMMANDS[keyof typeof CONSOLE_SCHEDULE_COMMANDS];

const CONSOLE_SCHEDULE_WRITE_METHODS: readonly string[] = [
  CONSOLE_RPC_METHODS.scheduleCreate,
  CONSOLE_RPC_METHODS.scheduleUpdate,
  CONSOLE_RPC_METHODS.schedulePause,
  CONSOLE_RPC_METHODS.scheduleResume,
  CONSOLE_RPC_METHODS.scheduleDelete,
];

/** What the gateway offers this viewer: the list, and the writes. */
export interface ConsoleScheduleOffer {
  list: boolean;
  write: boolean;
}

export const NO_CONSOLE_SCHEDULES: ConsoleScheduleOffer = { list: false, write: false };

/**
 * What `meta/capabilities` offers: the list where the gateway keeps console
 * schedules, the writes only where it advertises every one of them (a
 * writable console with a schedule host that sends them).
 */
export function consoleScheduleOffer(methods: readonly unknown[] | null | undefined): ConsoleScheduleOffer {
  const offered = new Set((Array.isArray(methods) ? methods : []).filter(
    (method): method is string => typeof method === "string",
  ));
  const list = offered.has(CONSOLE_RPC_METHODS.scheduleList);
  return {
    list,
    write: list && CONSOLE_SCHEDULE_WRITE_METHODS.every((method) => offered.has(method)),
  };
}

/** Whether the viewer may change the agent's schedules, and why not. */
export interface ConsoleScheduleEditing {
  canChange: boolean;
  note: string | null;
}

/**
 * Whether the pane offers schedule changes for `agent`: the console is
 * writable, the gateway offers the writes, and the agent takes the viewer's
 * sends. Under enforced access `can_send_message` is the viewer's
 * `agent.send` on the agent; without it, whether the agent is addressable.
 * Either way a schedule could not be sent, so the list stays read-only. The
 * gateway still judges every write.
 */
export function consoleScheduleEditing(input: {
  offer: ConsoleScheduleOffer;
  readOnly: boolean;
  accessEnforcing: boolean;
  agent: ConsoleAgent | null | undefined;
}): ConsoleScheduleEditing {
  if (input.readOnly) {
    return { canChange: false, note: "This console is read-only, so schedules can be viewed but not changed." };
  }
  if (!input.offer.write) {
    return { canChange: false, note: "Schedules can be viewed but not changed on this console." };
  }
  if (input.agent?.affordances?.can_send_message === false) {
    return {
      canChange: false,
      note: input.accessEnforcing
        ? "You may view this agent but not message it, so you cannot change its schedules."
        : "This agent takes no console messages, so it cannot be scheduled.",
    };
  }
  return { canChange: true, note: null };
}

/** One calendar field: any value, or exactly these. */
export type ConsoleScheduleCalendarField =
  | { kind: "any" }
  | { kind: "values"; values: number[] };

/** When a schedule fires, in the scheduler's wire form. */
export type ConsoleScheduleTrigger =
  | { type: "once"; due_at_utc: string }
  | { type: "interval"; start_at_utc: string; every_seconds: number; end_at_utc?: string }
  | {
    type: "calendar";
    timezone: string;
    minute: ConsoleScheduleCalendarField;
    hour: ConsoleScheduleCalendarField;
    day_of_month: ConsoleScheduleCalendarField;
    month: ConsoleScheduleCalendarField;
    day_of_week: ConsoleScheduleCalendarField;
    year?: ConsoleScheduleCalendarField;
  };

/** One console schedule as the pane shows it. */
export interface ConsoleSchedule {
  scheduleId: string;
  identity: string;
  name: string | null;
  content: string;
  /** Null for a trigger form this console does not know. */
  trigger: ConsoleScheduleTrigger | null;
  /** `active` or `paused`; any other phase is shown as it is. */
  phase: string;
  revision: number | null;
  /** The principal the schedule sends as; null for an anonymous console, or when withheld. */
  sender: string | null;
  /** Whether the gateway withheld the sender: the viewer may not message the agent. */
  senderHidden: boolean;
  definedAt: string | null;
  createdAt: string | null;
  updatedAt: string | null;
  nextDueAt: string | null;
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function nonEmptyString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function wholeNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function parseCalendarField(value: unknown): ConsoleScheduleCalendarField | null {
  if (value === undefined || value === null) return { kind: "any" };
  const field = record(value);
  if (field?.kind === "any") return { kind: "any" };
  if (field?.kind === "values" && Array.isArray(field.values)) {
    const values = field.values.map(wholeNumber);
    if (values.some((entry) => entry === null)) return null;
    return { kind: "values", values: values as number[] };
  }
  return null;
}

/** The trigger in `value`, else null (an unknown or malformed form). */
export function parseScheduleTrigger(value: unknown): ConsoleScheduleTrigger | null {
  const trigger = record(value);
  if (trigger?.type === "once") {
    const due = nonEmptyString(trigger.due_at_utc);
    return due ? { type: "once", due_at_utc: due } : null;
  }
  if (trigger?.type === "interval") {
    const start = nonEmptyString(trigger.start_at_utc);
    const every = wholeNumber(trigger.every_seconds);
    if (!start || !every) return null;
    const end = nonEmptyString(trigger.end_at_utc);
    return { type: "interval", start_at_utc: start, every_seconds: every, ...(end ? { end_at_utc: end } : {}) };
  }
  if (trigger?.type === "calendar") {
    const timezone = nonEmptyString(trigger.timezone);
    const minute = parseCalendarField(trigger.minute);
    const hour = parseCalendarField(trigger.hour);
    const dayOfMonth = parseCalendarField(trigger.day_of_month);
    const month = parseCalendarField(trigger.month);
    const dayOfWeek = parseCalendarField(trigger.day_of_week);
    const year = trigger.year === undefined || trigger.year === null ? undefined : parseCalendarField(trigger.year);
    if (!timezone || !minute || !hour || !dayOfMonth || !month || !dayOfWeek || year === null) return null;
    return {
      type: "calendar",
      timezone,
      minute,
      hour,
      day_of_month: dayOfMonth,
      month,
      day_of_week: dayOfWeek,
      ...(year ? { year } : {}),
    };
  }
  return null;
}

/** One schedule item of a `meta/schedule/*` answer; null when it is not one. */
export function parseConsoleSchedule(value: unknown): ConsoleSchedule | null {
  const item = record(value);
  const scheduleId = nonEmptyString(item?.schedule_id);
  const identity = nonEmptyString(item?.identity);
  if (!item || !scheduleId || !identity || typeof item.content !== "string") return null;
  return {
    scheduleId,
    identity,
    name: nonEmptyString(item.name),
    content: item.content,
    trigger: parseScheduleTrigger(item.trigger),
    phase: nonEmptyString(item.phase) ?? "unknown",
    revision: wholeNumber(item.revision),
    sender: item.sender_hidden === true ? null : nonEmptyString(item.sender),
    senderHidden: item.sender_hidden === true,
    definedAt: nonEmptyString(item.defined_at_utc),
    createdAt: nonEmptyString(item.created_at_utc),
    updatedAt: nonEmptyString(item.updated_at_utc),
    nextDueAt: nonEmptyString(item.next_due_at_utc),
  };
}

/**
 * The schedules of a `meta/schedule/list` answer. An answer without a
 * `schedules` array lists none, and an item that is not a schedule is left
 * out: the pane never shows a schedule it cannot read.
 */
export function parseConsoleScheduleList(value: unknown): ConsoleSchedule[] {
  const schedules = record(value)?.schedules;
  if (!Array.isArray(schedules)) return [];
  return schedules
    .map(parseConsoleSchedule)
    .filter((schedule): schedule is ConsoleSchedule => schedule !== null);
}

const WEEKDAY_NAMES = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function plural(count: number, unit: string): string {
  return `${count} ${unit}${count === 1 ? "" : "s"}`;
}

/** An interval in the largest unit that divides it. */
export function describeInterval(seconds: number): string {
  if (seconds % 86_400 === 0) return plural(seconds / 86_400, "day");
  if (seconds % 3_600 === 0) return plural(seconds / 3_600, "hour");
  if (seconds % 60 === 0) return plural(seconds / 60, "minute");
  return plural(seconds, "second");
}

function twoDigits(value: number): string {
  return String(value).padStart(2, "0");
}

/** The time of day and weekdays of a daily calendar trigger, else null. */
export function dailyCalendarParts(
  trigger: ConsoleScheduleTrigger | null,
): { hour: number; minute: number; weekdays: number[]; timezone: string } | null {
  if (trigger?.type !== "calendar") return null;
  if (
    trigger.minute.kind !== "values" || trigger.minute.values.length !== 1
    || trigger.hour.kind !== "values" || trigger.hour.values.length !== 1
    || trigger.day_of_month.kind !== "any" || trigger.month.kind !== "any" || trigger.year
  ) {
    return null;
  }
  const minute = trigger.minute.values[0];
  const hour = trigger.hour.values[0];
  if (minute > 59 || hour > 23) return null;
  const weekdays = trigger.day_of_week.kind === "any"
    ? []
    : [...new Set(trigger.day_of_week.values)].sort((left, right) => left - right);
  if (weekdays.some((day) => day > 6)) return null;
  return { hour, minute, weekdays: weekdays.length === 7 ? [] : weekdays, timezone: trigger.timezone };
}

function describeWeekdays(weekdays: readonly number[]): string {
  if (weekdays.length === 0) return "Daily";
  if (weekdays.join(",") === "1,2,3,4,5") return "Weekdays";
  if (weekdays.join(",") === "0,6") return "Weekends";
  return weekdays.map((day) => WEEKDAY_NAMES[day]).join(", ");
}

/** A local date and time for a schedule's instants. */
export function formatScheduleTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** When a schedule fires, in words. */
export function describeScheduleTrigger(
  trigger: ConsoleScheduleTrigger | null,
  format: (iso: string) => string = formatScheduleTime,
): string {
  if (!trigger) return "A timing this console cannot show";
  if (trigger.type === "once") return `Once at ${format(trigger.due_at_utc)}`;
  if (trigger.type === "interval") {
    const until = trigger.end_at_utc ? ` until ${format(trigger.end_at_utc)}` : "";
    return `Every ${describeInterval(trigger.every_seconds)} from ${format(trigger.start_at_utc)}${until}`;
  }
  const daily = dailyCalendarParts(trigger);
  if (daily) {
    return `${describeWeekdays(daily.weekdays)} at ${twoDigits(daily.hour)}:${twoDigits(daily.minute)} (${daily.timezone})`;
  }
  return `On a calendar (${trigger.timezone})`;
}

/** The phase of a schedule, in words. */
export function describeSchedulePhase(phase: string): string {
  if (phase === "active") return "Active";
  if (phase === "paused") return "Paused";
  return phase;
}

/** How the form says when a schedule fires. `keep` leaves an edited schedule's timing as it is. */
export type ConsoleScheduleTimingKind = "keep" | "once" | "interval" | "daily";

export type ConsoleScheduleIntervalUnit = "minutes" | "hours" | "days";

const UNIT_SECONDS: Record<ConsoleScheduleIntervalUnit, number> = {
  minutes: 60,
  hours: 3_600,
  days: 86_400,
};

/** The schedule form's fields, as the viewer typed them. */
export interface ConsoleScheduleDraft {
  name: string;
  content: string;
  timing: ConsoleScheduleTimingKind;
  /** Once: a local `YYYY-MM-DDTHH:MM`. */
  at: string;
  /** Repeating: how many units apart. */
  every: string;
  unit: ConsoleScheduleIntervalUnit;
  /** Repeating: the first send, a local `YYYY-MM-DDTHH:MM`; blank sends first one interval from now. */
  start: string;
  /** Daily: a `HH:MM` time of day. */
  time: string;
  /** Daily: the weekdays (0 is Sunday); none is every day. */
  weekdays: number[];
  timezone: string;
}

/** A local `YYYY-MM-DDTHH:MM`, the value of a `datetime-local` input. */
export function localDateTimeInput(date: Date): string {
  return `${date.getFullYear()}-${twoDigits(date.getMonth() + 1)}-${twoDigits(date.getDate())}`
    + `T${twoDigits(date.getHours())}:${twoDigits(date.getMinutes())}`;
}

/** The viewer's time zone, else UTC. */
export function viewerTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

/** A fresh form: once, an hour from `now`. */
export function emptyScheduleDraft(now: Date, timezone: string): ConsoleScheduleDraft {
  const inAnHour = new Date(now.getTime() + 3_600_000);
  inAnHour.setSeconds(0, 0);
  return {
    name: "",
    content: "",
    timing: "once",
    at: localDateTimeInput(inAnHour),
    every: "1",
    unit: "hours",
    start: "",
    time: `${twoDigits(inAnHour.getHours())}:00`,
    weekdays: [],
    timezone,
  };
}

/**
 * The form for editing `schedule`: its name and message, its timing kept,
 * and the timing fields filled from it where the form can show it.
 */
export function scheduleDraftFromSchedule(
  schedule: ConsoleSchedule,
  now: Date,
  timezone: string,
): ConsoleScheduleDraft {
  const draft = emptyScheduleDraft(now, timezone);
  draft.name = schedule.name ?? "";
  draft.content = schedule.content;
  draft.timing = "keep";
  const trigger = schedule.trigger;
  if (trigger?.type === "once") {
    const due = new Date(trigger.due_at_utc);
    if (!Number.isNaN(due.getTime())) draft.at = localDateTimeInput(due);
  } else if (trigger?.type === "interval") {
    const unit = (["days", "hours", "minutes"] as const).find(
      (candidate) => trigger.every_seconds % UNIT_SECONDS[candidate] === 0,
    );
    if (unit) {
      draft.unit = unit;
      draft.every = String(trigger.every_seconds / UNIT_SECONDS[unit]);
    }
    const start = new Date(trigger.start_at_utc);
    if (!Number.isNaN(start.getTime())) draft.start = localDateTimeInput(start);
  } else {
    const daily = dailyCalendarParts(trigger);
    if (daily) {
      draft.time = `${twoDigits(daily.hour)}:${twoDigits(daily.minute)}`;
      draft.weekdays = daily.weekdays;
      draft.timezone = daily.timezone;
    }
  }
  return draft;
}

function parseLocalDateTime(value: string): Date | null {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/.test(value.trim())) return null;
  const date = new Date(value.trim());
  return Number.isNaN(date.getTime()) ? null : date;
}

/** The trigger the form describes, or what is wrong with it. */
export function scheduleDraftTrigger(
  draft: ConsoleScheduleDraft,
  now: Date,
): { trigger: ConsoleScheduleTrigger } | { error: string } {
  if (draft.timing === "once") {
    const at = parseLocalDateTime(draft.at);
    if (!at) return { error: "Pick the date and time to send at." };
    if (at.getTime() <= now.getTime()) return { error: "Pick a time in the future." };
    return { trigger: { type: "once", due_at_utc: at.toISOString() } };
  }
  if (draft.timing === "interval") {
    const every = Number(draft.every.trim());
    if (!Number.isSafeInteger(every) || every < 1) return { error: "Repeat at least every 1 " + draft.unit.replace(/s$/, "") + "." };
    const seconds = every * UNIT_SECONDS[draft.unit];
    let start: Date;
    if (draft.start.trim()) {
      const parsed = parseLocalDateTime(draft.start);
      if (!parsed) return { error: "Pick the date and time of the first send." };
      start = parsed;
    } else {
      start = new Date(now.getTime() + seconds * 1_000);
      start.setMilliseconds(0);
    }
    return { trigger: { type: "interval", start_at_utc: start.toISOString(), every_seconds: seconds } };
  }
  if (draft.timing === "daily") {
    const time = /^(\d{1,2}):(\d{2})$/.exec(draft.time.trim());
    const hour = time ? Number(time[1]) : Number.NaN;
    const minute = time ? Number(time[2]) : Number.NaN;
    if (!time || hour > 23 || minute > 59) return { error: "Pick the time of day to send at." };
    const timezone = draft.timezone.trim();
    if (!timezone) return { error: "Name the time zone, for example UTC or Europe/Paris." };
    const weekdays = [...new Set(draft.weekdays)].filter((day) => day >= 0 && day <= 6).sort((left, right) => left - right);
    return {
      trigger: {
        type: "calendar",
        timezone,
        minute: { kind: "values", values: [minute] },
        hour: { kind: "values", values: [hour] },
        day_of_month: { kind: "any" },
        month: { kind: "any" },
        day_of_week: weekdays.length === 0 || weekdays.length === 7
          ? { kind: "any" }
          : { kind: "values", values: weekdays },
      },
    };
  }
  return { error: "Pick when to send." };
}

/**
 * The `meta/schedule/create` params the form describes (the console adds
 * `identity`), or what is wrong with the form.
 */
export function scheduleCreateParams(
  draft: ConsoleScheduleDraft,
  now: Date,
): { params: Record<string, unknown> } | { error: string } {
  const content = draft.content.trim();
  if (!content) return { error: "Write the message to send." };
  if (draft.timing === "keep") return { error: "Pick when to send." };
  const timing = scheduleDraftTrigger(draft, now);
  if ("error" in timing) return timing;
  const name = draft.name.trim();
  return { params: { content, trigger: timing.trigger, ...(name ? { name } : {}) } };
}

/**
 * The `meta/schedule/update` params for `schedule` the form describes: the
 * message and name, and the timing unless it is kept. An update also makes
 * the viewer the schedule's sender.
 */
export function scheduleUpdateParams(
  schedule: ConsoleSchedule,
  draft: ConsoleScheduleDraft,
  now: Date,
): { params: Record<string, unknown> } | { error: string } {
  const content = draft.content.trim();
  if (!content) return { error: "Write the message to send." };
  const params: Record<string, unknown> = { schedule_id: schedule.scheduleId, content };
  const name = draft.name.trim();
  if (name !== (schedule.name ?? "")) params.name = name;
  if (draft.timing !== "keep") {
    const timing = scheduleDraftTrigger(draft, now);
    if ("error" in timing) return timing;
    params.trigger = timing.trigger;
  }
  return { params };
}

/** The JSON-RPC error a failed console command carries, if any. */
function rpcErrorOf(error: unknown): { code: unknown; message: unknown; kind: string | null } | null {
  const rpc = record((error as { rpcError?: unknown } | null)?.rpcError);
  if (!rpc) return null;
  return { code: rpc.code, message: rpc.message, kind: nonEmptyString(record(rpc.data)?.kind) };
}

/**
 * The viewer-facing message for a failed schedule command, keyed by the
 * refusal's `data.kind`. Server text is shown only for invalid params (the
 * gateway's reason the schedule is not valid).
 */
export function scheduleErrorMessage(error: unknown, command: ConsoleScheduleCommand): string {
  const rpc = rpcErrorOf(error);
  switch (rpc?.kind) {
    case "read_only":
      return "This console is read-only, so schedules cannot be changed.";
    case "access_denied":
      return command === CONSOLE_SCHEDULE_COMMANDS.list
        ? "You may not view this agent's schedules."
        : "You may not message this agent, so you cannot change its schedules.";
    case "candidate_launch":
      return "This console runs a candidate launch, which changes no schedules.";
    case "schedule_not_found":
      return "This schedule no longer exists.";
    case "schedule_revision_conflict":
      return "This schedule changed meanwhile, so nothing was changed. Check it and try again.";
    case "unknown_identity":
      return "The console does not know this agent.";
    case "not_addressable":
      return "This agent takes no console messages, so it cannot be scheduled.";
    case "governed_target":
      return "This agent admits only governed input, which needs you present, so a schedule cannot send to it.";
    case "schedule_host_unavailable":
      return "No schedule host sends this gateway's schedules, so a schedule would never be sent.";
    default:
      break;
  }
  if (rpc?.code === -32602) {
    const detail = nonEmptyString(rpc.message);
    return detail ? `The gateway refused the schedule: ${detail}` : "The gateway refused the schedule as invalid.";
  }
  if (rpc?.code === -32601) return "This console no longer offers schedules.";
  if (rpc) return "The gateway could not complete the schedule request.";
  if (error instanceof Error && error.message) return error.message;
  return "The schedule request failed.";
}

/** Whether a failed command means the listed schedule is gone. */
export function scheduleIsGone(error: unknown): boolean {
  return rpcErrorOf(error)?.kind === "schedule_not_found";
}
