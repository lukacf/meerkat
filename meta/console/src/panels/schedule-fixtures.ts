// Demo schedules for the dev-only kit gallery and the Schedules tests (console
// v2 spec B10, B24 item 11): four agents and nine console schedules in the
// gateway's wire form (once, a repeat every 2 hours, a daily calendar, a
// weekdays calendar, two paused), and a transport that answers the console's
// schedule commands from memory, writes included. The seeded preview has no
// schedule host, so the Schedules shots come from this transport.

import type { ConsoleAgent } from "../types";
import { CONSOLE_SCHEDULE_COMMANDS, parseScheduleTrigger, type ConsoleScheduleCommand } from "../lib/schedules";
import { upcomingRuns } from "../lib/schedule-runs";

type Row = Record<string, unknown>;

function agent(identity: string, label: string, role: string, group: string): ConsoleAgent {
  return {
    identity,
    member_id: identity,
    agent_id: identity,
    label,
    kind: "mob_agent",
    role,
    group,
    state: "active",
    addressable: true,
    affordances: { can_send_message: true, can_respawn: true, can_retire: true },
    progress: { run_state: "idle", in_flight_work: 0, last_progress_at_ms: 0, last_progress_event: "became_idle", health: "healthy" },
  } as ConsoleAgent;
}

/** The demo team the schedules belong to. */
export const DEMO_SCHEDULE_AGENTS: ConsoleAgent[] = [
  agent("coordinator", "Coordinator", "coordinator", "Coordinators"),
  agent("support", "Support", "responder", "Domains"),
  agent("health-monitor", "Health Monitor", "monitor", "Internal"),
  agent("billing", "Billing", "specialist", "Domains"),
];

const values = (...items: number[]) => ({ kind: "values", values: items });
const ANY = { kind: "any" };

function calendar(timezone: string, hour: number, minute: number, weekdays?: number[]): Row {
  return {
    type: "calendar",
    timezone,
    minute: values(minute),
    hour: values(hour),
    day_of_month: ANY,
    month: ANY,
    day_of_week: weekdays ? values(...weekdays) : ANY,
  };
}

/** The local start of today plus `days` days at `hour:minute`, as an ISO instant. */
function localAt(now: Date, days: number, hour: number, minute: number): string {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() + days, hour, minute, 0, 0).toISOString();
}

function nextDue(trigger: Row, phase: string, now: number): string | null {
  if (phase !== "active") return null;
  const parsed = parseScheduleTrigger(trigger);
  const first = upcomingRuns(parsed, { from: now, max: 1, until: now + 400 * 24 * 3_600_000 }).runs[0];
  return first === undefined ? null : new Date(first).toISOString();
}

/** The nine demo schedules in wire form, timed from `now`. */
export function demoScheduleRows(now: Date): Row[] {
  const at = now.getTime();
  const defined = new Date(at - 3 * 24 * 3_600_000).toISOString();
  const rows: Array<Omit<Row, "next_due_at_utc">> = [
    {
      schedule_id: "sch_monday_summary",
      identity: "coordinator",
      name: "Monday summary",
      content: "Summarise last week's escalations and post the summary to the team.",
      trigger: calendar("Europe/Paris", 9, 0, [1]),
      phase: "active",
      sender: "console:shared",
    },
    {
      schedule_id: "sch_morning_brief",
      identity: "coordinator",
      name: "Morning brief",
      content: "Read the overnight queue and brief the team on what needs a decision today.",
      trigger: calendar("Europe/Paris", 8, 30),
      phase: "active",
      sender: "ops@example.com",
    },
    {
      schedule_id: "sch_inbox_sweep",
      identity: "support",
      name: "Inbox sweep",
      content: "Sweep the shared inbox, answer what you can and hand the rest to the right agent.",
      trigger: { type: "interval", start_at_utc: localAt(now, 0, 7, 30), every_seconds: 7_200 },
      phase: "active",
      sender: "coordinator",
    },
    {
      schedule_id: "sch_weekday_triage",
      identity: "support",
      name: "Weekday triage",
      content: "Triage the tickets that came in overnight and flag anything urgent.",
      trigger: calendar("America/New_York", 9, 0, [1, 2, 3, 4, 5]),
      phase: "active",
      sender: "console:shared",
    },
    {
      schedule_id: "sch_weekend_handover",
      identity: "support",
      name: "Weekend handover",
      content: "Write the weekend handover notes for the on-call team.",
      trigger: calendar("Europe/London", 17, 0, [6]),
      phase: "paused",
      sender: "console:shared",
    },
    {
      schedule_id: "sch_heartbeat",
      identity: "health-monitor",
      name: "Heartbeat check",
      content: "Check that every agent answered in the last ten minutes, until the migration window closes.",
      trigger: {
        type: "interval",
        start_at_utc: localAt(now, -1, 0, 0),
        every_seconds: 600,
        end_at_utc: new Date(Math.ceil((at + 10 * 3_600_000) / 600_000) * 600_000).toISOString(),
      },
      phase: "active",
      sender: "console:shared",
    },
    {
      schedule_id: "sch_health_digest",
      identity: "health-monitor",
      name: "Health digest",
      content: "Post the day's health digest: stalls, retries and slow tools.",
      trigger: calendar("UTC", 18, 0),
      phase: "active",
      sender: "console:shared",
    },
    {
      schedule_id: "sch_invoice_run",
      identity: "billing",
      name: "Invoice run",
      content: "Run the invoice batch and report anything that failed to send.",
      trigger: { type: "once", due_at_utc: localAt(now, 1, 14, 0) },
      phase: "active",
      sender: "ops@example.com",
    },
    {
      schedule_id: "sch_payment_reminders",
      identity: "billing",
      name: "Payment reminders",
      content: "Send reminders for invoices more than 14 days overdue.",
      trigger: calendar("Europe/Paris", 10, 30, [1, 2, 3, 4, 5]),
      phase: "paused",
      sender: "console:shared",
    },
  ];
  return rows.map((row) => ({
    revision: 1,
    defined_at_utc: defined,
    created_at_utc: defined,
    updated_at_utc: defined,
    ...row,
    next_due_at_utc: nextDue(row.trigger as Row, String(row.phase), at),
  }));
}

