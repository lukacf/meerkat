import assert from "node:assert/strict";
import test from "node:test";
import { settledToolCompletion, toolCompletionFromFrame } from "./tool-completion";

test("tool success requires exact matched owner result and explicit boolean", () => {
  for (const data of [{ id: "call", result: "ok" }, { id: "call", is_error: false }, { id: "other", is_error: false }, { id: "call", is_error: "false" }]) {
    assert.equal(toolCompletionFromFrame({ event: "tool_execution_completed", data }, "call").outcome, "unknown");
  }
  assert.equal(toolCompletionFromFrame({ event: "tool_execution_completed", data: { id: "call", is_error: false, content: [] } }, "call").outcome, "success");
  assert.equal(toolCompletionFromFrame({ event: "tool_result_received", sourceKind: "session_history", data: { tool_call_id: "call", is_error: true, content: [] } }, "call").outcome, "error");
});
test("cancelled or interrupted results cannot establish success", () => {
  for (const status of ["cancelled", "interrupted"] as const) {
    assert.equal(toolCompletionFromFrame({ event: "tool_execution_completed", data: { id: "call", is_error: false, status } }, "call").outcome, status);
  }
  assert.equal(toolCompletionFromFrame({ event: "server_tool_content", data: { id: "call", is_error: false } }, "call").outcome, "unknown");
});
test("the execution fact carries no body, so it is no evidence and never settles a call", () => {
  const completed = toolCompletionFromFrame({ event: "tool_execution_completed", data: { id: "call", is_error: false, duration_ms: 3 } }, "call");
  assert.equal(completed.outcome, "unknown");
  assert.equal(settledToolCompletion(completed), false);
  const received = toolCompletionFromFrame({ event: "tool_result_received", data: { id: "call", is_error: false, result: "ok", content: [{ type: "text", text: "ok" }] } }, "call");
  assert.equal(received.outcome, "success");
  assert.equal(settledToolCompletion(received), true);
  assert.equal(settledToolCompletion({ outcome: "running", source: "runtime-start", toolCallId: "call" }), false);
  assert.equal(settledToolCompletion(undefined), false);
});
