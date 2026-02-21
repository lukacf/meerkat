/**
 * Conformance tests for Meerkat TypeScript SDK types and events.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  CONTRACT_VERSION,
  MeerkatError,
  CapabilityUnavailableError,
  SessionNotFoundError,
  SkillNotFoundError,
  parseEvent,
  isTextDelta,
  isRunCompleted,
  isTurnCompleted,
  MeerkatClient,
} from "../src/index";
import type {
  AgentEvent,
  TextDeltaEvent,
  TurnCompletedEvent,
  RunCompletedEvent,
  ToolCallRequestedEvent,
  BudgetWarningEvent,
  CompactionStartedEvent,
  RetryingEvent,
  UnknownEvent,
  RunResult,
} from "../src/index";

// ---------------------------------------------------------------------------
// Contract version
// ---------------------------------------------------------------------------

describe("Contract Version", () => {
  it("should be semver format", () => {
    const parts = CONTRACT_VERSION.split(".");
    assert.equal(parts.length, 3);
    for (const part of parts) {
      assert.ok(!isNaN(Number(part)));
    }
  });
});

// ---------------------------------------------------------------------------
// Error hierarchy
// ---------------------------------------------------------------------------

describe("Error Types", () => {
  it("MeerkatError should capture code and message", () => {
    const err = new MeerkatError("TEST_CODE", "test message");
    assert.equal(err.code, "TEST_CODE");
    assert.equal(err.message, "test message");
    assert.ok(err instanceof Error);
  });

  it("CapabilityUnavailableError should extend MeerkatError", () => {
    const err = new CapabilityUnavailableError("CAP_UNAVAIL", "not available");
    assert.ok(err instanceof MeerkatError);
    assert.equal(err.name, "CapabilityUnavailableError");
  });

  it("SessionNotFoundError should extend MeerkatError", () => {
    const err = new SessionNotFoundError("NOT_FOUND", "session gone");
    assert.ok(err instanceof MeerkatError);
    assert.equal(err.name, "SessionNotFoundError");
  });

  it("SkillNotFoundError should extend MeerkatError", () => {
    const err = new SkillNotFoundError("SKILL_NOT_FOUND", "no such skill");
    assert.ok(err instanceof MeerkatError);
    assert.equal(err.name, "SkillNotFoundError");
  });

  it("MeerkatError should support capability hints", () => {
    const err = new MeerkatError("CAP", "msg", undefined, {
      capability_id: "comms",
      message: "build with --features comms",
    });
    assert.ok(err.capabilityHint);
    assert.equal(err.capabilityHint!.capability_id, "comms");
  });
});

// ---------------------------------------------------------------------------
// Typed events — parseEvent
// ---------------------------------------------------------------------------

describe("Typed Events", () => {
  it("should parse text_delta with camelCase fields", () => {
    const event = parseEvent({ type: "text_delta", delta: "Hello" });
    assert.equal(event.type, "text_delta");
    if (isTextDelta(event)) {
      assert.equal(event.delta, "Hello");
    } else {
      assert.fail("Expected TextDeltaEvent");
    }
  });

  it("should parse turn_started", () => {
    const event = parseEvent({ type: "turn_started", turn_number: 3 });
    assert.equal(event.type, "turn_started");
    if (event.type === "turn_started") {
      assert.equal(event.turnNumber, 3);
    }
  });

  it("should parse turn_completed with usage in camelCase", () => {
    const event = parseEvent({
      type: "turn_completed",
      stop_reason: "end_turn",
      usage: { input_tokens: 50, output_tokens: 20 },
    });
    if (isTurnCompleted(event)) {
      assert.equal(event.stopReason, "end_turn");
      assert.equal(event.usage.inputTokens, 50);
      assert.equal(event.usage.outputTokens, 20);
      assert.equal(event.usage.cacheCreationTokens, undefined);
    } else {
      assert.fail("Expected TurnCompletedEvent");
    }
  });

  it("should parse run_completed", () => {
    const event = parseEvent({
      type: "run_completed",
      session_id: "abc-123",
      result: "Done!",
      usage: { input_tokens: 100, output_tokens: 50 },
    });
    if (isRunCompleted(event)) {
      assert.equal(event.sessionId, "abc-123");
      assert.equal(event.result, "Done!");
      assert.equal(event.usage.inputTokens, 100);
    } else {
      assert.fail("Expected RunCompletedEvent");
    }
  });

  it("should parse tool_call_requested", () => {
    const event = parseEvent({
      type: "tool_call_requested",
      id: "t1",
      name: "search",
      args: { query: "rust" },
    });
    assert.equal(event.type, "tool_call_requested");
    if (event.type === "tool_call_requested") {
      assert.equal(event.name, "search");
      assert.deepEqual(event.args, { query: "rust" });
    }
  });

  it("should parse tool_execution_completed", () => {
    const event = parseEvent({
      type: "tool_execution_completed",
      id: "t1",
      name: "search",
      result: "found it",
      is_error: false,
      duration_ms: 42,
    });
    assert.equal(event.type, "tool_execution_completed");
    if (event.type === "tool_execution_completed") {
      assert.equal(event.name, "search");
      assert.equal(event.durationMs, 42);
      assert.equal(event.isError, false);
    }
  });

  it("should parse budget_warning with camelCase", () => {
    const event = parseEvent({
      type: "budget_warning",
      budget_type: "tokens",
      used: 900,
      limit: 1000,
      percent: 90.0,
    });
    assert.equal(event.type, "budget_warning");
    if (event.type === "budget_warning") {
      assert.equal(event.budgetType, "tokens");
      assert.equal(event.percent, 90.0);
    }
  });

  it("should parse compaction_started", () => {
    const event = parseEvent({
      type: "compaction_started",
      input_tokens: 5000,
      estimated_history_tokens: 4000,
      message_count: 12,
    });
    assert.equal(event.type, "compaction_started");
    if (event.type === "compaction_started") {
      assert.equal(event.inputTokens, 5000);
      assert.equal(event.messageCount, 12);
    }
  });

  it("should parse retrying", () => {
    const event = parseEvent({
      type: "retrying",
      attempt: 2,
      max_attempts: 3,
      error: "rate limit",
      delay_ms: 2000,
    });
    assert.equal(event.type, "retrying");
    if (event.type === "retrying") {
      assert.equal(event.attempt, 2);
      assert.equal(event.delayMs, 2000);
      assert.equal(event.maxAttempts, 3);
    }
  });

  it("should return UnknownEvent for unrecognised types", () => {
    const event = parseEvent({ type: "future_event_v2", data: "something" });
    // Unknown events have the raw type string
    assert.equal(event.type, "future_event_v2");
  });

  it("should handle missing type field", () => {
    const event = parseEvent({ delta: "oops" });
    assert.equal(event.type, "");
  });
});

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

describe("Type Guards", () => {
  it("isTextDelta narrows type", () => {
    const event: AgentEvent = parseEvent({ type: "text_delta", delta: "hi" });
    if (isTextDelta(event)) {
      // TypeScript should narrow: event.delta is accessible
      assert.equal(event.delta, "hi");
    } else {
      assert.fail("isTextDelta should return true");
    }
  });

  it("isTextDelta rejects non-text events", () => {
    const event = parseEvent({ type: "turn_started", turn_number: 1 });
    assert.equal(isTextDelta(event), false);
  });
});

// ---------------------------------------------------------------------------
// RunResult parsing (camelCase)
// ---------------------------------------------------------------------------

describe("RunResult parsing", () => {
  it("should convert wire format to camelCase", () => {
    const raw = {
      session_id: "s1",
      session_ref: "my-ref",
      text: "Hello!",
      turns: 2,
      tool_calls: 1,
      usage: {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
      },
      structured_output: { answer: 42 },
      schema_warnings: [{ provider: "openai", path: "$.foo", message: "bad" }],
    };
    const result: RunResult = MeerkatClient.parseRunResult(raw);
    assert.equal(result.sessionId, "s1");
    assert.equal(result.sessionRef, "my-ref");
    assert.equal(result.text, "Hello!");
    assert.equal(result.turns, 2);
    assert.equal(result.toolCalls, 1);
    assert.equal(result.usage.inputTokens, 100);
    assert.equal(result.usage.outputTokens, 50);
    assert.equal(result.usage.cacheCreationTokens, 10);
    assert.deepEqual(result.structuredOutput, { answer: 42 });
    assert.equal(result.schemaWarnings?.length, 1);
    assert.equal(result.schemaWarnings?.[0].provider, "openai");
  });
});
