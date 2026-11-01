// A demo team for the dev-only kit gallery and the agents panels' tests
// (console v2 spec B8, B9, B11): thirteen agents in four sections, two
// working now, one degraded, one stalled, a side chat and a retired member,
// their Health rows and a fixed activity ledger with a day of hourly
// activity. The seeded preview cannot produce degraded or stalled health (a
// busy member's status read reports health "unknown"), so the Roster and
// Health shots with those states come from here. Generic names only.

import type { IdentityStatusRow } from "@console-core";
import type { ConsoleAgent } from "../types";
import type {
  ActivityLedger,
  LedgerCounts,
  LedgerLiveState,
  LedgerMetric,
  LedgerRunSpan,
  LedgerSeriesWindow,
  LedgerWindow,
} from "../lib/activity-ledger";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

type Progress = NonNullable<ConsoleAgent["progress"]>;

interface DemoMember {
  identity: string;
  label: string;
  role: string;
  group: string;
  state?: string;
  phase?: ConsoleAgent["response_phase"];
  progress: (now: number) => Progress;
  wired?: string[];
  addressable?: boolean;
  sideChatOf?: string;
  /** Activity per hour over the last day, oldest first (a shape, scaled). */
  shape: number[];
  /** Busy now: the open run and tool started this long ago. */
  busyFor?: { runMs: number; toolMs?: number };
}

const idle = (agoMs: number, health = "healthy"): ((now: number) => Progress) => (now) => ({
  run_state: "idle",
  in_flight_work: 0,
  last_progress_at_ms: now - agoMs,
  last_progress_event: "became_idle",
  health,
});

/** A day's shape: quiet nights, a morning and an afternoon peak. */
function dayShape(seed: number, scale: number): number[] {
  let state = seed * 9301 + 49297;
  const random = () => {
    state = (state * 9301 + 49297) % 233280;
    return state / 233280;
  };
  return Array.from({ length: 24 }, (_, hour) => {
    const base = hour < 6 ? 0.05 : hour < 9 ? 0.5 : hour < 12 ? 1 : hour < 14 ? 0.6 : hour < 18 ? 0.9 : 0.3;
    return Math.max(0, Math.round(base * scale * (0.4 + random())));
  });
}

const MEMBERS: DemoMember[] = [
  {
    identity: "dispatch",
    label: "Dispatch",
    role: "coordinator",
    group: "Coordinators",
    phase: "generating",
    progress: (now) => ({ run_state: "run_open", in_flight_work: 1, last_progress_at_ms: now - 20_000, last_progress_event: "execution_advanced", health: "unknown" }),
    wired: ["intake", "research", "billing", "shipping", "returns", "release"],
    shape: dayShape(1, 9),
    busyFor: { runMs: 8_000 },
  },
  {
    identity: "intake",
    label: "Intake",
    role: "coordinator",
    group: "Coordinators",
    progress: idle(2 * MINUTE),
    wired: ["dispatch", "support", "billing"],
    shape: dayShape(2, 7),
  },
  {
    identity: "research",
    label: "Research Analyst",
    role: "analyst",
    group: "Specialists",
    phase: "tool-executing",
    progress: (now) => ({ run_state: "run_open", in_flight_work: 2, last_progress_at_ms: now - 12_000, last_progress_event: "execution_advanced", health: "unknown" }),
    wired: ["dispatch", "knowledge"],
    shape: dayShape(3, 6),
    busyFor: { runMs: 95_000, toolMs: 12_000 },
  },
  {
    identity: "release",
    label: "Release Manager",
    role: "release",
    group: "Specialists",
    progress: (now) => ({ run_state: "idle", in_flight_work: 1, last_progress_at_ms: now - 18 * MINUTE, last_progress_event: "became_idle", health: "degraded" }),
    wired: ["dispatch", "support"],
    shape: dayShape(4, 3),
  },
  {
    identity: "support",
    label: "Support Triage",
    role: "responder",
    group: "Domains",
    progress: (now) => ({ run_state: "run_open", in_flight_work: 1, last_progress_at_ms: now - 47 * MINUTE, last_progress_event: "execution_advanced", health: "wedged" }),
    wired: ["intake", "release", "billing"],
    shape: dayShape(5, 5).map((value, hour) => (hour >= 23 ? 0 : value)),
  },
  {
    identity: "billing",
    label: "Billing",
    role: "specialist",
    group: "Domains",
    progress: idle(4 * MINUTE),
    wired: ["dispatch", "intake", "support", "returns"],
    shape: dayShape(6, 8),
  },
  {
    identity: "shipping",
    label: "Shipping",
    role: "specialist",
    group: "Domains",
    progress: idle(26 * MINUTE),
    wired: ["dispatch", "returns"],
    shape: dayShape(7, 4),
  },
  {
    identity: "returns",
    label: "Returns",
    role: "specialist",
    group: "Domains",
    progress: idle(9 * MINUTE),
    wired: ["dispatch", "billing", "shipping"],
    shape: dayShape(8, 5),
  },
  {
    identity: "knowledge",
    label: "Knowledge Base",
    role: "researcher",
    group: "Specialists",
    progress: idle(55 * MINUTE),
    wired: ["research"],
    shape: dayShape(9, 2),
  },
  {
    identity: "compliance",
    label: "Compliance Scribe",
    role: "scribe",
    group: "Internal",
    addressable: false,
    progress: idle(14 * MINUTE),
    wired: ["dispatch"],
    shape: dayShape(10, 2),
  },
  {
    identity: "health-monitor",
    label: "Health Monitor",
    role: "monitor",
    group: "Internal",
    addressable: false,
    progress: idle(3 * MINUTE),
    shape: dayShape(11, 3),
  },
  {
    identity: "billing-refunds",
    label: "Refund review",
    role: "specialist",
    group: "Domains",
    sideChatOf: "billing",
    progress: idle(31 * MINUTE),
    shape: dayShape(12, 1),
  },
  {
    identity: "weekend-cover",
    label: "Weekend Cover",
    role: "specialist",
    group: "Domains",
    state: "retired",
    progress: idle(3 * 60 * MINUTE, "healthy"),
    shape: dayShape(13, 1).map((value, hour) => (hour < 8 ? value : 0)),
  },
];

