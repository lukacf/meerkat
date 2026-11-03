/**
 * `MobHandle.runFlow` with `roleTargets`: the client checks the gateway's
 * `flow_role_targets` capability before sending a binding, accepts the run
 * only when the gateway echoes the binding, cancels and throws otherwise, and
 * maps a refused binding to a typed error.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  CapabilityUnavailableError,
  FlowRoleTargetsNotAppliedError,
  FlowRoleTargetsRefusedError,
  FlowRunIdempotencyConflictError,
  RpcError,
} from "../src/errors.js";
import { parseMobRun, parseMobRunSnapshot } from "../src/types.js";

type Reply = { result: unknown } | { error: Record<string, unknown> };

const CAPABILITIES: Reply = {
  result: {
    contract_version: "0.9.5",
    methods: ["meta/run_flow", "meta/cancel_flow"],
    loaded_modules: [],
    feature_capabilities: ["flow_role_targets"],
  },
};

/**
 * A started runtime whose transport answers each method from a script and
 * records every request it was sent.
 */
async function scriptedRuntime(replies: Record<string, Reply[]>) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const rt = new MetaRuntime({
    mobConfigPath: null,
    sessionBuilder: null,
    errorCallback: null,
    eventLog: null,
    consoleConfigPath: null,
    consoleRequireAppAuth: null,
    consoleReadOnly: null,
    consoleFetchTimeoutMs: null,
    gatingConfigPath: null,
    routingConfigPath: null,
    workgraphEnabled: null,
    memoryConfig: null,
    agentMemoryConfig: null,
    authConfig: null,
    implicitDelegateIdleRetireSecs: undefined,
    maxSessions: null,
    gatewayTimeoutMs: null,
    gatewayBin: null,
    modules: [],
    persistentState: null,
    rosterProvider: null,
    agentCustomizer: null,
    topologyProvider: null,
  } as never);
  const queues = new Map(Object.entries(replies).map(([k, v]) => [k, [...v]]));
  const sent: Array<{ method: string; params: unknown }> = [];
  (rt as unknown as Record<string, unknown>)._running = true;
  (rt as unknown as Record<string, unknown>)._transport = {
    sendAsync: async (request: Record<string, unknown>) => {
      await Promise.resolve();
      const method = String(request.method);
      sent.push({ method, params: request.params });
      const reply = queues.get(method)?.shift();
      if (reply === undefined) throw new Error(`unscripted ${method}`);
      return { jsonrpc: "2.0", id: request.id, ...reply };
    },
  };
  return { handle: rt.mobHandle(), sent };
}

