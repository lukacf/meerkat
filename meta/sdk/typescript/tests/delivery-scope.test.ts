/**
 * Scope-bound dispatch for the TypeScript SDK.
 *
 * A host reads an identity's `DeliveryScope` from `status()`, persists it,
 * dispatches with `expectedScope` and recovers a lost reply from the scope's
 * ORIGINAL session with `recoverDelivery`. The SDK carries the scope
 * byte-exact, refuses scope versions it cannot read, throws `StaleScopeError`
 * for a moved scope, and keeps every recovery class distinct.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  ContractMismatchError,
  RpcError,
  STALE_DELIVERY_SCOPE_CODE,
  StaleScopeError,
} from "../src/errors.js";
import {
  DELIVERY_SCOPE_VERSION,
  deliveryScopeToDict,
  parseDeliveryScope,
  parseIdentityStatus,
  parseScopedRecovery,
  type DeliveryScope,
} from "../src/types.js";
import { runtimeConfig } from "./runtime-config.js";

const SCOPE = {
  version: 1,
  identity: "personal:alice",
  agent_runtime_id: "rt-alice-1",
  generation: 2,
  lease_fencing_token: 5,
  member: {
    version: 1,
    runtime_id: { identity: "personal:alice", generation: 3 },
    fence_token: 7,
    session_id: "019245f0-0000-7000-8000-000000000001",
  },
};

type Reply = { result: unknown } | { error: Record<string, unknown> };

/** A started runtime whose transport answers each method from a script. */
async function scriptedRuntime(replies: Record<string, Reply[]>) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  const queues = new Map(Object.entries(replies).map(([k, v]) => [k, [...v]]));
  (rt as unknown as Record<string, unknown>)._running = true;
  (rt as unknown as Record<string, unknown>)._transport = {
    sendAsync: async (request: Record<string, unknown>) => {
      await Promise.resolve();
      const method = String(request.method);
      calls.push({ method, params: (request.params ?? {}) as Record<string, unknown> });
      const reply = queues.get(method)?.shift();
      if (reply === undefined) throw new Error(`unscripted ${method}`);
      return { jsonrpc: "2.0", id: request.id, ...reply };
    },
  };
  return { rt, calls };
}

function scope(): DeliveryScope {
  return parseDeliveryScope(JSON.parse(JSON.stringify(SCOPE)));
}

describe("DeliveryScope model", () => {
  it("round-trips the persisted form byte-exact", () => {
    const parsed = scope();
    assert.equal(parsed.version, DELIVERY_SCOPE_VERSION);
    assert.equal(parsed.sessionId, SCOPE.member.session_id);
    assert.deepEqual(deliveryScopeToDict(parsed), SCOPE);
    assert.deepEqual(deliveryScopeToDict(parseDeliveryScope(deliveryScopeToDict(parsed))), SCOPE);
  });

  it("carries the member scope opaque and never shares it", () => {
    const raw = JSON.parse(JSON.stringify(SCOPE));
    const parsed = parseDeliveryScope(raw);
    raw.member.fence_token = 99;
    assert.equal((deliveryScopeToDict(parsed).member as Record<string, unknown>).fence_token, 7);
    const out = deliveryScopeToDict(parsed);
    (out.member as Record<string, unknown>).fence_token = 42;
    assert.equal(parsed.member.fence_token, 7);
  });

  it("refuses an unknown version as a contract mismatch, never a guess", () => {
    assert.throws(
      () => parseDeliveryScope({ ...SCOPE, version: 2 }),
      (error: unknown) => error instanceof ContractMismatchError && /version 2/.test(error.message),
    );
  });

  for (const patch of [
    { version: "1" },
    { identity: "" },
    { agent_runtime_id: 7 },
    { generation: -1 },
    { lease_fencing_token: 1.5 },
    { member: "opaque" },
  ]) {
    it(`refuses a malformed scope (${JSON.stringify(patch)})`, () => {
      assert.throws(() => parseDeliveryScope({ ...SCOPE, ...patch }), TypeError);
    });
  }

  it("status carries the scope, or why it is unavailable, without breaking", () => {
    const status = parseIdentityStatus({ identity: "personal:alice", state: "active", delivery_scope: SCOPE });
    assert.deepEqual(status.deliveryScope && deliveryScopeToDict(status.deliveryScope), SCOPE);
    assert.equal(status.deliveryScopeUnavailable, null);
    const future = parseIdentityStatus({
      identity: "personal:alice",
      state: "active",
      session_id: "s-1",
      delivery_scope: { ...SCOPE, version: 3 },
    });
    assert.equal(future.sessionId, "s-1");
    assert.equal(future.deliveryScope, null);
    assert.equal(future.deliveryScopeUnavailable?.kind, "unsupported_delivery_scope_version");
  });
});

