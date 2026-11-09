/**
 * Tests for backward-compatible low-level MetaAsyncClient.
 */

import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";

import { MetaAsyncClient, RpcError, MetaRpcError } from "../dist/index.js";

// ---------------------------------------------------------------------------
// Mock transport helper
// ---------------------------------------------------------------------------

interface TransportCall {
  id: string;
  method: string;
  params: Record<string, unknown>;
}

function createMockClient(): {
  client: MetaAsyncClient;
  calls: TransportCall[];
  setResponse: (fn: (req: any) => unknown) => void;
} {
  const calls: TransportCall[] = [];
  let responseFn: (req: any) => unknown = () => ({});

  const mockTransport = async (request: any): Promise<unknown> => {
    calls.push({
      id: request.id,
      method: request.method,
      params: request.params,
    });
    return responseFn(request);
  };

  // Construct with the mock transport
  const client = new MetaAsyncClient(mockTransport as any);

  return {
    client,
    calls,
    setResponse: (fn) => { responseFn = fn; },
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("MetaRpcError backward compat", () => {
  it("MetaRpcError is the same as RpcError", () => {
    assert.equal(MetaRpcError, RpcError);
  });

  it("MetaRpcError instances are instances of RpcError", () => {
    const err = new MetaRpcError(-32600, "invalid", "req-1", "test/method");
    assert.ok(err instanceof RpcError);
    assert.equal(err.code, -32600);
    assert.equal(err.message, "invalid");
    assert.equal(err.requestId, "req-1");
    assert.equal(err.method, "test/method");
  });
});

describe("MetaAsyncClient.status()", () => {
  it("sends meta/status and returns typed result", async () => {
    const { client, calls, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: {
        contract_version: "0.5.0",
        running: true,
        loaded_modules: ["mod-a"],
      },
    }));

    const result = await client.status("test-status");
    assert.equal(calls[0].method, "meta/status");
    assert.equal(calls[0].id, "test-status");
    assert.deepEqual(calls[0].params, {});
    assert.equal(result.contract_version, "0.5.0");
    assert.equal(result.running, true);
    assert.deepEqual(result.loaded_modules, ["mod-a"]);
  });
});

describe("MetaAsyncClient.capabilities()", () => {
  it("sends meta/capabilities and returns typed result", async () => {
    const { client, calls, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: {
        contract_version: "0.5.0",
        methods: ["meta/status", "meta/capabilities"],
        loaded_modules: ["mod-a", "mod-b"],
      },
    }));

    const result = await client.capabilities("test-caps");
    assert.equal(calls[0].method, "meta/capabilities");
    assert.equal(calls[0].id, "test-caps");
    assert.equal(result.contract_version, "0.5.0");
    assert.deepEqual(result.methods, ["meta/status", "meta/capabilities"]);
    assert.deepEqual(result.loaded_modules, ["mod-a", "mod-b"]);
  });
});

describe("MetaAsyncClient.reconcile()", () => {
  it("sends meta/reconcile and returns typed result", async () => {
    const { client, calls, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: {
        accepted: true,
        reconciled_modules: ["mod-a", "mod-b"],
        added: 2,
      },
    }));

    const result = await client.reconcile(["mod-a", "mod-b"], "test-reconcile");
    assert.equal(calls[0].method, "meta/reconcile");
    assert.deepEqual(calls[0].params, { modules: ["mod-a", "mod-b"] });
    assert.equal(result.accepted, true);
    assert.deepEqual(result.reconciled_modules, ["mod-a", "mod-b"]);
    assert.equal(result.added, 2);
  });
});

describe("MetaAsyncClient.spawnMember()", () => {
  it("sends meta/spawn_member and returns typed result", async () => {
    const { client, calls, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: {
        accepted: true,
        module_id: "mod-x",
      },
    }));

    const result = await client.spawnMember("mod-x", "test-spawn");
    assert.equal(calls[0].method, "meta/spawn_member");
    assert.deepEqual(calls[0].params, { module_id: "mod-x" });
    assert.equal(result.accepted, true);
    assert.equal(result.module_id, "mod-x");
  });
});

