"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { assertCapturedSource } = require("./stream-parity.cjs");

const source = "First  line\nSecond line  \n";
function capturedFrames() {
  const identity = { run_id: "run-a", interaction_id: "interaction-a" };
  const frame = (id, kind, payload, sourceKind = "console_event") => ({
    id, kind, source: { kind: sourceKind }, runtime_key: "runtime", identity: "router:main",
    ...identity, payload,
  });
  return [
    frame("delta-a", "text_delta", { delta: "First  line\n", identity }),
    frame("history", "text_complete", { text: source, result: source,
      message: { role: "block_assistant", identity, blocks: [{ block_type: "text", data: { text: source } }], stop_reason: "end_turn" } }, "session_history"),
    frame("delta-b", "text_delta", { delta: "Second line  \n", identity }),
    frame("text", "text_complete", { content: source, identity }),
    frame("terminal", "interaction_complete", { result: source, identity }),
  ];
}

test("stream parity accepts saved authored text before the distinct live completion", () => {
  assert.doesNotThrow(() => assertCapturedSource(capturedFrames(), source, "router"));
});

test("stream parity never substitutes history for a missing live completion or terminal", () => {
  const frames = capturedFrames();
  for (const id of ["text", "terminal"]) {
    assert.throws(() => assertCapturedSource(frames.filter(frame => frame.id !== id), source, "router"), /live .*completion/);
  }
  const forgedHistory = structuredClone(frames).filter(frame => frame.id !== "text");
  forgedHistory[1].payload.content = source;
  assert.throws(() => assertCapturedSource(forgedHistory, source, "router"), /live text completion/);
});

test("stream parity rejects changed bytes, repeated IDs and duplicate live completions", () => {
  const frames = capturedFrames();
  for (const [index, key] of [[0, "delta"], [1, "text"], [3, "content"], [4, "result"]]) {
    const changed = structuredClone(frames); changed[index].payload[key] = changed[index].payload[key].trim();
    assert.throws(() => assertCapturedSource(changed, source, "router"));
  }
  assert.throws(() => assertCapturedSource([...frames, frames[0]], source, "router"), /duplicate canonical/);
  assert.throws(() => assertCapturedSource([...frames, { ...frames[3], id: "second-live-text" }], source, "router"), /one live text completion/);
  const wrongOwner = structuredClone(frames); wrongOwner[1].payload.message.identity.run_id = "other-run";
  assert.throws(() => assertCapturedSource(wrongOwner, source, "router"), /saved message run owner/);
});
