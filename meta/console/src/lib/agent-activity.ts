// What one agent did and is doing, in numbers and words, from the console's
// activity ledger and the agent's own record: the counts of today, the
// hourly series of the last day, the live line ("Running a tool · 12s")
// and the reason a health tile gives ("stalled 47m · run open"). Roster,
// Health and Agent details read these, so the three say the same thing.
// Pure functions (no React): they take a ledger reader and a clock.

import { formatRelativeTime } from "../../../packages/console-components/src/panel-kit/format";
import { agentHealth, isAgentWorking, type AgentHealth, type AgentStatusFields } from "./agent-status";
import { responsePhaseWords } from "./agents";
import type { ActivityLedgerReader, LedgerCounts, LedgerLiveState, LedgerMetric } from "./activity-ledger";
import type { ConsoleAgent } from "../types";

export const MINUTE_MS = 60_000;
export const HOUR_MS = 60 * MINUTE_MS;
export const DAY_MS = 24 * HOUR_MS;

/** The key the ledger files an agent under: its identity, else its member id. */
export function agentKey(agent: Pick<ConsoleAgent, "identity" | "member_id" | "agent_id">): string {
  return (agent.identity || agent.member_id || agent.agent_id || "").trim();
}

/** The start of the local day `nowMs` falls in. */
export function localDayStart(nowMs: number): number {
  const date = new Date(nowMs);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** One agent's activity: today's counts, the last day by hour, what runs now. */
export interface AgentActivity {
  key: string;
  /** Since local midnight. */
  today: LedgerCounts;
  /** Events per hour over the last 24 hours, oldest first (24 bins). */
  hourly: number[];
  /** Turns started per hour over the last 24 hours, oldest first. */
  hourlyTurns: number[];
  live: LedgerLiveState;
  /** A run the console saw start and not end. */
  openRun: boolean;
  /** The newest time anything said the agent acted (ms), or null. */
  lastActivityMs: number | null;
  /** The hourly series ends here. */
  endMs: number;
}

const EMPTY_COUNTS: LedgerCounts = { turnsStarted: 0, turnsFinished: 0, failures: 0, toolCalls: 0, messagesSent: 0, events: 0 };

/** The hour bin end at or after `nowMs`, so the series slide by whole hours. */
export function hourCeiling(nowMs: number): number {
  return Math.ceil(nowMs / HOUR_MS) * HOUR_MS;
}

export function agentActivity(
  ledger: ActivityLedgerReader | null | undefined,
  agent: Pick<ConsoleAgent, "identity" | "member_id" | "agent_id" | "last_activity_ms" | "progress">,
  nowMs: number,
): AgentActivity {
  const key = agentKey(agent);
  const endMs = hourCeiling(nowMs);
  const window = { bins: 24, binMs: HOUR_MS, endMs };
  const live: LedgerLiveState = ledger && key
    ? ledger.liveState(key)
    : { openRun: null, openTool: null, callingModel: false };
  const today = ledger && key ? ledger.totals(key, { sinceMs: localDayStart(nowMs), untilMs: nowMs }) : EMPTY_COUNTS;
  const series = (metric: LedgerMetric) => (ledger && key ? ledger.series(key, metric, window) : new Array<number>(24).fill(0));
  const candidates = [
    agent.last_activity_ms,
    agent.progress && agent.progress.last_progress_at_ms > 0 ? agent.progress.last_progress_at_ms : undefined,
    live.lastActivityMs,
  ].filter((value): value is number => typeof value === "number" && Number.isFinite(value) && value > 0);
  return {
    key,
    today,
    hourly: series("events"),
    hourlyTurns: series("turnsStarted"),
    live,
    openRun: Boolean(ledger && key && ledger.hasOpenRun(key)),
    lastActivityMs: candidates.length ? Math.max(...candidates) : null,
    endMs,
  };
}

/** "3", "12" with the unit: "1 turn", "12 turns". */
export function countWords(count: number, singular: string, pluralWord = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : pluralWord}`;
}

const pad = (value: number) => String(value).padStart(2, "0");

/** "14:00" for the start of an hour bin. */
export function hourLabel(ms: number): string {
  const date = new Date(ms);
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * An hourly series in words, for a chart's accessible name: "12 turns in
 * the last 24 hours, most at 14:00", or "No turns in the last 24 hours".
 */
export function seriesWords(values: readonly number[], endMs: number, unit: string, unitPlural = `${unit}s`): string {
  const total = values.reduce((sum, value) => sum + (Number.isFinite(value) ? value : 0), 0);
  if (total <= 0) return `No ${unitPlural} in the last 24 hours`;
  let best = 0;
  values.forEach((value, index) => {
    if (value > values[best]) best = index;
  });
  const binMs = values.length > 0 ? DAY_MS / values.length : HOUR_MS;
  const start = endMs - (values.length - best) * binMs;
  return `${countWords(total, unit, unitPlural)} in the last 24 hours, most at ${hourLabel(start)}`;
}

/** A short span: "12s", "4m", "2h 5m", "3d". */
export function shortSpan(ms: number): string {
  const value = Math.max(0, ms);
  if (value < MINUTE_MS) return `${Math.max(1, Math.round(value / 1000))}s`;
  if (value < HOUR_MS) return `${Math.floor(value / MINUTE_MS)}m`;
  if (value < DAY_MS) {
    const hours = Math.floor(value / HOUR_MS);
    const minutes = Math.floor((value % HOUR_MS) / MINUTE_MS);
    return minutes ? `${hours}h ${minutes}m` : `${hours}h`;
  }
  return `${Math.floor(value / DAY_MS)}d`;
}

function sentence(text: string): string {
  return text ? text.charAt(0).toUpperCase() + text.slice(1) : text;
}

/** The status fields the rules read, from the agent's record. */
export function statusFields(agent: ConsoleAgent): AgentStatusFields {
  return {
    state: agent.state,
    response_phase: agent.response_phase,
    progress: agent.progress,
    labels: agent.labels,
    unobserved: agent.unobserved,
  };
}

/** The agent's health with the console's open-run fallback. */
export function agentHealthOf(agent: ConsoleAgent, activity?: Pick<AgentActivity, "openRun"> | null): AgentHealth {
  return agentHealth(statusFields(agent), { openRun: activity?.openRun === true });
}

export function agentWorking(agent: ConsoleAgent, activity?: Pick<AgentActivity, "openRun"> | null): boolean {
  return isAgentWorking(statusFields(agent), { openRun: activity?.openRun === true });
}

/** The live line: what the agent is doing now, in words. */
export interface AgentNowLine {
  text: string;
  /** Happening now: the live mark leads it. */
  live: boolean;
}

/**
 * What the agent is doing now: "Running a tool · 12s", "Writing a reply",
 * "Working on 3 tasks"; for a stuck or idle agent, how long it has been so:
 * "No progress for 47m", "Idle for 18m"; "Retired".
 */
export function agentNowLine(agent: ConsoleAgent, activity: AgentActivity, nowMs: number): AgentNowLine {
  const health = agentHealthOf(agent, activity);
  const progress = agent.progress;
  const lastProgress = progress && progress.last_progress_at_ms > 0 ? progress.last_progress_at_ms : null;
  if (health.kind === "retired" || health.kind === "retiring") return { text: health.word, live: false };
  if (health.kind === "broken") return { text: "Broken: it needs a respawn or a reset", live: false };
  if (health.kind === "stalled") {
    return { text: lastProgress ? `No progress for ${shortSpan(nowMs - lastProgress)}, its run still open` : "No progress, its run still open", live: false };
  }
  if (health.kind === "needs_repair") return { text: "Its saved session waits for a repair", live: false };
  if (agentWorking(agent, activity)) {
    const tool = activity.live.openTool;
    const run = activity.live.openRun;
    const phase = responsePhaseWords(agent.response_phase);
    const since = tool?.startMs ?? run?.startMs;
    const base = tool
      ? "Running a tool"
      : phase
        ? sentence(phase)
        : progress && progress.in_flight_work > 1
          ? `Working on ${progress.in_flight_work} tasks`
          : "Working";
    return { text: since && nowMs - since >= 1000 ? `${base} · ${shortSpan(nowMs - since)}` : base, live: true };
  }
  const idleSince = activity.lastActivityMs ?? lastProgress;
  if (health.kind === "degraded") {
    return { text: idleSince ? `Degraded, idle for ${shortSpan(nowMs - idleSince)}` : "Degraded", live: false };
  }
  if (health.kind === "unobserved") return { text: "Not observed yet", live: false };
  if (progress?.last_progress_event === "became_idle" && lastProgress) {
    return { text: `Finished and went idle ${formatRelativeTime(lastProgress, nowMs)}`, live: false };
  }
  return { text: idleSince ? `Idle for ${shortSpan(nowMs - idleSince)}` : "Idle", live: false };
}

/**
 * Why a health tile reads as it does, in a few words: "stalled 47m · run
 * open", "idle 18m · 1 in flight", "working · 2m", "not observed: refresh
 * pending".
 */
export function healthReason(
  health: AgentHealth,
  input: {
    progress?: ConsoleAgent["progress"];
    unobserved?: ConsoleAgent["unobserved"];
    lastActivityMs?: number | null;
    /** When the work in progress started (the open tool or run), if the console saw it. */
    workingSinceMs?: number | null;
    nowMs: number;
  },
): string {
  const { progress, nowMs } = input;
  const last = progress && progress.last_progress_at_ms > 0 ? progress.last_progress_at_ms : input.lastActivityMs ?? null;
  const inFlight = progress?.in_flight_work ?? 0;
  const age = last ? shortSpan(nowMs - last) : "";
  const parts: string[] = [];
  switch (health.kind) {
    case "stalled":
      parts.push(age ? `stalled ${age}` : "stalled");
      if (progress?.run_state === "run_open") parts.push("run open");
      break;
    case "broken":
      parts.push("broken");
      break;
    case "needs_repair":
      parts.push("saved session held for repair");
      break;
    case "degraded":
      parts.push(age ? `idle ${age}` : "degraded");
      break;
    case "working": {
      const since = input.workingSinceMs ?? null;
      parts.push(since ? `working · ${shortSpan(nowMs - since)}` : "working");
      break;
    }
    case "unobserved": {
      const reason = input.unobserved?.reason;
      parts.push(reason ? `not observed: ${String(reason).replace(/_/g, " ")}` : "not observed");
      break;
    }
    case "retired":
    case "retiring":
      parts.push(health.word.toLowerCase());
      break;
    default:
      parts.push(progress?.run_state === "run_open" ? "run open" : age ? `idle ${age}` : "idle");
  }
  if (inFlight > 0 && health.kind !== "working") parts.push(`${inFlight} in flight`);
  return parts.join(" · ");
}
