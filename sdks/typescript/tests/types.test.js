/**
 * Conformance tests for Meerkat TypeScript SDK types and events.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import fs from "node:fs";
import packageJson from "../package.json" with { type: "json" };
import {
  CONTRACT_VERSION,
  MeerkatError,
  CapabilityUnavailableError,
  SessionNotFoundError,
  SkillNotFoundError,
  parseEvent,
  isTextDelta,
  isRunCompleted,
  isExtractionSucceeded,
  isExtractionFailed,
  isTurnCompleted,
  MeerkatClient,
  Mob,
  Session,
  DeferredSession,
  liveWebrtcMediaConstraints,
  parseWorkItem,
} from "../dist/index.js";

/**
 * Build an opaque wire `member_ref` token using the same shape the server
 * emits (``base64url({"m": mob_id, "a": agent_identity})``).
 *
 * The TypeScript SDK only forwards `member_ref` opaquely — it never decodes
 * the token — so any non-empty string would satisfy the parser. Mirroring
 * the server-side encoding here keeps fixtures realistic and leaves the
 * door open for future round-trip decode assertions without churn.
 */
function makeMemberRef(mobId, agentIdentity) {
  const payload = JSON.stringify({ m: mobId, a: agentIdentity });
  return Buffer.from(payload, "utf-8")
    .toString("base64")
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

// ---------------------------------------------------------------------------
// Contract version
// ---------------------------------------------------------------------------

describe("Contract Version", () => {
  it("should be semver format", () => {
    const core = CONTRACT_VERSION.split("-", 1)[0].split("+", 1)[0];
    const parts = core.split(".");
    assert.equal(parts.length, 3);
    for (const part of parts) {
      assert.ok(!isNaN(Number(part)));
    }
  });

  it("matches the published package version", () => {
    assert.equal(CONTRACT_VERSION, packageJson.version);
  });

  // J56: realtime channel-framing types (RealtimeOpenInfo / RealtimeProtocolVersion /
  // RealtimeChannelOpenFrame) deleted as part of the live-adapter-mvp J55 sweep.
  // The corresponding generated-type inventory test was removed.
  it("generated wire types include RuntimeStateResult", () => {
    const generated = fs.readFileSync(
      new URL("../src/generated/types.ts", import.meta.url),
      "utf8",
    );

    assert.match(generated, /export interface RuntimeStateResult \{\n  state: WireRuntimeState;\n\}/);
  });

  it("live WebRTC browser helper requests AEC noise suppression and AGC", () => {
    const constraints = liveWebrtcMediaConstraints();
    assert.deepEqual(constraints, {
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
      video: false,
    });
  });
});

describe("Transcript Rewrite Serialization", () => {
  it("serializes edited parsed messages over their preserved raw payload", () => {
    const parsed = MeerkatClient.parseSessionMessage({
      role: "user",
      content: "old",
      created_at: "2026-05-26T10:00:00Z",
      interaction_id: "019405c8-1234-7000-8000-000000000001",
      run_id: "019405c8-1234-7000-8000-000000000002",
      provider_trace_id: "trace-1",
    });
    assert.equal(parsed.interactionId, "019405c8-1234-7000-8000-000000000001");
    assert.equal(parsed.runId, "019405c8-1234-7000-8000-000000000002");

    const serialized = MeerkatClient.serializeTranscriptRewriteMessage({
      ...parsed,
      content: "new",
    });

    assert.deepEqual(serialized, {
      role: "user",
      content: "new",
      created_at: "2026-05-26T10:00:00Z",
      interaction_id: "019405c8-1234-7000-8000-000000000001",
      run_id: "019405c8-1234-7000-8000-000000000002",
      provider_trace_id: "trace-1",
    });
  });

  it("serializes parsed block assistant messages back to wire-shaped blocks", () => {
    const parsed = MeerkatClient.parseSessionMessage({
      role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
      created_at: "2026-05-26T10:00:00Z",
      blocks: [
        {
          block_type: "text",
          data: {
            text: "old",
            source: "spoken",
          },
          provider_block_id: "block-1",
        },
      ],
    });

    const serialized = MeerkatClient.serializeTranscriptRewriteMessage({
      ...parsed,
      blocks: [{ ...parsed.blocks[0], text: "new" }],
    });

    assert.deepEqual(serialized, {
      role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
      created_at: "2026-05-26T10:00:00Z",
      blocks: [
        {
          block_type: "text",
          data: {
            text: "new",
            source: "spoken",
          },
          provider_block_id: "block-1",
        },
      ],
    });
  });

  it("serializes parsed system_notice messages with a single body alias", () => {
    const parsed = MeerkatClient.parseSessionMessage({
      role: "system_notice",
      kind: "background_job",
      body: "done",
      created_at: "2026-05-26T10:00:00Z",
    });

    const serialized = MeerkatClient.serializeTranscriptRewriteMessage(parsed);

    assert.deepEqual(serialized, {
      role: "system_notice",
      kind: "background_job",
      body: "done",
      created_at: "2026-05-26T10:00:00Z",
    });
    assert.equal(Object.hasOwn(serialized, "content"), false);
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
    assert.equal(err.capabilityHint.capability_id, "comms");
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

  it("should parse interaction_complete structured output", () => {
    const event = parseEvent({
      type: "interaction_complete",
      interaction_id: "i1",
      result: "{\"answer\":42}",
      structured_output: { answer: 42 },
    });
    assert.equal(event.type, "interaction_complete");
    if (event.type === "interaction_complete") {
      assert.equal(event.interactionId, "i1");
      assert.deepEqual(event.structuredOutput, { answer: 42 });
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

  it("preserves run_failed typed terminal cause report", () => {
    const event = parseEvent({
      type: "run_failed",
      session_id: "s1",
      error_class: "terminal",
      error: "display text changed by caller",
      error_report: {
        class: "llm",
        message: "machine terminalized LLM failure",
        reason: {
          reason_type: "turn_terminal_cause",
          outcome: "failed",
          cause_kind: "llm_failure",
        },
      },
    });

    assert.equal(event.type, "run_failed");
    if (event.type === "run_failed") {
      assert.equal(event.error, "display text changed by caller");
      assert.equal(event.errorReport?.class, "llm");
      assert.equal(event.errorReport?.message, "machine terminalized LLM failure");
      assert.equal(event.errorReport?.reason?.reasonType, "turn_terminal_cause");
      if (event.errorReport?.reason?.reasonType === "turn_terminal_cause") {
        assert.equal(event.errorReport.reason.outcome, "failed");
        assert.equal(event.errorReport.reason.causeKind, "llm_failure");
      }
    }
  });

  it("does not infer run_failed terminal cause from display fields", () => {
    const event = parseEvent({
      type: "run_failed",
      session_id: "s1",
      error_class: "llm",
      error: "LLM failure terminal turn",
      error_report: {
        class: "llm",
        message: "LLM failure terminal turn",
        reason: {
          reason_type: "turn_terminal_cause",
          outcome: "failed",
          cause_kind: "not_a_machine_cause",
        },
      },
    });

    assert.equal(event.type, "run_failed");
    if (event.type === "run_failed") {
      assert.equal(event.errorReport?.reason, undefined);
    }
  });

  it("preserves malformed known event payloads instead of fabricating semantics", () => {
    const cases = [
      {
        raw: { type: "turn_completed", usage: { input_tokens: 50, output_tokens: 20 } },
        reason: "missing stop_reason must not become end_turn",
      },
      {
        raw: {
          type: "turn_completed",
          stop_reason: "end_turn",
          usage: { input_tokens: "oops", output_tokens: 20 },
        },
        reason: "malformed usage must not become token counts",
      },
      {
        raw: {
          type: "tool_config_changed",
          payload: {
            operation: "add",
            target: "filesystem",
            status: "staged",
            persisted: "false",
          },
        },
        reason: "non-boolean persisted must not become false",
      },
      {
        raw: {
          type: "tool_config_changed",
          payload: {
            operation: "bogus",
            target: 0,
            status: 0,
            persisted: "false",
          },
        },
        reason: "malformed operation/status semantics must not become an empty typed payload",
      },
      {
        raw: {
          type: "tool_call_requested",
          id: "t1",
          name: "search",
          args: "{\"query\":\"rust\"}",
        },
        reason: "tool-call args string JSON must not become semantic args",
      },
    ];

    for (const { raw, reason } of cases) {
      const event = parseEvent(raw);
      assert.equal(event.type, "malformed_event", reason);
      assert.equal(event.rawType, raw.type);
      assert.deepEqual(event.raw, raw);
    }
  });

  it("should parse run_completed", () => {
    const event = parseEvent({
      type: "run_completed",
      session_id: "abc-123",
      result: "Done!",
      structured_output: { answer: 42 },
      usage: { input_tokens: 100, output_tokens: 50 },
      extraction_required: true,
    });
    if (isRunCompleted(event)) {
      assert.equal(event.sessionId, "abc-123");
      assert.equal(event.result, "Done!");
      assert.deepEqual(event.structuredOutput, { answer: 42 });
      assert.equal(event.extractionRequired, true);
      assert.equal(event.usage.inputTokens, 100);
    } else {
      assert.fail("Expected RunCompletedEvent");
    }
  });

  it("should parse extraction terminal events", () => {
    const succeeded = parseEvent({
      type: "extraction_succeeded",
      session_id: "abc-123",
      structured_output: { answer: 42 },
      schema_warnings: [{ provider: "openai", path: "$", message: "warn" }],
    });
    assert.equal(isExtractionSucceeded(succeeded), true);
    if (isExtractionSucceeded(succeeded)) {
      assert.equal(succeeded.sessionId, "abc-123");
      assert.deepEqual(succeeded.structuredOutput, { answer: 42 });
      assert.equal(succeeded.schemaWarnings?.[0].provider, "openai");
    }

    const failed = parseEvent({
      type: "extraction_failed",
      session_id: "abc-123",
      last_output: "main answer",
      attempts: 2,
      reason: "Invalid JSON",
    });
    assert.equal(isExtractionFailed(failed), true);
    if (isExtractionFailed(failed)) {
      assert.equal(failed.lastOutput, "main answer");
      assert.equal(failed.attempts, 2);
      assert.equal(failed.reason, "Invalid JSON");
    }
  });

  it("preserves hook-denied error_report on run_failed", () => {
    const event = parseEvent({
      type: "run_failed",
      session_id: "session-1",
      error_class: "hook",
      error: "denied",
      error_report: {
        class: "hook",
        message: "denied",
        reason: {
          reason_type: "hook_denied",
          hook_id: "policy-gate",
          point: "pre_tool_execution",
          reason_code: "policy",
        },
      },
    });

    assert.equal(event.type, "run_failed");
    if (event.type === "run_failed") {
      assert.equal(event.errorReport?.class, "hook");
      assert.equal(event.errorReport?.reason?.reason_type, "hook_denied");
      assert.equal(event.errorReport?.reason?.hook_id, "policy-gate");
    }
  });

  it("does not promote string-only hook id mirrors on run_failed error_report", () => {
    const stringOnly = parseEvent({
      type: "run_failed",
      session_id: "session-1",
      error_class: "hook",
      error: "denied",
      error_report: {
        class: "hook",
        message: "denied",
        reason: {
          reason_type: "hook_denied",
          hook_id_string: "legacy-policy-gate",
          point: "pre_tool_execution",
          reason_code: "policy",
        },
      },
    });

    assert.equal(stringOnly.type, "run_failed");
    if (stringOnly.type === "run_failed") {
      assert.equal(stringOnly.errorReport?.reason?.hook_id, undefined);
      assert.equal(
        Object.hasOwn(stringOnly.errorReport?.reason ?? {}, "hook_id_string"),
        false,
      );
    }

    const malformedHookId = parseEvent({
      type: "run_failed",
      session_id: "session-1",
      error_class: "hook",
      error: "denied",
      error_report: {
        class: "hook",
        message: "denied",
        reason: {
          reason_type: "hook_denied",
          hook_id: { value: "policy-gate" },
          point: "pre_tool_execution",
          reason_code: "policy",
        },
      },
    });

    assert.equal(malformedHookId.type, "malformed_event");
    assert.equal(malformedHookId.rawType, "run_failed");

    const timeoutStringMirror = parseEvent({
      type: "run_failed",
      session_id: "session-1",
      error_class: "hook",
      error: "timeout",
      error_report: {
        class: "hook",
        message: "timeout",
        reason: {
          reason_type: "hook_timeout",
          hook_id_string: "legacy-policy-gate",
          timeout_ms: 100,
        },
      },
    });

    assert.equal(timeoutStringMirror.type, "malformed_event");
    assert.equal(timeoutStringMirror.rawType, "run_failed");
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
      content: [
        { type: "text", text: "found it" },
        { type: "image", media_type: "image/png", source: "inline", data: "AAAA" },
      ],
      is_error: false,
      duration_ms: 42,
    });
    assert.equal(event.type, "tool_execution_completed");
    if (event.type === "tool_execution_completed") {
      assert.equal(event.name, "search");
      assert.equal(event.durationMs, 42);
      assert.equal(event.isError, false);
      assert.deepEqual(event.content, [
        { type: "text", text: "found it" },
        { type: "image", media_type: "image/png", source: "inline", data: "AAAA" },
      ]);
    }
  });

  it("should parse tool_result_received content blocks", () => {
    const event = parseEvent({
      type: "tool_result_received",
      id: "t1",
      name: "search",
      content: [{ type: "text", text: "found it" }],
      is_error: false,
    });
    assert.equal(event.type, "tool_result_received");
    if (event.type === "tool_result_received") {
      assert.deepEqual(event.content, [{ type: "text", text: "found it" }]);
      assert.equal(event.isError, false);
    }
  });

  it("should preserve malformed tool result content blocks", () => {
    const event = parseEvent({
      type: "tool_execution_completed",
      id: "t1",
      name: "search",
      result: "found it",
      content: "not blocks",
      is_error: false,
      duration_ms: 42,
    });

    assert.equal(event.type, "malformed_event");
    if (event.type === "malformed_event") {
      assert.equal(event.error, "content must be a content block array");
    }
  });

  it("should not coerce missing or malformed tool is_error", () => {
    const missing = parseEvent({
      type: "tool_execution_completed",
      id: "t1",
      name: "search",
      content: [{ type: "text", text: "found it" }],
    });
    const malformed = parseEvent({
      type: "tool_execution_completed",
      id: "t1",
      name: "search",
      content: [{ type: "text", text: "found it" }],
      is_error: "false",
    });

    assert.equal(missing.type, "tool_execution_completed");
    assert.equal(malformed.type, "tool_execution_completed");
    if (missing.type === "tool_execution_completed" && malformed.type === "tool_execution_completed") {
      assert.equal(missing.isError, undefined);
      assert.equal(malformed.isError, undefined);
      assert.equal(missing.durationMs, undefined);
      assert.equal(malformed.durationMs, undefined);
      assert.deepEqual(missing.content, [{ type: "text", text: "found it" }]);
      assert.deepEqual(malformed.content, [{ type: "text", text: "found it" }]);
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

  it("should parse skills_resolved with typed skill identities", () => {
    const sourceUuid = "00000000-0000-4b11-8111-000000000001";
    const event = parseEvent({
      type: "skills_resolved",
      skills: [{ source_uuid: sourceUuid, skill_name: "email-extractor" }],
      injection_bytes: 128,
    });

    assert.equal(event.type, "skills_resolved");
    if (event.type === "skills_resolved") {
      assert.deepEqual(event.skills, [
        { sourceUuid, skillName: "email-extractor" },
      ]);
      assert.equal(event.injectionBytes, 128);
    }
  });

  it("should reject legacy string-only skills_resolved payloads", () => {
    const event = parseEvent({
      type: "skills_resolved",
      skills: ["legacy/ref"],
      injection_bytes: 128,
    });

    assert.equal(event.type, "malformed_event");
    if (event.type === "malformed_event") {
      assert.equal(event.rawType, "skills_resolved");
      assert.match(event.error, /SkillKey/);
    }
  });

  it("should reject skills_resolved missing typed skill identity fields", () => {
    const event = parseEvent({
      type: "skills_resolved",
      skills: [{ source_uuid: "00000000-0000-4b11-8111-000000000001" }],
      injection_bytes: 128,
    });

    assert.equal(event.type, "malformed_event");
    if (event.type === "malformed_event") {
      assert.equal(event.rawType, "skills_resolved");
      assert.match(event.error, /SkillKey/);
    }
  });

  it("should parse skill_resolution_failed with typed key and reason", () => {
    const sourceUuid = "00000000-0000-4b11-8111-000000000001";
    const event = parseEvent({
      type: "skill_resolution_failed",
      skill_key: { source_uuid: sourceUuid, skill_name: "email-extractor" },
      reason: {
        reason_type: "not_found",
        key: { source_uuid: sourceUuid, skill_name: "email-extractor" },
      },
    });

    assert.equal(event.type, "skill_resolution_failed");
    if (event.type === "skill_resolution_failed") {
      assert.deepEqual(event.skillKey, {
        sourceUuid,
        skillName: "email-extractor",
      });
      assert.equal(event.reason.reasonType, "not_found");
      if (event.reason.reasonType === "not_found") {
        assert.deepEqual(event.reason.key, {
          sourceUuid,
          skillName: "email-extractor",
        });
      }
      assert.equal("reference" in event, false);
      assert.equal("error" in event, false);
    }
  });

  it("should parse legacy skill_resolution_failed payloads", () => {
    const event = parseEvent({
      type: "skill_resolution_failed",
      reference: "legacy/ref",
      error: "missing",
    });

    assert.equal(event.type, "skill_resolution_failed");
    if (event.type === "skill_resolution_failed") {
      assert.equal(event.skillKey, undefined);
      assert.equal(event.reason, undefined);
      assert.equal("reference" in event, false);
      assert.equal("error" in event, false);
    }
  });

  it("should not fabricate skill resolution reasons from malformed semantics", () => {
    const event = parseEvent({
      type: "skill_resolution_failed",
      reason: { reason_type: 0, message: "bad" },
      reference: "legacy/ref",
      error: "missing",
    });

    assert.equal(event.type, "skill_resolution_failed");
    if (event.type === "skill_resolution_failed") {
      assert.equal(event.reason, undefined);
    }
  });

  it("should preserve unknown skill resolution reason types as typed unknown", () => {
    const event = parseEvent({
      type: "skill_resolution_failed",
      reason: { reason_type: "future_status", message: "future details" },
      reference: "legacy/ref",
      error: "missing",
    });

    assert.equal(event.type, "skill_resolution_failed");
    if (event.type === "skill_resolution_failed") {
      assert.equal(event.skillKey, undefined);
      assert.deepEqual(event.reason, {
        reasonType: "unknown",
        message: "future details",
        rawReasonType: "future_status",
      });
    }
  });

  it("should not fabricate empty skill keys for keyed failure reasons", () => {
    const event = parseEvent({
      type: "skill_resolution_failed",
      reason: { reason_type: "not_found" },
      reference: "legacy/ref",
      error: "missing",
    });

    assert.equal(event.type, "skill_resolution_failed");
    if (event.type === "skill_resolution_failed") {
      assert.equal(event.skillKey, undefined);
      assert.equal(event.reason, undefined);
    }
  });

  it("fails closed (throws) on an unrecognised type not in the schema inventory", () => {
    // An event whose `type` is not in the generated KNOWN_AGENT_EVENT_TYPES
    // inventory is a contract violation for a version-matched client and must
    // be rejected with a typed error, not laundered into a fabricated
    // UnknownEvent. (Version skew is handled separately.)
    assert.throws(
      () => parseEvent({ type: "future_event_v2", data: "something" }),
      (err) => err instanceof MeerkatError && err.code === "UNKNOWN_EVENT_TYPE",
    );
  });

  it("preserves a frame missing the type discriminant as malformed_event", () => {
    // A frame with no `type` is a malformed wire shape — preserved as a
    // `malformed_event` frame (not fabricated into a typed event, and not a hard
    // throw that would crash a streaming consumer on a control/keepalive frame).
    const event = parseEvent({ delta: "oops" });
    assert.equal(event.type, "malformed_event");
  });

  it("should parse scoped wrapper events", () => {
    const event = parseEvent({
      scope_id: "mob:writer",
      scope_path: [
        { scope: "mob_member", flow_run_id: "run_1", agent_identity: "writer", agent_runtime_id: "writer:1", fence_token: 1 },
      ],
      event: { type: "text_delta", delta: "hello" },
    });
    assert.equal(event.type, "scoped_agent_event");
    if (event.type === "scoped_agent_event") {
      assert.equal(event.scopeId, "mob:writer");
      assert.equal(event.event.type, "text_delta");
      assert.equal(event.scopePath[0].scope, "mob_member");
      if (event.scopePath[0].scope === "mob_member") {
        assert.equal(event.scopePath[0].agent_identity, "writer");
        assert.equal(event.scopePath[0].agent_runtime_id, undefined);
        assert.equal(event.scopePath[0].fence_token, undefined);
      }
    }
  });

  it("should parse tool_config_changed payload", () => {
    const event = parseEvent({
      type: "tool_config_changed",
      payload: {
        operation: "remove",
        target: "filesystem",
        status_info: {
          kind: "boundary_applied",
          base_changed: true,
          visible_changed: false,
          revision: 9,
        },
        persisted: false,
        applied_at_turn: 7,
      },
    });
    assert.equal(event.type, "tool_config_changed");
    if (event.type === "tool_config_changed") {
      assert.equal(event.payload.operation, "remove");
      assert.equal(event.payload.target, "filesystem");
      assert.deepEqual(event.payload.status_info, {
        kind: "boundary_applied",
        base_changed: true,
        visible_changed: false,
        revision: 9,
      });
      assert.equal(event.payload.persisted, false);
      assert.equal(event.payload.applied_at_turn, 7);
    }
  });

  it("should parse session history payloads", () => {
    const history = MeerkatClient.parseSessionHistory({
      session_id: "s1",
      session_ref: "ref-1",
      message_count: 3,
      offset: 1,
      limit: 2,
      has_more: true,
      messages: [
        { role: "system", content: "rules", created_at: "2026-01-01T00:00:00Z" },
        {
          role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
          blocks: [
            {
              block_type: "tool_use",
              data: { id: "tc_1", name: "search", args: { q: "rust" } },
            },
          ],
          stop_reason: "tool_use",
        },
        {
          role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
          blocks: [
            {
              block_type: "tool_use",
              data: { id: "tc_2", name: "lookup", args: { item: "history" } },
            },
          ],
          stop_reason: "end_turn",
        },
      ],
    });

    assert.equal(history.sessionId, "s1");
    assert.equal(history.sessionRef, "ref-1");
    assert.equal(history.limit, 2);
    assert.equal(history.hasMore, true);
    assert.equal(history.messages[1].blocks[0].name, "search");
    assert.equal(history.messages[2].blocks[0].name, "lookup");
  });

  it("should preserve assistant image blocks from session history", () => {
    const history = MeerkatClient.parseSessionHistory({
      session_id: "s1",
      message_count: 1,
      offset: 0,
      has_more: false,
      messages: [
        {
          role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
          blocks: [
            {
              block_type: "image",
              data: {
                image_id: "img_1",
                blob_ref: { blob_id: "blob_1", media_type: "image/png" },
                media_type: "image/png",
                width: 1024,
                height: 1536,
                revised_prompt: { disposition: "not_requested" },
                meta: { provider: "open_ai", target_model: "gpt-image-1" },
              },
            },
          ],
        },
      ],
    });

    const block = history.messages[0].blocks[0];
    assert.equal(block.blockType, "image");
    assert.equal(block.imageId, "img_1");
    assert.equal(block.blobId, "blob_1");
    assert.equal(block.mediaType, "image/png");
    assert.equal(block.width, 1024);
    assert.equal(block.height, 1536);
    assert.deepEqual(block.revisedPrompt, { disposition: "not_requested" });
    assert.deepEqual(block.meta, { provider: "open_ai", target_model: "gpt-image-1" });
  });

  it("should parse inline video content blocks from history payloads", () => {
    const history = MeerkatClient.parseSessionHistory({
      session_id: "s1",
      message_count: 1,
      offset: 0,
      has_more: false,
      messages: [
        {
          role: "user",
          created_at: "2026-01-01T00:00:00Z",
          content: [
            {
              type: "video",
              media_type: "video/mp4",
              duration_ms: 12000,
              source: "inline",
              data: "AAAA",
            },
          ],
        },
      ],
    });

    assert.equal(history.messages[0].content[0].type, "video");
    assert.equal(history.messages[0].content[0].media_type, "video/mp4");
    assert.equal(history.messages[0].content[0].duration_ms, 12000);
    assert.equal(history.messages[0].content[0].data, "AAAA");
  });

  it("Session.history should delegate to readSessionHistory", async () => {
    const expected = {
      sessionId: "s1",
      messageCount: 0,
      offset: 0,
      limit: undefined,
      hasMore: false,
      messages: [],
    };
    const calls = [];
    const session = new Session(
      {
        readSessionHistory: async (sessionId, options) => {
          calls.push({ sessionId, options });
          return expected;
        },
      },
      {
        sessionId: "s1",
        text: "ok",
        turns: 1,
        toolCalls: 0,
        usage: { inputTokens: 0, outputTokens: 0 },
      },
    );

    const history = await session.history({ offset: 2, limit: 5 });
    assert.deepEqual(calls, [{ sessionId: "s1", options: { offset: 2, limit: 5 } }]);
    assert.equal(history, expected);
  });

  it("DeferredSession.history should delegate to readSessionHistory", async () => {
    const expected = {
      sessionId: "s2",
      messageCount: 0,
      offset: 0,
      limit: undefined,
      hasMore: false,
      messages: [],
    };
    const calls = [];
    const session = new DeferredSession(
      {
        readSessionHistory: async (sessionId, options) => {
          calls.push({ sessionId, options });
          return expected;
        },
      },
      "s2",
    );

    const history = await session.history({ offset: 1 });
    assert.deepEqual(calls, [{ sessionId: "s2", options: { offset: 1 } }]);
    assert.equal(history, expected);
  });

  it("should preserve malformed tool_config_changed payload", () => {
    const raw = {
      type: "tool_config_changed",
      payload: "not-an-object",
    };
    const event = parseEvent({
      type: "tool_config_changed",
      payload: "not-an-object",
    });
    assert.equal(event.type, "malformed_event");
    assert.equal(event.rawType, "tool_config_changed");
    assert.deepEqual(event.raw, raw);
  });

  it("should ignore malformed applied_at_turn in tool_config_changed", () => {
    const event = parseEvent({
      type: "tool_config_changed",
      payload: {
        operation: "add",
        target: "filesystem",
        status: "staged",
        persisted: true,
        applied_at_turn: "oops",
      },
    });
    assert.equal(event.type, "tool_config_changed");
    if (event.type === "tool_config_changed") {
      assert.equal(event.payload.applied_at_turn, undefined);
    }
  });

  it("should preserve non-boolean persisted in tool_config_changed", () => {
    const raw = {
      type: "tool_config_changed",
      payload: {
        operation: "add",
        target: "filesystem",
        status: "staged",
        persisted: "false",
      },
    };
    const event = parseEvent(raw);
    assert.equal(event.type, "malformed_event");
    assert.equal(event.rawType, "tool_config_changed");
    assert.deepEqual(event.raw, raw);
  });

  it("should not default malformed tool_config_changed operation/status semantics", () => {
    const raw = {
      type: "tool_config_changed",
      payload: {
        operation: "bogus",
        target: 0,
        status: 0,
        persisted: "false",
      },
    };
    const event = parseEvent(raw);

    assert.equal(event.type, "malformed_event");
    assert.equal(event.rawType, "tool_config_changed");
    assert.deepEqual(event.raw, raw);
  });

  it("should parse background_job_completed from typed terminal status", () => {
    const event = parseEvent({
      type: "background_job_completed",
      job_id: "j_123",
      display_name: "sleep 2",
      terminal_status: "failed",
      detail: "exit_code: 1",
    });

    assert.equal(event.type, "background_job_completed");
    if (event.type === "background_job_completed") {
      assert.equal(event.jobId, "j_123");
      assert.equal(event.displayName, "sleep 2");
      assert.equal(event.terminalStatus, "failed");
      assert.equal(event.detail, "exit_code: 1");
    }
  });

  it("should reject background_job_completed without typed terminal status", () => {
    const raw = {
      type: "background_job_completed",
      job_id: "j_123",
      display_name: "sleep 2",
      status: "completed",
      detail: "exit_code: 0",
    };
    const event = parseEvent(raw);

    assert.equal(event.type, "malformed_event");
    assert.equal(event.rawType, "background_job_completed");
    assert.deepEqual(event.raw, raw);
  });

  it("should reject unknown background_job_completed terminal status", () => {
    const raw = {
      type: "background_job_completed",
      job_id: "j_123",
      display_name: "sleep 2",
      status: "completed",
      terminal_status: "success",
      detail: "exit_code: 0",
    };
    const event = parseEvent(raw);

    assert.equal(event.type, "malformed_event");
    assert.equal(event.rawType, "background_job_completed");
    assert.deepEqual(event.raw, raw);
  });

  it("should parse transcript_rewrite_committed with typed record access", () => {
    const record = {
      commit: {
        parent_revision: "rev-parent",
        revision: "rev-next",
      },
      parent_body: { revision: "rev-parent" },
      revision_body: { revision: "rev-next" },
    };
    const event = parseEvent({
      type: "transcript_rewrite_committed",
      session_id: "session-123",
      record,
    });

    assert.equal(event.type, "transcript_rewrite_committed");
    if (event.type === "transcript_rewrite_committed") {
      assert.equal(event.sessionId, "session-123");
      assert.deepEqual(event.record, record);
    }
  });

  it("should reject transcript_rewrite_committed without record", () => {
    const raw = {
      type: "transcript_rewrite_committed",
      session_id: "session-123",
    };
    const event = parseEvent(raw);

    assert.equal(event.type, "malformed_event");
    assert.equal(event.rawType, "transcript_rewrite_committed");
    assert.deepEqual(event.raw, raw);
  });

  it("should not fabricate standalone event envelope metadata or payload", () => {
    const envelope = MeerkatClient.parseAgentEventEnvelope({
      event_id: 3,
      seq: "0",
      timestamp_ms: "0",
      payload: "not-an-object",
    });

    assert.equal(envelope.eventId, undefined);
    assert.equal(envelope.seq, undefined);
    assert.equal(envelope.timestampMs, undefined);
    assert.equal(envelope.payload, undefined);
  });

  it("should use typed event source without trusting legacy source id", () => {
    const envelope = MeerkatClient.parseAgentEventEnvelope({
      source: {
        type: "session",
        session_id: "00000000-0000-4000-8000-000000000001",
      },
      payload: { type: "text_delta", delta: "hi" },
    });

    assert.deepEqual(envelope.source, {
      type: "session",
      sessionId: "00000000-0000-4000-8000-000000000001",
    });
  });

  it("should not classify source from legacy session strings", () => {
    const envelope = MeerkatClient.parseAgentEventEnvelope({
      source_id: "session:00000000-0000-4000-8000-000000000001",
      payload: { type: "text_delta", delta: "hi" },
    });

    // The legacy envelope-level string never classifies a typed source and is
    // no longer surfaced on the typed envelope.
    assert.equal(envelope.source, undefined);
    assert.equal(envelope.sourceId, undefined);
  });
});

describe("WorkGraph parsers", () => {
  const timestamp = "2026-05-12T12:00:00Z";
  const claimedItem = {
    id: "prep-dentist-ride",
    realm_id: "homecore",
    namespace: "family/appointments",
    title: "Prep A for non-preferred dentist car",
    status: "in_progress",
    priority: "high",
    completion_policy: { kind: "host_confirmed" },
    labels: ["autism-support", "dentist"],
    owner: {
      key: { kind: "agent", id: "homecore-kapellmeister" },
      display_name: "Homecore Kapellmeister",
    },
    claim: {
      owner: { key: { kind: "agent", id: "homecore-kapellmeister" } },
      claimed_at: timestamp,
    },
    machine_state: { lifecycle_phase: "claimed", revision: 4 },
    revision: 4,
    created_at: timestamp,
    updated_at: timestamp,
    external_refs: [{ kind: "calendar_event", id: "dentist-visit" }],
    evidence_refs: [{ kind: "message_draft", id: "draft-1" }],
  };

  it("parses typed owners and claims", () => {
    const item = parseWorkItem(claimedItem);

    assert.equal(item.realm_id, "homecore");
    assert.equal(item.owner?.key.kind, "agent");
    assert.equal(item.claim?.owner.key.id, "homecore-kapellmeister");
    assert.deepEqual(item.completion_policy, { kind: "host_confirmed" });
  });

  it("rejects malformed WorkGraph lifecycle truth", () => {
    assert.throws(
      () => parseWorkItem({ ...claimedItem, status: undefined }),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        String(error.message).includes("WorkItem"),
    );
  });

  it("requires typed WorkGraph claim owners", () => {
    assert.throws(
      () =>
        parseWorkItem({
          ...claimedItem,
          claim: { claimed_at: timestamp },
        }),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        String(error.message).includes("owner"),
    );
  });

  it("rejects payloads missing the required completion policy (no silent default)", () => {
    const { completion_policy: _completionPolicy, ...truncatedItem } = claimedItem;
    assert.throws(
      () => parseWorkItem(truncatedItem),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        String(error.message).includes("completion_policy"),
    );
  });

  it("rejects unknown completion-policy kinds (fail-closed union parse)", () => {
    assert.throws(
      () => parseWorkItem({ ...claimedItem, completion_policy: { kind: "vibes" } }),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        String(error.message).includes("kind"),
    );
  });

  it("rejects malformed WorkGraph attention authority enums", async () => {
    const attention = {
      binding_id: "attention-1",
      target: { kind: "session", session_id: "session-1" },
      work_ref: {
        realm_id: "homecore",
        namespace: "family/appointments",
        item_id: "prep-dentist-ride",
      },
      mode: "pursue",
      status: { state: "active" },
      delegated_authority: "request_closure",
      created_at: timestamp,
      updated_at: timestamp,
    };

    const clientFor = (entry) => {
      const client = new MeerkatClient();
      client.request = async () => ({ attention: [entry] });
      return client;
    };

    await assert.rejects(
      clientFor({ ...attention, mode: "not_a_mode" }).listWorkGraphAttention(),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        String(error.message).includes("WorkAttentionMode"),
    );
    await assert.rejects(
      clientFor({ ...attention, delegated_authority: "not_authority" }).listWorkGraphAttention(),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        String(error.message).includes("AttentionDelegatedAuthority"),
    );
  });

  it("fails closed when WorkGraph list collections are not arrays", async () => {
    const clientFor = (response) => {
      const client = new MeerkatClient();
      client.request = async () => response;
      return client;
    };
    const invalidResponse = (error) =>
      error instanceof MeerkatError && error.code === "INVALID_RESPONSE";

    await assert.rejects(
      clientFor({ items: "oops" }).listWorkGraphItems({ realm_id: "homecore" }),
      invalidResponse,
    );
    await assert.rejects(
      clientFor({ items: { not: "a list" } }).listReadyWorkGraphItems({
        realm_id: "homecore",
      }),
      invalidResponse,
    );
    await assert.rejects(
      clientFor({ events: 42 }).listWorkGraphEvents({ realm_id: "homecore" }),
      invalidResponse,
    );
  });
});

describe("Session wrappers", () => {
  it("createSession returns a runtime-backed Session wrapper", async () => {
    const client = new MeerkatClient();
    const seen = [];
    client.request = async (method, params) => {
      seen.push([method, params]);
      return {
        session_id: "sess-1",
        session_ref: "team/runtime",
        text: "ready",
        turns: 1,
        tool_calls: 0,
        usage: { input_tokens: 12, output_tokens: 4 },
      };
    };

    const session = await client.createSession("Summarise the runtime path");

    assert.ok(session instanceof Session);
    assert.equal(session.id, "sess-1");
    assert.equal(session.ref, "team/runtime");
    assert.equal(session.text, "ready");
    assert.equal(session.lastResult.sessionId, "sess-1");
    assert.deepEqual(seen, [["session/create", { prompt: "Summarise the runtime path" }]]);
  });

  it("createDeferredSession returns a runtime-backed DeferredSession wrapper", async () => {
    const client = new MeerkatClient();
    const seen = [];
    client.request = async (method, params) => {
      seen.push([method, params]);
      return {
        session_id: "sess-2",
        session_ref: "team/deferred",
      };
    };

    const deferred = await client.createDeferredSession("Hold until first turn");

    assert.ok(deferred instanceof DeferredSession);
    assert.equal(deferred.id, "sess-2");
    assert.equal(deferred.ref, "team/deferred");
    assert.deepEqual(seen, [[
      "session/create",
      { prompt: "Hold until first turn", initial_turn: "deferred" },
    ]]);
  });

  it("createSession forwards extended creation options", async () => {
    const client = new MeerkatClient();
    const seen = [];
    client.request = async (method, params) => {
      seen.push({ method, params });
      return {
        session_id: "sess-extended",
        text: "ok",
        turns: 1,
        tool_calls: 0,
        usage: { input_tokens: 1, output_tokens: 1 },
      };
    };

    await client.createSession("Hello", {
      labels: { team: "sdk" },
      additionalInstructions: ["be terse"],
      appContext: { tenant: "acme" },
      shellEnv: { FOO: "bar" },
      enableSchedule: true,
      enableWorkGraph: true,
      enableWebSearch: true,
      authBinding: { realm: "dev", binding: "default_openai" },
      externalTools: [{ name: "x", description: "x", input_schema: { type: "object" } }],
    });

    assert.deepEqual(seen[0], {
      method: "session/create",
      params: {
        prompt: "Hello",
        labels: { team: "sdk" },
        additional_instructions: ["be terse"],
        app_context: { tenant: "acme" },
        shell_env: { FOO: "bar" },
        enable_schedule: true,
        enable_workgraph: true,
        enable_web_search: true,
        auth_binding: { realm: "dev", binding: "default_openai" },
        external_tools: [{ name: "x", description: "x", input_schema: { type: "object" } }],
      },
    });
  });

  it("listSessions and readSession parse typed metadata", async () => {
    const client = new MeerkatClient();
    client.request = async (method) => {
      if (method === "session/list") {
        return {
          sessions: [
            {
              session_id: "s1",
              created_at: 10,
              updated_at: 20,
              message_count: 2,
              total_tokens: 9,
              is_active: true,
              labels: { team: "infra" },
            },
          ],
        };
      }
      return {
        session_id: "s1",
        created_at: 10,
        updated_at: 20,
        message_count: 2,
        is_active: false,
        model: "claude-sonnet-4-6",
        provider: "anthropic",
        last_assistant_text: "hello",
        resolved_capabilities: {
          vision: true,
          image_input: true,
          image_tool_results: true,
          inline_video: false,
          realtime: false,
          web_search: true,
          image_generation: true,
        },
        labels: { team: "infra" },
      };
    };

    const sessions = await client.listSessions({ labels: { team: "infra" }, limit: 5, offset: 1 });
    const details = await client.readSession("s1");

    assert.equal(sessions[0].sessionId, "s1");
    assert.equal(sessions[0].totalTokens, 9);
    assert.equal(sessions[0].labels.team, "infra");
    assert.equal(details.model, "claude-sonnet-4-6");
    assert.equal(details.provider, "anthropic");
    assert.equal(details.lastAssistantText, "hello");
    assert.deepEqual(details.resolvedCapabilities, {
      vision: true,
      imageInput: true,
      imageToolResults: true,
      inlineVideo: false,
      realtime: false,
      webSearch: true,
      imageGeneration: true,
    });
  });

  it("drops malformed resolved capabilities instead of fabricating false defaults", async () => {
    const client = new MeerkatClient();
    client.request = async (method) => {
      assert.equal(method, "session/read");
      return {
        session_id: "s1",
        created_at: 10,
        updated_at: 20,
        message_count: 2,
        is_active: false,
        resolved_capabilities: {
          vision: true,
          image_input: true,
          image_tool_results: true,
          inline_video: false,
          realtime: false,
          web_search: "yes",
          image_generation: true,
        },
      };
    };

    const details = await client.readSession("s1");

    assert.equal(details.resolvedCapabilities, undefined);
  });

  it("session/deferred injectContext call public wrapper", async () => {
    const calls = [];
    const client = new MeerkatClient();
    client.request = async (method, params) => {
      calls.push({ method, params });
      return { status: "staged" };
    };
    const session = new Session(
      client,
      {
        sessionId: "s1",
        text: "ok",
        turns: 1,
        toolCalls: 0,
        usage: { inputTokens: 0, outputTokens: 0 },
      },
    );
    const deferred = new DeferredSession(client, "s2");

    const a = await session.injectContext("ctx", { source: "test", idempotencyKey: "k1" });
    const b = await deferred.injectContext("ctx2");

    assert.equal(a.status, "staged");
    assert.equal(b.status, "staged");
    assert.deepEqual(calls, [
      {
        method: "session/inject_context",
        params: {
          session_id: "s1",
          content: { type: "text", text: "ctx" },
          source: "test",
          idempotency_key: "k1",
        },
      },
      {
        method: "session/inject_context",
        params: { session_id: "s2", content: { type: "text", text: "ctx2" } },
      },
    ]);
  });

  it("sendPeerResponseTerminal forwards canonical peer id and correlation id", async () => {
    const calls = [];
    const client = new MeerkatClient();
    client.request = async (method, params) => {
      calls.push({ method, params });
      return { status: "accepted" };
    };

    await client.sendPeerResponseTerminal(
      "s1",
      "00000000-0000-4000-8000-000000000161",
      "00000000-0000-4000-8000-000000000162",
      "completed",
      { ok: true },
      { displayName: "analyst" },
    );

    assert.deepEqual(calls, [
      {
        method: "session/peer_response_terminal",
        params: {
          session_id: "s1",
          peer_id: "00000000-0000-4000-8000-000000000161",
          request_id: "00000000-0000-4000-8000-000000000162",
          status: "completed",
          result: { ok: true },
          display_name: "analyst",
        },
      },
    ]);
    assert.equal("peer_name" in calls[0].params, false);
  });

  it("turn/start wrappers include per-turn overrides on streaming and non-streaming calls", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        session_id: "s1",
        text: "ok",
        turns: 1,
        tool_calls: 0,
        usage: { input_tokens: 1, output_tokens: 1 },
      };
    };
    client.process = { stdin: { write: () => {} } };
    client.registerRequest = async () => ({
      session_id: "s1",
      text: "ok",
      turns: 1,
      tool_calls: 0,
      usage: { input_tokens: 1, output_tokens: 1 },
    });

    await client._startTurn("s1", "hello", {
      additionalInstructions: ["a"],
      keepAlive: true,
      model: "m",
      provider: "p",
      maxTokens: 42,
      systemPrompt: "sys",
      outputSchema: { type: "object" },
      structuredOutputRetries: 3,
      providerParams: { x: 1 },
    });
    client._startTurnStreaming("s1", "hello", {
      additionalInstructions: ["a"],
      keepAlive: true,
      model: "m",
      provider: "p",
      maxTokens: 42,
      systemPrompt: "sys",
      outputSchema: { type: "object" },
      structuredOutputRetries: 3,
      providerParams: { x: 1 },
    });

    assert.equal(calls[0].method, "turn/start");
    assert.equal(calls[0].params.additional_instructions[0], "a");
    assert.equal(calls[0].params.keep_alive, true);
  });
});

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

describe("Type Guards", () => {
  it("isTextDelta narrows type", () => {
    const event = parseEvent({ type: "text_delta", delta: "hi" });
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
      extraction_error: { last_output: "Hello!", attempts: 2, reason: "Invalid JSON" },
    };
    const result = MeerkatClient.parseRunResult(raw);
    assert.equal(result.sessionId, "s1");
    assert.equal(result.sessionRef, "my-ref");
    assert.equal(result.text, "Hello!");
    assert.equal(result.turns, 2);
    assert.equal(result.toolCalls, 1);
    assert.equal(result.usage.inputTokens, 100);
    assert.equal(result.usage.outputTokens, 50);
    assert.equal(result.usage.cacheCreationTokens, 10);
    assert.deepEqual(result.structuredOutput, { answer: 42 });
    assert.deepEqual(result.extractionError, {
      lastOutput: "Hello!",
      attempts: 2,
      reason: "Invalid JSON",
    });
    assert.equal(result.schemaWarnings?.length, 1);
    assert.equal(result.schemaWarnings?.[0].provider, "openai");
  });

  it("should include skillDiagnostics when present", () => {
    const raw = {
      session_id: "s1",
      text: "ok",
      turns: 1,
      tool_calls: 0,
      usage: { input_tokens: 10, output_tokens: 5 },
      skill_diagnostics: {
        source_health: {
          state: "degraded",
          invalid_ratio: 0.25,
          invalid_count: 1,
          total_count: 4,
          failure_streak: 2,
          handshake_failed: false,
        },
        quarantined: [
          {
            source_uuid: "src-1",
            skill_id: "extract/email",
            location: "project",
            error_code: "bad_frontmatter",
            error_class: "ValidationError",
            message: "missing description",
            first_seen_unix_secs: 10,
            last_seen_unix_secs: 20,
          },
        ],
      },
    };
    const result = MeerkatClient.parseRunResult(raw);
    assert.deepEqual(result.skillDiagnostics, {
      sourceHealth: {
        state: "degraded",
        invalidRatio: 0.25,
        invalidCount: 1,
        totalCount: 4,
        failureStreak: 2,
        handshakeFailed: false,
      },
      quarantined: [
        {
          sourceUuid: "src-1",
          skillId: "extract/email",
          location: "project",
          errorCode: "bad_frontmatter",
          errorClass: "ValidationError",
          message: "missing description",
          firstSeenUnixSecs: 10,
          lastSeenUnixSecs: 20,
        },
      ],
    });
  });

  it("should have undefined skillDiagnostics when absent", () => {
    const raw = {
      session_id: "s1",
      text: "ok",
      turns: 1,
      tool_calls: 0,
      usage: { input_tokens: 10, output_tokens: 5 },
    };
    const result = MeerkatClient.parseRunResult(raw);
    assert.equal(result.skillDiagnostics, undefined);
  });

  it("rejects malformed run results instead of fabricating counters", () => {
    assert.throws(
      () => MeerkatClient.parseRunResult({ session_id: "s1", text: "ok", turns: 1, tool_calls: 0 }),
      /missing usage/,
    );
    assert.throws(
      () => MeerkatClient.parseRunResult({
        session_id: "s1",
        text: "ok",
        turns: "1",
        tool_calls: 0,
        usage: { input_tokens: 1, output_tokens: 1 },
      }),
      /turns must be number/,
    );
    assert.throws(
      () => MeerkatClient.parseRunResult({
        session_id: "s1",
        text: "ok",
        turns: 1,
        tool_calls: 0,
        usage: { input_tokens: "oops", output_tokens: 1 },
      }),
      /usage.input_tokens must be number/,
    );
  });
});

describe("Comms methods", () => {
  it("send/peers route through comms RPC methods", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      if (method === "comms/peers") {
        return { peers: [{ name: "agent-a" }] };
      }
      return { kind: "peer_message_sent", acked: true };
    };

    const sendReceipt = await client.send("s1", {
      kind: "peer_message",
      to: "agent-a",
      body: "hello",
      blocks: [
        { type: "text", text: "hello" },
        { type: "image", media_type: "image/png", source: "inline", data: "AAAA" },
      ],
    });
    const peers = await client.peers("s1");

    assert.equal(sendReceipt.kind, "peer_message_sent");
    assert.deepEqual(peers.peers, [{ name: "agent-a" }]);
    assert.deepEqual(calls.map((call) => call.method), ["comms/send", "comms/peers"]);
    assert.deepEqual(calls[0].params.blocks, [
      { type: "text", text: "hello" },
      { type: "image", media_type: "image/png", source: "inline", data: "AAAA" },
    ]);
  });

  it("readSessionHistory routes through session/history and parses typed messages", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        session_id: params.session_id,
        session_ref: "history-ref",
        message_count: 3,
        offset: params.offset,
        limit: params.limit,
        has_more: false,
        messages: [
          { role: "user", content: "hello", created_at: "2026-01-01T00:00:00Z" },
          {
            role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
            blocks: [{ block_type: "text", data: { text: "ok" } }],
            stop_reason: "end_turn",
          },
        ],
      };
    };

    const history = await client.readSessionHistory("s1", { offset: 1, limit: 2 });

    assert.equal(history.sessionId, "s1");
    assert.equal(history.sessionRef, "history-ref");
    assert.equal(history.offset, 1);
    assert.equal(history.limit, 2);
    assert.equal(history.messages[1].role, "block_assistant");
    assert.deepEqual(calls, [
      {
        method: "session/history",
        params: { session_id: "s1", offset: 1, limit: 2 },
      },
    ]);
  });

  it("mcpAdd/mcpRemove/mcpReload send correct RPC methods and payloads", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        session_id: params.session_id,
        operation: method.split("/")[1],
        status: "staged",
        persisted: params.persisted ?? false,
      };
    };

    await client.mcpAdd({
      session_id: "s1",
      server_config: { name: "filesystem", command: "npx" },
      persisted: false,
    });
    await client.mcpRemove({
      session_id: "s1",
      server_name: "filesystem",
      persisted: true,
    });
    await client.mcpReload({
      session_id: "s1",
      server_name: "filesystem",
      persisted: false,
    });

    assert.deepEqual(calls.map((c) => c.method), ["mcp/add", "mcp/remove", "mcp/reload"]);
    assert.equal(calls[0].params.server_config.name, "filesystem");
    assert.equal(Object.hasOwn(calls[0].params, "server_name"), false);
    assert.equal(calls[1].params.persisted, true);
  });

  it("propagates transport/request failures for mcp methods", async () => {
    const client = new MeerkatClient();
    client.request = async () => {
      throw new MeerkatError("TRANSPORT", "boom");
    };

    await assert.rejects(
      () => client.mcpAdd({ session_id: "s1", server_config: { name: "fs", command: "npx" } }),
      /boom/,
    );
    await assert.rejects(
      () => client.mcpRemove({ session_id: "s1", server_name: "fs", persisted: false }),
      /boom/,
    );
    await assert.rejects(
      () => client.mcpReload({ session_id: "s1", persisted: false }),
      /boom/,
    );
  });

  it("rejects malformed mcp response payloads", async () => {
    const client = new MeerkatClient();
    client.request = async () => ({
      session_id: "s1",
      operation: "add",
      status: "staged",
      persisted: "false",
    });

    await assert.rejects(
      () => client.mcpAdd({ session_id: "s1", server_config: { name: "fs", command: "npx" } }),
      /persisted must be boolean/,
    );
  });
});

