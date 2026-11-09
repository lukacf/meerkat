/**
 * `MetaRuntime.pruneMemberSystemRows`: a refused row rejects with the typed
 * `SystemRowPruneRefusedError` (its index and cause, from the closed cause
 * vocabulary), another `-32602` answer stays a plain `RpcError`, and a
 * candidate launch's refusal of the verb is the typed `candidate_launch`
 * refusal.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  AnnouncementRefusedError,
  RpcError,
  SYSTEM_ROW_PRUNE_REFUSAL_CAUSES,
  SYSTEM_ROW_PRUNE_REFUSED_KIND,
  SystemRowPruneRefusedError,
} from "../src/errors.js";
import { runtimeConfig } from "./runtime-config.js";

type Reply = { result: unknown } | { error: Record<string, unknown> };

async function scriptedRuntime(replies: Reply[]) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const rt = new MetaRuntime(runtimeConfig());
  const queue = [...replies];
  const sent: Array<{ method: string; params: unknown }> = [];
  (rt as unknown as Record<string, unknown>)._running = true;
  (rt as unknown as Record<string, unknown>)._transport = {
    sendAsync: async (request: Record<string, unknown>) => {
      await Promise.resolve();
      const method = String(request.method);
      sent.push({ method, params: request.params });
      const reply = queue.shift();
      if (reply === undefined) throw new Error(`unscripted ${method}`);
      return { jsonrpc: "2.0", id: request.id, ...reply };
    },
  };
  return { rt, sent };
}

const REMOVE = [{ index: 3, content_sha256: `sha256:${"0".repeat(64)}` }];

describe("MetaRuntime.pruneMemberSystemRows refusals", () => {
  it("maps a refused row to SystemRowPruneRefusedError", async () => {
    const { rt } = await scriptedRuntime([
      {
        error: {
          code: -32602,
          message: "system row prune refused the row at message index 3: digest_mismatch",
          data: { kind: SYSTEM_ROW_PRUNE_REFUSED_KIND, index: 3, cause: "digest_mismatch" },
        },
      },
      { error: { code: -32602, message: "remove must be a list" } },
    ]);
    await assert.rejects(
      rt.pruneMemberSystemRows("worker:1", REMOVE),
      (error: unknown) =>
        error instanceof SystemRowPruneRefusedError &&
        error.code === -32602 &&
        error.index === 3 &&
        error.cause === "digest_mismatch",
    );
    await assert.rejects(
      rt.pruneMemberSystemRows("worker:1", REMOVE),
      (error: unknown) =>
        error instanceof RpcError && !(error instanceof SystemRowPruneRefusedError),
    );
  });

  it("keeps Meerkat's prune refusal causes, one for one", () => {
    assert.deepEqual(
      [...SYSTEM_ROW_PRUNE_REFUSAL_CAUSES].sort(),
      [
        "before_operator_instruction_entry",
        "digest_mismatch",
        "duplicate_index",
        "instruction_activation_row",
        "not_a_system_row",
        "operator_instruction_entry_row",
        "out_of_bounds",
        "prompt_version_row",
      ],
    );
  });

  it("maps a candidate launch's refusal of the verb to its typed refusal", async () => {
    const { rt } = await scriptedRuntime([
      {
        error: {
          code: -32047,
          message: "meta/prune_member_system_rows is refused: this gateway is a candidate launch",
          data: { kind: "candidate_launch" },
        },
      },
    ]);
    await assert.rejects(
      rt.pruneMemberSystemRows("worker:1", REMOVE),
      (error: unknown) =>
        error instanceof AnnouncementRefusedError && error.kind === "candidate_launch",
    );
  });
});