describe("MetaAsyncClient.subscribeEvents()", () => {
  it("sends meta/events/subscribe and returns typed result", async () => {
    const { client, calls, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: {
        scope: "mob",
        replay_from_event_id: null,
        keep_alive: { interval_ms: 30000, event: "ping" },
        keep_alive_comment: "keep-alive comment",
        event_frames: ["frame-1"],
        events: [
          {
            event_id: "e-1",
            source: "system",
            timestamp_ms: 1000,
            event: { type: "test" },
          },
        ],
      },
    }));

    const result = await client.subscribeEvents(
      { scope: "mob", last_event_id: "evt-0", agent_id: "a-1" },
      "test-subscribe",
    );
    assert.equal(calls[0].method, "meta/events/subscribe");
    assert.equal(calls[0].params.scope, "mob");
    assert.equal(calls[0].params.last_event_id, "evt-0");
    assert.equal(calls[0].params.agent_id, "a-1");
    assert.equal(result.scope, "mob");
    assert.equal(result.replay_from_event_id, null);
    assert.equal(result.keep_alive.interval_ms, 30000);
    assert.equal(result.keep_alive.event, "ping");
    assert.equal(result.keep_alive_comment, "keep-alive comment");
    assert.deepEqual(result.event_frames, ["frame-1"]);
    assert.equal(result.events.length, 1);
    assert.equal(result.events[0].event_id, "e-1");
  });

  it("uses defaults when no params provided", async () => {
    const { client, calls, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: {
        scope: "mob",
        replay_from_event_id: null,
        keep_alive: { interval_ms: 30000, event: "ping" },
        keep_alive_comment: "",
        event_frames: [],
        events: [],
      },
    }));

    await client.subscribeEvents();
    assert.deepEqual(calls[0].params, {});
  });
});

describe("MetaAsyncClient RPC error handling", () => {
  it("throws RpcError on error response", async () => {
    const { client, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      error: {
        code: -32601,
        message: "Method not found",
      },
    }));

    await assert.rejects(
      () => client.status("err-test"),
      (err: any) => {
        assert.ok(err instanceof RpcError);
        assert.equal(err.code, -32601);
        assert.equal(err.message, "Method not found");
        assert.equal(err.requestId, "err-test");
        assert.equal(err.method, "meta/status");
        return true;
      },
    );
  });
});

describe("MetaAsyncClient invalid response handling", () => {
  it("throws on invalid JSON-RPC envelope", async () => {
    const { client, setResponse } = createMockClient();
    setResponse(() => ({ not_jsonrpc: true }));

    await assert.rejects(
      () => client.status("bad-envelope"),
      (err: any) => {
        assert.ok(err instanceof Error);
        assert.match(err.message, /invalid JSON-RPC response envelope/);
        return true;
      },
    );
  });

  it("throws on invalid result payload", async () => {
    const { client, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: { unexpected: true },
    }));

    await assert.rejects(
      () => client.status("bad-payload"),
      (err: any) => {
        assert.ok(err instanceof Error);
        assert.match(err.message, /invalid result payload/);
        return true;
      },
    );
  });
});

describe("MetaAsyncClient.rpc() raw access", () => {
  it("returns raw JsonRpcResponse", async () => {
    const { client, setResponse } = createMockClient();
    setResponse((req: any) => ({
      jsonrpc: "2.0",
      id: req.id,
      result: { custom: "data" },
    }));

    const response = await client.rpc("raw-1", "custom/method", { key: "val" });
    assert.equal((response as any).jsonrpc, "2.0");
    assert.equal((response as any).id, "raw-1");
    assert.deepEqual((response as any).result, { custom: "data" });
  });
});
