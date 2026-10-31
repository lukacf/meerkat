// One status vocabulary for agents: whether an agent is working now, how
// healthy it is, and the words every surface shows for that. The sidebar,
// the roster, Health and the topology map read these rules instead of each
// keeping its own copy. Pure module (no React), so node test bundles can
// import it.

import type { Tone } from "../../../packages/console-components/src/panel-kit/tone";
import type { ConsoleAgent } from "../types";

/** The agent fields the status rules read. */
export type AgentStatusFields = Pick<ConsoleAgent, "state" | "response_phase" | "progress" | "labels" | "unobserved">;

/** The owner-reported response phases in which an agent is doing something. */
export const ACTIVE_RESPONSE_PHASES: ReadonlySet<string> = new Set(["waiting", "tool-executing", "generating"]);

/** Member states in which an agent has left the team or is leaving it. */
const RETIRED_STATES: ReadonlySet<string> = new Set(["retired", "retiring", "stopped", "completed"]);

/** The agent's member state word: trimmed, lower case. */
export function agentStateWord(agent: Pick<AgentStatusFields, "state">): string {
  return (agent.state || "").trim().toLowerCase();
}

/** Retired, retiring, stopped or completed: the agent no longer works for the team. */
export function isAgentRetired(agent: Pick<AgentStatusFields, "state">): boolean {
  return RETIRED_STATES.has(agentStateWord(agent));
}

/** Whether the owner reports a response phase in which the agent is doing something. */
export function hasActiveResponsePhase(agent: Pick<AgentStatusFields, "response_phase">): boolean {
  return typeof agent.response_phase === "string" && ACTIVE_RESPONSE_PHASES.has(agent.response_phase);
}

/**
 * What the owner's typed response phase says: `working` (a phase is
 * active), `quiet` (the owner reports no phase: null) or `unknown` (no
 * report). The sidebar's activity filter reads this, and never infers
 * activity from a member state or prose.
 */
export type ResponsePhaseActivity = "working" | "quiet" | "unknown";

export function responsePhaseActivity(agent: Pick<AgentStatusFields, "response_phase">): ResponsePhaseActivity {
  // Null is an owner-reported quiet phase; absent activity stays unknown.
  if (agent.response_phase === null) return "quiet";
  return hasActiveResponsePhase(agent) ? "working" : "unknown";
}

/** Stuck: broken, or stalled (its run is open but nothing moves). Stuck is not working. */
function isAgentStuck(agent: Pick<AgentStatusFields, "state" | "progress">): boolean {
  return agentStateWord(agent) === "broken" || agent.progress?.health === "wedged";
}

export interface AgentWorkingOptions {
  /**
   * The console saw a run of this agent start and not end (the activity
   * ledger's open run span). It counts only while the owner reports no
   * phase at all and no idle run state: the owner's typed facts win.
   */
  openRun?: boolean;
}

/**
 * The one "working now" rule: the agent is in the team and not stuck, and
 * the owner reports an active response phase (starting, writing a reply,
 * running a tool) or an open run (`progress.run_state` "run_open"), or,
 * with no owner report, the console saw a run start that has not ended.
 * A live mark (the pulse) shows exactly when this holds.
 */
export function isAgentWorking(agent: AgentStatusFields, options: AgentWorkingOptions = {}): boolean {
  if (isAgentRetired(agent) || isAgentStuck(agent)) return false;
  if (hasActiveResponsePhase(agent)) return true;
  if (agent.progress?.run_state === "run_open") return true;
  return options.openRun === true
    && agent.response_phase === undefined
    && agent.progress?.run_state !== "idle";
}

/** How healthy an agent is, as one kind. */
export type AgentHealthKind =
  | "healthy"
  | "degraded"
  | "needs_repair"
  | "stalled"
  | "broken"
  | "retiring"
  | "retired"
  | "unobserved";

export interface AgentHealth {
  kind: AgentHealthKind;
  /** The tone of its mark (A2.4): crit for stuck, warn for needs attention. */
  tone: Tone;
  /** The word people read: "Stalled", "Degraded", "Healthy". */
  word: string;
  /** Whether the health is a problem someone should look at. */
  problem: boolean;
}

const HEALTH: Readonly<Record<AgentHealthKind, AgentHealth>> = {
  healthy: { kind: "healthy", tone: "ok", word: "Healthy", problem: false },
  degraded: { kind: "degraded", tone: "warn", word: "Degraded", problem: true },
  needs_repair: { kind: "needs_repair", tone: "warn", word: "Needs repair", problem: true },
  stalled: { kind: "stalled", tone: "crit", word: "Stalled", problem: true },
  broken: { kind: "broken", tone: "crit", word: "Broken", problem: true },
  retiring: { kind: "retiring", tone: "neutral", word: "Retiring", problem: false },
  retired: { kind: "retired", tone: "neutral", word: "Retired", problem: false },
  unobserved: { kind: "unobserved", tone: "locked", word: "Not observed", problem: false },
};

/**
 * The one health rule, from the member state, the owner's progress health
 * and the console's degraded label: retired and retiring agents first, then
 * broken, stalled (`progress.health` "wedged"), needs repair, degraded
 * (`progress.health` "degraded", a `console_degraded` label or a degraded
 * state), then not observed; everything else is healthy.
 */
export function agentHealth(agent: AgentStatusFields): AgentHealth {
  const state = agentStateWord(agent);
  if (state === "retiring") return HEALTH.retiring;
  if (isAgentRetired(agent)) return HEALTH.retired;
  if (state === "broken") return HEALTH.broken;
  if (agent.progress?.health === "wedged") return HEALTH.stalled;
  if (state === "needs_repair") return HEALTH.needs_repair;
  if (
    agent.progress?.health === "degraded"
    || agent.labels?.console_degraded === "true"
    || state.includes("degrade")
  ) {
    return HEALTH.degraded;
  }
  if (agent.unobserved) return HEALTH.unobserved;
  return HEALTH.healthy;
}
