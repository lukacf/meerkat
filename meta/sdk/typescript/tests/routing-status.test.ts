/**
 * Executed contract for `meta/identity/routing_status`.
 *
 * A typecheck proves the shapes line up; it does not prove the parser reads the
 * wire keys the gateway actually sends, nor that the method sends the params the
 * gateway expects. Both of those have shipped broken in this SDK before while
 * `tsc` stayed green, so this test drives the real method against a scripted
 * `_rpc` and asserts on values.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { parseIdentityRoutingStatusResult } from "../src/types.js";
import { runtimeConfig } from "./runtime-config.js";

async function makeRuntime(answer: unknown | (() => never)) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params?: Record<string, unknown>,
  ) => {
    calls.push({ method, params: params ?? {} });
    if (typeof answer === "function") return (answer as () => never)();
    return answer;
  };
  return { rt, calls };
}

describe("identity routing status", () => {
  it("sends the wire method and identity param the gateway dispatches on", async () => {
    const { rt, calls } = await makeRuntime({
      identity: "domain:security",
      session_id: "sid-1",
      baseline_model: "gpt-5.6-sol",
      effective_model: "gpt-5.6-sol",
      session_provider: "openai",
    });
    const status = await rt.mobHandle().identityRoutingStatus("domain:security");

    // The method string is the contract with Meta's two dispatchers; a typo
    // here is a -32601 at runtime that no typecheck can see.
    assert.equal(calls[0].method, "meta/identity/routing_status");
    assert.deepEqual(calls[0].params, { identity: "domain:security" });

    assert.equal(status.identity, "domain:security");
    assert.equal(status.sessionId, "sid-1");
    assert.equal(status.baselineModel, "gpt-5.6-sol");
    assert.equal(status.effectiveModel, "gpt-5.6-sol");
    assert.equal(status.sessionProvider, "openai");
  });

  it("keeps an absent session_provider as null rather than coercing it", () => {
    // session_provider is Option + skip_serializing_if upstream, so a
    // pre-hydration session omits the key entirely. Coercing that to "" or
    // "undefined" would let a provider comparison pass against a value that was
    // never read - the exact fabrication the typed field exists to prevent.
    const status = parseIdentityRoutingStatusResult({
      identity: "domain:plain",
      session_id: "sid-2",
      baseline_model: "claude-fable-5",
      effective_model: "claude-fable-5",
    });
    assert.equal(status.sessionProvider, null);
    assert.equal(status.baselineModel, "claude-fable-5");
    assert.equal(status.activeTurnOverride, null);
    assert.equal(status.activeOperationOverride, null);
    assert.equal(status.pendingSwitchTurn, null);
  });

  it("reads the snake_case override summaries the gateway actually emits", () => {
    // Guards the rename half of the parser: reading `activeTurnOverride` off the
    // raw payload would silently yield null for every real response.
    const status = parseIdentityRoutingStatusResult({
      identity: "domain:security",
      session_id: "sid-3",
      baseline_model: "gpt-5.5",
      effective_model: "gpt-5.6-sol",
      active_turn_override: { id: "ovr-1", target_model: "gpt-5.6-sol" },
      pending_switch_turn: { request_id: "req-1", phase: "requested" },
    });
    assert.equal(status.effectiveModel, "gpt-5.6-sol");
    assert.deepEqual(status.activeTurnOverride, {
      id: "ovr-1",
      target_model: "gpt-5.6-sol",
    });
    assert.deepEqual(status.pendingSwitchTurn, {
      request_id: "req-1",
      phase: "requested",
    });
    assert.equal(status.activeOperationOverride, null);
  });

  it("does not swallow a typed refusal, and preserves its reason", async () => {
    // The refusal payload is what a fleet sweep branches on. An SDK that
    // returned a partially-parsed object here, or dropped `data`, would leave
    // the caller unable to tell "no session yet" from a real defect.
    const { RpcError } = await import("../src/errors.js");
    const refusal = new RpcError(
      -32000,
      "routing_status unavailable: the roster reports no current session",
      "req-1",
      "meta/identity/routing_status",
      {
        kind: "routing_status_unavailable",
        reason: "no_current_session",
        identity: "domain:absent",
      },
    );
    const { rt } = await makeRuntime(() => {
      throw refusal;
    });

    await assert.rejects(
      () => rt.mobHandle().identityRoutingStatus("domain:absent"),
      (err: unknown) => {
        assert.ok(err instanceof RpcError);
        const data = err.data as Record<string, unknown>;
        assert.equal(data.kind, "routing_status_unavailable");
        assert.equal(data.reason, "no_current_session");
        assert.equal(data.identity, "domain:absent");
        return true;
      },
    );
  });
});
