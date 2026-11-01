// The schedule run calculator against the schedule service's own rules,
// with fixed IANA zones across daylight-saving changes (console v2 spec
// B10: "unit tests for the run calculator (DST cases)").

import { describe, expect, it } from "vitest";

import type { ConsoleSchedule, ConsoleScheduleTrigger } from "./schedules";
import {
  SCHEDULE_RUN_CAP,
  countRuns,
  countRunsBetween,
  formatRunTime,
  isKnownTimeZone,
  nextPlannedRun,
  planScheduleRuns,
  runsPerHour,
  scheduleKind,
  scheduleKindWords,
  shortIntervalWords,
  upcomingRuns,
  zonedFields,
} from "./schedule-runs";

const at = (iso: string) => Date.parse(iso);
const isos = (runs: number[]) => runs.map((run) => new Date(run).toISOString());
const HOUR = 3_600_000;
const DAY = 24 * HOUR;

function calendar(
  timezone: string,
  hour: number | null,
  minute: number | null,
  extra: Partial<Extract<ConsoleScheduleTrigger, { type: "calendar" }>> = {},
): ConsoleScheduleTrigger {
  return {
    type: "calendar",
    timezone,
    minute: minute === null ? { kind: "any" } : { kind: "values", values: [minute] },
    hour: hour === null ? { kind: "any" } : { kind: "values", values: [hour] },
    day_of_month: { kind: "any" },
    month: { kind: "any" },
    day_of_week: { kind: "any" },
    ...extra,
  };
}

function schedule(trigger: ConsoleScheduleTrigger | null, extra: Partial<ConsoleSchedule> = {}): ConsoleSchedule {
  return {
    scheduleId: "sch_1",
    identity: "alpha",
    name: "Digest",
    content: "Send the digest",
    trigger,
    phase: "active",
    revision: 1,
    sender: null,
    senderHidden: false,
    definedAt: null,
    createdAt: null,
    updatedAt: null,
    nextDueAt: null,
    ...extra,
  };
}

describe("calendar runs across daylight saving (Europe/Stockholm)", () => {
  it("never fires in the spring gap: 02:30 does not exist on 2026-03-29", () => {
    // The schedule service's own case: a daily 02:30 searched from 23:00 UTC
    // the evening before the change.
    const { runs, truncated, invalidZone } = upcomingRuns(calendar("Europe/Stockholm", 2, 30), {
      from: at("2026-03-28T23:00:00Z"),
      until: at("2026-03-31T23:00:00Z"),
    });
    expect(invalidZone).toBe(false);
    expect(truncated).toBe(false);
    expect(isos(runs)).toEqual(["2026-03-30T00:30:00.000Z", "2026-03-31T00:30:00.000Z"]);
    expect(isos(runs).some((run) => run.startsWith("2026-03-29"))).toBe(false);
  });

  it("fires twice in the autumn repeat: 02:30 happens at 00:30 and 01:30 UTC on 2026-10-25", () => {
    const { runs } = upcomingRuns(calendar("Europe/Stockholm", 2, 30), {
      from: at("2026-10-24T12:00:00Z"),
      until: at("2026-10-26T12:00:00Z"),
    });
    expect(isos(runs)).toEqual([
      "2026-10-25T00:30:00.000Z",
      "2026-10-25T01:30:00.000Z",
      "2026-10-26T01:30:00.000Z",
    ]);
    expect(runs[1] - runs[0]).toBe(HOUR);
    for (const run of runs) expect(zonedFields(run, "Europe/Stockholm")).toMatchObject({ hour: 2, minute: 30 });
  });
});

describe("calendar runs across daylight saving (America/New_York)", () => {
  it("skips the spring gap on 2026-03-08 and keeps 02:30 local on either side", () => {
    const { runs } = upcomingRuns(calendar("America/New_York", 2, 30), {
      from: at("2026-03-07T00:00:00Z"),
      until: at("2026-03-10T00:00:00Z"),
    });
    expect(isos(runs)).toEqual(["2026-03-07T07:30:00.000Z", "2026-03-09T06:30:00.000Z"]);
  });

  it("fires 01:30 twice on 2026-11-01, once at each offset", () => {
    const { runs } = upcomingRuns(calendar("America/New_York", 1, 30), {
      from: at("2026-10-31T12:00:00Z"),
      until: at("2026-11-02T12:00:00Z"),
    });
    expect(isos(runs)).toEqual([
      "2026-11-01T05:30:00.000Z",
      "2026-11-01T06:30:00.000Z",
      "2026-11-02T06:30:00.000Z",
    ]);
  });
});

