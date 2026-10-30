import assert from "node:assert/strict";
import test from "node:test";

import type { ConsoleAgent } from "../types";
import { CONSOLE_RPC_METHODS } from "./contract";
import {
  CONSOLE_SCHEDULE_COMMANDS,
  NO_CONSOLE_SCHEDULES,
  consoleScheduleEditing,
  consoleScheduleOffer,
  describeInterval,
  describeScheduleTrigger,
  emptyScheduleDraft,
  localDateTimeInput,
  parseConsoleSchedule,
  parseConsoleScheduleList,
  scheduleCreateParams,
  scheduleDraftFromSchedule,
  scheduleErrorMessage,
  scheduleIsGone,
  scheduleUpdateParams,
  type ConsoleSchedule,
  type ConsoleScheduleDraft,
  type ConsoleScheduleTrigger,
} from "./schedules";

const WRITES = [
  CONSOLE_RPC_METHODS.scheduleCreate,
  CONSOLE_RPC_METHODS.scheduleUpdate,
  CONSOLE_RPC_METHODS.schedulePause,
  CONSOLE_RPC_METHODS.scheduleResume,
  CONSOLE_RPC_METHODS.scheduleDelete,
];

function agent(extra: Partial<ConsoleAgent> = {}): ConsoleAgent {
  return {
    agent_id: "alpha",
    member_id: "alpha",
    identity: "alpha",
    label: "Alpha",
    kind: "mob_agent",
    ...extra,
  };
}

function rpcError(code: number, data?: unknown, message = `rpc ${code}`): Error {
  return Object.assign(new Error(`rpc ${code}`), { rpcError: { code, message, data } });
}

function item(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    schedule_id: "sch_1",
    identity: "alpha",
    name: "Standup",
    content: "Post the standup summary",
    trigger: { type: "interval", start_at_utc: "2030-01-01T09:00:00Z", every_seconds: 7200 },
    phase: "active",
    revision: 3,
    sender: "console:shared",
    defined_at_utc: "2030-01-01T08:00:00Z",
    created_at_utc: "2030-01-01T08:00:00Z",
    updated_at_utc: "2030-01-01T08:30:00Z",
    next_due_at_utc: "2030-01-01T11:00:00Z",
    ...extra,
  };
}

function schedule(extra: Record<string, unknown> = {}): ConsoleSchedule {
  const parsed = parseConsoleSchedule(item(extra));
  assert.notEqual(parsed, null);
  return parsed as ConsoleSchedule;
}

function draft(now: Date, extra: Partial<ConsoleScheduleDraft> = {}): ConsoleScheduleDraft {
  return { ...emptyScheduleDraft(now, "UTC"), content: "Check the queue", ...extra };
}

const verbatim = (iso: string) => iso;

test("the gateway offers the list alone, or the list with every write", () => {
  assert.deepEqual(consoleScheduleOffer([]), NO_CONSOLE_SCHEDULES);
  assert.deepEqual(consoleScheduleOffer(undefined), NO_CONSOLE_SCHEDULES);
  assert.deepEqual(consoleScheduleOffer([CONSOLE_RPC_METHODS.scheduleList]), { list: true, write: false });
  assert.deepEqual(
    consoleScheduleOffer([CONSOLE_RPC_METHODS.scheduleList, ...WRITES]),
    { list: true, write: true },
  );
  assert.deepEqual(
    consoleScheduleOffer([CONSOLE_RPC_METHODS.scheduleList, ...WRITES.slice(1)]),
    { list: true, write: false },
  );
  assert.deepEqual(consoleScheduleOffer([...WRITES]), NO_CONSOLE_SCHEDULES);
  assert.deepEqual(consoleScheduleOffer([42, null, CONSOLE_RPC_METHODS.scheduleList]), { list: true, write: false });
});

