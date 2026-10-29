"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { assertReasoningSource } = require("./real-reasoning.cjs");

function exactFrames() {
  const source = "Check again again and check again.";
  let next = 0;
  const frame = (kind, payload) => {
    const id = `source-${++next}`;
    return { id, source_event_id: id, source: { kind: "console_event" }, interaction_id: "operator", kind, payload };
  };
  return [0, 1].flatMap(() => [
    ...source.match(/.{1,6}/gu).map(delta => frame("reasoning_delta", { delta })),
    frame("reasoning_complete", { content: source }),
  ]);
}

test("real reasoning oracle requires every equal fragment and both identical block identities", () => {
  const frames = exactFrames();
  assert.equal(assertReasoningSource(frames, "operator", "exact").eventIds.length, 14);
  assert.throws(() => assertReasoningSource(frames.filter((_, index) => index !== 2), "operator", "lost repeated fragment"), /exact delta join/);
  assert.throws(() => assertReasoningSource(frames.slice(0, 7), "operator", "lost repeated block"), /identical sequential blocks/);
  assert.throws(() => assertReasoningSource([...frames, frames[0]], "operator", "replayed event duplicated"), /source event IDs/);
  const damagedCompletion = structuredClone(frames);
  damagedCompletion[6].payload.content = "Check again and check again.";
  assert.throws(() => assertReasoningSource(damagedCompletion, "operator", "damaged completion"), /exact owner completion/);
});

test("a read of the log keeps each block's first delta and its exact completion", () => {
  // Each block is six deltas then its completion; the log keeps the first delta and the completion.
  const frames = exactFrames();
  const stored = frames.filter((_, index) => index % 7 === 0 || index % 7 === 6);
  assert.deepEqual(assertReasoningSource(stored, "operator", "owner", { compacted: true }).eventIds,
    ["source-1", "source-7", "source-8", "source-14"]);
  assert.throws(() => assertReasoningSource(frames, "operator", "uncompacted", { compacted: true }), /first delta row only/);
  assert.throws(() => assertReasoningSource(stored.filter((_, index) => index !== 2), "operator", "lost first delta", { compacted: true }), /first delta row only/);
  assert.throws(() => assertReasoningSource(stored, "operator", "live"), /exact delta join/);
  const changedChunk = structuredClone(stored); changedChunk[2].payload.delta = "again ";
  assert.throws(() => assertReasoningSource(changedChunk, "operator", "changed chunk", { compacted: true }), /first chunk/);
  const damagedCompletion = structuredClone(stored); damagedCompletion[3].payload.content = "Check again and check again.";
  assert.throws(() => assertReasoningSource(damagedCompletion, "operator", "damaged completion", { compacted: true }), /exact owner completion/);
  assert.throws(() => assertReasoningSource(stored.slice(0, 2), "operator", "lost block", { compacted: true }), /identical sequential blocks/);
});
