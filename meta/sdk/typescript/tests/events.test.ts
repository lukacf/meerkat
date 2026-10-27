import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  parseAgentEvent,
  parseMobEventFromSse,
  parseAgentEventFromSse,
  EventStream,
  isTextDelta,
  isTextComplete,
  isRunCompleted,
  isRunFailed,
  isTurnCompleted,
  isToolCallRequested,
} from "../dist/index.js";
import type { AgentEvent } from "../dist/index.js";

// ---------------------------------------------------------------------------
// parseAgentEvent — all 11 known event types + unknown
// ---------------------------------------------------------------------------

describe("parseAgentEvent", () => {
  it("preserves the typed fallback resume hold", () => {
    const reason = {
      reason_type: "model_fallback_resume_held",
      provider: "openai",
      model: "gpt-5.5",
      reason: "context_fit",
    };
    const event = parseAgentEvent({
      type: "run_failed",
      session_id: "session-held",
      error_report: { class: "config", message: "held", reason },
    });
    assert.ok(isRunFailed(event));
    assert.equal(event.errorReport?.reasonType, "model_fallback_resume_held");
    assert.deepEqual(event.errorReport?.reason, reason);
  });

  it("preserves fallback observations through the open event payload", () => {
    for (const type of [
      "model_fallback_skipped", "model_fallback_staged",
      "model_fallback_committed", "model_fallback_target_failed",
    ]) {
      const raw = {
        type,
        target: { provider: "openai", model: "gpt-5.5" },
        retry: { plan: { attempt: 3 } },
      };
      const event = parseAgentEvent(raw);
      assert.equal(event.type, type);
      assert.ok("data" in event);
      assert.deepEqual(event.data, raw);
    }
  });

  it("parses run_started", () => {
    const event = parseAgentEvent({
      type: "run_started",
      session_id: "sess-1",
      prompt: "hello world",
    });
    assert.equal(event.type, "run_started");
    assert.equal((event as any).sessionId, "sess-1");
    assert.equal((event as any).prompt, "hello world");
  });

  it("parses run_completed", () => {
    const event = parseAgentEvent({
      type: "run_completed",
      session_id: "sess-2",
      result: "done",
    });
    assert.equal(event.type, "run_completed");
    assert.equal((event as any).sessionId, "sess-2");
    assert.equal((event as any).result, "done");
  });

  it("parses run_failed", () => {
    const event = parseAgentEvent({
      type: "run_failed",
      session_id: "sess-3",
      error: "boom",
    });
    assert.equal(event.type, "run_failed");
    assert.equal((event as any).sessionId, "sess-3");
    assert.equal((event as any).error, "boom");
    assert.equal((event as any).errorReport, undefined);
  });

  it("parses run_failed from meerkat's typed error_report (no flat error on the wire)", () => {
    // Wire shape of meerkat-core 0.8.32 `AgentEvent::RunFailed`: the typed
    // `error_report` is the only failure truth; there is no flat `error`.
    const event = parseAgentEvent({
      type: "run_failed",
      session_id: "01a068a0-f911-7331-8187-d436145ac895",
      error_report: {
        class: "llm",
        reason: { reason_type: "llm_auth_error" },
        message: "LLM error: authentication failed (401)",
      },
    });
    assert.equal(event.type, "run_failed");
    assert.equal((event as any).error, "LLM error: authentication failed (401)");
    assert.deepEqual((event as any).errorReport, {
      class: "llm",
      message: "LLM error: authentication failed (401)",
      reason: { reason_type: "llm_auth_error" },
      reasonType: "llm_auth_error",
    });
  });

  it("parses run_failed whose error_report carries no typed reason", () => {
    const event = parseAgentEvent({
      type: "run_failed",
      session_id: "s",
      error_report: { class: "internal", message: "runner gave up" },
    });
    assert.equal((event as any).error, "runner gave up");
    assert.deepEqual((event as any).errorReport, { class: "internal", message: "runner gave up" });
  });

  it("parses turn_started", () => {
    const event = parseAgentEvent({
      type: "turn_started",
      turn_number: 5,
    });
    assert.equal(event.type, "turn_started");
    assert.equal((event as any).turnNumber, 5);
  });

  it("parses text_delta", () => {
    const event = parseAgentEvent({
      type: "text_delta",
      delta: "chunk",
    });
    assert.equal(event.type, "text_delta");
    assert.equal((event as any).delta, "chunk");
  });

  it("parses text_complete", () => {
    const event = parseAgentEvent({
      type: "text_complete",
      content: "full text",
    });
    assert.equal(event.type, "text_complete");
    assert.equal((event as any).content, "full text");
  });

  it("parses tool_call_requested", () => {
    const args = { key: "value" };
    const event = parseAgentEvent({
      type: "tool_call_requested",
      id: "tc-1",
      name: "search",
      args,
    });
    assert.equal(event.type, "tool_call_requested");
    assert.equal((event as any).id, "tc-1");
    assert.equal((event as any).name, "search");
    assert.deepEqual((event as any).args, args);
  });

  it("parses tool_result_received", () => {
    const event = parseAgentEvent({
      type: "tool_result_received",
      id: "tr-1",
      tool_call_id: "tr-1",
      name: "search",
      content: [{ type: "text", text: "not found" }],
      result: "not found",
      is_error: true,
    });
    assert.equal(event.type, "tool_result_received");
    assert.equal((event as any).id, "tr-1");
    assert.equal((event as any).name, "search");
    assert.equal((event as any).result, "not found");
    assert.equal((event as any).isError, true);
  });

  it("defaults tool_result_received result to an empty string", () => {
    const event = parseAgentEvent({ type: "tool_result_received", id: "tr-1", name: "search", is_error: false });
    assert.equal((event as any).result, "");
  });

  it("parses tool_result_received with is_error false", () => {
    const event = parseAgentEvent({
      type: "tool_result_received",
      id: "tr-2",
      name: "calc",
      is_error: false,
    });
    assert.equal((event as any).isError, false);
  });

  it("parses turn_completed", () => {
    const event = parseAgentEvent({
      type: "turn_completed",
      stop_reason: "end_turn",
    });
    assert.equal(event.type, "turn_completed");
    assert.equal((event as any).stopReason, "end_turn");
  });

  it("parses tool_execution_started", () => {
    const event = parseAgentEvent({
      type: "tool_execution_started",
      id: "tex-1",
      name: "bash",
    });
    assert.equal(event.type, "tool_execution_started");
    assert.equal((event as any).id, "tex-1");
    assert.equal((event as any).name, "bash");
  });

  it("parses tool_execution_completed, which carries no output", () => {
    const event = parseAgentEvent({
      type: "tool_execution_completed",
      id: "tex-2",
      tool_call_id: "tex-2",
      name: "bash",
      is_error: false,
      duration_ms: 120,
    });
    assert.deepEqual(event, {
      type: "tool_execution_completed",
      id: "tex-2",
      name: "bash",
      isError: false,
      durationMs: 120,
    });
  });

  it("ignores the output on a tool_execution_completed from an older server", () => {
    const event = parseAgentEvent({
      type: "tool_execution_completed",
      id: "tex-2",
      name: "bash",
      content: [{ type: "text", text: "ok" }],
      result: "ok",
      is_error: false,
      duration_ms: 120,
    });
    assert.equal(event.type, "tool_execution_completed");
    assert.equal("result" in event, false);
    assert.equal((event as any).durationMs, 120);
  });

  it("returns UnknownEvent for unrecognized type", () => {
    const raw = { type: "custom_event", foo: "bar" };
    const event = parseAgentEvent(raw);
    assert.equal(event.type, "custom_event");
    assert.deepEqual((event as any).data, raw);
  });

  it("handles missing fields with defaults", () => {
    const event = parseAgentEvent({ type: "run_started" });
    assert.equal(event.type, "run_started");
    assert.equal((event as any).sessionId, "");
    assert.equal((event as any).prompt, "");
  });

  it("defaults turn_number to 0 when missing", () => {
    const event = parseAgentEvent({ type: "turn_started" });
    assert.equal((event as any).turnNumber, 0);
  });

  it("defaults tool_execution_completed duration_ms to 0", () => {
    const event = parseAgentEvent({ type: "tool_execution_completed" });
    assert.equal((event as any).durationMs, 0);
  });

  it("defaults tool_call_requested args to null when missing", () => {
    const event = parseAgentEvent({ type: "tool_call_requested" });
    assert.equal((event as any).args, null);
  });

  it("treats missing type as empty string → unknown event", () => {
    const raw = { foo: "bar" };
    const event = parseAgentEvent(raw);
    assert.equal(event.type, "");
    assert.deepEqual((event as any).data, raw);
  });
});