test("a viewer changes schedules on a writable console that offers the writes, for an agent that takes their sends", () => {
  const offer = { list: true, write: true };
  assert.deepEqual(
    consoleScheduleEditing({ offer, readOnly: false, accessEnforcing: true, agent: agent({ affordances: { can_send_message: true } }) }),
    { canChange: true, note: null },
  );
  assert.equal(consoleScheduleEditing({ offer, readOnly: false, accessEnforcing: false, agent: undefined }).canChange, true);

  const readOnly = consoleScheduleEditing({ offer, readOnly: true, accessEnforcing: false, agent: agent() });
  assert.equal(readOnly.canChange, false);
  assert.match(readOnly.note ?? "", /read-only/);

  const listOnly = consoleScheduleEditing({ offer: { list: true, write: false }, readOnly: false, accessEnforcing: false, agent: agent() });
  assert.equal(listOnly.canChange, false);
  assert.match(listOnly.note ?? "", /viewed but not changed/);

  const noSend = consoleScheduleEditing({
    offer,
    readOnly: false,
    accessEnforcing: true,
    agent: agent({ affordances: { can_send_message: false } }),
  });
  assert.equal(noSend.canChange, false);
  assert.match(noSend.note ?? "", /not message it/);

  const internal = consoleScheduleEditing({
    offer,
    readOnly: false,
    accessEnforcing: false,
    agent: agent({ affordances: { can_send_message: false } }),
  });
  assert.equal(internal.canChange, false);
  assert.match(internal.note ?? "", /takes no console messages/);
});

test("a list answer is read fail-closed: no schedules array lists none, and unreadable items are left out", () => {
  assert.deepEqual(parseConsoleScheduleList({}), []);
  assert.deepEqual(parseConsoleScheduleList(null), []);
  assert.deepEqual(parseConsoleScheduleList({ schedules: "none" }), []);
  const listed = parseConsoleScheduleList({
    identity: "alpha",
    schedules: [
      item(),
      item({ schedule_id: "" }),
      item({ schedule_id: "sch_2", identity: null }),
      item({ schedule_id: "sch_3", content: 7 }),
      "not a schedule",
      item({ schedule_id: "sch_4", name: "", sender: null, next_due_at_utc: null, phase: "paused", trigger: { type: "cron", expression: "* * * * *" } }),
    ],
  });
  assert.deepEqual(listed.map((entry) => entry.scheduleId), ["sch_1", "sch_4"]);
  assert.deepEqual(listed[0], {
    scheduleId: "sch_1",
    identity: "alpha",
    name: "Standup",
    content: "Post the standup summary",
    trigger: { type: "interval", start_at_utc: "2030-01-01T09:00:00Z", every_seconds: 7200 },
    phase: "active",
    revision: 3,
    sender: "console:shared",
    definedAt: "2030-01-01T08:00:00Z",
    createdAt: "2030-01-01T08:00:00Z",
    updatedAt: "2030-01-01T08:30:00Z",
    nextDueAt: "2030-01-01T11:00:00Z",
  });
  assert.equal(listed[1].name, null);
  assert.equal(listed[1].sender, null);
  assert.equal(listed[1].nextDueAt, null);
  assert.equal(listed[1].trigger, null);
  assert.equal(listed[1].phase, "paused");
});

test("a schedule's timing reads in words", () => {
  const calendar = (extra: Partial<Extract<ConsoleScheduleTrigger, { type: "calendar" }>>): ConsoleScheduleTrigger => ({
    type: "calendar",
    timezone: "Europe/Paris",
    minute: { kind: "values", values: [5] },
    hour: { kind: "values", values: [9] },
    day_of_month: { kind: "any" },
    month: { kind: "any" },
    day_of_week: { kind: "any" },
    ...extra,
  });
  assert.equal(
    describeScheduleTrigger({ type: "once", due_at_utc: "2030-01-01T09:00:00Z" }, verbatim),
    "Once at 2030-01-01T09:00:00Z",
  );
  assert.equal(
    describeScheduleTrigger({ type: "interval", start_at_utc: "S", every_seconds: 7200, end_at_utc: "E" }, verbatim),
    "Every 2 hours from S until E",
  );
  assert.equal(describeScheduleTrigger(calendar({}), verbatim), "Daily at 09:05 (Europe/Paris)");
  assert.equal(
    describeScheduleTrigger(calendar({ day_of_week: { kind: "values", values: [5, 1, 2, 3, 4] } }), verbatim),
    "Weekdays at 09:05 (Europe/Paris)",
  );
  assert.equal(
    describeScheduleTrigger(calendar({ day_of_week: { kind: "values", values: [6, 0] } }), verbatim),
    "Weekends at 09:05 (Europe/Paris)",
  );
  assert.equal(
    describeScheduleTrigger(calendar({ day_of_week: { kind: "values", values: [3, 1] } }), verbatim),
    "Mon, Wed at 09:05 (Europe/Paris)",
  );
  assert.equal(
    describeScheduleTrigger(calendar({ minute: { kind: "values", values: [0, 30] } }), verbatim),
    "On a calendar (Europe/Paris)",
  );
  assert.equal(describeScheduleTrigger(null, verbatim), "A timing this console cannot show");
  assert.equal(describeInterval(60), "1 minute");
  assert.equal(describeInterval(90), "90 seconds");
  assert.equal(describeInterval(172_800), "2 days");
});

