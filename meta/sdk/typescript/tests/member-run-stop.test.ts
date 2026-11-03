/**
 * Executed contract for `meta/stop_member_run`: the method sends the exact
 * wire params the gateway dispatches on and relays meerkat's typed run-stop
 * receipt, failing closed on anything else.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
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

describe("member run stop", () => {
  it("sends member, run and reason and returns the typed receipt", async () => {
    const receipt = { outcome: "not_current", run_id: "r-1", current_run_id: "r-2" };
    const { rt, calls } = await makeRuntime({ member_id: "w1", receipt });
    const result = await rt.mobHandle().stopMemberRun("w1", "r-1", "stop");
    assert.deepEqual(calls, [
      {
        method: "meta/stop_member_run",
        params: { member_id: "w1", run_id: "r-1", reason: "stop" },
      },
    ]);
    assert.deepEqual(result, receipt);
  });

  it("returns a stopped receipt with its contributors", async () => {
    const receipt = {
      outcome: "stopped",
      run_id: "r-1",
      contributors: [{ input_id: "i-1", completion: "cancelled", terminal: "cancelled" }],
    };
    const { rt } = await makeRuntime({ member_id: "w1", receipt });
    assert.deepEqual(await rt.mobHandle().stopMemberRun("w1", "r-1", "stop"), receipt);
  });

  it("fails closed on a malformed receipt", async () => {
    const { rt } = await makeRuntime({ member_id: "w1", receipt: { outcome: "?" } });
    await assert.rejects(
      rt.mobHandle().stopMemberRun("w1", "r-1", "stop"),
      /stop_member_run/,
    );
  });
});