describe("local refusal SSE compatibility", () => {
  it("keeps unfamiliar hook reasons opaque and tool status unchanged", async () => {
    // This SDK has no receiving-refusal carrier. These are open-payload
    // compatibility probes, not a proposed receiver event or permission fact.
    const reasons = [
      { reason_code: "future_hook_cause", version: 99, details: { kind: "denied" } },
      { reason_code: 17, refusal: "denied" },
      ["denied", null],
      null,
    ];
    const hooks = reasons.map((reason) => ({
      type: "hook_launch_refused", hook_id: "host-hook", point: "pre_tool_execution",
      tool_use_id: "  opaque-call  ", reason, future_detail: { version: 99, values: [null, 1] },
    }));
    const result = '{"version":99,"cause":{"kind":"denied","future":[null,true]}}';
    const payloads = [
      ...hooks,
      { type: "tool_execution_completed", id: "  opaque-call  ", name: "read_file", is_error: true, duration_ms: 1 },
      { type: "tool_result_received", id: "  opaque-call  ", name: "read_file",
        content: [{ type: "text", text: result }], result, is_error: true, future_detail: { kind: "denied" } },
      { type: "run_completed", session_id: "session-opaque", result: "done" },
    ];
    async function* wire() {
      for (const [index, payload] of payloads.entries()) {
        yield { id: String(index), event: payload.type,
          data: JSON.stringify({ member_id: "agent-1", payload }) };
      }
    }
    const events: AgentEvent[] = [];
    for await (const event of new EventStream(wire(), parseMobEventFromSse)) events.push(event.event);
    for (const [index, hook] of hooks.entries()) {
      const event = events[index];
      assert.equal(event.type, "hook_launch_refused");
      assert.ok("data" in event);
      assert.deepEqual(event.data, hook);
    }
    const [tool, feedback, completed] = events.slice(hooks.length);
    assert.equal(tool.type, "tool_execution_completed");
    assert.ok("id" in tool && "isError" in tool);
    assert.equal("result" in tool, false);
    assert.deepEqual([tool.id, tool.isError], ["  opaque-call  ", true]);
    assert.equal(feedback.type, "tool_result_received");
    assert.ok("id" in feedback && "result" in feedback && "isError" in feedback);
    assert.deepEqual([feedback.id, feedback.result, feedback.isError], ["  opaque-call  ", result, true]);
    assert.ok(isRunCompleted(completed));
    assert.deepEqual([completed.sessionId, completed.result], ["session-opaque", "done"]);
    assert.equal(events.some(isRunFailed), false);
  });

  it("keeps call feedback and continues through subsequent packets", async () => {
    const refusal = '{"error":"operation_refused","message":"operation unavailable under current authorization"}';
    const hook = {
      type: "hook_launch_refused", hook_id: "host-hook", point: "pre_tool_execution",
      tool_use_id: "  mechanical-call  ",
      reason: { reason_code: "confinement_refused", refusal: "unsupported_platform" },
      future_detail: { owner: ["opaque", 1] },
    };
    // The host projector keeps native content on the result event and derives
    // `result` there; the execution fact for the same call carries no body.
    const payloads = [
      hook,
      { type: "tool_execution_completed", id: "  blocked-call  ",
        tool_call_id: "  blocked-call  ", name: "delete_file", is_error: true, duration_ms: 0 },
      { type: "tool_result_received", id: "  blocked-call  ", tool_call_id: "  blocked-call  ", name: "delete_file",
        content: [{ type: "text", text: refusal }], result: refusal, is_error: true },
      { type: "tool_execution_completed", id: "permitted-call", tool_call_id: "permitted-call",
        name: "read_file", is_error: false, duration_ms: 1 },
      { type: "tool_result_received", id: "permitted-call", tool_call_id: "permitted-call", name: "read_file",
        content: [{ type: "text", text: "permitted result" }], result: "permitted result", is_error: false },
      { type: "run_completed", session_id: "session-1", result: "done" },
    ];
    async function* wire() {
      for (const [index, payload] of payloads.entries()) {
        yield { id: String(index), event: payload.type,
          data: JSON.stringify({ member_id: "agent-1", source: "agent-1", payload }) };
      }
    }
    const events = [];
    for await (const event of new EventStream(wire(), parseMobEventFromSse)) events.push(event);
    assert.deepEqual(events.map(({ event }) => event.type), payloads.map(({ type }) => type));
    assert.ok(events.every(({ memberId }) => memberId === "agent-1"));
    const [unknown, blocked, blockedFeedback, permitted, permittedFeedback, completed] = events.map(({ event }) => event);
    assert.ok("data" in unknown);
    assert.deepEqual(unknown.data, hook);
    assert.equal(blocked.type, "tool_execution_completed");
    assert.ok("isError" in blocked && "id" in blocked);
    assert.deepEqual([blocked.id, blocked.isError], ["  blocked-call  ", true]);
    assert.ok("id" in blockedFeedback && "result" in blockedFeedback && "isError" in blockedFeedback);
    assert.deepEqual([blockedFeedback.id, blockedFeedback.result, blockedFeedback.isError], ["  blocked-call  ", refusal, true]);
    assert.ok("isError" in permitted && "id" in permitted);
    assert.deepEqual([permitted.id, permitted.isError], ["permitted-call", false]);
    assert.ok("id" in permittedFeedback && "result" in permittedFeedback && "isError" in permittedFeedback);
    assert.deepEqual([permittedFeedback.id, permittedFeedback.result, permittedFeedback.isError], ["permitted-call", "permitted result", false]);
    assert.ok(isRunCompleted(completed));
    assert.deepEqual([completed.sessionId, completed.result], ["session-1", "done"]);
  });
});

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