test("a new schedule's params carry its message, timing and optional name", () => {
  const now = new Date(2030, 0, 2, 8, 0, 0);
  const at = new Date(2030, 0, 2, 9, 30, 0);
  assert.deepEqual(
    scheduleCreateParams(draft(now, { timing: "once", at: localDateTimeInput(at), name: "  " }), now),
    { params: { content: "Check the queue", trigger: { type: "once", due_at_utc: at.toISOString() } } },
  );
  assert.deepEqual(
    scheduleCreateParams(draft(now, { timing: "once", at: localDateTimeInput(at), name: " Queue " }), now),
    { params: { content: "Check the queue", trigger: { type: "once", due_at_utc: at.toISOString() }, name: "Queue" } },
  );
  assert.deepEqual(
    scheduleCreateParams(draft(now, { timing: "once", at: localDateTimeInput(new Date(2030, 0, 2, 7, 0, 0)) }), now),
    { error: "Pick a time in the future." },
  );
  assert.deepEqual(scheduleCreateParams(draft(now, { timing: "once", at: "soon" }), now), {
    error: "Pick the date and time to send at.",
  });
  assert.deepEqual(scheduleCreateParams(draft(now, { content: "  " }), now), { error: "Write the message to send." });

  assert.deepEqual(
    scheduleCreateParams(draft(now, { timing: "interval", every: "15", unit: "minutes", start: "" }), now),
    {
      params: {
        content: "Check the queue",
        trigger: {
          type: "interval",
          start_at_utc: new Date(now.getTime() + 15 * 60_000).toISOString(),
          every_seconds: 900,
        },
      },
    },
  );
  const start = new Date(2030, 0, 3, 6, 0, 0);
  assert.deepEqual(
    scheduleCreateParams(draft(now, { timing: "interval", every: "2", unit: "days", start: localDateTimeInput(start) }), now),
    { params: { content: "Check the queue", trigger: { type: "interval", start_at_utc: start.toISOString(), every_seconds: 172_800 } } },
  );
  assert.deepEqual(scheduleCreateParams(draft(now, { timing: "interval", every: "0", unit: "hours" }), now), {
    error: "Repeat at least every 1 hour.",
  });

  assert.deepEqual(
    scheduleCreateParams(draft(now, { timing: "daily", time: "07:45", weekdays: [5, 1, 1], timezone: "Europe/Paris" }), now),
    {
      params: {
        content: "Check the queue",
        trigger: {
          type: "calendar",
          timezone: "Europe/Paris",
          minute: { kind: "values", values: [45] },
          hour: { kind: "values", values: [7] },
          day_of_month: { kind: "any" },
          month: { kind: "any" },
          day_of_week: { kind: "values", values: [1, 5] },
        },
      },
    },
  );
  const everyDay = scheduleCreateParams(draft(now, { timing: "daily", time: "07:45", weekdays: [0, 1, 2, 3, 4, 5, 6] }), now);
  assert.deepEqual("params" in everyDay ? (everyDay.params.trigger as { day_of_week: unknown }).day_of_week : null, { kind: "any" });
  assert.deepEqual(scheduleCreateParams(draft(now, { timing: "daily", time: "25:00" }), now), {
    error: "Pick the time of day to send at.",
  });
  assert.deepEqual(scheduleCreateParams(draft(now, { timing: "daily", time: "07:45", timezone: " " }), now), {
    error: "Name the time zone, for example UTC or Europe/Paris.",
  });
  assert.deepEqual(scheduleCreateParams(draft(now, { timing: "keep" }), now), { error: "Pick when to send." });
});