describe("calendar runs", () => {
  it("keeps a weekdays schedule at its local time across the change (Europe/Paris)", () => {
    const { runs } = upcomingRuns(
      calendar("Europe/Paris", 9, 0, { day_of_week: { kind: "values", values: [1, 2, 3, 4, 5] } }),
      { from: at("2026-03-26T00:00:00Z"), until: at("2026-04-01T00:00:00Z") },
    );
    // Thu and Fri at UTC+1, then Mon and Tue at UTC+2: always 09:00 in Paris.
    expect(isos(runs)).toEqual([
      "2026-03-26T08:00:00.000Z",
      "2026-03-27T08:00:00.000Z",
      "2026-03-30T07:00:00.000Z",
      "2026-03-31T07:00:00.000Z",
    ]);
    for (const run of runs) {
      const local = zonedFields(run, "Europe/Paris");
      expect(local?.hour).toBe(9);
      expect(local?.minute).toBe(0);
      expect(local && local.weekday >= 1 && local.weekday <= 5).toBe(true);
    }
  });

  it("reads a zone with a quarter-hour offset (Asia/Kathmandu, UTC+05:45)", () => {
    const { runs } = upcomingRuns(calendar("Asia/Kathmandu", 9, 0), {
      from: at("2026-06-01T00:00:00Z"),
      until: at("2026-06-03T00:00:00Z"),
    });
    expect(isos(runs)).toEqual(["2026-06-01T03:15:00.000Z", "2026-06-02T03:15:00.000Z"]);
    expect(zonedFields(at("2026-06-01T03:15:00Z"), "Asia/Kathmandu")).toMatchObject({ hour: 9, minute: 0, day: 1, month: 6 });
  });

  it("starts at the minute after the cursor, as the service does", () => {
    const everyMinute = calendar("UTC", null, null);
    expect(isos(upcomingRuns(everyMinute, { from: at("2026-01-01T10:00:00Z"), max: 2 }).runs)).toEqual([
      "2026-01-01T10:01:00.000Z",
      "2026-01-01T10:02:00.000Z",
    ]);
    expect(isos(upcomingRuns(everyMinute, { from: at("2026-01-01T10:00:30Z"), max: 1 }).runs)).toEqual([
      "2026-01-01T10:01:00.000Z",
    ]);
  });

  it("ends at the last whole minute of the window", () => {
    const everyMinute = calendar("UTC", null, null);
    expect(isos(upcomingRuns(everyMinute, { from: at("2026-01-01T10:00:00Z"), until: at("2026-01-01T10:02:30Z") }).runs)).toEqual([
      "2026-01-01T10:01:00.000Z",
      "2026-01-01T10:02:00.000Z",
    ]);
  });

  it("plans a large team's week of calendars quickly", () => {
    const zones = ["Europe/Paris", "America/New_York", "Asia/Kathmandu", "UTC", "Australia/Lord_Howe"];
    const started = performance.now();
    let total = 0;
    for (let index = 0; index < 600; index += 1) {
      const trigger = calendar(zones[index % zones.length], index % 24, (index * 7) % 60);
      total += upcomingRuns(trigger, { from: at("2026-03-25T00:00:00Z") }).runs.length;
    }
    expect(total).toBe(600 * 7);
    expect(performance.now() - started).toBeLessThan(1500);
  });

  it("matches day of month, month and year fields", () => {
    const monthEnd = calendar("UTC", 12, 0, { day_of_month: { kind: "values", values: [31] } });
    const runs = upcomingRuns(monthEnd, { from: at("2026-04-01T00:00:00Z"), until: at("2026-08-01T00:00:00Z") }).runs;
    expect(isos(runs)).toEqual(["2026-05-31T12:00:00.000Z", "2026-07-31T12:00:00.000Z"]);
    const nextYearOnly = calendar("UTC", 0, 0, { year: { kind: "values", values: [2027] } });
    expect(upcomingRuns(nextYearOnly, { from: at("2026-12-30T00:00:00Z"), until: at("2027-01-02T00:00:00Z") }).runs.map((run) => new Date(run).getUTCFullYear()))
      .toEqual([2027, 2027]);
  });

  it("plans at most the cap and says so", () => {
    const { runs, truncated } = upcomingRuns(calendar("UTC", null, null), { from: at("2026-01-01T00:00:00Z") });
    expect(runs.length).toBe(SCHEDULE_RUN_CAP);
    expect(truncated).toBe(true);
  });

  it("plans nothing for a zone this browser does not know", () => {
    const result = upcomingRuns(calendar("Mars/Olympus_Mons", 9, 0), { from: at("2026-01-01T00:00:00Z") });
    expect(result).toEqual({ runs: [], truncated: false, invalidZone: true });
    expect(isKnownTimeZone("Mars/Olympus_Mons")).toBe(false);
    expect(isKnownTimeZone("Europe/Paris")).toBe(true);
  });

  it("scans a week of minutes quickly", () => {
    // February 30 never comes: the whole window is scanned.
    const never = calendar("Europe/Paris", 9, 0, { day_of_month: { kind: "values", values: [30] }, month: { kind: "values", values: [2] } });
    const started = performance.now();
    const result = upcomingRuns(never, { from: at("2026-02-01T00:00:00Z") });
    expect(result.runs).toEqual([]);
    expect(performance.now() - started).toBeLessThan(500);
  });
});

