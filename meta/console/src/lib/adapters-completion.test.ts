import assert from "node:assert/strict";
import test from "node:test";
import { mapFramesToTimelineEntries as stock } from "./adapters";
import { mapFramesToTimelineEntries as shared } from "../../../packages/console-core/src/adapters";
const agent = { agent_id: "worker", member_id: "worker", identity: "worker", label: "Worker", kind: "mob_agent" };
const call = { id: "history-call", event: "text_complete", identity: "worker", sourceKind: "session_history", timestampMs: 10, data: { message: { role: "block_assistant", blocks: [{ block_type: "tool_use", data: { id: "call-1", name: "read_file", args: { path: "notes.txt" } } }], stop_reason: "tool_use" }, text: "", result: "" } };
for (const [name, mapper] of [["stock", stock], ["shared", shared]] as const) {
  function block(frames: any[]) { const entries = mapper(agent, frames); return entries.flatMap((entry: any) => entry.blocks ?? []).find((item: any) => item.type === "tool-call"); }
  test(`${name} history missing, malformed and partial result pages remain unknown`, () => {
    assert.equal(block([call]).completionEvidence.outcome, "unknown");
    assert.equal(block([call]).status, "pending");
    const incomplete = { id: "result", event: "tool_execution_completed", identity: "worker", sourceKind: "session_history", timestampMs: 11, data: { id: "call-1", result: "Success in prose" } };
    assert.equal(block([call, incomplete]).completionEvidence.outcome, "unknown");
    assert.equal(block([call, { ...incomplete, data: { id: "different-call", is_error: false } }]).completionEvidence.outcome, "unknown");
  });
  test(`${name} live owner result pairs in either order and preserves raw content`, () => {
    const start = { id: "start", event: "tool_execution_started", timestampMs: 1, data: { id: "live-call", name: "read_file", args: { path: "a" } } };
    const result = { id: "result", event: "tool_execution_completed", timestampMs: 2, data: { id: "live-call", name: "read_file", content: "  exact\n", is_error: false } };
    for (const frames of [[start, result], [result, start]]) {
      assert.equal(block(frames).completionEvidence.outcome, "success");
      assert.equal(block(frames).result, "  exact\n");
    }
  });
  test(`${name} matched late owner result upgrades success without inferring from content`, () => {
    const result = { id: "result", event: "tool_execution_completed", identity: "worker", sourceKind: "session_history", timestampMs: 11, data: { id: "call-1", result: "Permission denied is a quoted file line", is_error: false } };
    assert.equal(block([call, result]).completionEvidence.outcome, "success");
    assert.equal(block([call, { ...result, data: { ...result.data, is_error: true } }]).completionEvidence.outcome, "error");
    assert.equal(block([call, { ...result, data: { ...result.data, status: "interrupted" } }]).completionEvidence.outcome, "interrupted");
    assert.equal(block([call, { ...result, data: { ...result.data, status: "cancelled" } }]).completionEvidence.outcome, "cancelled");
  });
}