describe("Auth wrappers", () => {
  it("forwards optional profile overrides for status and logout", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {};
    };

    await client.authStatusGet("prod", "claude-console", "primary");
    await client.authLogout("prod", "claude-console", "primary");

    assert.deepEqual(calls, [
      {
        method: "auth/status/get",
        params: {
          realm_id: "prod",
          binding_id: "claude-console",
          profile_id: "primary",
        },
      },
      {
        method: "auth/logout",
        params: {
          realm_id: "prod",
          binding_id: "claude-console",
          profile_id: "primary",
        },
      },
    ]);
  });

  it("preserves explicit empty profile overrides for backend validation", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {};
    };

    await client.authStatusGet("prod", "claude-console", "");
    await client.authLogout("prod", "claude-console", "");

    assert.deepEqual(calls, [
      {
        method: "auth/status/get",
        params: {
          realm_id: "prod",
          binding_id: "claude-console",
          profile_id: "",
        },
      },
      {
        method: "auth/logout",
        params: {
          realm_id: "prod",
          binding_id: "claude-console",
          profile_id: "",
        },
      },
    ]);
  });

  it("keeps status and logout params unchanged without profile overrides", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {};
    };

    await client.authStatusGet("prod", "claude-console");
    await client.authLogout("prod", "claude-console");

    assert.deepEqual(calls, [
      {
        method: "auth/status/get",
        params: {
          realm_id: "prod",
          binding_id: "claude-console",
        },
      },
      {
        method: "auth/logout",
        params: {
          realm_id: "prod",
          binding_id: "claude-console",
        },
      },
    ]);
  });
});

