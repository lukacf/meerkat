"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { assertRoutineFrames, expectedTools } = require("./real-routine-tools.cjs");

function frames(sourceKind = "console_event") {
  let seq = 0;
  return expectedTools("unit").flatMap(tool => [
    { kind: "tool_call_requested", payload: { tool_call_id: tool.id, name: tool.name, args: tool.args } },
    { kind: sourceKind === "console_event" ? "tool_result_received" : "tool_execution_completed", payload: { tool_call_id: tool.id, is_error: tool.error, content: [{ type: "text", text: tool.result }] } },
  ]).map(frame => { const id = `event-${++seq}`; return { ...frame, id, source_event_id: id, source: { kind: sourceKind } }; });
}

test("routine owner oracle checks source IDs, exact bytes, real outcomes, and complete call sets", () => {
  for (const source of ["console_event", "session_history"]) {
    const good = frames(source);
    assert.equal(assertRoutineFrames(good, "unit", source).length, 5);
    assert.throws(() => assertRoutineFrames(good.slice(1), "unit", source), /one call/);
    assert.throws(() => assertRoutineFrames([...good, good[0]], "unit", source), /distinct event IDs/);
    const rewritten = structuredClone(good); rewritten[1].payload.content[0].text = rewritten[1].payload.content[0].text.trim();
    assert.throws(() => assertRoutineFrames(rewritten, "unit", source), /exact result/);
    const falseSuccess = structuredClone(good); falseSuccess[5].payload.is_error = false;
    assert.throws(() => assertRoutineFrames(falseSuccess, "unit", source), /authoritative outcome/);
    const wrongArgs = structuredClone(good); wrongArgs[0].payload.args.path = "other.txt";
    assert.throws(() => assertRoutineFrames(wrongArgs, "unit", source), /exact arguments/);
  }
});

test("routine durable owner oracle reads canonical messages without demanding duplicate console frames", () => {
  const { assertRoutineHistory } = require("./real-routine-tools.cjs");
  const identity = { interaction_id: "interaction-a", run_id: "run-a" };
  const messages = expectedTools("unit").flatMap(tool => [
    { role: "block_assistant", identity, blocks: [{ block_type: "tool_use", data: { id: tool.id, name: tool.name, args: tool.args } }] },
    { role: "tool_results", results: [{ tool_use_id: tool.id, content: [{ type: "text", text: tool.result }], is_error: tool.error }] },
  ]);
  const page = { session_id: "session-a", offset: 0, message_count: messages.length, has_more: false, messages };
  const accepted = { session_id: "session-a", interaction_id: "interaction-a" };
  assert.equal(assertRoutineHistory(page, "unit", accepted, "run-a").length, 5);
  const missing = structuredClone(page); missing.messages.shift(); missing.message_count--;
  assert.throws(() => assertRoutineHistory(missing, "unit", accepted, "run-a"), /one call/);
  const duplicate = structuredClone(page); duplicate.messages.push(duplicate.messages[0]); duplicate.message_count++;
  assert.throws(() => assertRoutineHistory(duplicate, "unit", accepted, "run-a"), /one call/);
  const changed = structuredClone(page); changed.messages[1].results[0].content[0].text = changed.messages[1].results[0].content[0].text.trim();
  assert.throws(() => assertRoutineHistory(changed, "unit", accepted, "run-a"), /exact result/);
  const wrongOwner = structuredClone(page); wrongOwner.messages[0].identity.run_id = "run-b";
  assert.throws(() => assertRoutineHistory(wrongOwner, "unit", accepted, "run-a"), /canonical run/);
  const falseSuccess = structuredClone(page); falseSuccess.messages[5].results[0].is_error = false;
  assert.throws(() => assertRoutineHistory(falseSuccess, "unit", accepted, "run-a"), /authoritative outcome/);
  assert.throws(() => assertRoutineHistory({ ...page, has_more: true }, "unit", accepted, "run-a"), /complete owner history/);
  assert.throws(() => assertRoutineHistory({ ...page, session_id: "session-b" }, "unit", accepted, "run-a"), /accepted session/);
});
