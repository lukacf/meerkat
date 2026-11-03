/**
 * `MetaRuntime.pruneMemberSystemRows`: a stale `expectedParentRevision`
 * rejects with the typed `TranscriptRevisionConflictError`, which names the
 * revision the host passed and the current one; nothing was written.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  SystemRowPruneRefusedError,
  TRANSCRIPT_REVISION_CONFLICT_KIND,
  TranscriptRevisionConflictError,
} from "../src/errors.js";
import * as sdk from "../src/index.js";
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

describe("MetaRuntime.pruneMemberSystemRows stale revision", () => {
  it("exports the conflict kind and its error", () => {
    assert.equal(TRANSCRIPT_REVISION_CONFLICT_KIND, "transcript_revision_conflict");
    assert.equal(sdk.TRANSCRIPT_REVISION_CONFLICT_KIND, TRANSCRIPT_REVISION_CONFLICT_KIND);
    assert.equal(sdk.TranscriptRevisionConflictError, TranscriptRevisionConflictError);
  });

  it("maps a stale expected revision to TranscriptRevisionConflictError", async () => {
    const { rt, sent } = await scriptedRuntime([
      {
        error: {
          code: -32602,
          message: "transcript revision conflict: expected rev-a, actual rev-b",
          data: { kind: TRANSCRIPT_REVISION_CONFLICT_KIND, expected: "rev-a", actual: "rev-b" },
        },
      },
    ]);
    await assert.rejects(
      rt.pruneMemberSystemRows("worker:1", REMOVE, { expectedParentRevision: "rev-a" }),
      (error: unknown) =>
        error instanceof TranscriptRevisionConflictError &&
        !(error instanceof SystemRowPruneRefusedError) &&
        error.code === -32602 &&
        error.expected === "rev-a" &&
        error.actual === "rev-b",
    );
    assert.equal(
      (sent[0]?.params as Record<string, unknown>).expected_parent_revision,
      "rev-a",
    );
  });
});
