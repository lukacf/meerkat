import assert from "node:assert/strict";
import test from "node:test";

import type { ConsoleAgent, ConsoleFrame } from "../types";
import { activeRunIdFromFrames, describeRunStopReceipt, parseRunStopResult, runStopOffered } from "./run-stop";

function frame(id: string, event: string, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, data: {}, ...extra };
}

function agent(memberId: string, affordances: ConsoleAgent["affordances"]): ConsoleAgent {
  return { agent_id: memberId, member_id: memberId, identity: memberId, label: memberId, kind: "mob_agent", affordances };
}

test("Stop is offered for a named run of an agent the viewer may retire, unless its row says the gateway stops no run of it", () => {
  const retirable = agent("alpha", { can_retire: true });
  const running = { readOnly: false, activeRunId: "r-1" };
  assert.equal(runStopOffered({ ...running, agent: retirable }), true, "a gateway that does not say leaves Stop offered");
  assert.equal(runStopOffered({ ...running, agent: agent("alpha", { can_retire: true, can_stop_run: true }) }), true);
  assert.equal(
    runStopOffered({ ...running, agent: agent("research-team/archivist", { can_retire: true, can_stop_run: false }) }),
    false,
    "a run of a member of a nested mob is stopped by no gateway route",
  );
  assert.equal(runStopOffered({ ...running, agent: agent("alpha", { can_retire: false }) }), false);
  assert.equal(runStopOffered({ ...running, readOnly: true, agent: retirable }), false);
  assert.equal(runStopOffered({ readOnly: false, activeRunId: null, agent: retirable }), false);
  assert.equal(runStopOffered({ ...running, agent: null }), false);
});

test("Stop follows the row's own word on stopping, and the retirement only where the row says nothing of stopping", () => {
  const running = { readOnly: false, activeRunId: "r-1" };
  assert.equal(
    runStopOffered({ ...running, agent: agent("support/reviewer", { can_retire: false, can_stop_run: true }) }),
    true,
    "a member of a hosted mob the gateway stops runs of and does not retire",
  );
  assert.equal(runStopOffered({ ...running, agent: agent("support/reviewer", { can_stop_run: true }) }), true);
  assert.equal(runStopOffered({ ...running, agent: agent("alpha", { can_retire: true, can_stop_run: false }) }), false);
  assert.equal(runStopOffered({ ...running, agent: agent("alpha", { can_retire: true }) }), true, "a launch mob's member as before");
  assert.equal(runStopOffered({ ...running, agent: agent("alpha", {}) }), false, "a row that says neither");
  assert.equal(runStopOffered({ ...running, readOnly: true, agent: agent("support/reviewer", { can_stop_run: true }) }), false);
  assert.equal(runStopOffered({ readOnly: false, activeRunId: null, agent: agent("support/reviewer", { can_stop_run: true }) }), false);
});

test("the active run is the latest run_started no terminal has closed", () => {
  assert.equal(activeRunIdFromFrames([]), null);
  assert.equal(
    activeRunIdFromFrames([frame("1", "run_started", { runId: "r-1" }), frame("2", "text_delta")]),
    "r-1",
  );
  assert.equal(
    activeRunIdFromFrames([
      frame("1", "run_started", { runId: "r-1" }),
      frame("2", "run_completed", { runId: "r-1" }),
    ]),
    null,
  );
  assert.equal(
    activeRunIdFromFrames([
      frame("1", "run_started", { runId: "r-1" }),
      frame("2", "run_failed", { runId: "r-1" }),
      frame("3", "run_started", { runId: "r-2" }),
    ]),
    "r-2",
  );
});

test("run ids are read from identity.run_id when the frame carries no runId", () => {
  assert.equal(
    activeRunIdFromFrames([frame("1", "run_started", { data: { identity: { run_id: "r-9" } } })]),
    "r-9",
  );
});

test("a steer delivery does not end the run, and another run's terminal does not either", () => {
  assert.equal(
    activeRunIdFromFrames([
      frame("1", "run_started", { runId: "r-1" }),
      frame("2", "interaction_complete", { data: { reason: "steer_delivered" } }),
      frame("3", "run_completed", { runId: "r-0" }),
    ]),
    "r-1",
  );
  assert.equal(
    activeRunIdFromFrames([
      frame("1", "run_started", { runId: "r-1" }),
      frame("2", "interaction_complete"),
    ]),
    null,
  );
});

test("reading back from the current run equals replaying the whole timeline", () => {
  // The forward replay this function used before reading from the end.
  const runIdOf = (f: ConsoleFrame) => f.runId?.trim()
    || ((f.data as { identity?: { run_id?: string } } | undefined)?.identity?.run_id?.trim() || null);
  const terminal = (f: ConsoleFrame) => ["run_completed", "run_failed", "interaction_failed"].includes(f.event)
    || (f.event === "interaction_complete" && (f.data as { reason?: string }).reason !== "steer_delivered");
  const replay = (frames: ConsoleFrame[]) => {
    let active: string | null = null;
    for (const f of frames) {
      if (f.event === "run_started") { const id = runIdOf(f); if (id) active = id; continue; }
      if (active && terminal(f)) { const id = runIdOf(f); if (!id || id === active) active = null; }
    }
    return active;
  };
  let state = 7;
  const random = () => ((state = (state * 1103515245 + 12345) % 2147483648) / 2147483648);
  const events = ["run_started", "run_completed", "run_failed", "interaction_failed", "interaction_complete", "text_delta", "tool_call"];
  for (let sequence = 0; sequence < 400; sequence += 1) {
    const frames = Array.from({ length: 1 + Math.floor(random() * 14) }, (_, i) => {
      const event = events[Math.floor(random() * events.length)];
      const roll = random();
      const runId = roll < 0.2 ? undefined : `r-${Math.floor(random() * 3)}`;
      const data = event === "interaction_complete" && random() < 0.3 ? { reason: "steer_delivered" }
        : roll < 0.3 && runId ? { identity: { run_id: runId } } : {};
      return frame(`${sequence}-${i}`, event, { data, ...(roll >= 0.3 ? { runId } : {}) });
    });
    assert.equal(activeRunIdFromFrames(frames), replay(frames), JSON.stringify(frames.map((f) => [f.event, f.runId, f.data])));
  }
});

test("receipts are validated and described for each outcome", () => {
  const stopped = parseRunStopResult({
    member_id: "w1",
    receipt: {
      outcome: "stopped",
      run_id: "r-1",
      contributors: [
        { input_id: "i-1", completion: "cancelled", terminal: "cancelled" },
        { input_id: "i-2", completion: "runtime_terminated", terminal: "cancelled" },
      ],
    },
  });
  assert.equal(describeRunStopReceipt(stopped), "Run stopped: 2 inputs cancelled.");
  const late = parseRunStopResult({ receipt: { outcome: "not_current", run_id: "r-1" } });
  assert.equal(describeRunStopReceipt(late), "That run already ended; nothing was stopped.");
  const refused = parseRunStopResult({
    receipt: { outcome: "not_stoppable", run_id: "r-1", state: "stopped" },
  });
  assert.equal(
    describeRunStopReceipt(refused),
    "The run cannot be stopped right now (runtime stopped).",
  );
  for (const bad of [
    {},
    { receipt: { outcome: "stopped", run_id: "r-1" } },
    { receipt: { outcome: "not_stoppable", run_id: "r-1" } },
    { receipt: { outcome: "mystery", run_id: "r-1" } },
  ]) {
    assert.throws(() => parseRunStopResult(bad));
  }
});