describe("Parity wrappers", () => {
  it("retired runtime session wrappers fail before transport", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      throw new Error("retired wrapper reached transport");
    };

    for (const methodName of [
      "_runtimeStatus",
      "_runtimeSubmit",
      "_runtimeSubmission",
      "_runtimeSubmissions",
      "_runtimeRetire",
      "_runtimeReset",
    ]) {
      await assert.rejects(
        async () => {
          await client[methodName]({ session_id: "session-1" });
        },
        (err) => {
          assert.ok(err instanceof MeerkatError);
          assert.equal(err.code, "METHOD_NOT_FOUND");
          assert.match(err.message, /Retired runtime session control methods/);
          return true;
        },
      );
    }

    assert.deepEqual(calls, []);
  });

  it("adds wrappers for session external events and model catalog", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      if (method === "help/ask") {
        return {
          session_id: "help-1",
          text: "rkat mcp add ...",
          turns: 1,
          tool_calls: 0,
          usage: { input_tokens: 10, output_tokens: 20 },
        };
      }
      if (method === "models/catalog") {
        return {
          contract_version: { major: 0, minor: 5, patch: 1 },
          providers: [
            {
              provider: "anthropic",
              default_model_id: "claude-sonnet-4-6",
              models: [
                {
                  id: "claude-sonnet-4-6",
                  display_name: "Claude Sonnet 4.6",
                  tier: "recommended",
                  profile: {
                    model_family: "claude",
                    supports_temperature: true,
                    supports_thinking: true,
                    supports_reasoning: true,
                    vision: true,
                    image_input: true,
                    image_tool_results: true,
                    inline_video: false,
                    realtime: false,
                    supports_web_search: true,
                    image_generation: true,
                    params_schema: {},
                  },
                },
              ],
            },
          ],
        };
      }
      return { status: "accepted" };
    };

    const help = await client.askHelp("How do I add an MCP server?", {
      prompt: "Write a game",
      executionMode: "plan_execution",
      model: "claude-sonnet-4-6",
      provider: "anthropic",
      maxTokens: 512,
    });
    const external = await client.sendExternalEvent("s1", "test", { type: "webhook" });
    const catalog = await client.getModelsCatalog();

    assert.equal(help.text, "rkat mcp add ...");
    assert.equal(external.status, "accepted");
    assert.equal(catalog.providers[0].defaultModelId, "claude-sonnet-4-6");
    assert.equal(catalog.providers[0].models[0].profile?.imageGeneration, true);
    assert.equal(catalog.providers[0].models[0].profile?.webSearch, true);
    assert.deepEqual(catalog.contractVersion, { major: 0, minor: 5, patch: 1 });
    assert.deepEqual(calls.map((c) => c.method), [
      "help/ask",
      "session/external_event",
      "models/catalog",
    ]);
    assert.deepEqual(calls[0].params, {
      question: "How do I add an MCP server?",
      prompt: "Write a game",
      execution_mode: "plan_execution",
      model: "claude-sonnet-4-6",
      provider: "anthropic",
      max_tokens: 512,
    });
    assert.equal(calls[1].params.session_id, "s1");
    assert.equal(calls[1].params.kind, "generic_json");
    assert.equal(calls[1].params.event_type, "test");
  });

  it("rejects missing or malformed models/catalog contract_version", async () => {
    const malformedResponses = [
      { providers: [] },
      { contract_version: { major: -1, minor: 5, patch: 1 }, providers: [] },
    ];

    for (const response of malformedResponses) {
      const client = new MeerkatClient();
      client.request = async (method) => {
        assert.equal(method, "models/catalog");
        return response;
      };

      await assert.rejects(
        () => client.getModelsCatalog(),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          String(error.message).includes("contract_version"),
      );
    }
  });

  it("adds wrappers for schedule APIs", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      if (method === "schedule/list") {
        return {
          schedules: [
            {
              schedule_id: "sch-1",
              phase: "active",
              revision: 1,
              trigger: {},
              target: {},
              labels: {},
            },
          ],
        };
      }
      if (method === "schedule/occurrences") {
        return { occurrences: [] };
      }
      if (method === "schedule/tools") {
        return { tools: [{ name: "schedule.create" }] };
      }
      return {
        schedule_id: "sch-1",
        phase: "active",
        revision: 1,
        trigger: {},
        target: {},
        labels: {},
      };
    };

    const created = await client.createSchedule({ trigger: {}, target: {} });
    const fetched = await client.getSchedule("sch-1");
    const listed = await client.listSchedules({ labels: { env: "prod" }, limit: 5, offset: 2 });
    const updated = await client.updateSchedule({ scheduleId: "sch-1", update: { name: "new" } });
    await client.pauseSchedule("sch-1");
    await client.resumeSchedule("sch-1");
    await client.deleteSchedule("sch-1");
    const occurrences = await client.listScheduleOccurrences("sch-1", { includeTerminal: false });
    const tools = await client.listScheduleTools();
    await client.callScheduleTool({ name: "schedule.create", arguments: { a: 1 } });

    assert.equal(created.scheduleId, "sch-1");
    assert.equal(fetched.scheduleId, "sch-1");
    assert.equal(listed[0].scheduleId, "sch-1");
    assert.equal(updated.scheduleId, "sch-1");
    assert.equal(occurrences.occurrences.length, 0);
    assert.equal(tools.tools.length, 1);
    assert.deepEqual(calls.map((c) => c.method), [
      "schedule/create",
      "schedule/get",
      "schedule/list",
      "schedule/update",
      "schedule/pause",
      "schedule/resume",
      "schedule/delete",
      "schedule/occurrences",
      "schedule/tools",
      "schedule/call",
    ]);
    assert.deepEqual(calls[2].params, { labels: { env: "prod" }, limit: 5, offset: 2 });
    assert.deepEqual(calls[7].params, { schedule_id: "sch-1", include_terminal: false });
  });

  it("adds wrappers for WorkGraph read-only APIs", async () => {
    const client = new MeerkatClient();
    const calls = [];
    const timestamp = "2026-05-12T12:00:00Z";
    const item = {
      id: "prep-dentist-ride",
      realm_id: "homecore",
      namespace: "family/appointments",
      title: "Prep A for non-preferred dentist car",
      status: "open",
      priority: "high",
      completion_policy: { kind: "self_attest" },
      labels: ["autism-support", "dentist"],
      machine_state: { lifecycle_phase: "open", revision: 1 },
      revision: 1,
      created_at: timestamp,
      updated_at: timestamp,
      external_refs: [{ kind: "calendar_event", id: "dentist-visit" }],
      evidence_refs: [{ kind: "message_draft", id: "draft-1" }],
    };
    const attention = {
      binding_id: "attention-1",
      target: { kind: "session", session_id: "session-1" },
      work_ref: {
        realm_id: "homecore",
        namespace: "family/appointments",
        item_id: "prep-dentist-ride",
      },
      mode: "pursue",
      status: { state: "active" },
      delegated_authority: "request_closure",
      projection_policy: { include_parent_context: true, max_text_chars: 12000 },
      machine_state: { revision: 1 },
      created_at: timestamp,
      updated_at: timestamp,
    };
    client.request = async (method, params) => {
      calls.push({ method, params });
      if (method === "workgraph/get") {
        return item;
      }
      if (method === "workgraph/list" || method === "workgraph/ready") {
        return { items: [item] };
      }
      if (method === "workgraph/snapshot") {
        return {
          realm_id: "homecore",
          namespace: "family/appointments",
          all_namespaces: false,
          captured_at: timestamp,
          event_high_water_mark: 7,
          items: [item],
          edges: [],
          attention: [attention],
          ready_item_ids: ["prep-dentist-ride"],
        };
      }
      if (method === "workgraph/events") {
        return {
          events: [{
            seq: 7,
            realm_id: "homecore",
            namespace: "family/appointments",
            item_id: "prep-dentist-ride",
            kind: "created",
            at: timestamp,
            payload: {},
          }],
        };
      }
      if (method === "workgraph/goal/status") {
        return { item, attention };
      }
      if (method === "workgraph/attention/list") {
        return { attention: [attention] };
      }
      throw new Error(`unexpected method ${method}`);
    };

    const fetched = await client.getWorkGraphItem("prep-dentist-ride", {
      realmId: "homecore",
      namespace: "family/appointments",
    });
    const listed = await client.listWorkGraphItems({ realm_id: "homecore", limit: 5 });
    const ready = await client.listReadyWorkGraphItems({
      namespace: "family/appointments",
      limit: 3,
    });
    const snapshot = await client.getWorkGraphSnapshot({
      realm_id: "homecore",
      namespace: "family/appointments",
    });
    const events = await client.listWorkGraphEvents({ realm_id: "homecore", limit: 10 });
    const goal = await client.getWorkGraphGoalStatus({
      binding_id: "attention-1",
      realm_id: "homecore",
      namespace: "family/appointments",
    });
    const attentionList = await client.listWorkGraphAttention({
      realm_id: "homecore",
      status: { state: "active" },
      target: { kind: "session", session_id: "session-1" },
    });

    assert.equal(fetched.id, "prep-dentist-ride");
    assert.equal(fetched.claim, undefined);
    assert.deepEqual(fetched.evidence_refs, [{ kind: "message_draft", id: "draft-1" }]);
    assert.equal(listed.items[0].priority, "high");
    assert.equal(ready.items[0].status, "open");
    assert.deepEqual(snapshot.ready_item_ids, ["prep-dentist-ride"]);
    assert.equal(snapshot.attention[0].binding_id, "attention-1");
    assert.equal(events.events[0].kind, "created");
    assert.equal(goal.item.id, "prep-dentist-ride");
    assert.equal(goal.attention.binding_id, "attention-1");
    assert.equal(attentionList.attention[0].binding_id, "attention-1");
    assert.deepEqual(attentionList.attention[0].target, {
      kind: "session",
      session_id: "session-1",
    });
    assert.deepEqual(calls.map((c) => c.method), [
      "workgraph/get",
      "workgraph/list",
      "workgraph/ready",
      "workgraph/snapshot",
      "workgraph/events",
      "workgraph/goal/status",
      "workgraph/attention/list",
    ]);
    assert.deepEqual(calls[0].params, {
      id: "prep-dentist-ride",
      realm_id: "homecore",
      namespace: "family/appointments",
    });
    assert.deepEqual(calls[1].params, { realm_id: "homecore", limit: 5 });
    assert.deepEqual(calls[2].params, { namespace: "family/appointments", limit: 3 });
    assert.deepEqual(calls[5].params, {
      binding_id: "attention-1",
      realm_id: "homecore",
      namespace: "family/appointments",
    });
    assert.deepEqual(calls[6].params, {
      realm_id: "homecore",
      status: { state: "active" },
      target: { kind: "session", session_id: "session-1" },
    });
  });

  it("fails closed on malformed WorkGraph list entries (generated parsers)", async () => {
    const client = new MeerkatClient();
    client.request = async () => ({ items: [{ id: "only-an-id" }] });
    await assert.rejects(
      client.listWorkGraphItems({ realm_id: "homecore" }),
      (error) =>
        error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
    );
  });

  it("adds wrappers for mob events, batch spawn, and profile CRUD", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      if (method === "mob/spawn") {
        return {
          mob_id: params.mob_id,
          agent_identity: params.agent_identity,
          member_ref: makeMemberRef(params.mob_id, params.agent_identity),
        };
      }
      if (method === "mob/append_system_context") {
        return { status: "staged", mob_id: params.mob_id, agent_identity: params.agent_identity };
      }
      if (method === "mob/spawn_many") {
        return {
          results: [{
            status: "spawned",
            result: {
              agent_identity: "worker-1",
              member_ref: makeMemberRef(params.mob_id, "worker-1"),
            },
          }, {
            status: "failed",
            result: {
              cause: "profile_not_found",
              message: "profile missing",
            },
          }],
        };
      }
      if (method === "mob/turn_start") {
        return { status: "started" };
      }
      if (method === "mob/events") {
        return { events: [{ cursor: 1 }] };
      }
      if (method === "mob/profile/list") {
        return {
          profiles: [
            {
              name: "worker",
              revision: 1,
              profile: { model: "claude-sonnet-4-6", tools: { comms: true } },
            },
          ],
        };
      }
      if (method === "mob/profile/delete") {
        return { name: "worker", deleted_revision: 2 };
      }
      if (method === "mob/profile/get") {
        return { not_found: true, name: "missing" };
      }
      return {
        name: "worker",
        revision: 1,
        profile: { model: "claude-sonnet-4-6", tools: { comms: true } },
      };
    };

    const spawnedOne = await client.spawnMobMember("mob-1", {
      profile: "worker",
      agentIdentity: "worker-0",
      initialMessage: [{ type: "text", text: "hello" }],
      runtimeMode: "autonomous_host",
      backend: "session",
      labels: { role: "worker" },
      context: { ticket: "LUC-134" },
      additionalInstructions: ["stay focused"],
      binding: { kind: "session" },
      shellEnv: { TEST_MODE: "1" },
      autoWireParent: true,
      launchMode: { mode: "fresh" },
      toolAccessPolicy: { type: "allow_list", value: ["grep"] },
      budgetSplitPolicy: { type: "remaining" },
      inheritedToolFilter: { Allow: ["grep"] },
      overrideProfile: {
        model: "claude-sonnet-4-6",
        tools: { shell: true },
      },
      authBinding: { realm: "dev", binding: "default_anthropic" },
    });
    const spawned = await client.spawnMobMembers("mob-1", [{
      profile: "worker",
      agentIdentity: "worker-1",
      authBinding: { realm: "dev", binding: "default_anthropic" },
    }]);
    await client.mobTurnStart(
      "mob-1",
      "worker-1",
      [{ type: "text", text: "continue" }],
      {
        skillRefs: [{ sourceUuid: "00000000-0000-4000-8000-000000000001", skillName: "read" }],
        flowToolOverlay: { allowedTools: ["read"], blockedTools: [] },
        additionalInstructions: ["stay concise"],
        keepAlive: true,
        model: "gpt-test",
        provider: "openai",
        maxTokens: 128,
        systemPrompt: "system",
        outputSchema: { type: "object" },
        structuredOutputRetries: 2,
        providerParams: { action: "set", value: { temperature: 0.2 } },
        authBinding: {
          action: "set",
          value: { realm: "dev", binding: "default_openai" },
        },
      },
    );
    const append = await client.appendMobSystemContext("mob-1", "worker-1", "remember this");
    const events = await client.readMobEvents("mob-1", { afterCursor: 10, limit: 5 });
    const created = await client.createMobProfile("worker", { model: "claude-sonnet-4-6" });
    const got = await client.getMobProfile("missing");
    const listed = await client.listMobProfiles();
    const updated = await client.updateMobProfile("worker", { model: "claude-opus-4-8" }, 1);
    const deleted = await client.deleteMobProfile("worker", 2);

    assert.equal(spawnedOne.agentIdentity, "worker-0");
    assert.equal(spawnedOne.memberRef, makeMemberRef("mob-1", "worker-0"));
    assert.equal(spawnedOne.agentRuntimeId, undefined);
    assert.equal(spawnedOne.fenceToken, undefined);
    assert.equal(spawned[0].status, "spawned");
    assert.equal(spawned[0].result.agent_identity, "worker-1");
    assert.equal(spawned[0].result.member_ref, makeMemberRef("mob-1", "worker-1"));
    assert.equal(spawned[1].status, "failed");
    assert.equal(spawned[1].result.cause, "profile_not_found");
    assert.equal(spawned[1].result.message, "profile missing");
    assert.equal(append.agent_identity, "worker-1");
    assert.equal(events.events.length, 1);
    assert.equal(created.notFound, false);
    assert.equal(got.notFound, true);
    assert.equal(listed.length, 1);
    assert.equal(updated.notFound, false);
    assert.equal(deleted.deletedRevision, 2);
    assert.deepEqual(calls.map((c) => c.method), [
      "mob/spawn",
      "mob/spawn_many",
      "mob/turn_start",
      "mob/append_system_context",
      "mob/events",
      "mob/profile/create",
      "mob/profile/get",
      "mob/profile/list",
      "mob/profile/update",
      "mob/profile/delete",
    ]);
    assert.deepEqual(calls[0].params, {
      mob_id: "mob-1",
      profile: "worker",
      agent_identity: "worker-0",
      initial_message: [{ type: "text", text: "hello" }],
      runtime_mode: "autonomous_host",
      backend: "session",
      labels: { role: "worker" },
      context: { ticket: "LUC-134" },
      additional_instructions: ["stay focused"],
      binding: { kind: "session" },
      shell_env: { TEST_MODE: "1" },
      auto_wire_parent: true,
      launch_mode: { mode: "fresh" },
      tool_access_policy: { type: "allow_list", value: ["grep"] },
      budget_split_policy: { type: "remaining" },
      inherited_tool_filter: { Allow: ["grep"] },
      override_profile: {
        model: "claude-sonnet-4-6",
        tools: { shell: true },
      },
      auth_binding: { realm: "dev", binding: "default_anthropic" },
    });
    assert.deepEqual(calls[1].params.specs[0].auth_binding, {
      realm: "dev",
      binding: "default_anthropic",
    });
    assert.deepEqual(calls[2].params, {
      mob_id: "mob-1",
      agent_identity: "worker-1",
      prompt: [{ type: "text", text: "continue" }],
      skill_refs: [
        {
          kind: "structured",
          source_uuid: "00000000-0000-4000-8000-000000000001",
          skill_name: "read",
        },
      ],
      flow_tool_overlay: { allowed_tools: ["read"], blocked_tools: [] },
      additional_instructions: ["stay concise"],
      keep_alive: true,
      model: "gpt-test",
      provider: "openai",
      max_tokens: 128,
      system_prompt: "system",
      output_schema: { type: "object" },
      structured_output_retries: 2,
      provider_params: { action: "set", value: { temperature: 0.2 } },
      auth_binding: {
        action: "set",
        value: { realm: "dev", binding: "default_openai" },
      },
    });
    assert.equal(calls[4].params.after_cursor, 10);
    assert.equal(calls[4].params.limit, 5);
  });

  it("passes mob_turn_start tri-state overrides through unchanged", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return { status: "started" };
    };

    // Clear coverage: the tagged `clear` override passes through unchanged.
    await client.mobTurnStart("mob-1", "worker-1", "continue", {
      providerParams: { action: "clear" },
      authBinding: { action: "clear" },
    });
    assert.deepEqual(calls[0].params, {
      mob_id: "mob-1",
      agent_identity: "worker-1",
      prompt: "continue",
      provider_params: { action: "clear" },
      auth_binding: { action: "clear" },
    });

    // Inherit coverage: omitted -> the field is omitted on the wire.
    await client.mobTurnStart("mob-1", "worker-1", "continue");
    assert.equal(calls[1].params.provider_params, undefined);
    assert.equal(calls[1].params.auth_binding, undefined);
  });

  it("rejects malformed mob spawn_many result envelopes", async () => {
    const malformedResponses = [
      { results: [{ ok: true, agent_identity: "worker-1", member_ref: "ref-worker-1" }] },
      { results: [{ status: "spawned" }] },
      {
        results: [
          {
            status: "ok",
            result: { agent_identity: "worker-1", member_ref: "ref-worker-1" },
          },
        ],
      },
      { results: [{ status: "spawned", result: { agent_identity: "worker-1" } }] },
      { results: [{ status: "failed", result: { message: "profile missing" } }] },
      {
        results: [{
          status: "failed",
          result: { cause: "future_failure", message: "profile missing" },
        }],
      },
      { results: [{ status: "failed", result: { message: "" } }] },
      {
        results: [{
          status: "spawned",
          result: { agent_identity: "worker-1", member_ref: "ref-worker-1" },
          ok: true,
        }],
      },
      {
        results: [{
          status: "failed",
          result: { cause: "profile_not_found", message: "profile missing" },
          error: "legacy profile missing",
        }],
      },
      {
        results: [{
          status: "spawned",
          result: { agent_identity: "worker-1", member_ref: "ref-worker-1", ok: true },
        }],
      },
      {
        results: [{
          status: "failed",
          result: {
            cause: "profile_not_found",
            message: "profile missing",
            error: "legacy profile missing",
          },
        }],
      },
      {},
    ];

    for (const response of malformedResponses) {
      const client = new MeerkatClient();
      client.request = async () => response;

      await assert.rejects(
        () => client.spawnMobMembers("mob-1", [{ profile: "worker", agentIdentity: "worker-1" }]),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          String(error.message).includes("mob/spawn_many"),
      );
    }
  });

  it("builds mob turn_start params against the generated contract shape", () => {
    const clientSource = fs.readFileSync(
      new URL("../src/client.ts", import.meta.url),
      "utf8",
    );
    const helperStart = clientSource.indexOf("function mobTurnStartPayload(");
    const helperEnd = clientSource.indexOf("\n}\n", helperStart);
    assert.notEqual(helperStart, -1, "mobTurnStartPayload helper should exist");
    assert.notEqual(helperEnd, -1, "mobTurnStartPayload helper should have a body");
    const helperSource = clientSource.slice(helperStart, helperEnd);

    assert.match(helperSource, /\): MobTurnStartParams \{/);
    assert.match(helperSource, /const payload: MobTurnStartParams = \{/);
    assert.doesNotMatch(helperSource, /Record<string, unknown>/);
  });

  it("returns identity-native mob member listings", async () => {
    const client = new MeerkatClient();
    const expectedRef = makeMemberRef("mob-1", "worker-1");
    client.request = async (method) => {
      if (method === "mob/members") {
        // Canonical wire field is `role` (MobMemberListEntryWire.role); the
        // SDK no longer accepts phantom `profile`/`profile_name` aliases.
        return {
          members: [
            {
              agent_identity: "worker-1",
              member_ref: expectedRef,
              role: "worker",
            },
          ],
        };
      }
      throw new Error(`unexpected method ${method}`);
    };

    const members = await client.listMobMembers("mob-1");

    assert.equal(members.length, 1);
    assert.equal(members[0].agentIdentity, "worker-1");
    assert.equal(members[0].memberRef, expectedRef);
    assert.equal(members[0].profile, "worker");
    assert.equal(members[0].agentRuntimeId, undefined);
    assert.equal(members[0].fenceToken, undefined);
  });

  it("normalizes and parses batched mob member wiring", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        requested: params.edges.length,
        wired: [{ a: "lead-1", b: "reviewer-1" }],
        already_wired: [{ a: "lead-1", b: "writer-1" }],
      };
    };

    const result = await client.mobWireMembersBatch("mob-1", [
      ["lead-1", "reviewer-1"],
      { a: "lead-1", b: "writer-1" },
    ]);

    assert.deepEqual(calls, [
      {
        method: "mob/wire_members_batch",
        params: {
          mob_id: "mob-1",
          edges: [
            { a: "lead-1", b: "reviewer-1" },
            { a: "lead-1", b: "writer-1" },
          ],
        },
      },
    ]);
    assert.deepEqual(result, {
      requested: 2,
      wired: [{ a: "lead-1", b: "reviewer-1" }],
      alreadyWired: [{ a: "lead-1", b: "writer-1" }],
    });
  });

  it("routes Mob.wireMembersBatch through the parent client", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return { requested: 1, wired: [{ a: "a", b: "b" }], already_wired: [] };
    };
    const mob = new Mob(client, "mob-1");

    const result = await mob.wireMembersBatch([["a", "b"]]);

    assert.equal(calls[0].method, "mob/wire_members_batch");
    assert.deepEqual(calls[0].params, { mob_id: "mob-1", edges: [{ a: "a", b: "b" }] });
    assert.deepEqual(result.wired, [{ a: "a", b: "b" }]);
  });

  it("uses canonical role_name for helper APIs while accepting profileName alias", async () => {
    const client = new MeerkatClient();
    const calls = [];
    const expectedRef = makeMemberRef("mob-1", "generated-helper");
    client.request = async (method, params) => {
      calls.push({ method, params });
      const agentIdentity = params.agent_identity ?? "generated-helper";
      return {
        output: "ok",
        tokens_used: 1,
        agent_identity: agentIdentity,
        member_ref: makeMemberRef("mob-1", agentIdentity),
      };
    };

    const spawnByRole = await client.spawnMobHelper("mob-1", "help", { roleName: "worker" });
    const spawnByProfile = await client.spawnMobHelper("mob-1", "help", { profileName: "legacy-worker" });
    const forkByRole = await client.forkMobHelper("mob-1", "a", "help", { roleName: "worker" });
    const forkByProfile = await client.forkMobHelper("mob-1", "a", "help", { profileName: "legacy-worker" });

    assert.equal(calls[0].params.role_name, "worker");
    assert.equal(calls[1].params.role_name, "legacy-worker");
    assert.equal(calls[2].params.role_name, "worker");
    assert.equal(calls[3].params.role_name, "legacy-worker");
    // App-facing helper receipts expose only `member_ref`; binding-era
    // `agent_runtime_id` / `fence_token` are retired per dogma #10.
    assert.equal(spawnByRole.memberRef, expectedRef);
    assert.equal(spawnByProfile.memberRef, expectedRef);
    assert.equal(forkByRole.memberRef, expectedRef);
    assert.equal(forkByProfile.memberRef, expectedRef);
    assert.equal(spawnByRole.agentRuntimeId, undefined);
    assert.equal(spawnByRole.fenceToken, undefined);
    assert.equal(forkByRole.agentRuntimeId, undefined);
    assert.equal(forkByRole.fenceToken, undefined);
  });
});