test("an edit keeps the timing unless it is changed, and names only a changed name", () => {
  const now = new Date(2030, 0, 2, 8, 0, 0);
  const existing = schedule();
  const edit = scheduleDraftFromSchedule(existing, now, "UTC");
  assert.equal(edit.timing, "keep");
  assert.equal(edit.name, "Standup");
  assert.equal(edit.content, "Post the standup summary");
  assert.equal(edit.every, "2");
  assert.equal(edit.unit, "hours");
  assert.equal(edit.start, localDateTimeInput(new Date("2030-01-01T09:00:00Z")));

  assert.deepEqual(scheduleUpdateParams(existing, { ...edit, content: "Post it now" }, now), {
    params: { schedule_id: "sch_1", content: "Post it now" },
  });
  assert.deepEqual(scheduleUpdateParams(existing, { ...edit, name: "Daily standup" }, now), {
    params: { schedule_id: "sch_1", content: "Post the standup summary", name: "Daily standup" },
  });
  assert.deepEqual(scheduleUpdateParams(existing, { ...edit, name: "" }, now), {
    params: { schedule_id: "sch_1", content: "Post the standup summary", name: "" },
  });
  const at = new Date(2030, 0, 5, 10, 0, 0);
  assert.deepEqual(scheduleUpdateParams(existing, { ...edit, timing: "once", at: localDateTimeInput(at) }, now), {
    params: {
      schedule_id: "sch_1",
      content: "Post the standup summary",
      trigger: { type: "once", due_at_utc: at.toISOString() },
    },
  });
  assert.deepEqual(scheduleUpdateParams(existing, { ...edit, content: "" }, now), { error: "Write the message to send." });

  const daily = scheduleDraftFromSchedule(
    schedule({
      trigger: {
        type: "calendar",
        timezone: "Asia/Tokyo",
        minute: { kind: "values", values: [0] },
        hour: { kind: "values", values: [18] },
        day_of_month: { kind: "any" },
        month: { kind: "any" },
        day_of_week: { kind: "values", values: [1, 3] },
      },
    }),
    now,
    "UTC",
  );
  assert.equal(daily.time, "18:00");
  assert.deepEqual(daily.weekdays, [1, 3]);
  assert.equal(daily.timezone, "Asia/Tokyo");
});

test("a refusal reads by its kind, and server text shows only for invalid params", () => {
  const list = CONSOLE_SCHEDULE_COMMANDS.list;
  const create = CONSOLE_SCHEDULE_COMMANDS.create;
  assert.match(scheduleErrorMessage(rpcError(-32010, { kind: "read_only" }), create), /read-only/);
  assert.match(scheduleErrorMessage(rpcError(-32030, { kind: "access_denied" }), list), /not view this agent's schedules/);
  assert.match(scheduleErrorMessage(rpcError(-32030, { kind: "access_denied" }), create), /not message this agent/);
  assert.match(scheduleErrorMessage(rpcError(-32047, { kind: "candidate_launch" }), create), /candidate launch/);
  assert.equal(scheduleErrorMessage(rpcError(-32001, { kind: "schedule_not_found" }), create), "This schedule no longer exists.");
  assert.match(scheduleErrorMessage(rpcError(-32001, { kind: "unknown_identity" }), create), /does not know this agent/);
  assert.match(scheduleErrorMessage(rpcError(-32002, { kind: "not_addressable" }), create), /takes no console messages/);
  assert.match(scheduleErrorMessage(rpcError(-32004, { kind: "governed_target" }), create), /governed input/);
  assert.match(scheduleErrorMessage(rpcError(-32004, { kind: "schedule_host_unavailable" }), create), /No schedule host/);
  assert.equal(
    scheduleErrorMessage(rpcError(-32602, undefined, "invalid trigger: unknown timezone"), create),
    "The gateway refused the schedule: invalid trigger: unknown timezone",
  );
  assert.equal(scheduleErrorMessage(rpcError(-32601), create), "This console no longer offers schedules.");
  assert.equal(
    scheduleErrorMessage(rpcError(-32000, { error: "internal_error", detail: "store path /private" }, "store path /private"), create),
    "The gateway could not complete the schedule request.",
  );
  assert.equal(scheduleErrorMessage(new Error("network down"), create), "network down");
  assert.equal(scheduleErrorMessage("odd", create), "The schedule request failed.");
  assert.equal(scheduleIsGone(rpcError(-32001, { kind: "schedule_not_found" })), true);
  assert.equal(scheduleIsGone(rpcError(-32001, { kind: "unknown_identity" })), false);
  assert.equal(scheduleIsGone(new Error("x")), false);
});