describe("scoped dispatch", () => {
  it("sends the exact scope and returns the receipt", async () => {
    const receipt = { work_ref: "wr-1", stage: "ingress_accepted", session_id: SCOPE.member.session_id };
    const { rt, calls } = await scriptedRuntime({
      "meta/dispatch": [{ result: { receipt, delivery_scope: SCOPE, fencing_token: 5 } }],
    });
    const result = await rt.dispatch(
      "personal:alice",
      { content: "occurrence", origin: "system", idempotencyKey: "k-1", correlationId: "c-1" },
      { expectedScope: scope() },
    );
    assert.deepEqual(result.receipt, { workRef: "wr-1", stage: "ingress_accepted", sessionId: SCOPE.member.session_id });
    assert.deepEqual(deliveryScopeToDict(result.deliveryScope), SCOPE);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0]?.params.expected_scope, SCOPE);
    assert.equal(calls[0]?.params.track_turn, undefined);
  });

  it("never combines a scope with trackTurn", async () => {
    const { rt, calls } = await scriptedRuntime({});
    await assert.rejects(
      rt.dispatch(
        "personal:alice",
        { content: "x", origin: "system", idempotencyKey: "k", correlationId: "c" },
        { expectedScope: scope(), trackTurn: true as false },
      ),
      TypeError,
    );
    assert.equal(calls.length, 0);
  });

  it("throws StaleScopeError for a moved scope", async () => {
    const { rt } = await scriptedRuntime({
      "meta/dispatch": [
        {
          error: {
            code: STALE_DELIVERY_SCOPE_CODE,
            message: "stale delivery scope (generation): moved",
            data: { kind: "stale_delivery_scope", admission_possible: false, mismatch: "generation" },
          },
        },
      ],
    });
    await assert.rejects(
      rt.dispatch(
        "personal:alice",
        { content: "x", origin: "system", idempotencyKey: "k", correlationId: "c" },
        { expectedScope: scope() },
      ),
      (error: unknown) =>
        error instanceof StaleScopeError &&
        error instanceof RpcError &&
        error.code === -32007 &&
        error.mismatch === "generation",
    );
  });

  it("keeps an uncertain admission a typed RpcError", async () => {
    const { rt } = await scriptedRuntime({
      "meta/dispatch": [
        {
          error: {
            code: -32603,
            message: "scoped delivery outcome uncertain: timeout",
            data: { kind: "scoped_delivery_uncertain", admission_possible: true },
          },
        },
      ],
    });
    await assert.rejects(
      rt.dispatch(
        "personal:alice",
        { content: "x", origin: "system", idempotencyKey: "k", correlationId: "c" },
        { expectedScope: scope() },
      ),
      (error: unknown) =>
        error instanceof RpcError &&
        !(error instanceof StaleScopeError) &&
        (error.data as Record<string, unknown>).admission_possible === true,
    );
  });
});

describe("recoverDelivery", () => {
  const cases: [Record<string, unknown>, string][] = [
    [{ state: "absent" }, "absent"],
    [{ state: "in_flight", input_id: "in-1", phase: "queued", durable_witness: true }, "in_flight"],
    [
      { state: "completed", input_id: "in-1", output_status: "text", output: "answer", output_truncated: true },
      "completed",
    ],
    [{ state: "failed", input_id: "in-1", error: "boom" }, "failed"],
    [
      { state: "terminal_without_run", input_id: "in-1", terminal: { outcome_type: "abandoned" }, last_run_id: null },
      "terminal_without_run",
    ],
    [{ state: "broken", input_id: null, reason: "inconsistent" }, "broken"],
    [{ state: "unresolved", cause: "original_owner_unavailable", detail: "store down" }, "unresolved"],
  ];
  for (const [wire, state] of cases) {
    it(`keeps ${state} distinct`, async () => {
      const { rt, calls } = await scriptedRuntime({
        "meta/recover_delivery": [
          { result: { identity: "personal:alice", delivery_scope: SCOPE, recovery: wire } },
        ],
      });
      const recovery = await rt.recoverDelivery("personal:alice", scope(), {
        idempotencyKey: "k-1",
        correlationId: "c-1",
        timeoutMs: 2500,
      });
      assert.equal(recovery.state, state);
      assert.deepEqual(recovery.deliveryScope && deliveryScopeToDict(recovery.deliveryScope), SCOPE);
      assert.deepEqual(calls[0]?.params, {
        identity: "personal:alice",
        scope: SCOPE,
        idempotency_key: "k-1",
        correlation_id: "c-1",
        timeout_ms: 2500,
      });
      if (state === "in_flight") {
        assert.equal(recovery.durableWitness, true);
        assert.equal(recovery.phase, "queued");
      }
      if (state === "completed") {
        assert.deepEqual([recovery.outputStatus, recovery.output, recovery.outputTruncated], ["text", "answer", true]);
      }
      if (state === "unresolved") {
        assert.equal(recovery.cause, "original_owner_unavailable");
        assert.equal(recovery.detail, "store down");
      }
    });
  }

  it("reads an unknown state as unresolved, never absent", () => {
    const recovery = parseScopedRecovery({ recovery: { state: "superseded" } });
    assert.equal(recovery.state, "unresolved");
    assert.equal(recovery.cause, "unknown_state:superseded");
  });
});