describe("Mob kickoff wait wrappers", () => {
  it("waitMobKickoff/wait_mob_kickoff/mob.waitForKickoffComplete preserve canonical call shape", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        members: [
          {
            agent_identity: "lead",
            agent_runtime_id: { identity: "lead", generation: 1 },
            fence_token: 1,
            status: "active",
            tokens_used: 42,
            is_final: false,
          },
        ],
      };
    };

    const direct = await client.waitMobKickoff("mob-1", {
      memberIds: ["lead", "writer"],
      timeoutMs: 1234,
    });
    const legacy = await client.wait_mob_kickoff("mob-1", {
      memberIds: ["lead"],
    });
    const mob = new Mob(client, "mob-1");
    const fromHandle = await mob.waitForKickoffComplete({ timeoutMs: 99 });

    assert.equal(calls.length, 3);
    assert.deepEqual(calls.map((call) => call.method), [
      "mob/wait_kickoff",
      "mob/wait_kickoff",
      "mob/wait_kickoff",
    ]);
    assert.deepEqual(calls[0].params, {
      mob_id: "mob-1",
      member_ids: ["lead", "writer"],
      timeout_ms: 1234,
    });
    assert.deepEqual(calls[1].params, {
      mob_id: "mob-1",
      member_ids: ["lead"],
    });
    assert.deepEqual(calls[2].params, {
      mob_id: "mob-1",
      timeout_ms: 99,
    });
    assert.equal(direct[0].agentIdentity, "lead");
    assert.equal(direct[0].agentRuntimeId, undefined);
    assert.equal(direct[0].fenceToken, undefined);
    assert.equal(direct[0].tokensUsed, 42);
    assert.equal(direct[0].status, "active");
    assert.equal(legacy[0].agentIdentity, "lead");
    assert.equal(legacy[0].agentRuntimeId, undefined);
    assert.equal(fromHandle[0].agentIdentity, "lead");
    assert.equal(fromHandle[0].agentRuntimeId, undefined);
  });
});

