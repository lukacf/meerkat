"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { compactedLog, assertCompactedText } = require("./compacted-log.cjs");

const source = "First chunk, second chunk.";
let sequence = 0;
function row(id, kind, payload, { message = "message-a", sourceKind = "console_event" } = {}) {
  return { id, kind, runtime_key: "runtime", identity: "router:main", source: { kind: sourceKind },
    payload: { ...payload, assistant_message_id: message, source_sequence: ++sequence } };
}
function streamed() {
  return [
    row("start", "turn_started", {}),
    row("r1-a", "reasoning_delta", { delta: "Think " }),
    row("r1-b", "reasoning_delta", { delta: "once." }),
    row("r1", "reasoning_complete", { content: "Think once." }),
    row("r2-a", "reasoning_delta", { delta: "Think " }),
    row("r2-b", "reasoning_delta", { delta: "twice." }),
    row("r2", "reasoning_complete", { content: "Think twice." }),
    row("t-a", "text_delta", { delta: "First chunk, " }),
    row("t-b", "text_delta", { delta: "second chunk." }),
    row("t", "text_complete", { content: source }),
    row("done", "turn_completed", {}),
  ];
}

test("the log keeps each completed block's first delta row, reasoning blocks one by one", () => {
  assert.deepEqual(compactedLog(streamed()).map(frame => frame.id),
    ["start", "r1-a", "r1", "r2-a", "r2", "t-a", "t", "done"]);
});

test("a block without its complete row, a row the log cannot compact and another message keep their delta rows", () => {
  const open = streamed().filter(frame => frame.id !== "t");
  assert.deepEqual(compactedLog(open).filter(frame => frame.kind === "text_delta").map(frame => frame.id), ["t-a", "t-b"]);
  const unnamed = streamed().map(frame => {
    const copy = structuredClone(frame);
    if (copy.id === "t-b") delete copy.payload.source_sequence;
    return copy;
  });
  assert(compactedLog(unnamed).some(frame => frame.id === "t-b"), "a row without its stream sequence stays");
  const otherMessage = [...streamed().slice(0, 9), row("t-other", "text_complete", { content: source }, { message: "message-b" })];
  assert.deepEqual(compactedLog(otherMessage).filter(frame => frame.kind === "text_delta").map(frame => frame.id), ["t-a", "t-b"],
    "another message's complete row removes nothing");
});

test("a compacted text read keeps the first delta row and the exact complete text", () => {
  const read = compactedLog(streamed());
  assert.equal(assertCompactedText(read, source, "read").first.id, "t-a");
  assert.throws(() => assertCompactedText(streamed(), source, "live"), /first delta row and its complete row/);
  assert.throws(() => assertCompactedText(read.filter(frame => frame.id !== "t-a"), source, "lost"), /first delta row and its complete row/);
  assert.throws(() => assertCompactedText(read, `${source} More.`, "changed"), /exact complete text/);
  const changedDelta = structuredClone(read); changedDelta.find(frame => frame.id === "t-a").payload.delta = "chunk";
  assert.throws(() => assertCompactedText(changedDelta, source, "changed"), /kept delta row begins the text/);
});