/** The demo team as the console's agents. */
export function demoTeamAgents(now: number): ConsoleAgent[] {
  return MEMBERS.map((member) => ({
    identity: member.identity,
    member_id: member.identity,
    agent_id: member.identity,
    session_id: `session-${member.identity}-01JC7Q2M`,
    label: member.label,
    kind: "mob_agent",
    role: member.role,
    group: member.group,
    state: member.state ?? "active",
    addressable: member.addressable !== false,
    addressability: member.addressable === false ? "internal_only" : "addressable",
    response_phase: member.phase === undefined ? null : member.phase,
    progress: member.progress(now),
    wired_to: member.wired ?? [],
    side_chat_of: member.sideChatOf,
    labels: { display_name: member.label, group: member.group, ...(member.sideChatOf ? { side_chat_of: member.sideChatOf } : {}) },
    last_activity_ms: member.progress(now).last_progress_at_ms,
    affordances: {
      addressable: member.addressable !== false,
      can_send_message: member.addressable !== false,
      can_respawn: !member.sideChatOf,
      can_retire: member.addressable !== false,
    },
  } as ConsoleAgent));
}

/** The demo team's Health rows (the experience's `health_overview`). */
export function demoTeamHealthRows(now: number): IdentityStatusRow[] {
  return MEMBERS.map((member) => ({
    identity: member.identity,
    display_name: member.label,
    role: member.role,
    state: member.state ?? "active",
    addressability: member.addressable === false ? "internal_only" : "addressable",
    labels: { display_name: member.label },
    progress: member.progress(now),
  }));
}

const METRIC_SHARE: Readonly<Record<LedgerMetric, number>> = {
  events: 1,
  turnsStarted: 0.34,
  turnsFinished: 0.3,
  toolCalls: 0.5,
  messagesSent: 0.22,
  failures: 0,
};

/**
 * A fixed activity ledger over the demo team: each member's hourly shape
 * (scaled per metric), today's totals, the open runs and tools of the two
 * working members and a few finished and failed runs. Reads only; it never
 * changes, so its version stays 1.
 */
