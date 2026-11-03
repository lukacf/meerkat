/**
 * meerkat's typed reload-required refusal from `meta/force_cancel_member`.
 *
 * The gateway answers a member whose runtime registration must be reloaded
 * with meerkat's own code (-32029) and data naming the member and the repair;
 * `forceCancelMember` rejects with `MemberReloadRequiredError` carrying them.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  MEMBER_RELOAD_REQUIRED_CODE,
  MEMBER_RELOAD_REQUIRED_KIND,
  MemberReloadRequiredError,
  RpcError,
} from "../src/errors.js";
import { runtimeConfig } from "./runtime-config.js";

type Reply = { result: unknown } | { error: Record<string, unknown> };

/** A started runtime whose transport answers each method from a script. */
async function scriptedRuntime(replies: Record<string, Reply[]>) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const rt = new MetaRuntime(runtimeConfig());
  const queues = new Map(Object.entries(replies).map(([k, v]) => [k, [...v]]));
  (rt as unknown as Record<string, unknown>)._running = true;
  (rt as unknown as Record<string, unknown>)._transport = {
    sendAsync: async (request: Record<string, unknown>) => {
      await Promise.resolve();
      const method = String(request.method);
      const reply = queues.get(method)?.shift();
      if (reply === undefined) throw new Error(`unscripted ${method}`);
      return { jsonrpc: "2.0", id: request.id, ...reply };
    },
  };
  return rt;
}

describe("MemberReloadRequiredError", () => {
  it("is a typed RpcError with meerkat's code", () => {
    const error = new MemberReloadRequiredError("reload first");
    assert.ok(error instanceof RpcError);
    assert.equal(error.code, MEMBER_RELOAD_REQUIRED_CODE);
    assert.equal(MEMBER_RELOAD_REQUIRED_CODE, -32029);
    assert.equal(MEMBER_RELOAD_REQUIRED_KIND, "mob_member_reload_required");
    assert.equal(error.name, "MemberReloadRequiredError");
    assert.equal(error.memberId, null);
    assert.equal(error.requiredAction, null);
  });

  it("is what forceCancelMember rejects with, carrying the member and the repair", async () => {
    const rt = await scriptedRuntime({
      "meta/force_cancel_member": [
        {
          error: {
            code: -32029,
            message: "member worker requires a runtime reload before it can accept work",
            data: {
              kind: "mob_member_reload_required",
              member_id: "worker",
              reason: "runtime registration is degraded",
              retryable: false,
              authority_retained: true,
              required_action: "reload_member_registration",
            },
          },
        },
      ],
    });
    await assert.rejects(
      rt.mobHandle().forceCancelMember("worker"),
      (error: unknown) =>
        error instanceof MemberReloadRequiredError &&
        error.code === -32029 &&
        error.memberId === "worker" &&
        error.reason === "runtime registration is degraded" &&
        error.requiredAction === "reload_member_registration",
    );
  });

  it("keeps the code without the kind a plain RpcError", async () => {
    const rt = await scriptedRuntime({
      "meta/force_cancel_member": [
        { error: { code: -32029, message: "member requires a runtime reload" } },
        {
          error: {
            code: -32029,
            message: "member requires a runtime reload",
            data: { kind: "something_else", member_id: "worker" },
          },
        },
      ],
    });
    for (let attempt = 0; attempt < 2; attempt += 1) {
      await assert.rejects(
        rt.mobHandle().forceCancelMember("worker"),
        (error: unknown) =>
          error instanceof RpcError &&
          !(error instanceof MemberReloadRequiredError) &&
          error.code === -32029,
      );
    }
  });

  it("keeps another refusal a plain RpcError", async () => {
    const rt = await scriptedRuntime({
      "meta/force_cancel_member": [
        { error: { code: -32602, message: "mob member not found: missing" } },
      ],
    });
    await assert.rejects(
      rt.mobHandle().forceCancelMember("missing"),
      (error: unknown) =>
        error instanceof RpcError &&
        !(error instanceof MemberReloadRequiredError) &&
        error.code === -32602,
    );
  });
});
