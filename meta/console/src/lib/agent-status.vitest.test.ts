import { describe, expect, it } from "vitest";

import {
  agentHealth,
  agentStateWord,
  isAgentRetired,
  isAgentWorking,
  responsePhaseActivity,
  type AgentStatusFields,
} from "./agent-status";

const progress = (run_state: string, health = "healthy") => ({
  run_state,
  in_flight_work: run_state === "run_open" ? 1 : 0,
  last_progress_at_ms: 1,
  last_progress_event: "unchanged",
  health,
});

const agent = (fields: Partial<AgentStatusFields> = {}): AgentStatusFields => ({ state: "active", ...fields });

describe("the one working rule", () => {
  it("works while the owner reports an active phase or an open run", () => {
    expect(isAgentWorking(agent({ response_phase: "generating" }))).toBe(true);
    expect(isAgentWorking(agent({ response_phase: "tool-executing" }))).toBe(true);
    expect(isAgentWorking(agent({ response_phase: "waiting" }))).toBe(true);
    expect(isAgentWorking(agent({ response_phase: null, progress: progress("run_open") }))).toBe(true);
    expect(isAgentWorking(agent({ response_phase: null, progress: progress("idle") }))).toBe(false);
    expect(isAgentWorking(agent())).toBe(false);
  });

  it("never calls a retired or stuck agent working", () => {
    for (const state of ["retired", "retiring", "stopped", "completed", "Retired "]) {
      expect(isAgentWorking(agent({ state, response_phase: "generating" }))).toBe(false);
    }
    expect(isAgentWorking(agent({ state: "broken", response_phase: "generating" }))).toBe(false);
    // A wedged agent still has its run open; it is stuck, not working.
    expect(isAgentWorking(agent({ progress: progress("run_open", "wedged") }))).toBe(false);
    // Degraded is a warning, not stuck.
    expect(isAgentWorking(agent({ progress: progress("run_open", "degraded") }))).toBe(true);
  });

  it("counts a run the console saw open only while the owner reports nothing", () => {
    expect(isAgentWorking(agent(), { openRun: true })).toBe(true);
    expect(isAgentWorking(agent({ progress: progress("unknown") }), { openRun: true })).toBe(true);
    // The owner's typed facts win over the console's own view.
    expect(isAgentWorking(agent({ response_phase: null }), { openRun: true })).toBe(false);
    expect(isAgentWorking(agent({ progress: progress("idle") }), { openRun: true })).toBe(false);
    expect(isAgentWorking(agent({ state: "retired" }), { openRun: true })).toBe(false);
  });
});

describe("the owner's response phase", () => {
  it("reads working, quiet or unknown from the typed phase only", () => {
    expect(responsePhaseActivity({ response_phase: "generating" })).toBe("working");
    expect(responsePhaseActivity({ response_phase: "waiting" })).toBe("working");
    expect(responsePhaseActivity({ response_phase: "tool-executing" })).toBe("working");
    expect(responsePhaseActivity({ response_phase: null })).toBe("quiet");
    expect(responsePhaseActivity({})).toBe("unknown");
  });
});

describe("the one health rule", () => {
  it("names each health kind with its tone and word", () => {
    const health = (fields: Partial<AgentStatusFields>) => {
      const { kind, tone, word, problem } = agentHealth(agent(fields));
      return `${kind}/${tone}/${word}/${problem}`;
    };
    expect(health({ progress: progress("idle") })).toBe("healthy/ok/Healthy/false");
    expect(health({ progress: progress("run_open", "degraded") })).toBe("degraded/warn/Degraded/true");
    expect(health({ labels: { console_degraded: "true" } })).toBe("degraded/warn/Degraded/true");
    expect(health({ state: "degraded" })).toBe("degraded/warn/Degraded/true");
    expect(health({ state: "needs_repair" })).toBe("needs_repair/warn/Needs repair/true");
    expect(health({ progress: progress("run_open", "wedged") })).toBe("stalled/crit/Stalled/true");
    expect(health({ state: "broken", progress: progress("idle", "degraded") })).toBe("broken/crit/Broken/true");
    expect(health({ state: "retiring", progress: progress("run_open", "wedged") })).toBe("retiring/neutral/Retiring/false");
    expect(health({ state: "completed" })).toBe("retired/neutral/Retired/false");
    expect(health({ unobserved: { reason: "not_observed", fields: ["progress"] } }))
      .toBe("unobserved/locked/Not observed/false");
  });

  it("reads the member state without case or padding", () => {
    expect(agentStateWord({ state: "  Active " })).toBe("active");
    expect(isAgentRetired({ state: "STOPPED" })).toBe(true);
    expect(isAgentRetired({ state: "dormant" })).toBe(false);
  });
});
