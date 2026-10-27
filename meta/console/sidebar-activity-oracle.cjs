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

module.exports = { assertExplicitIdleRoster, assertSidebarCompletion, waitForExplicitIdleRoster, waitForSidebarCompletion };
