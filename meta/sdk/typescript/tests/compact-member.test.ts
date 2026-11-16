/**
 * `MetaRuntime.compactMember`: it sends `meta/compact_member` with the params
 * given and no others, waits for the reply longer than the gateway holds the
 * call, passes the gateway's result through, and rejects with the RPC error
 * whose `data.kind` names what happened.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { RpcError } from "../src/errors.js";
import { runtimeConfig } from "./runtime-config.js";

type Reply = { result: unknown } | { error: Record<string, unknown> };

/** The gateway's `timeout_ms` when the caller names none. */
const GATEWAY_DEFAULT_TIMEOUT_MS = 60_000;
/**
 * The gateway keeps observing for this long after `timeout_ms` before it
 * answers that the maintenance turn is still pending.
 */
const GATEWAY_SECONDARY_BUDGET_MS = 5_000;

async function scriptedRuntime(replies: Reply[]) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const rt = new MetaRuntime(runtimeConfig());
  const queue = [...replies];
  const sent: Array<{ method: string; params: unknown; timeoutMs: number | undefined }> = [];
  (rt as unknown as Record<string, unknown>)._running = true;
  (rt as unknown as Record<string, unknown>)._transport = {
    sendAsync: async (
      request: Record<string, unknown>,
      options: { timeoutMs?: number } = {},
    ) => {
      await Promise.resolve();
      const method = String(request.method);
      sent.push({ method, params: request.params, timeoutMs: options.timeoutMs });
      const reply = queue.shift();
      if (reply === undefined) throw new Error(`unscripted ${method}`);
      return { jsonrpc: "2.0", id: request.id, ...reply };
    },
  };
  return { rt, sent };
}

const OK: Reply = { result: { identity: "worker:1" } };

describe("MetaRuntime.compactMember", () => {
  it("sends only the identity by default", async () => {
    const { rt, sent } = await scriptedRuntime([OK]);
    await rt.compactMember("worker:1");
    assert.equal(sent[0].method, "meta/compact_member");
    assert.deepEqual(sent[0].params, { identity: "worker:1" });
  });

  it("sends the floor and the timeout given, and no others", async () => {
    const { rt, sent } = await scriptedRuntime([OK, OK, OK]);
    await rt.compactMember("worker:1", { floorTokens: 2048, timeoutMs: 120_000 });
    assert.equal(sent[0].method, "meta/compact_member");
    assert.deepEqual(sent[0].params, {
      identity: "worker:1",
      floor_tokens: 2048,
      timeout_ms: 120_000,
    });
    await rt.compactMember("worker:1", { timeoutMs: 2_500 });
    assert.deepEqual(sent[1].params, { identity: "worker:1", timeout_ms: 2_500 });
    await rt.compactMember("worker:1", { floorTokens: 4096 });
    assert.deepEqual(sent[2].params, { identity: "worker:1", floor_tokens: 4096 });
  });

  it("waits for the reply longer than the gateway's default hold", async () => {
    const { rt, sent } = await scriptedRuntime([OK]);
    await rt.compactMember("worker:1");
    const waited = sent[0].timeoutMs;
    // Never the transport's own request timeout: the gateway holds the call
    // for its default 60 s and then its secondary budget.
    assert.equal(typeof waited, "number");
    assert.ok(
      (waited as number) > GATEWAY_DEFAULT_TIMEOUT_MS + GATEWAY_SECONDARY_BUDGET_MS,
      `transport wait ${waited}ms is not above the gateway's hold`,
    );
  });

  it("waits for the reply longer than an explicit timeout's hold", async () => {
    for (const timeoutMs of [500, 30_000, 60_000, 120_000, 900_000]) {
      const { rt, sent } = await scriptedRuntime([OK]);
      await rt.compactMember("worker:1", { timeoutMs });
      const waited = sent[0].timeoutMs;
      assert.equal(typeof waited, "number");
      assert.ok(
        (waited as number) > timeoutMs + GATEWAY_SECONDARY_BUDGET_MS,
        `transport wait ${waited}ms is not above the gateway's hold for ${timeoutMs}ms`,
      );
    }
  });

  it("rejects with the RPC error whose data.kind the gateway sent", async () => {
    for (const kind of [
      "compact_member_busy",
      "compact_member_timeout",
      "compact_member_refused",
      "compact_member_superseded",
      "compact_member_completion_failed",
    ]) {
      const data = {
        kind,
        stage: "terminal_pending",
        operation_id: "0198c0de-0000-7000-8000-000000000001",
        identity: "worker:1",
      };
      const { rt } = await scriptedRuntime([
        { error: { code: -32000, message: "compaction did not complete", data } },
      ]);
      await assert.rejects(rt.compactMember("worker:1"), (error: unknown) => {
        assert.ok(error instanceof RpcError);
        assert.equal(error.constructor, RpcError);
        assert.equal(error.code, -32000);
        assert.equal(error.method, "meta/compact_member");
        assert.deepEqual(error.data, data);
        return true;
      });
    }
  });

  it("resolves to the gateway's result fields as sent", async () => {
    const result = {
      identity: "worker:1",
      session_id: "0198c0de-0000-7000-8000-0000000000aa",
      floor_tokens: 1024,
      messages_before: 42,
      messages_after: 7,
      compaction_applied: true,
      head_revision: `sha256:${"a".repeat(64)}`,
      last_rewrite_reason: "compaction",
    };
    // A gateway without a transcript-edit service answers the counts and the
    // revision facts as null: they stay null.
    const sparse = {
      identity: "worker:1",
      session_id: "0198c0de-0000-7000-8000-0000000000aa",
      floor_tokens: 2048,
      messages_before: null,
      messages_after: null,
      compaction_applied: null,
      head_revision: null,
      last_rewrite_reason: null,
    };
    const { rt } = await scriptedRuntime([{ result }, { result: sparse }]);
    assert.deepEqual(await rt.compactMember("worker:1"), result);
    assert.deepEqual(await rt.compactMember("worker:1", { floorTokens: 2048 }), sparse);
  });

  it("refuses an unusable timeout before any request", async () => {
    for (const timeoutMs of [0, -100, 0.5, Number.NaN, Number.POSITIVE_INFINITY]) {
      const { rt, sent } = await scriptedRuntime([OK]);
      await assert.rejects(rt.compactMember("worker:1", { timeoutMs }), RangeError);
      assert.equal(sent.length, 0);
    }
  });
});