export function createDemoLedger(now: number, aliases: Readonly<Record<string, string>> = {}): ActivityLedger {
  const endBin = Math.ceil(now / HOUR) * HOUR;
  const byKey = new Map(MEMBERS.map((member) => [member.identity, member]));
  // Another fixture's agents can borrow a demo member's day ("coordinator"
  // reads as "dispatch").
  for (const [alias, identity] of Object.entries(aliases)) {
    const member = byKey.get(identity);
    if (member) byKey.set(alias, { ...member, identity: alias });
  }
  const hourly = (member: DemoMember, metric: LedgerMetric): number[] => {
    if (metric === "failures") return member.shape.map((value, hour) => (member.identity === "support" && hour === 21 ? 2 : member.identity === "release" && hour === 15 ? 1 : 0));
    return member.shape.map((value) => Math.round(value * METRIC_SHARE[metric]));
  };
  const seriesOf = (identity: string | null, metric: LedgerMetric): number[] => {
    if (identity !== null) {
      const member = byKey.get(identity);
      return member ? hourly(member, metric) : new Array(24).fill(0);
    }
    const sum = new Array<number>(24).fill(0);
    for (const member of MEMBERS) hourly(member, metric).forEach((value, index) => { sum[index] += value; });
    return sum;
  };
  const series = (identity: string | null, metric: LedgerMetric, window: LedgerSeriesWindow): number[] => {
    const bins = window.bins;
    const values = seriesOf(identity, metric);
    // The demo has hourly bins over the last day; other windows read zeros.
    if (bins === 24 && (window.binMs ?? HOUR) === HOUR) {
      const shift = Math.round(((window.endMs ?? endBin) - endBin) / HOUR);
      return values.map((_, index) => values[index + shift] ?? 0);
    }
    return new Array<number>(bins).fill(0);
  };
  const dayStart = new Date(new Date(now).getFullYear(), new Date(now).getMonth(), new Date(now).getDate()).getTime();
  const hoursToday = Math.max(1, Math.ceil((now - dayStart) / HOUR));
  const totals = (identity: string | null, window?: LedgerWindow): LedgerCounts => {
    const since = window?.sinceMs ?? now - 24 * HOUR;
    const take = Math.min(24, Math.max(1, Math.ceil((now - since) / HOUR)));
    const counts = {} as LedgerCounts;
    for (const metric of Object.keys(METRIC_SHARE) as LedgerMetric[]) {
      const values = seriesOf(identity, metric);
      counts[metric] = values.slice(24 - Math.min(take, hoursToday)).reduce((sum, value) => sum + value, 0);
    }
    return counts;
  };
  const liveState = (identity: string): LedgerLiveState => {
    const member = byKey.get(identity);
    const busy = member?.busyFor;
    return {
      openRun: busy ? { key: `${identity}:run`, identity, startMs: now - busy.runMs, toolCalls: busy.toolMs ? 1 : 0 } : null,
      openTool: busy?.toolMs ? { key: `${identity}:tool`, identity, startMs: now - busy.toolMs } : null,
      callingModel: Boolean(busy && !busy.toolMs),
      lastActivityMs: member ? member.progress(now).last_progress_at_ms : undefined,
    };
  };
  const runSpans = (identity: string | null): LedgerRunSpan[] => {
    const members = identity ? [byKey.get(identity)].filter((member): member is DemoMember => Boolean(member)) : MEMBERS;
    const spans: LedgerRunSpan[] = [];
    for (const member of members) {
      const offsets = [190, 142, 96, 61, 23];
      offsets.forEach((minutesAgo, index) => {
        const startMs = now - minutesAgo * MINUTE;
        const failed = (member.identity === "support" && index === 2) || (member.identity === "release" && index === 3);
        spans.push({
          key: `${member.identity}:run:${index}`,
          identity: member.identity,
          startMs,
          endMs: startMs + (failed ? 12_000 : 4_200 + index * 1_700),
          outcome: failed ? "failed" : "ok",
          toolCalls: index % 3,
        });
      });
      if (member.busyFor) {
        spans.push({ key: `${member.identity}:run:open`, identity: member.identity, startMs: now - member.busyFor.runMs, toolCalls: member.busyFor.toolMs ? 1 : 0 });
      }
    }
    return spans.sort((left, right) => (left.startMs ?? 0) - (right.startMs ?? 0));
  };
  const ledger: ActivityLedger = {
    version: 1,
    identities: () => MEMBERS.map((member) => member.identity),
    series,
    totals,
    liveState,
    hasOpenRun: (identity) => Boolean(byKey.get(identity)?.busyFor),
    runSpans: (identity) => runSpans(identity),
    toolSpans: () => [],
    traffic: () => [],
    ingest: () => false,
    ingestMany: () => false,
    setResolver: () => {},
    reset: () => {},
    subscribe: () => () => {},
    getVersion: () => 1,
  };
  return ledger;
}