describe("once and interval runs", () => {
  it("plans a once schedule only while it is ahead", () => {
    const once: ConsoleScheduleTrigger = { type: "once", due_at_utc: "2026-05-01T09:00:00Z" };
    expect(isos(upcomingRuns(once, { from: at("2026-04-30T09:00:00Z") }).runs)).toEqual(["2026-05-01T09:00:00.000Z"]);
    // Strictly after the cursor: a due time at the cursor is past.
    expect(upcomingRuns(once, { from: at("2026-05-01T09:00:00Z") }).runs).toEqual([]);
    expect(upcomingRuns(once, { from: at("2026-05-02T09:00:00Z") }).runs).toEqual([]);
    // Beyond the window.
    expect(upcomingRuns(once, { from: at("2026-04-01T09:00:00Z"), until: at("2026-04-02T09:00:00Z") }).runs).toEqual([]);
  });

  it("plans the strictly future steps of an interval from its start, none past its end", () => {
    const every2h: ConsoleScheduleTrigger = { type: "interval", start_at_utc: "2026-05-01T09:00:00Z", every_seconds: 7_200 };
    // Before the start: the start itself.
    expect(isos(upcomingRuns(every2h, { from: at("2026-05-01T08:00:00Z"), max: 2 }).runs)).toEqual([
      "2026-05-01T09:00:00.000Z",
      "2026-05-01T11:00:00.000Z",
    ]);
    // On a step: the next one.
    expect(isos(upcomingRuns(every2h, { from: at("2026-05-01T11:00:00Z"), max: 1 }).runs)).toEqual(["2026-05-01T13:00:00.000Z"]);
    // Between steps.
    expect(isos(upcomingRuns(every2h, { from: at("2026-05-01T11:59:59.500Z"), max: 1 }).runs)).toEqual(["2026-05-01T13:00:00.000Z"]);
    const bounded: ConsoleScheduleTrigger = { ...every2h, end_at_utc: "2026-05-01T15:00:00Z" };
    expect(isos(upcomingRuns(bounded, { from: at("2026-05-01T10:00:00Z") }).runs)).toEqual([
      "2026-05-01T11:00:00.000Z",
      "2026-05-01T13:00:00.000Z",
      "2026-05-01T15:00:00.000Z",
    ]);
  });

  it("caps a fast interval and keeps it exact", () => {
    const everyMinute: ConsoleScheduleTrigger = { type: "interval", start_at_utc: "2026-05-01T00:00:00Z", every_seconds: 60 };
    const { runs, truncated } = upcomingRuns(everyMinute, { from: at("2026-05-01T00:00:00Z") });
    expect(runs.length).toBe(SCHEDULE_RUN_CAP);
    expect(truncated).toBe(true);
    expect(runs[1] - runs[0]).toBe(60_000);
  });
});