describe("type guards", () => {
  it("isTextDelta returns true for text_delta", () => {
    const event = parseAgentEvent({ type: "text_delta", delta: "x" });
    assert.equal(isTextDelta(event), true);
  });

  it("isTextDelta returns false for other types", () => {
    const event = parseAgentEvent({ type: "run_completed", session_id: "s", result: "r" });
    assert.equal(isTextDelta(event), false);
  });

  it("isTextComplete returns true for text_complete", () => {
    const event = parseAgentEvent({ type: "text_complete", content: "c" });
    assert.equal(isTextComplete(event), true);
  });

  it("isTextComplete returns false for other types", () => {
    const event = parseAgentEvent({ type: "text_delta", delta: "x" });
    assert.equal(isTextComplete(event), false);
  });

  it("isRunCompleted returns true for run_completed", () => {
    const event = parseAgentEvent({ type: "run_completed", session_id: "s", result: "r" });
    assert.equal(isRunCompleted(event), true);
  });

  it("isRunCompleted returns false for run_started", () => {
    const event = parseAgentEvent({ type: "run_started", session_id: "s", prompt: "p" });
    assert.equal(isRunCompleted(event), false);
  });

  it("isRunFailed returns true for run_failed", () => {
    const event = parseAgentEvent({ type: "run_failed", session_id: "s", error: "e" });
    assert.equal(isRunFailed(event), true);
  });

  it("isRunFailed returns false for run_completed", () => {
    const event = parseAgentEvent({ type: "run_completed", session_id: "s", result: "r" });
    assert.equal(isRunFailed(event), false);
  });

  it("isTurnCompleted returns true for turn_completed", () => {
    const event = parseAgentEvent({ type: "turn_completed", stop_reason: "end" });
    assert.equal(isTurnCompleted(event), true);
  });

  it("isTurnCompleted returns false for turn_started", () => {
    const event = parseAgentEvent({ type: "turn_started", turn_number: 1 });
    assert.equal(isTurnCompleted(event), false);
  });

  it("isToolCallRequested returns true for tool_call_requested", () => {
    const event = parseAgentEvent({ type: "tool_call_requested", id: "1", name: "t", args: {} });
    assert.equal(isToolCallRequested(event), true);
  });

  it("isToolCallRequested returns false for tool_result_received", () => {
    const event = parseAgentEvent({ type: "tool_result_received", id: "1", name: "t", is_error: false });
    assert.equal(isToolCallRequested(event), false);
  });
});

