"use strict";
const assert = require("node:assert/strict");
const { eventually } = require("./acceptance-runtime.cjs");

function assertExplicitIdleRoster(agents) {
  assert(Array.isArray(agents), "experience contains the actual owner roster");
  assert.deepEqual(agents.map(agent => agent.identity).sort(), ["domain:delivery", "router:main"],
    "this fixture owns exactly the expected two live agents");
  for (const agent of agents) {
    assert.equal(agent.response_phase, null, `known idle owner ${agent.identity} must explicitly report null, not omit response_phase`);
  }
  return agents;
}

function assertSidebarCompletion(frames, { toolId, answer }) {
  assert(Array.isArray(frames), "timeline contains actual owner frames");
  assert(frames.some(frame => frame.kind === "interaction_complete" && frame.payload?.result === answer),
    "the exact expected model answer completed");
  const liveTool = frames.filter(frame => frame.source?.kind === "console_event" && frame.payload?.id === toolId);
  const savedTool = frames.filter(frame => frame.source?.kind === "session_history" && frame.payload?.id === toolId);
  assert.equal(liveTool.filter(frame => frame.kind === "tool_call_requested").length, 1, "one actual runtime tool call");
  assert.equal(liveTool.filter(frame => frame.kind === "tool_result_received" && frame.payload?.is_error === false).length, 1, "one actual successful runtime tool result");
  assert.equal(savedTool.filter(frame => frame.kind === "tool_call_requested").length, 1, "one retained tool-call source counterpart");
  assert.equal(savedTool.filter(frame => frame.kind === "tool_execution_completed" && frame.payload?.is_error === false).length, 1, "one retained successful tool-result source counterpart");
  return frames;
}

async function waitForExplicitIdleRoster({ readRoster, onObservation = () => {}, timeoutMs = 20_000 }) {
  return eventually(async () => {
    const agents = await readRoster();
    onObservation(agents);
    return assertExplicitIdleRoster(agents);
  }, "owner reports both agents explicitly idle", timeoutMs);
}

async function waitForSidebarCompletion({ readFrames, toolId, answer, onObservation = () => {}, timeoutMs = 20_000 }) {
  return eventually(async () => {
    const frames = await readFrames();
    onObservation(frames);
    return assertSidebarCompletion(frames, { toolId, answer });
  }, "real model and WorkGraph readiness tool finish", timeoutMs);
}

const ACTIVITY_NAME_COLLATOR = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

// The roster order the owner's activity facts imply for Sort: Last activity:
// newest last_activity_ms first, unknown last, ties by name, then identity.
function expectedActivityOrder(agents) {
  assert(Array.isArray(agents) && agents.length > 0, "experience contains the actual owner roster");
  const rows = agents.map(agent => {
    assert(typeof agent.identity === "string" && agent.identity, "every owner row names its identity");
    const at = agent.last_activity_ms;
    assert(at === undefined || (Number.isFinite(at) && at >= 0), `${agent.identity} reports last_activity_ms as a time or not at all`);
    return { identity: agent.identity, label: agent.label || agent.identity, at };
  });
  return rows.sort((a, b) => {
    if (a.at !== b.at) {
      if (a.at === undefined) return 1;
      if (b.at === undefined) return -1;
      return b.at - a.at;
    }
    return ACTIVITY_NAME_COLLATOR.compare(a.label, b.label) || (a.identity < b.identity ? -1 : a.identity > b.identity ? 1 : 0);
  }).map(row => row.identity);
}

// The owner recorded `identity`'s activity: its row carries a last_activity_ms
// and the roster's activity facts are well formed.
function assertActivityRecorded(agents, identity) {
  expectedActivityOrder(agents);
  const row = agents.find(agent => agent.identity === identity);
  assert(row && Number.isFinite(row.last_activity_ms) && row.last_activity_ms > 0, `owner records last_activity_ms for ${identity}`);
  return agents;
}

module.exports = { assertActivityRecorded, assertExplicitIdleRoster, assertSidebarCompletion, expectedActivityOrder, waitForExplicitIdleRoster, waitForSidebarCompletion };
