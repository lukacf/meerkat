/**
 * Executed contract for `meta/schedule/tools`: the method sends no params and
 * returns Meerkat's schedule tool definitions typed, rejecting anything else.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { runtimeConfig } from "./runtime-config.js";

const LIST_TOOL = {
  name: "meerkat_schedule_list",
  description: "List all schedules in the active realm.",
  inputSchema: { type: "object", properties: {}, additionalProperties: false },
};

async function makeRuntime(answer: unknown) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params?: Record<string, unknown>,
  ) => {
    calls.push({ method, params: params ?? {} });
    return answer;
  };
  return { rt, calls };
}

describe("schedule tools", () => {
  it("sends meta/schedule/tools and returns the typed definitions", async () => {
    const { rt, calls } = await makeRuntime({ tools: [LIST_TOOL] });
    const result = await rt.mobHandle().scheduleTools();
    assert.deepEqual(calls, [{ method: "meta/schedule/tools", params: {} }]);
    assert.deepEqual(result, { tools: [LIST_TOOL] });
  });

  it("fails closed on a malformed result", async () => {
    const { inputSchema: _schema, ...withoutSchema } = LIST_TOOL;
    for (const answer of [
      {},
      { tools: {} },
      { tools: ["meerkat_schedule_list"] },
      { tools: [{ ...LIST_TOOL, name: 7 }] },
      { tools: [{ ...LIST_TOOL, description: null }] },
      { tools: [withoutSchema] },
      { tools: [{ ...LIST_TOOL, inputSchema: "object" }] },
    ]) {
      const { rt } = await makeRuntime(answer);
      await assert.rejects(rt.mobHandle().scheduleTools(), /meta\/schedule\/tools/);
    }
  });
});
