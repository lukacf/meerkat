/**
 * `MetaRuntime` wrappers of the scoped tool grant methods
 * (`meta/tool_grants/*`): each sends its method with exactly the params it
 * names, a grant names exactly one target, and a refusal of the methods
 * rejects with the typed `ToolGrantRequestRefusedError`, whose kinds are a
 * closed vocabulary; another `-32602` answer stays a plain `RpcError`.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  RpcError,
  TOOL_GRANT_REQUEST_REFUSAL_KINDS,
  TOOL_GRANT_REQUEST_REFUSED_KIND,
  ToolGrantRequestRefusedError,
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

describe("MetaRuntime tool grant wrappers", () => {
  it("send each method with exactly its params", async () => {
    const ok = { result: {} };
    const { rt, sent } = await scriptedRuntime([ok, ok, ok, ok, ok, ok]);
    await rt.grantTool("agent-a", {
      tool: "datetime",
      durationMs: 600_000,
      reason: "approved",
      decisionRef: "approval-7",
    });
    await rt.grantTool("agent-a", { family: "shell", expiresAtMs: 1_900_000_000_000 });
    await rt.revokeToolGrant("g1", { reason: "done" });
    await rt.listToolGrants("agent-a", { includeEnded: true });
    await rt.toolGrantAudit("agent-a", { afterSequence: 4, limit: 10 });
    await rt.toolGrantAudit("agent-a");
    assert.deepEqual(sent, [
      {
        method: "meta/tool_grants/grant",
        params: {
          identity: "agent-a",
          target: { kind: "tool", name: "datetime" },
          duration_ms: 600_000,
          reason: "approved",
          decision_ref: "approval-7",
        },
      },
      {
        method: "meta/tool_grants/grant",
        params: {
          identity: "agent-a",
          target: { kind: "family", family: "shell" },
          expires_at_ms: 1_900_000_000_000,
        },
      },
      { method: "meta/tool_grants/revoke", params: { grant_id: "g1", reason: "done" } },
      {
        method: "meta/tool_grants/list",
        params: { identity: "agent-a", include_ended: true },
      },
      {
        method: "meta/tool_grants/audit",
        params: { identity: "agent-a", after_sequence: 4, limit: 10 },
      },
      { method: "meta/tool_grants/audit", params: { identity: "agent-a" } },
    ]);
  });

  it("refuses a grant that names no target or two", async () => {
    const { rt, sent } = await scriptedRuntime([]);
    await assert.rejects(rt.grantTool("agent-a", { durationMs: 1_000 }), TypeError);
    await assert.rejects(
      rt.grantTool("agent-a", { tool: "datetime", family: "shell", durationMs: 1_000 }),
      TypeError,
    );
    assert.deepEqual(sent, []);
  });

  it("maps a grant request refusal to ToolGrantRequestRefusedError", async () => {
    const { rt } = await scriptedRuntime([
      {
        error: {
          code: -32602,
          message: "the subject's grant gate does not name the target",
          data: {
            kind: TOOL_GRANT_REQUEST_REFUSED_KIND,
            tool_grant_request_refusal: { kind: "target_not_gated" },
          },
        },
      },
      { error: { code: -32602, message: "Invalid params" } },
    ]);
    await assert.rejects(
      rt.grantTool("agent-a", { tool: "datetime", durationMs: 1_000 }),
      (error: unknown) =>
        error instanceof ToolGrantRequestRefusedError &&
        error.code === -32602 &&
        error.refusalKind === "target_not_gated",
    );
    await assert.rejects(
      rt.grantTool("agent-a", { tool: "datetime", durationMs: 1_000 }),
      (error: unknown) =>
        error instanceof RpcError && !(error instanceof ToolGrantRequestRefusedError),
    );
  });

  it("keeps the refusal kinds a closed vocabulary", () => {
    assert.deepEqual(
      [...TOOL_GRANT_REQUEST_REFUSAL_KINDS].sort(),
      [
        "empty_target",
        "invalid_decision_ref",
        "invalid_page_limit",
        "invalid_wait",
        "invalid_window",
        "no_live_grant",
        "not_found",
        "server_not_installed",
        "subject_unknown",
        "target_not_gated",
      ],
    );
  });
});