describe("Mob decoder strictness", () => {
  it("rejects missing mob status instead of fabricating unknown", async () => {
    const client = new MeerkatClient();
    client.request = async () => ({ mob_id: "mob-1" });

    await assert.rejects(
      () => client.mobStatus("mob-1"),
      /missing status/,
    );
  });

  it("rejects malformed wait member snapshots instead of fabricating booleans", async () => {
    const client = new MeerkatClient();
    client.request = async () => ({
      members: [
        {
          agent_identity: "lead",
          status: "active",
          tokens_used: 42,
        },
      ],
    });

    await assert.rejects(
      () => client.waitMobReady("mob-1"),
      /is_final must be boolean/,
    );
  });
});

describe("Mob surface fail-closed status parsing (DOGMA Rule 6)", () => {
  // SITE 1 — listMobs: lifecycle status is require-present-non-empty-string.
  describe("listMobs", () => {
    function mobListClient(response) {
      const client = new MeerkatClient();
      // listMobs gates on the "mob" capability; force it on for the unit path.
      client.hasCapability = () => true;
      client.request = async () => response;
      return client;
    }

    it("parses a present lifecycle status", async () => {
      const client = mobListClient({
        mobs: [{ mob_id: "mob-1", status: "running" }],
      });
      const mobs = await client.listMobs();
      assert.equal(mobs.length, 1);
      assert.equal(mobs[0].mobId, "mob-1");
      assert.equal(mobs[0].status, "running");
    });

    it("rejects an absent status instead of coalescing to empty string", async () => {
      const client = mobListClient({ mobs: [{ mob_id: "mob-1" }] });
      await assert.rejects(
        () => client.listMobs(),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /missing status/.test(String(error.message)),
      );
    });

    it("rejects an empty status string", async () => {
      const client = mobListClient({ mobs: [{ mob_id: "mob-1", status: "" }] });
      await assert.rejects(
        () => client.listMobs(),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });

    it("rejects a missing mobs envelope instead of coalescing to empty success", async () => {
      const client = mobListClient({});
      await assert.rejects(
        () => client.listMobs(),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /mob\/list/.test(String(error.message)),
      );
    });

    it("rejects a non-array mobs envelope", async () => {
      const client = mobListClient({ mobs: "nope" });
      await assert.rejects(
        () => client.listMobs(),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });
  });

  // SITE 2 — mobStatus: require record + non-empty status + non-empty mob_id;
  // no Object.keys(...)[0] enum-key reconstruction.
  describe("mobStatus", () => {
    function mobStatusClient(response) {
      const client = new MeerkatClient();
      client.request = async () => response;
      return client;
    }

    it("parses a present lifecycle status", async () => {
      const client = mobStatusClient({ mob_id: "mob-1", status: "running" });
      const result = await client.mobStatus("mob-1");
      assert.equal(result.mobId, "mob-1");
      assert.equal(result.status, "running");
    });

    it("rejects an absent status (no enum-key reconstruction)", async () => {
      const client = mobStatusClient({ mob_id: "mob-1" });
      await assert.rejects(
        () => client.mobStatus("mob-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /missing status/.test(String(error.message)),
      );
    });

    it("does NOT reconstruct status from an externally-tagged enum object", async () => {
      // Old permissive behavior fabricated status via Object.keys(obj)[0].
      const client = mobStatusClient({
        mob_id: "mob-1",
        status: { DisabledByPolicy: { description: "nope" } },
      });
      await assert.rejects(
        () => client.mobStatus("mob-1"),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });

    it("rejects an absent mob_id", async () => {
      const client = mobStatusClient({ status: "running" });
      await assert.rejects(
        () => client.mobStatus("mob-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /missing mob_id/.test(String(error.message)),
      );
    });
  });

  // SITE 3 — listMobMembers: profile derives from `role` only; member status,
  // when present, is enum-validated.
  describe("listMobMembers", () => {
    function membersClient(members) {
      const client = new MeerkatClient();
      client.request = async () => ({ members });
      return client;
    }

    it("rejects a member missing role (no phantom profile/profile_name aliases)", async () => {
      const ref = makeMemberRef("mob-1", "worker-1");
      const client = membersClient([
        { agent_identity: "worker-1", member_ref: ref, profile: "worker" },
      ]);
      await assert.rejects(
        () => client.listMobMembers("mob-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /missing role/.test(String(error.message)),
      );
    });

    it("parses a present member status against the closed enum", async () => {
      const ref = makeMemberRef("mob-1", "worker-1");
      const client = membersClient([
        {
          agent_identity: "worker-1",
          member_ref: ref,
          role: "worker",
          status: "retiring",
        },
      ]);
      const members = await client.listMobMembers("mob-1");
      assert.equal(members[0].profile, "worker");
      assert.equal(members[0].status, "retiring");
    });

    it("rejects a non-variant member status string", async () => {
      const ref = makeMemberRef("mob-1", "worker-1");
      const client = membersClient([
        {
          agent_identity: "worker-1",
          member_ref: ref,
          role: "worker",
          status: "definitely-not-a-variant",
        },
      ]);
      await assert.rejects(
        () => client.listMobMembers("mob-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /invalid member status/.test(String(error.message)),
      );
    });

    it("rejects malformed (non-object) member entries instead of skipping them", async () => {
      const client = membersClient(["not-an-object"]);
      await assert.rejects(
        () => client.listMobMembers("mob-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE",
      );
    });

    it("rejects a missing members collection instead of returning empty success", async () => {
      const client = new MeerkatClient();
      client.request = async () => ({});
      await assert.rejects(
        () => client.listMobMembers("mob-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /expected array/.test(String(error.message)),
      );
    });
  });

  // Malformed kickoff/ready member collections are contract violations and
  // must raise — never be silently collapsed into empty success.
  describe("waitMobKickoff/waitMobReady malformed collections", () => {
    for (const [label, invoke] of [
      ["waitMobKickoff", (client) => client.waitMobKickoff("mob-1")],
      ["waitMobReady", (client) => client.waitMobReady("mob-1")],
    ]) {
      it(`${label} rejects a missing members collection`, async () => {
        const client = new MeerkatClient();
        client.request = async () => ({});
        await assert.rejects(
          () => invoke(client),
          (error) =>
            error instanceof MeerkatError &&
            error.code === "INVALID_RESPONSE" &&
            /expected array/.test(String(error.message)),
        );
      });

      it(`${label} rejects malformed member entries`, async () => {
        const client = new MeerkatClient();
        client.request = async () => ({ members: [42] });
        await assert.rejects(
          () => invoke(client),
          (error) =>
            error instanceof MeerkatError &&
            error.code === "INVALID_RESPONSE",
        );
      });
    }
  });

  // SITE 4 — respawnMobMember: status must be present and in the closed set;
  // never default to "completed".
  describe("respawnMobMember", () => {
    function respawnClient(response) {
      const client = new MeerkatClient();
      client.request = async () => response;
      return client;
    }

    const receipt = {
      member_ref: makeMemberRef("mob-1", "worker-1"),
      identity: "worker-1",
    };

    it("parses a valid completed status", async () => {
      const client = respawnClient({ status: "completed", receipt });
      const result = await client.respawnMobMember("mob-1", "worker-1");
      assert.equal(result.status, "completed");
      assert.equal(result.receipt.memberRef, receipt.member_ref);
    });

    it("parses a valid topology_restore_failed status", async () => {
      const client = respawnClient({
        status: "topology_restore_failed",
        receipt,
        failed_peer_ids: ["peer-9"],
      });
      const result = await client.respawnMobMember("mob-1", "worker-1");
      assert.equal(result.status, "topology_restore_failed");
      assert.deepEqual(result.failedPeerIds, ["peer-9"]);
    });

    it("rejects an absent status instead of defaulting to completed", async () => {
      const client = respawnClient({ receipt });
      await assert.rejects(
        () => client.respawnMobMember("mob-1", "worker-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /missing status|status/.test(String(error.message)),
      );
    });

    it("rejects an out-of-set status (no collapse to completed)", async () => {
      const client = respawnClient({ status: "weird", receipt });
      await assert.rejects(
        () => client.respawnMobMember("mob-1", "worker-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /invalid status/.test(String(error.message)),
      );
    });
  });

  // SITE 5 — mobMemberStatus: member status is enum-validated; "unknown" is a
  // valid runtime variant but must NOT be fabricated on absence.
  describe("mobMemberStatus", () => {
    function memberStatusClient(response) {
      const client = new MeerkatClient();
      client.request = async () => response;
      return client;
    }

    for (const variant of ["active", "retiring", "broken", "completed", "unknown"]) {
      it(`accepts the valid member status variant '${variant}'`, async () => {
        const client = memberStatusClient({
          status: variant,
          tokens_used: 0,
          is_final: false,
        });
        const result = await client.mobMemberStatus("mob-1", "worker-1");
        assert.equal(result.status, variant);
      });
    }

    it("rejects an absent status instead of fabricating 'unknown'", async () => {
      const client = memberStatusClient({ tokens_used: 0, is_final: false });
      await assert.rejects(
        () => client.mobMemberStatus("mob-1", "worker-1"),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });

    it("rejects a non-variant status string", async () => {
      const client = memberStatusClient({
        status: "halfbaked",
        tokens_used: 0,
        is_final: false,
      });
      await assert.rejects(
        () => client.mobMemberStatus("mob-1", "worker-1"),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });
  });

  // SITE 6 — mobSnapshot: lifecycle status is require-present-non-empty-string.
  describe("mobSnapshot", () => {
    function snapshotClient(response) {
      const client = new MeerkatClient();
      client.request = async () => response;
      return client;
    }

    it("parses a present lifecycle status", async () => {
      const client = snapshotClient({
        mob_id: "mob-1",
        status: "running",
        members: [],
      });
      const result = await client.mobSnapshot("mob-1");
      assert.equal(result.status, "running");
    });

    it("rejects an absent status instead of fabricating 'unknown'", async () => {
      const client = snapshotClient({ mob_id: "mob-1", members: [] });
      await assert.rejects(
        () => client.mobSnapshot("mob-1"),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          /missing status/.test(String(error.message)),
      );
    });
  });

  // SITE 7 — parseAttributedMobEvent: typed source {identity, generation} +
  // role; nothing coalesced to "".
  describe("parseAttributedMobEvent", () => {
    const validEnvelope = {
      source: { type: "session", session_id: "00000000-0000-4000-8000-000000000001" },
      payload: { type: "text_delta", delta: "hi" },
    };

    it("projects validated identity + role from the typed source", () => {
      const event = MeerkatClient.parseAttributedMobEvent({
        source: { identity: "worker-1", generation: 2 },
        role: "worker",
        envelope: validEnvelope,
      });
      assert.equal(event.source, "worker-1");
      assert.equal(event.profile, "worker");
      assert.deepEqual(event.envelope.source, {
        type: "session",
        sessionId: "00000000-0000-4000-8000-000000000001",
      });
    });

    it("rejects a missing source instead of coalescing to empty string", () => {
      assert.throws(
        () =>
          MeerkatClient.parseAttributedMobEvent({
            role: "worker",
            envelope: validEnvelope,
          }),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });

    it("rejects a non-string source (no [object Object] stringification)", () => {
      assert.throws(
        () =>
          MeerkatClient.parseAttributedMobEvent({
            source: { generation: 0 },
            role: "worker",
            envelope: validEnvelope,
          }),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });

    it("rejects a non-integer / negative generation", () => {
      assert.throws(
        () =>
          MeerkatClient.parseAttributedMobEvent({
            source: { identity: "worker-1", generation: -1 },
            role: "worker",
            envelope: validEnvelope,
          }),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });

    it("rejects a missing role instead of coalescing to empty string", () => {
      assert.throws(
        () =>
          MeerkatClient.parseAttributedMobEvent({
            source: { identity: "worker-1", generation: 0 },
            envelope: validEnvelope,
          }),
        (error) =>
          error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
      );
    });
  });
});

describe("Mob ready wait wrappers", () => {
  it("waitMobReady/wait_mob_ready/mob.waitForReady preserve canonical call shape", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        members: [
          {
            agent_identity: "lead",
            agent_runtime_id: { identity: "lead", generation: 1 },
            fence_token: 1,
            status: "active",
            tokens_used: 42,
            is_final: false,
          },
        ],
      };
    };

    const direct = await client.waitMobReady("mob-1", {
      memberIds: ["lead", "writer"],
      timeoutMs: 1234,
    });
    const legacy = await client.wait_mob_ready("mob-1", {
      memberIds: ["lead"],
    });
    const mob = new Mob(client, "mob-1");
    const fromHandle = await mob.waitForReady({ timeoutMs: 99 });

    assert.equal(calls.length, 3);
    assert.deepEqual(calls.map((call) => call.method), [
      "mob/wait_ready",
      "mob/wait_ready",
      "mob/wait_ready",
    ]);
    assert.deepEqual(calls[0].params, {
      mob_id: "mob-1",
      member_ids: ["lead", "writer"],
      timeout_ms: 1234,
    });
    assert.deepEqual(calls[1].params, {
      mob_id: "mob-1",
      member_ids: ["lead"],
    });
    assert.deepEqual(calls[2].params, {
      mob_id: "mob-1",
      timeout_ms: 99,
    });
    assert.equal(direct[0].agentIdentity, "lead");
    assert.equal(legacy[0].agentIdentity, "lead");
    assert.equal(fromHandle[0].agentIdentity, "lead");
  });
});

describe("Live wrappers", () => {
  it("returns generated live/close result only when required fields are present", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return { status: "closed" };
    };

    const result = await client.liveClose({ channel_id: "live_channel_41" });

    assert.deepEqual(calls, [
      { method: "live/close", params: { channel_id: "live_channel_41" } },
    ]);
    assert.deepEqual(result, { status: "closed" });
  });

  it("rejects missing or unknown live/close generated status", async () => {
    const malformedResponses = [
      { closed: true },
      { status: "already_closed" },
    ];

    for (const response of malformedResponses) {
      const client = new MeerkatClient();
      client.request = async (method) => {
        assert.equal(method, "live/close");
        return response;
      };

      await assert.rejects(
        () => client.liveClose({ channel_id: "live_channel_41" }),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          String(error.message).includes("Invalid live/close response"),
      );
    }
  });

  it("returns generated live/refresh result only when required fields are present", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return { status: "queued" };
    };

    const result = await client.liveRefresh({ channel_id: "live_channel_42" });

    assert.deepEqual(calls, [
      { method: "live/refresh", params: { channel_id: "live_channel_42" } },
    ]);
    assert.deepEqual(result, { status: "queued" });
  });

  it("rejects missing or unknown live/refresh generated status", async () => {
    const malformedResponses = [
      { refresh_enqueued: true },
      { status: "applied_sync" },
    ];

    for (const response of malformedResponses) {
      const client = new MeerkatClient();
      client.request = async (method) => {
        assert.equal(method, "live/refresh");
        return response;
      };

      await assert.rejects(
        () => client.liveRefresh({ channel_id: "live_channel_43" }),
        (error) =>
          error instanceof MeerkatError &&
          error.code === "INVALID_RESPONSE" &&
          String(error.message).includes("Invalid live/refresh response"),
      );
    }
  });
});

describe("Mob member host ingress", () => {
  it("routes member sends through the canonical host member-send lane", async () => {
    const client = new MeerkatClient();
    const calls = [];
    const expectedRef = makeMemberRef("mob-1", "reviewer-1");
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        agent_identity: "reviewer-1",
        member_ref: expectedRef,
        handling_mode: "steer",
      };
    };

    const receipt = await new Mob(client, "mob-1").member("reviewer-1").send(
      "hello reviewer",
      {
        handlingMode: "steer",
        renderMetadata: {
          class: "peer_request",
          salience: "urgent",
        },
      },
    );

    assert.deepEqual(receipt, {
      agentIdentity: "reviewer-1",
      memberRef: expectedRef,
      handlingMode: "steer",
    });
    assert.deepEqual(calls, [
      {
        method: "mob/member_send",
        params: {
          mob_id: "mob-1",
          agent_identity: "reviewer-1",
          content: "hello reviewer",
          handling_mode: "steer",
          render_metadata: {
            class: "peer_request",
            salience: "urgent",
          },
        },
      },
    ]);
  });

  it("rejects malformed member-send receipts", async () => {
    const client = new MeerkatClient();
    client.request = async () => ({ handling_mode: "queue" });

    await assert.rejects(
      () => new Mob(client, "mob-1").member("reviewer-1").send("hello reviewer"),
      /missing member_ref/,
    );
  });
});

// ---------------------------------------------------------------------------
// SDK fail-closed parse boundaries (DOGMA remediation rows 55/88/161/192/246/357)
// ---------------------------------------------------------------------------

describe("Member-send handling-mode parse boundary (row 357)", () => {
  it("returns the runtime-reported handling mode without substitution", async () => {
    const client = new MeerkatClient();
    const expectedRef = makeMemberRef("mob-1", "reviewer-1");
    client.request = async () => ({
      agent_identity: "reviewer-1",
      member_ref: expectedRef,
      handling_mode: "queue",
    });

    // Client requested "steer"; runtime receipt says "queue" — the receipt wins.
    const receipt = await new Mob(client, "mob-1").member("reviewer-1").send("hi", {
      handlingMode: "steer",
    });
    assert.equal(receipt.handlingMode, "queue");
  });

  it("throws INVALID_RESPONSE on a missing handling_mode instead of substituting the requested mode", async () => {
    const client = new MeerkatClient();
    const expectedRef = makeMemberRef("mob-1", "reviewer-1");
    client.request = async () => ({
      agent_identity: "reviewer-1",
      member_ref: expectedRef,
      // handling_mode intentionally absent
    });

    await assert.rejects(
      () => new Mob(client, "mob-1").member("reviewer-1").send("hi", { handlingMode: "queue" }),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        /handling_mode/.test(error.message),
    );
  });

  it("throws INVALID_RESPONSE on an unknown handling_mode variant", async () => {
    const client = new MeerkatClient();
    const expectedRef = makeMemberRef("mob-1", "reviewer-1");
    client.request = async () => ({
      agent_identity: "reviewer-1",
      member_ref: expectedRef,
      handling_mode: "interrupt",
    });

    await assert.rejects(
      () => new Mob(client, "mob-1").member("reviewer-1").send("hi"),
      (error) => error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
    );
  });
});

describe("Session fork result parse boundary (row 88)", () => {
  it("parses a well-formed fork result", () => {
    const result = MeerkatClient.parseSessionForkResult({
      source_session_id: "src-1",
      session_id: "fork-1",
      session_ref: "ref-1",
      message_count: 4,
    });
    assert.equal(result.sourceSessionId, "src-1");
    assert.equal(result.sessionId, "fork-1");
    assert.equal(result.sessionRef, "ref-1");
    assert.equal(result.messageCount, 4);
  });

  it("throws INVALID_RESPONSE when session_id is missing instead of fabricating an empty handle", () => {
    assert.throws(
      () =>
        MeerkatClient.parseSessionForkResult({
          source_session_id: "src-1",
          message_count: 4,
        }),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        /session_id/.test(error.message),
    );
  });

  it("throws INVALID_RESPONSE when message_count is malformed instead of coercing to zero", () => {
    assert.throws(
      () =>
        MeerkatClient.parseSessionForkResult({
          source_session_id: "src-1",
          session_id: "fork-1",
          message_count: "many",
        }),
      (error) => error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
    );
  });
});

describe("Session history parse boundary (row 192)", () => {
  it("throws INVALID_RESPONSE when session_id is missing instead of returning an empty transcript", () => {
    assert.throws(
      () =>
        MeerkatClient.parseSessionHistory({
          message_count: 0,
          offset: 0,
          has_more: false,
          messages: [],
        }),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        /session_id/.test(error.message),
    );
  });

  it("throws INVALID_RESPONSE when message_count is malformed instead of coercing to zero", () => {
    assert.throws(
      () =>
        MeerkatClient.parseSessionHistory({
          session_id: "s1",
          message_count: null,
          offset: 0,
          has_more: false,
          messages: [],
        }),
      (error) => error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
    );
  });

  it("throws INVALID_RESPONSE for transcript revisions missing required fields", () => {
    assert.throws(
      () =>
        MeerkatClient.parseSessionTranscriptRevision({
          session_id: "s1",
          // revision/head_revision/message_count missing
          offset: 0,
          has_more: false,
          messages: [],
        }),
      (error) => error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
    );
  });
});

describe("Capability status parse boundary (row 161)", () => {
  it("parses a bare-string capability status", () => {
    const cap = MeerkatClient.parseWireCapabilityEntry({
      id: "memory",
      description: "Semantic memory",
      status: "Available",
    });
    assert.deepEqual(cap, {
      id: "memory",
      description: "Semantic memory",
      status: "Available",
    });
  });

  it("parses an externally-tagged capability status object without fabricating 'Unknown'", () => {
    const cap = MeerkatClient.parseWireCapabilityEntry({
      id: "memory",
      description: "Semantic memory",
      status: { DisabledByPolicy: { description: "blocked" } },
    });
    assert.equal(cap.status, "DisabledByPolicy");
  });

  it("throws INVALID_RESPONSE on a missing capability status instead of coalescing to 'Available'/'Unknown'", () => {
    assert.throws(
      () =>
        MeerkatClient.parseWireCapabilityEntry({
          id: "memory",
          description: "Semantic memory",
          // status absent
        }),
      (error) => error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
    );
  });

  it("throws INVALID_RESPONSE on an unparseable (numeric) capability status", () => {
    assert.throws(
      () =>
        MeerkatClient.parseWireCapabilityEntry({
          id: "memory",
          description: "Semantic memory",
          status: 7,
        }),
      (error) => error instanceof MeerkatError && error.code === "INVALID_RESPONSE",
    );
  });

  it("throws INVALID_RESPONSE on a missing capability id instead of coercing to ''", () => {
    assert.throws(
      () =>
        MeerkatClient.parseWireCapabilityEntry({
          description: "Semantic memory",
          status: "Available",
        }),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        /id/.test(error.message),
    );
  });
});

describe("Transport corrupted-frame fault (row 246)", () => {
  it("surfaces a typed PROTOCOL_ERROR to pending callers instead of silently dropping a corrupted frame", async () => {
    const client = new MeerkatClient();
    client.process = { stdin: { write: () => {}, destroy: () => {} }, kill: () => {} };

    // Register a pending request through the real request bookkeeping.
    const pending = client.registerRequest(client.requestId + 1);

    // Feed a corrupted (non-JSON) frame into the read loop.
    client.handleLine("this is not json{");

    await assert.rejects(
      () => pending,
      (error) => error instanceof MeerkatError && error.code === "PROTOCOL_ERROR",
    );
  });

  it("marks the client unusable after transport corruption: later calls reject with the recorded fault, process is torn down", async () => {
    let killed = false;
    const client = new MeerkatClient();
    client.process = {
      stdin: { write: () => {}, destroy: () => {} },
      kill: () => {
        killed = true;
      },
    };

    client.handleLine("this is not json{");

    // The condemned process must be torn down, not left writable.
    assert.equal(killed, true);
    assert.equal(client.process, null);

    // A later SDK call must reject immediately with the recorded typed fault —
    // never NOT_CONNECTED ambiguity, never a write into the dead stream.
    await assert.rejects(
      async () => client.request("ping", {}),
      (error) => error instanceof MeerkatError && error.code === "PROTOCOL_ERROR",
    );
  });
});

describe("Callback tool result content type (row 55)", () => {
  it("delivers a multimodal block array to the runtime as blocks rather than a stringified blob", async () => {
    const client = new MeerkatClient();
    const blocks = [
      { type: "text", text: "see image" },
      { type: "image", media_type: "image/png", source: "blob", blob_id: "blob_1" },
    ];
    // Register the tool before attaching a process so the eager
    // `tools/register` round-trip is skipped and only the callback response
    // write is captured below.
    client.registerTool("render", "Render", { type: "object" }, async () => blocks);

    const writes = [];
    client.process = { stdin: { write: (line) => writes.push(line) } };

    client.handleLine(
      JSON.stringify({
        jsonrpc: "2.0",
        id: 42,
        method: "tool/execute",
        params: { name: "render", arguments: {} },
      }),
    );

    // Allow the handler promise + writeCallbackResponse microtask to settle.
    await new Promise((resolve) => setImmediate(resolve));

    assert.equal(writes.length, 1);
    const response = JSON.parse(writes[0]);
    assert.equal(response.id, 42);
    assert.equal(response.result.is_error, false);
    assert.deepEqual(response.result.content, blocks);
  });
});

describe("Session transcript fail-closed parsing", () => {
  it("rejects messages missing identity facts instead of fabricating them", () => {
    // Missing role.
    assert.throws(
      () => MeerkatClient.parseSessionMessage({ content: "no role" }),
      /missing role/,
    );
    // Missing created_at (required on every WireSessionMessage variant).
    assert.throws(
      () => MeerkatClient.parseSessionMessage({ role: "user", content: "x" }),
      /missing created_at/,
    );
    // Tool result missing tool_use_id.
    assert.throws(
      () =>
        MeerkatClient.parseSessionMessage({
          role: "tool_results",
          created_at: "2026-05-26T10:00:00Z",
          results: [{ content: "missing tool_use_id" }],
        }),
      /missing tool_use_id/,
    );
    // Tool result missing mandatory content — must not be coalesced into
    // an empty transcript string.
    assert.throws(
      () =>
        MeerkatClient.parseSessionMessage({
          role: "tool_results",
          created_at: "2026-05-26T10:00:00Z",
          results: [{ tool_use_id: "tc_1", is_error: false }],
        }),
      /missing content/,
    );
    // Present-but-non-list blocks.
    assert.throws(
      () =>
        MeerkatClient.parseSessionMessage({
          role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
          created_at: "2026-05-26T10:00:00Z",
          blocks: "not-a-list",
        }),
      /blocks must be a list/,
    );
    // Block missing block_type.
    assert.throws(
      () =>
        MeerkatClient.parseSessionMessage({
          role: "block_assistant",
          created_at: "2026-01-01T00:00:00Z",
          created_at: "2026-05-26T10:00:00Z",
          blocks: [{ data: { text: "missing block_type" } }],
        }),
      /missing block_type/,
    );
  });

  it("rejects transcript rewrite results missing identity facts", () => {
    assert.throws(
      () =>
        MeerkatClient.parseSessionTranscriptRewriteResult({
          revision: "r2",
          parent_revision: "r1",
          message_count: 1,
          commit: {},
        }),
      /missing session_id/,
    );
    assert.throws(
      () =>
        MeerkatClient.parseSessionTranscriptRewriteResult({
          session_id: "s1",
          revision: "r2",
          parent_revision: "r1",
          message_count: 1,
        }),
      /commit must be an object/,
    );
  });
});