function rpcError(code: number, kind: string): Error {
  return Object.assign(new Error(`schedule request refused: ${kind}`), {
    rpcError: { code, message: kind, data: { kind } },
  });
}

export interface DemoScheduleTransport {
  /** Answers one console schedule command for `identity`, like the gateway. */
  run: (identity: string, command: ConsoleScheduleCommand, params: Record<string, unknown>) => Promise<unknown>;
  /** The rows as they stand. */
  rows: () => Row[];
}

/**
 * A schedule transport over the demo rows: lists per agent, and creates,
 * updates, pauses, resumes and deletes in memory. `refuse` names agents
 * whose list is refused (access denied); `latencyMs` delays each answer.
 */
export function createDemoScheduleTransport(
  now: Date,
  options: { refuse?: readonly string[]; latencyMs?: number } = {},
): DemoScheduleTransport {
  let rows = demoScheduleRows(now);
  let serial = rows.length;
  const refuse = new Set(options.refuse ?? []);
  const wait = () => new Promise<void>((resolve) => setTimeout(resolve, options.latencyMs ?? 0));
  const clock = () => Math.max(now.getTime(), Date.now());
  const run: DemoScheduleTransport["run"] = async (identity, command, params) => {
    await wait();
    if (refuse.has(identity)) throw rpcError(-32030, "access_denied");
    const index = rows.findIndex((row) => row.schedule_id === params.schedule_id && row.identity === identity);
    const found = index >= 0 ? rows[index] : null;
    switch (command) {
      case CONSOLE_SCHEDULE_COMMANDS.list:
        return { identity, schedules: rows.filter((row) => row.identity === identity) };
      case CONSOLE_SCHEDULE_COMMANDS.create: {
        serial += 1;
        const stamp = new Date(clock()).toISOString();
        const trigger = params.trigger as Row;
        const created: Row = {
          schedule_id: `sch_new_${serial}`,
          identity,
          name: typeof params.name === "string" ? params.name : null,
          content: params.content,
          trigger,
          phase: "active",
          revision: 1,
          sender: "console:shared",
          defined_at_utc: stamp,
          created_at_utc: stamp,
          updated_at_utc: stamp,
          next_due_at_utc: nextDue(trigger, "active", clock()),
        };
        rows = [...rows, created];
        return created;
      }
      default:
        break;
    }
    if (!found) throw rpcError(-32001, "schedule_not_found");
    const stamp = new Date(clock()).toISOString();
    let next: Row = found;
    switch (command) {
      case CONSOLE_SCHEDULE_COMMANDS.update: {
        const trigger = (params.trigger as Row | undefined) ?? (found.trigger as Row);
        next = {
          ...found,
          ...(params.content !== undefined ? { content: params.content } : {}),
          ...(params.name !== undefined ? { name: params.name || null } : {}),
          trigger,
          revision: Number(found.revision ?? 1) + 1,
          sender: "console:shared",
          updated_at_utc: stamp,
          next_due_at_utc: nextDue(trigger, String(found.phase), clock()),
        };
        break;
      }
      case CONSOLE_SCHEDULE_COMMANDS.pause:
        next = { ...found, phase: "paused", revision: Number(found.revision ?? 1) + 1, updated_at_utc: stamp, next_due_at_utc: null };
        break;
      case CONSOLE_SCHEDULE_COMMANDS.resume:
        next = {
          ...found,
          phase: "active",
          revision: Number(found.revision ?? 1) + 1,
          sender: "console:shared",
          updated_at_utc: stamp,
          next_due_at_utc: nextDue(found.trigger as Row, "active", clock()),
        };
        break;
      case CONSOLE_SCHEDULE_COMMANDS.delete:
        rows = rows.filter((row) => row !== found);
        return { identity, schedule_id: found.schedule_id, deleted: true };
      default:
        throw rpcError(-32601, "unknown_command");
    }
    rows = rows.map((row) => (row === found ? next : row));
    return next;
  };
  return { run, rows: () => rows };
}