describe("schedule plans", () => {
  const now = at("2026-05-04T08:00:00Z");

  it("takes the gateway's next due time while it is current, else the first planned run", () => {
    const daily = calendar("UTC", 9, 0);
    expect(planScheduleRuns(schedule(daily, { nextDueAt: "2026-05-04T09:00:00Z" }), now).next).toBe(at("2026-05-04T09:00:00Z"));
    // A stale next due time (an hour ago) gives way to the plan.
    expect(planScheduleRuns(schedule(daily, { nextDueAt: "2026-05-04T07:00:00Z" }), now).next).toBe(at("2026-05-04T09:00:00Z"));
    expect(planScheduleRuns(schedule(daily), now).runs.length).toBe(7);
  });

  it("plans a paused schedule's would-be runs without a next run", () => {
    const plan = planScheduleRuns(schedule(calendar("UTC", 9, 0), { phase: "paused" }), now);
    expect(plan.next).toBe(null);
    expect(plan.runs.length).toBe(7);
  });

  it("finds the earliest next run and counts active runs in a window", () => {
    const plans = [
      planScheduleRuns(schedule(calendar("UTC", 9, 0), { scheduleId: "a" }), now),
      planScheduleRuns(schedule({ type: "once", due_at_utc: "2026-05-04T08:30:00Z" }, { scheduleId: "b" }), now),
      planScheduleRuns(schedule(calendar("UTC", 8, 45), { scheduleId: "c", phase: "paused" }), now),
    ];
    expect(nextPlannedRun(plans)?.plan.schedule.scheduleId).toBe("b");
    expect(nextPlannedRun(plans)?.at).toBe(at("2026-05-04T08:30:00Z"));
    // a's 09:00 and b's 08:30; the paused c does not count.
    expect(countRunsBetween(plans, now, now + DAY)).toBe(2);
  });

  it("counts runs per local hour over the next seven days, exactly", () => {
    const today = new Date(2026, 4, 4, 8, 0, 0);
    const nowLocal = today.getTime();
    const local = (dayOffset: number, hour: number, minute = 0) =>
      new Date(today.getFullYear(), today.getMonth(), today.getDate() + dayOffset, hour, minute).toISOString();
    const viewerZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    const plans = [
      schedule({ type: "once", due_at_utc: local(0, 9, 30) }, { scheduleId: "a" }),
      schedule({ type: "once", due_at_utc: local(1, 14) }, { scheduleId: "b" }),
      // Every 30 minutes from 10:00 to 12:00 today: 10:00, 10:30, 11:00, 11:30, 12:00.
      schedule({ type: "interval", start_at_utc: local(0, 10), every_seconds: 1_800, end_at_utc: local(0, 12) }, { scheduleId: "c" }),
      // 23:59 every day on the viewer's clock.
      schedule(calendar(viewerZone, 23, 59), { scheduleId: "d" }),
      // Paused, and beyond the grid: not counted.
      schedule({ type: "once", due_at_utc: local(2, 9) }, { scheduleId: "e", phase: "paused" }),
      schedule({ type: "once", due_at_utc: local(7, 1) }, { scheduleId: "f" }),
    ].map((entry) => planScheduleRuns(entry, nowLocal));
    const rows = runsPerHour(plans, nowLocal);
    expect(rows.length).toBe(7);
    expect(rows.every((row) => row.hours.length === 24)).toBe(true);
    expect([rows[0].hours[9], rows[0].hours[10], rows[0].hours[11], rows[0].hours[12], rows[0].hours[23]]).toEqual([1, 2, 2, 1, 1]);
    expect(rows[1].hours[14]).toBe(1);
    expect(rows.slice(1).every((row) => row.hours[23] === 1)).toBe(true);
    expect(rows.reduce((sum, row) => sum + row.hours.reduce((a, b) => a + b, 0), 0)).toBe(14);
  });

  it("counts a dense repeat beyond the drawing cap", () => {
    const heartbeat = schedule({ type: "interval", start_at_utc: "2026-05-04T00:00:00Z", every_seconds: 600 });
    const plan = planScheduleRuns(heartbeat, now);
    expect(plan.runs.length).toBe(SCHEDULE_RUN_CAP);
    expect(plan.truncated).toBe(true);
    // 08:10 to 08:00 the next day: 144 runs.
    expect(countRunsBetween([plan], now, now + DAY)).toBe(144);
    expect(countRuns(heartbeat.trigger, now, now + DAY)).toBe(144);
    expect(countRuns({ type: "once", due_at_utc: "2026-05-04T09:00:00Z" }, now, now + DAY)).toBe(1);
    expect(countRuns(calendar("UTC", null, 0), now, now + DAY)).toBe(24);
  });
});

describe("words", () => {
  it("names a calendar run in its own zone", () => {
    const paris = schedule(calendar("Europe/Paris", 7, 30));
    // Monday 2026-01-05, 06:30 UTC is 07:30 in Paris.
    expect(formatRunTime(at("2026-01-05T06:30:00Z"), paris)).toBe("Mon 07:30 Europe/Paris");
  });

  it("names kinds and short intervals", () => {
    expect(scheduleKind({ type: "once", due_at_utc: "2026-01-01T00:00:00Z" })).toBe("once");
    expect(scheduleKind(null)).toBe("unknown");
    expect(scheduleKindWords("interval")).toBe("Repeating");
    expect(shortIntervalWords(300)).toBe("every 5 min");
    expect(shortIntervalWords(7_200)).toBe("every 2 h");
    expect(shortIntervalWords(86_400)).toBe("every day");
    expect(shortIntervalWords(45)).toBe("every 45 s");
  });
});
