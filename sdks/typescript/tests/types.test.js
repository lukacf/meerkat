/**
 * Conformance tests for Meerkat TypeScript SDK types and events.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
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
  isTurnCompleted,
  MeerkatClient,
  Session,
  DeferredSession,
} from "../dist/index.js";

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

  it("matches the published package version", () => {
    assert.equal(CONTRACT_VERSION, packageJson.version);
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

  it("should parse scoped wrapper events", () => {
    const event = parseEvent({
      scope_id: "primary/sub:op-1",
      scope_path: [
        { scope: "primary", session_id: "s1" },
        { scope: "sub_agent", agent_id: "op-1", label: "spawn" },
      ],
      event: { type: "text_delta", delta: "hello" },
    });
    assert.equal(event.type, "scoped_agent_event");
    if (event.type === "scoped_agent_event") {
      assert.equal(event.scopeId, "primary/sub:op-1");
      assert.equal(event.event.type, "text_delta");
    }
  });

  it("should parse tool_config_changed payload", () => {
    const event = parseEvent({
      type: "tool_config_changed",
      payload: {
        operation: "remove",
        target: "filesystem",
        status: "staged",
        persisted: false,
        applied_at_turn: 7,
      },
    });
    assert.equal(event.type, "tool_config_changed");
    if (event.type === "tool_config_changed") {
      assert.equal(event.payload.operation, "remove");
      assert.equal(event.payload.target, "filesystem");
      assert.equal(event.payload.status, "staged");
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
        { role: "system", content: "rules" },
        {
          role: "assistant",
          content: "working",
          tool_calls: [{ id: "tc_1", name: "search", args: { q: "rust" } }],
          stop_reason: "tool_use",
        },
        {
          role: "block_assistant",
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
    assert.equal(history.messages[1].toolCalls[0].name, "search");
    assert.equal(history.messages[2].blocks[0].name, "lookup");
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

  it("should tolerate malformed tool_config_changed payload", () => {
    const event = parseEvent({
      type: "tool_config_changed",
      payload: "not-an-object",
    });
    assert.equal(event.type, "tool_config_changed");
    if (event.type === "tool_config_changed") {
      assert.equal(event.payload.operation, "reload");
      assert.equal(event.payload.target, "");
      assert.equal(event.payload.status, "");
      assert.equal(event.payload.persisted, false);
      assert.equal(event.payload.applied_at_turn, undefined);
    }
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

  it("should treat non-boolean persisted in tool_config_changed as false", () => {
    const event = parseEvent({
      type: "tool_config_changed",
      payload: {
        operation: "add",
        target: "filesystem",
        status: "staged",
        persisted: "false",
      },
    });
    assert.equal(event.type, "tool_config_changed");
    if (event.type === "tool_config_changed") {
      assert.equal(event.payload.persisted, false);
    }
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
    });
    const peers = await client.peers("s1");

    assert.equal(sendReceipt.kind, "peer_message_sent");
    assert.deepEqual(peers.peers, [{ name: "agent-a" }]);
    assert.deepEqual(calls.map((call) => call.method), ["comms/send", "comms/peers"]);
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
          { role: "user", content: "hello" },
          { role: "assistant", content: "ok", stop_reason: "end_turn" },
        ],
      };
    };

    const history = await client.readSessionHistory("s1", { offset: 1, limit: 2 });

    assert.equal(history.sessionId, "s1");
    assert.equal(history.sessionRef, "history-ref");
    assert.equal(history.offset, 1);
    assert.equal(history.limit, 2);
    assert.equal(history.messages[1].role, "assistant");
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
      server_name: "filesystem",
      server_config: { cmd: "npx" },
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
    assert.equal(calls[0].params.server_name, "filesystem");
    assert.equal(calls[1].params.persisted, true);
  });

  it("propagates transport/request failures for mcp methods", async () => {
    const client = new MeerkatClient();
    client.request = async () => {
      throw new MeerkatError("TRANSPORT", "boom");
    };

    await assert.rejects(
      () => client.mcpAdd({ session_id: "s1", server_name: "fs", server_config: {} }),
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
      () => client.mcpAdd({ session_id: "s1", server_name: "fs", server_config: {} }),
      /persisted must be boolean/,
    );
  });
});

describe("Mob prefab methods", () => {
  it("listMobPrefabs/list_mob_prefabs send mob/prefabs and return prefabs", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return {
        prefabs: [
          { key: "coding_swarm", toml_template: "id = \"coding_swarm\"" },
          { key: "pipeline", toml_template: "id = \"pipeline\"" },
        ],
      };
    };

    const first = await client.listMobPrefabs();
    const second = await client.list_mob_prefabs();

    assert.equal(calls.length, 2);
    assert.deepEqual(calls.map((call) => call.method), ["mob/prefabs", "mob/prefabs"]);
    assert.deepEqual(first.map((p) => p.key), ["coding_swarm", "pipeline"]);
    assert.deepEqual(second.map((p) => p.key), ["coding_swarm", "pipeline"]);
  });

  it("listMobPrefabs propagates request failures", async () => {
    const client = new MeerkatClient();
    client.request = async () => {
      throw new MeerkatError("TRANSPORT", "boom");
    };

    await assert.rejects(() => client.listMobPrefabs(), /boom/);
    await assert.rejects(() => client.list_mob_prefabs(), /boom/);
  });
});