describe("MobHandle.runFlow roleTargets", () => {
  it("checks the capability, then sends the binding and accepts its echo", async () => {
    const { handle, sent } = await scriptedRuntime({
      "meta/capabilities": [CAPABILITIES],
      "meta/run_flow": [
        { result: { run_id: "run-a", role_targets: { reviewer: ["rev-a2", "rev-a1"] } } },
      ],
    });

    const runId = await handle.runFlow(
      "review",
      { issue: 7 },
      { roleTargets: { reviewer: ["rev-a1", "rt:rev-a2:3"] } },
    );

    assert.equal(runId, "run-a");
    assert.deepEqual(
      sent.map((request) => request.method),
      ["meta/capabilities", "meta/run_flow"],
    );
    assert.deepEqual(sent[1].params, {
      flow_id: "review",
      params: { issue: 7 },
      role_targets: { reviewer: ["rev-a1", "rt:rev-a2:3"] },
    });
  });

  it("refuses role targets locally when the gateway lacks the capability", async () => {
    const { handle, sent } = await scriptedRuntime({
      "meta/capabilities": [
        {
          result: {
            contract_version: "0.9.5",
            methods: ["meta/run_flow"],
            loaded_modules: [],
            feature_capabilities: [],
          },
        },
      ],
    });

    await assert.rejects(
      handle.runFlow("review", null, { roleTargets: { reviewer: ["rev-a1"] } }),
      (error: unknown) =>
        error instanceof CapabilityUnavailableError &&
        error.method === "meta/run_flow" &&
        (error.data as Record<string, unknown>).capability === "flow_role_targets",
    );
    assert.deepEqual(
      sent.map((request) => request.method),
      ["meta/capabilities"],
      "an older gateway would run the flow role-wide, so nothing is started",
    );
  });

  for (const echoed of [
    undefined,
    { reviewer: ["rev-b1"] },
    { reviewer: ["rev-a1"], coder: ["coder-a"] },
    { reviewer: "rev-a1" },
    ["rev-a1"],
  ]) {
    it(`cancels the run and throws when the echo is ${JSON.stringify(echoed)}`, async () => {
      const result: Record<string, unknown> = { run_id: "run-a" };
      if (echoed !== undefined) result.role_targets = echoed;
      const { handle, sent } = await scriptedRuntime({
        "meta/capabilities": [CAPABILITIES],
        "meta/run_flow": [{ result }],
        "meta/cancel_flow": [{ result: {} }],
      });

      await assert.rejects(
        handle.runFlow("review", null, { roleTargets: { reviewer: ["rev-a1"] } }),
        (error: unknown) =>
          error instanceof FlowRoleTargetsNotAppliedError &&
          error.runId === "run-a" &&
          error.cancelError === null &&
          JSON.stringify(error.echoed) === JSON.stringify(echoed),
      );
      assert.deepEqual(sent[sent.length - 1], {
        method: "meta/cancel_flow",
        params: { run_id: "run-a" },
      });
    });
  }

  it("reports a cancel that failed after an unechoed run", async () => {
    const { handle, sent } = await scriptedRuntime({
      "meta/capabilities": [CAPABILITIES],
      "meta/run_flow": [{ result: { run_id: "run-a" } }],
      "meta/cancel_flow": [{ error: { code: -32000, message: "cancel_flow failed" } }],
    });

    await assert.rejects(
      handle.runFlow("review", null, { roleTargets: { reviewer: ["rev-a1"] } }),
      (error: unknown) =>
        error instanceof FlowRoleTargetsNotAppliedError &&
        error.cancelError instanceof RpcError &&
        error.message.includes("canceling it failed"),
    );
    assert.equal(sent[sent.length - 1].method, "meta/cancel_flow");
  });

  it("maps a refused binding to a typed error and nothing else to it", async () => {
    const { handle } = await scriptedRuntime({
      "meta/capabilities": [CAPABILITIES, CAPABILITIES],
      "meta/run_flow": [
        {
          error: {
            code: -32602,
            message: "run_flow failed: role targets refused",
            data: {
              kind: "flow_role_targets_refused",
              flow_id: "review",
              refusal: { kind: "not_a_member", role: "reviewer", member: "ghost" },
            },
          },
        },
        { error: { code: -32602, message: "Invalid params: flow_id required" } },
      ],
    });

    await assert.rejects(
      handle.runFlow("review", null, { roleTargets: { reviewer: ["ghost"] } }),
      (error: unknown) =>
        error instanceof FlowRoleTargetsRefusedError &&
        error.code === -32602 &&
        error.flowId === "review" &&
        error.refusalKind === "not_a_member" &&
        error.refusal.member === "ghost",
    );
    await assert.rejects(
      handle.runFlow("review", null, { roleTargets: { reviewer: ["rev-a1"] } }),
      (error: unknown) =>
        error instanceof RpcError && !(error instanceof FlowRoleTargetsRefusedError),
    );
  });

  it("reads no capabilities for a role-wide run", async () => {
    const { handle, sent } = await scriptedRuntime({
      "meta/run_flow": [{ result: { run_id: "run-a" } }],
    });

    assert.equal(await handle.runFlow("review"), "run-a");
    assert.deepEqual(sent, [
      { method: "meta/run_flow", params: { flow_id: "review", params: null } },
    ]);
  });

  it("refuses role targets that are not member id arrays before any call", async () => {
    const { handle, sent } = await scriptedRuntime({});

    await assert.rejects(
      handle.runFlow("review", null, {
        roleTargets: { reviewer: "rev-a1" } as unknown as Record<string, string[]>,
      }),
      TypeError,
    );
    await assert.rejects(
      handle.runFlow("review", null, {
        roleTargets: { reviewer: ["rev-a1", 7] } as unknown as Record<string, string[]>,
      }),
      TypeError,
    );
    assert.deepEqual(sent, []);
  });
});