// ---------------------------------------------------------------------------
// parseMobEventFromSse
// ---------------------------------------------------------------------------

describe("parseMobEventFromSse", () => {
  it("parses an attributed mob event from SSE", () => {
    const sse = {
      id: null,
      event: "message",
      data: JSON.stringify({
        member_id: "agent-1",
        timestamp_ms: 1700000000000,
        payload: { type: "text_delta", delta: "hello" },
      }),
    };
    const envelope = parseMobEventFromSse(sse);
    assert.equal(envelope.memberId, "agent-1");
    assert.equal(envelope.timestampMs, 1700000000000);
    assert.equal(envelope.event.type, "text_delta");
    assert.equal((envelope.event as any).delta, "hello");
  });

  it("falls back to source when member_id is absent", () => {
    const sse = {
      id: null,
      event: "message",
      data: JSON.stringify({
        source: "agent-2",
        payload: { type: "run_completed", session_id: "s", result: "ok" },
      }),
    };
    const envelope = parseMobEventFromSse(sse);
    assert.equal(envelope.memberId, "agent-2");
  });

  it("uses raw as payload if payload is not an object", () => {
    const sse = {
      id: null,
      event: "message",
      data: JSON.stringify({
        member_id: "a-3",
        type: "text_delta",
        delta: "hi",
      }),
    };
    const envelope = parseMobEventFromSse(sse);
    assert.equal(envelope.event.type, "text_delta");
    assert.equal((envelope.event as any).delta, "hi");
  });

  it("handles invalid JSON gracefully", () => {
    const sse = { id: null, event: "message", data: "not json" };
    const envelope = parseMobEventFromSse(sse);
    assert.equal(envelope.memberId, "");
    assert.equal(envelope.timestampMs, 0);
    // The event is an unknown event since raw is {}
    assert.equal(envelope.event.type, "");
  });

  it("defaults timestampMs to 0 when absent", () => {
    const sse = {
      id: null,
      event: "message",
      data: JSON.stringify({
        member_id: "a-4",
        payload: { type: "turn_started", turn_number: 1 },
      }),
    };
    const envelope = parseMobEventFromSse(sse);
    assert.equal(envelope.timestampMs, 0);
  });
});

// ---------------------------------------------------------------------------
// parseAgentEventFromSse
// ---------------------------------------------------------------------------

describe("parseAgentEventFromSse", () => {
  it("parses a per-agent SSE event", () => {
    const sse = {
      id: "evt-1",
      event: "text_delta",
      data: JSON.stringify({ type: "text_delta", delta: "chunk" }),
    };
    const result = parseAgentEventFromSse(sse);
    assert.equal(result.eventType, "text_delta");
    assert.equal(result.event.type, "text_delta");
    assert.equal((result.event as any).delta, "chunk");
  });

  it("preserves SSE event field as eventType", () => {
    const sse = {
      id: null,
      event: "custom_sse_event",
      data: JSON.stringify({ type: "run_started", session_id: "s1", prompt: "p" }),
    };
    const result = parseAgentEventFromSse(sse);
    assert.equal(result.eventType, "custom_sse_event");
    assert.equal(result.event.type, "run_started");
  });

  it("handles invalid JSON gracefully", () => {
    const sse = { id: null, event: "message", data: "broken" };
    const result = parseAgentEventFromSse(sse);
    assert.equal(result.eventType, "message");
    assert.equal(result.event.type, "");
  });
});
