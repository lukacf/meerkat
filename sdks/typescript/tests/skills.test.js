/**
 * Tests for skills v2.1 plus Session comms convenience methods.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { Session } from "../dist/session.js";
import { MeerkatClient } from "../dist/client.js";

describe("Skills v2.1", () => {
  it("Session.invokeSkill sends structured skillRefs to _startTurn", async () => {
    const calls = [];
    const mockClient = {
      hasCapability(name) {
        return name === "skills";
      },
      requireCapability(name) {
        if (!this.hasCapability(name)) {
          throw new Error(`Missing capability: ${name}`);
        }
      },
      async _startTurn(sessionId, prompt, options) {
        calls.push({ sessionId, prompt, options });
        return {
          sessionId: "s-1",
          text: "ok",
          turns: 1,
          toolCalls: 0,
          usage: { inputTokens: 10, outputTokens: 5 },
        };
      },
    };

    const initialResult = {
      sessionId: "s-1",
      text: "init",
      turns: 0,
      toolCalls: 0,
      usage: { inputTokens: 0, outputTokens: 0 },
    };
    const session = new Session(mockClient, initialResult);

    const result = await session.invokeSkill(
      { sourceUuid: "abc-123", skillName: "email-extractor" },
      "run the extractor",
    );

    assert.equal(result.text, "ok");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].prompt, "run the extractor");
    assert.deepEqual(calls[0].options.skillRefs, [
      { sourceUuid: "abc-123", skillName: "email-extractor" },
    ]);
  });

  it("Session.invokeSkill also accepts legacy string refs", async () => {
    const calls = [];
    const originalWarn = console.warn;
    const warnings = [];
    console.warn = (msg) => { warnings.push(String(msg ?? "")); };

    const mockClient = {
      hasCapability() { return true; },
      requireCapability() {},
      async _startTurn(sessionId, prompt, options) {
        calls.push({ sessionId, prompt, options });
        return {
          sessionId: "s-1",
          text: "ok",
          turns: 1,
          toolCalls: 0,
          usage: { inputTokens: 10, outputTokens: 5 },
        };
      },
    };

    const session = new Session(mockClient, {
      sessionId: "s-1", text: "init", turns: 0, toolCalls: 0,
      usage: { inputTokens: 0, outputTokens: 0 },
    });

    try {
      await session.invokeSkill(
        "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor",
        "run",
      );
    } finally {
      console.warn = originalWarn;
    }

    assert.equal(calls.length, 1);
    // Legacy string refs get passed through to _startTurn as-is; normalization
    // happens at the wire boundary in buildCreateParams / _startTurn
    assert.deepEqual(calls[0].options.skillRefs, [
      "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor",
    ]);
  });

  it("parseRunResult includes skillDiagnostics", () => {
    const raw = {
      session_id: "s1",
      text: "Hello!",
      turns: 1,
      tool_calls: 0,
      usage: { input_tokens: 100, output_tokens: 50 },
      skill_diagnostics: {
        source_health: {
          state: "healthy",
          invalid_ratio: 0.0,
          invalid_count: 0,
          total_count: 10,
          failure_streak: 0,
          handshake_failed: false,
        },
        quarantined: [],
      },
    };
    const result = MeerkatClient.parseRunResult(raw);
    assert.deepEqual(result.skillDiagnostics, {
      sourceHealth: {
        state: "healthy",
        invalidRatio: 0,
        invalidCount: 0,
        totalCount: 10,
        failureStreak: 0,
        handshakeFailed: false,
      },
      quarantined: [],
    });
  });

  it("parseRunResult handles missing skillDiagnostics", () => {
    const raw = {
      session_id: "s1",
      text: "Hello!",
      turns: 1,
      tool_calls: 0,
      usage: { input_tokens: 100, output_tokens: 50 },
    };
    const result = MeerkatClient.parseRunResult(raw);
    assert.equal(result.skillDiagnostics, undefined);
  });

  it("Session.send forwards command through _send", async () => {
    const calls = [];
    const mockClient = {
      async _send(sessionId, command) {
        calls.push({ sessionId, command });
        return { queued: true };
      },
    };

    const session = new Session(mockClient, {
      sessionId: "s-1", text: "init", turns: 0, toolCalls: 0,
      usage: { inputTokens: 0, outputTokens: 0 },
    });

    const result = await session.send({
      kind: "peer_message",
      to: "agent-b",
      body: "hello",
    });

    assert.deepEqual(result, { queued: true });
    assert.equal(calls.length, 1);
    assert.equal(calls[0].sessionId, "s-1");
    assert.deepEqual(calls[0].command, {
      kind: "peer_message",
      to: "agent-b",
      body: "hello",
    });
  });

  it("Session.peers returns peers list from _peers", async () => {
    const calls = [];
    const mockClient = {
      async _peers(sessionId) {
        calls.push(sessionId);
        return { peers: [{ id: "peer-a" }, { id: "peer-b" }] };
      },
    };

    const session = new Session(mockClient, {
      sessionId: "s-1", text: "init", turns: 0, toolCalls: 0,
      usage: { inputTokens: 0, outputTokens: 0 },
    });

    const peers = await session.peers();
    assert.deepEqual(peers, [{ id: "peer-a" }, { id: "peer-b" }]);
    assert.deepEqual(calls, ["s-1"]);
  });

  it("createSessionStreaming buffers early events before session id is bound", async () => {
    const client = new MeerkatClient();
    client.process = { stdin: { write() {} } };

    let resolveResponse;
    client.registerRequest = () =>
      new Promise((resolve) => {
        resolveResponse = resolve;
      });

    const stream = client.createSessionStreaming("hello");
    client.handleLine(
      JSON.stringify({
        method: "session/event",
        params: {
          session_id: "s-early",
          event: { type: "text_delta", delta: "early" },
        },
      }),
    );
    resolveResponse({
      session_id: "s-early",
      text: "done",
      turns: 1,
      tool_calls: 0,
      usage: { input_tokens: 1, output_tokens: 1 },
    });

    const [text, result] = await stream.collectText();
    assert.equal(text, "early");
    assert.equal(result.sessionId, "s-early");
  });

  it("createSessionStreaming does not clear buffered standalone stream events", async () => {
    const client = new MeerkatClient();
    client.process = { stdin: { write() {} } };
    client.unmatchedStandaloneStreamBuffer.set("stream-1", [{ event_id: "e1" }]);
    client.registerRequest = () => Promise.resolve({ session_id: "s-created" });

    client.createSessionStreaming("hello");
    await new Promise((resolve) => setImmediate(resolve));

    assert.deepEqual(client.unmatchedStandaloneStreamBuffer.get("stream-1"), [{ event_id: "e1" }]);
  });

  it("setConfig returns the config envelope response", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      return { config: { agent: { model: "x" } }, generation: 3 };
    };

    const response = await client.setConfig({ agent: { model: "x" } }, { expectedGeneration: 2 });
    assert.equal(calls.length, 1);
    assert.equal(calls[0].method, "config/set");
    assert.equal(calls[0].params.expected_generation, 2);
    assert.equal(response.generation, 3);
  });


  it("buffers standalone stream termination until the subscription queue is registered", async () => {
    const client = new MeerkatClient();
    client.request = async (method, params) => {
      if (method === "session/stream_open") {
        client.handleLine(JSON.stringify({
          jsonrpc: "2.0",
          method: "session/stream_end",
          params: {
            stream_id: "stream-end-1",
            ended: true,
            outcome: "remote_end",
          },
        }));
        return { stream_id: "stream-end-1" };
      }
      if (method === "session/stream_close") {
        return { closed: true };
      }
      return {};
    };

    const sub = await client.subscribeSessionEvents("s1");
    const iterator = sub[Symbol.asyncIterator]();
    const first = await Promise.race([
      iterator.next(),
      new Promise((_, reject) => setTimeout(() => reject(new Error("stream did not terminate")), 200)),
    ]);
    assert.equal(first.done, true);
    assert.deepEqual(sub.terminalOutcome, {
      stream_id: "stream-end-1",
      ended: true,
      outcome: "remote_end",
    });
    await sub.close();
  });

  it("buffers standalone stream notifications until the subscription queue is registered", async () => {
    const client = new MeerkatClient();
    client.request = async (method, params) => {
      if (method === "session/stream_open") {
        client.handleLine(JSON.stringify({
          jsonrpc: "2.0",
          method: "session/stream_event",
          params: {
            stream_id: "stream-1",
            event: { event_id: "e1", payload: { type: "text_delta", delta: "hi" } },
          },
        }));
        return { stream_id: "stream-1" };
      }
      if (method === "session/stream_close") {
        return { closed: true };
      }
      return {};
    };

    const sub = await client.subscribeSessionEvents("s1");
    const iterator = sub[Symbol.asyncIterator]();
    const first = await iterator.next();
    assert.equal(first.done, false);
    assert.equal(first.value.payload.type, "text_delta");
    assert.equal(first.value.payload.delta, "hi");
    await sub.close();
  });

  it("preserves buffered standalone stream notification order", async () => {
    const client = new MeerkatClient();
    client.request = async (method, params) => {
      if (method === "session/stream_open") {
        client.handleLine(JSON.stringify({
          jsonrpc: "2.0",
          method: "session/stream_event",
          params: {
            stream_id: "stream-ordered",
            event: { event_id: "e1", payload: { type: "text_delta", delta: "hi" } },
          },
        }));
        client.handleLine(JSON.stringify({
          jsonrpc: "2.0",
          method: "session/stream_event",
          params: {
            stream_id: "stream-ordered",
            event: { event_id: "e2", payload: { type: "text_delta", delta: "there" } },
          },
        }));
        return { stream_id: "stream-ordered" };
      }
      if (method === "session/stream_close") {
        return { closed: true };
      }
      return {};
    };

    const sub = await client.subscribeSessionEvents("s1");
    const iterator = sub[Symbol.asyncIterator]();
    const first = await iterator.next();
    const second = await iterator.next();
    assert.equal(first.done, false);
    assert.equal(second.done, false);
    assert.equal(first.value.payload.type, "text_delta");
    assert.equal(first.value.payload.delta, "hi");
    assert.equal(second.value.payload.type, "text_delta");
    assert.equal(second.value.payload.delta, "there");
    await sub.close();
  });

  it("listMobTools/callMobTool route through mob RPC methods", async () => {
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method, params) => {
      calls.push({ method, params });
      if (method === "mob/tools") {
        return { tools: [{ name: "mob_create" }] };
      }
      if (method === "mob/call") {
        return { ok: true, mob_id: "m1" };
      }
      return {};
    };

    const tools = await client.listMobTools();
    const result = await client.callMobTool("mob_create", { prefab: "coding_swarm" });

    assert.deepEqual(tools, [{ name: "mob_create" }]);
    assert.equal(result.mob_id, "m1");
    assert.deepEqual(calls, [
      { method: "mob/tools", params: {} },
      {
        method: "mob/call",
        params: { name: "mob_create", arguments: { prefab: "coding_swarm" } },
      },
    ]);
  });
});
