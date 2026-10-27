import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  MetaError,
  TransportError,
  RpcError,
  CapabilityUnavailableError,
  ConsoleTimelineReplayUnavailableError,
  WorkGraphUnavailableError,
  WorkGraphConflictError,
  ContractMismatchError,
  NotConnectedError,
  MetaRpcError,
} from "../dist/index.js";

// ---------------------------------------------------------------------------
// MetaError (base)
// ---------------------------------------------------------------------------

describe("MetaError", () => {
  it("is an instance of Error", () => {
    const err = new MetaError("base error");
    assert.ok(err instanceof Error);
  });

  it("is an instance of MetaError", () => {
    const err = new MetaError("base error");
    assert.ok(err instanceof MetaError);
  });

  it("has name set to MetaError", () => {
    const err = new MetaError("msg");
    assert.equal(err.name, "MetaError");
  });

  it("has the correct message", () => {
    const err = new MetaError("something went wrong");
    assert.equal(err.message, "something went wrong");
  });
});

// ---------------------------------------------------------------------------
// TransportError
// ---------------------------------------------------------------------------

describe("TransportError", () => {
  it("extends MetaError", () => {
    const err = new TransportError("transport fail");
    assert.ok(err instanceof MetaError);
  });

  it("is an instance of Error", () => {
    const err = new TransportError("transport fail");
    assert.ok(err instanceof Error);
  });

  it("has name set to TransportError", () => {
    const err = new TransportError("msg");
    assert.equal(err.name, "TransportError");
  });

  it("has the correct message", () => {
    const err = new TransportError("connection refused");
    assert.equal(err.message, "connection refused");
  });
});

// ---------------------------------------------------------------------------
// RpcError
// ---------------------------------------------------------------------------

describe("RpcError", () => {
  it("extends MetaError", () => {
    const err = new RpcError(-32600, "invalid request", "req-1", "mob.status");
    assert.ok(err instanceof MetaError);
  });

  it("is an instance of Error", () => {
    const err = new RpcError(-32600, "invalid request", "req-1", "mob.status");
    assert.ok(err instanceof Error);
  });

  it("has name set to RpcError", () => {
    const err = new RpcError(-32600, "msg", "req-1", "mob.status");
    assert.equal(err.name, "RpcError");
  });

  it("stores code", () => {
    const err = new RpcError(-32601, "not found", "req-2", "mob.spawn");
    assert.equal(err.code, -32601);
  });

  it("stores requestId", () => {
    const err = new RpcError(-32600, "bad", "req-42", "mob.status");
    assert.equal(err.requestId, "req-42");
  });

  it("stores method", () => {
    const err = new RpcError(-32600, "bad", "req-1", "mob.reconcile");
    assert.equal(err.method, "mob.reconcile");
  });

  it("has the correct message", () => {
    const err = new RpcError(-32600, "invalid params", "r", "m");
    assert.equal(err.message, "invalid params");
  });
});

// ---------------------------------------------------------------------------
// CapabilityUnavailableError
// ---------------------------------------------------------------------------

describe("CapabilityUnavailableError", () => {
  it("extends MetaError", () => {
    const err = new CapabilityUnavailableError("no memory");
    assert.ok(err instanceof MetaError);
  });

  it("is an instance of Error", () => {
    const err = new CapabilityUnavailableError("no memory");
    assert.ok(err instanceof Error);
  });

  it("has name set to CapabilityUnavailableError", () => {
    const err = new CapabilityUnavailableError("msg");
    assert.equal(err.name, "CapabilityUnavailableError");
  });
});

// ---------------------------------------------------------------------------
// ConsoleTimelineReplayUnavailableError
// ---------------------------------------------------------------------------

describe("ConsoleTimelineReplayUnavailableError", () => {
  it("extends MetaError and uses the dedicated console timeline code", () => {
    const data = {
      error: "replay_unavailable",
      stream: "timeline",
      requested_cursor: "console:500",
      latest_cursor: "console:42",
    };
    const err = new ConsoleTimelineReplayUnavailableError("replay unavailable", "rid", "meta/console/query_timeline", data);
    assert.ok(err instanceof MetaError);
    assert.equal(err.name, "ConsoleTimelineReplayUnavailableError");
    assert.equal(err.code, -32013);
    assert.deepEqual(err.data, data);
  });
});

// ---------------------------------------------------------------------------
// WorkGraphUnavailableError / WorkGraphConflictError
// ---------------------------------------------------------------------------

describe("WorkGraphUnavailableError", () => {
  it("extends MetaError and uses the dedicated workgraph-unavailable code", () => {
    const err = new WorkGraphUnavailableError(
      "workgraph service not configured",
      "rid",
      "meta/workgraph/snapshot",
      { kind: "workgraph_unavailable" },
    );
    assert.ok(err instanceof MetaError);
    assert.ok(err instanceof RpcError);
    assert.equal(err.name, "WorkGraphUnavailableError");
    assert.equal(err.code, -32041);
    assert.deepEqual(err.data, { kind: "workgraph_unavailable" });
  });
});

describe("WorkGraphConflictError", () => {
  it("extends MetaError and uses the dedicated CAS-conflict code", () => {
    const data = { kind: "workgraph_conflict", detail: "revision mismatch" };
    const err = new WorkGraphConflictError(
      "revision conflict",
      "rid",
      "meta/workgraph/update",
      data,
    );
    assert.ok(err instanceof MetaError);
    assert.ok(err instanceof RpcError);
    assert.equal(err.name, "WorkGraphConflictError");
    assert.equal(err.code, -32042);
    assert.deepEqual(err.data, data);
  });
});

// ---------------------------------------------------------------------------
// ContractMismatchError
// ---------------------------------------------------------------------------

describe("ContractMismatchError", () => {
  it("extends MetaError", () => {
    const err = new ContractMismatchError("version mismatch");
    assert.ok(err instanceof MetaError);
  });

  it("is an instance of Error", () => {
    const err = new ContractMismatchError("version mismatch");
    assert.ok(err instanceof Error);
  });

  it("has name set to ContractMismatchError", () => {
    const err = new ContractMismatchError("msg");
    assert.equal(err.name, "ContractMismatchError");
  });
});

// ---------------------------------------------------------------------------
// NotConnectedError
// ---------------------------------------------------------------------------

describe("NotConnectedError", () => {
  it("extends MetaError", () => {
    const err = new NotConnectedError("not connected");
    assert.ok(err instanceof MetaError);
  });

  it("is an instance of Error", () => {
    const err = new NotConnectedError("not connected");
    assert.ok(err instanceof Error);
  });

  it("has name set to NotConnectedError", () => {
    const err = new NotConnectedError("msg");
    assert.equal(err.name, "NotConnectedError");
  });
});

// ---------------------------------------------------------------------------
// MetaRpcError (backward-compat alias)
// ---------------------------------------------------------------------------

describe("MetaRpcError (backward compat)", () => {
  it("is the same constructor as RpcError", () => {
    assert.equal(MetaRpcError, RpcError);
  });

  it("instances are instanceof RpcError", () => {
    const err = new MetaRpcError(-32600, "msg", "req-1", "mob.status");
    assert.ok(err instanceof RpcError);
  });

  it("instances are instanceof MetaError", () => {
    const err = new MetaRpcError(-32600, "msg", "req-1", "mob.status");
    assert.ok(err instanceof MetaError);
  });
});
