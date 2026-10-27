"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { assertExplicitIdleRoster, assertSidebarCompletion, waitForExplicitIdleRoster, waitForSidebarCompletion } = require("./sidebar-activity-oracle.cjs");

function idleRoster() {
  return ["domain:delivery", "router:main"].map(identity => ({ identity, response_phase: null, progress: { run_state: "idle" } }));
}

const expected = { toolId: "fixture-sidebar-exact-peer-ready", answer: "Exact release readiness answer." };
function completedFrames() {
  const toolFrame = (kind, source, extra = {}) => ({ kind, source: { kind: source }, payload: { id: expected.toolId, ...extra } });
  return [
    toolFrame("tool_call_requested", "console_event"),
    toolFrame("tool_result_received", "console_event", { is_error: false }),
    { kind: "interaction_complete", source: { kind: "console_event" }, payload: { result: expected.answer } },
    toolFrame("tool_call_requested", "session_history"),
    toolFrame("tool_execution_completed", "session_history", { is_error: false }),
  ];
}

test("explicit idle accepts only the actual two owner identities", () => {
  const agents = idleRoster();
  assert.equal(assertExplicitIdleRoster(agents), agents);
});

for (const [name, corrupt] of [
  ["empty roster", agents => { agents.length = 0; }],
  ["missing owner", agents => { agents.pop(); }],
  ["unexpected owner", agents => { agents[1].identity = "other:agent"; }],
  ["duplicate owner", agents => { agents[1].identity = agents[0].identity; }],
  ["extra owner", agents => { agents.push({ identity: "other:agent", response_phase: null }); }],
  ["missing phase", agents => { delete agents[0].response_phase; }],
  ["undefined phase", agents => { agents[0].response_phase = undefined; }],
  ["unknown phase", agents => { agents[0].response_phase = "unknown"; }],
  ["active phase despite idle progress", agents => { agents[0].response_phase = "generating"; }],
]) {
  test(`idle evidence rejects ${name}`, () => {
    const agents = idleRoster(); corrupt(agents);
    assert.throws(() => assertExplicitIdleRoster(agents));
  });
}

test("idle polling rereads owner phases until both are explicitly null", async () => {
  const working = idleRoster(); working[0].response_phase = "generating";
  const unknown = idleRoster(); delete unknown[0].response_phase;
  const idle = idleRoster();
  const samples = [working, unknown, idle];
  const observations = [];
  let reads = 0;
  const actual = await waitForExplicitIdleRoster({
    readRoster: async () => samples[Math.min(reads++, samples.length - 1)],
    onObservation: value => observations.push(value), timeoutMs: 1000,
  });
  assert.equal(actual, idle);
  assert.equal(reads, 3, "each retry must read a fresh owner snapshot");
  assert.deepEqual(observations, samples);
  assert.equal(working[0].response_phase, "generating", "idle progress must not coerce the owner's active phase");
  assert.equal(Object.hasOwn(unknown[0], "response_phase"), false, "missing activity remains missing");
});

test("idle polling fails under its deadline when active owner evidence never changes", async () => {
  const agents = idleRoster(); agents[0].response_phase = "generating";
  await assert.rejects(waitForExplicitIdleRoster({ readRoster: async () => agents, timeoutMs: 1 }), /Timed out:.*explicitly report null/);
});

test("completion requires exact live and retained successful tool counterparts plus the answer", () => {
  const frames = completedFrames();
  assert.equal(assertSidebarCompletion(frames, expected), frames);
});

for (const [name, corrupt] of [
  ["missing exact answer", frames => { frames[2].payload.result += " Changed."; }],
  ["answer before terminal event", frames => { frames[2].kind = "text_delta"; }],
  ["missing live call", frames => { frames.splice(0, 1); }],
  ["missing live result", frames => { frames.splice(1, 1); }],
  ["missing retained call", frames => { frames.splice(3, 1); }],
  ["missing retained result", frames => { frames.splice(4, 1); }],
  ["wrong tool ID", frames => { frames[3].payload.id = "other-tool"; }],
  ["wrong retained source", frames => { frames[3].source.kind = "console_event"; }],
  ["wrong retained result kind", frames => { frames[4].kind = "tool_result_received"; }],
  ["failed live result", frames => { frames[1].payload.is_error = true; }],
  ["failed retained result", frames => { frames[4].payload.is_error = true; }],
  ["missing result status", frames => { delete frames[4].payload.is_error; }],
  ...[0, 1, 3, 4].map(index => [`duplicate counterpart ${index}`, frames => { frames.push(structuredClone(frames[index])); }]),
]) {
  test(`completion evidence rejects ${name}`, () => {
    const frames = completedFrames(); corrupt(frames);
    assert.throws(() => assertSidebarCompletion(frames, expected));
  });
}

test("completion polling rereads after live completion until retained counterparts arrive", async () => {
  const complete = completedFrames();
  const samples = [complete.slice(0, 3), complete.slice(0, 4), complete];
  const observations = [];
  let reads = 0;
  const actual = await waitForSidebarCompletion({ ...expected,
    readFrames: async () => samples[Math.min(reads++, samples.length - 1)],
    onObservation: value => observations.push(value), timeoutMs: 1000,
  });
  assert.equal(actual, complete);
  assert.equal(reads, 3, "a live terminal event does not freeze the pre-persistence snapshot");
  assert.deepEqual(observations, samples);
});

for (const [name, corrupt] of [
  ["missing retained evidence", frames => frames.slice(0, 3)],
  ["wrong retained tool ID", frames => { frames[4].payload.id = "other-tool"; return frames; }],
  ["duplicate retained evidence", frames => [...frames, structuredClone(frames[4])]],
]) {
  test(`completion polling fails under its deadline for ${name}`, async () => {
    const frames = corrupt(completedFrames());
    await assert.rejects(waitForSidebarCompletion({ ...expected, readFrames: async () => frames, timeoutMs: 1 }), /Timed out:.*retained/);
  });
}