describe("run records carry their role targets", () => {
  it("parses roleTargets for a bound run and leaves the key out for a role-wide one", () => {
    const bound = { run_id: "r-1", status: "running", role_targets: { reviewer: ["rev-a1"] } };
    assert.deepEqual(parseMobRunSnapshot(bound).roleTargets, { reviewer: ["rev-a1"] });
    assert.deepEqual(parseMobRun(bound).roleTargets, { reviewer: ["rev-a1"] });

    const roleWide = { run_id: "r-2", status: "running" };
    assert.equal("roleTargets" in parseMobRunSnapshot(roleWide), false);
    assert.equal("roleTargets" in parseMobRun(roleWide), false);
  });
});

const KEY_CAPABILITIES: Reply = {
  result: {
    contract_version: "0.9.5",
    methods: ["meta/run_flow", "meta/cancel_flow"],
    loaded_modules: [],
    feature_capabilities: ["flow_role_targets", "flow_run_idempotency_key"],
  },
};

describe("MobHandle.runFlow idempotencyKey", () => {
  it("checks the capability, then sends the key with the binding", async () => {
    const { handle, sent } = await scriptedRuntime({
      "meta/capabilities": [KEY_CAPABILITIES],
      "meta/run_flow": [
        { result: { run_id: "run-a", role_targets: { reviewer: ["rev-a1"] } } },
      ],
    });

    const runId = await handle.runFlow(
      "review",
      { candidate: "abc123" },
      { roleTargets: { reviewer: ["rev-a1"] }, idempotencyKey: "issue-7-review" },
    );

    assert.equal(runId, "run-a");
    assert.deepEqual(
      sent.map((request) => request.method),
      ["meta/capabilities", "meta/run_flow"],
    );
    assert.deepEqual(sent[1].params, {
      flow_id: "review",
      params: { candidate: "abc123" },
      role_targets: { reviewer: ["rev-a1"] },
      idempotency_key: "issue-7-review",
    });
  });

  it("refuses a key locally when the gateway lacks the capability", async () => {
    const { handle, sent } = await scriptedRuntime({
      "meta/capabilities": [CAPABILITIES],
    });

    await assert.rejects(
      handle.runFlow("review", null, {
        roleTargets: { reviewer: ["rev-a1"] },
        idempotencyKey: "issue-7-review",
      }),
      (error: unknown) =>
        error instanceof CapabilityUnavailableError &&
        (error.data as Record<string, unknown>).capability === "flow_run_idempotency_key",
    );
    assert.deepEqual(
      sent.map((request) => request.method),
      ["meta/capabilities"],
      "an older gateway would ignore the key, so nothing is started",
    );
  });

  it("maps a key conflict to a typed error", async () => {
    const { handle } = await scriptedRuntime({
      "meta/capabilities": [KEY_CAPABILITIES],
      "meta/run_flow": [
        {
          error: {
            code: -32602,
            message: "run_flow failed: flow run admitted under this key with other params",
            data: { kind: "flow_run_idempotency_conflict", run_id: "run-a", flow_id: "review" },
          },
        },
      ],
    });

    await assert.rejects(
      handle.runFlow("review", null, { idempotencyKey: "issue-7-review" }),
      (error: unknown) =>
        error instanceof FlowRunIdempotencyConflictError &&
        error.code === -32602 &&
        error.runId === "run-a" &&
        error.flowId === "review",
    );
  });

  it("refuses a key that is not a string before any call", async () => {
    const { handle, sent } = await scriptedRuntime({});

    await assert.rejects(
      handle.runFlow("review", null, {
        idempotencyKey: 7 as unknown as string,
      }),
      TypeError,
    );
    assert.deepEqual(sent, []);
  });
});
