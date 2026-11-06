/**
 * Meta SDK wrappers of the durable decision verbs (`meta/decisions/*`).
 *
 * Each wrapper sends its method with exactly the params it names, `decide`
 * carries a typed host attestation, and `decide` refuses inside a host tool
 * callback: an agent's tool call must never become the operator's decision.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { CallbackDispatcher } from "../src/agent-builder.js";
import { SessionBuildOptions } from "../src/models.js";
import { MetaRuntime } from "../src/runtime.js";
import { runtimeConfig } from "./runtime-config.js";

const DECISION_ID = `decision:${"a".repeat(64)}`;
const DIGEST = `sha256:${"b".repeat(64)}`;

interface RpcCall {
  method: string;
  params?: Record<string, unknown>;
}

function mockHandle(): { handle: any; calls: RpcCall[] } {
  const rt = new MetaRuntime(runtimeConfig());
  (rt as any)._running = true;
  const calls: RpcCall[] = [];
  (rt as any)._rpc = async (method: string, params?: Record<string, unknown>) => {
    calls.push({ method, params });
    return { decision: { decision_id: DECISION_ID, phase: "open" } };
  };
  return { handle: rt.mobHandle(), calls };
}

describe("MobHandle decision wrappers", () => {
  it("decision wrappers send the meta methods and their params", async () => {
    const { handle, calls } = mockHandle();
    await handle.decisionsList({
      kind: "release",
      phase: "open",
      approverId: "team:owners",
      requester: "principal:x",
      after: DECISION_ID,
      limit: 10,
    });
    await handle.decisionsList();
    await handle.decisionsGet(DECISION_ID);
    await handle.decisionsGet(DECISION_ID, { includeRequest: true });
    await handle.decisionsRendering(DECISION_ID, { maxPartBytes: 16 });
    const presentation = {
      audience: { kind: "approver", approver_id: "team:owners" },
      subject_digest: DIGEST,
      parts_shown: [{ index: 0, count: 1, part_digest: DIGEST }],
      channel_ref: "chat:owners",
      message_ref: "message-1",
      at_ms: 1,
    };
    await handle.decisionsPresent(DECISION_ID, presentation);
    await handle.decisionsWithdraw(DECISION_ID);
    await handle.decisionsDecide(DECISION_ID, { approve: true, presentedDigest: DIGEST });
    assert.deepEqual(calls, [
      {
        method: "meta/decisions/list",
        params: {
          kind: "release",
          phase: "open",
          approver_id: "team:owners",
          requester: "principal:x",
          after: DECISION_ID,
          limit: 10,
        },
      },
      { method: "meta/decisions/list", params: {} },
      { method: "meta/decisions/get", params: { decision_id: DECISION_ID } },
      {
        method: "meta/decisions/get",
        params: { decision_id: DECISION_ID, include_request: true },
      },
      {
        method: "meta/decisions/rendering",
        params: { decision_id: DECISION_ID, max_part_bytes: 16 },
      },
      {
        method: "meta/decisions/present",
        params: { decision_id: DECISION_ID, presentation },
      },
      { method: "meta/decisions/withdraw", params: { decision_id: DECISION_ID } },
      {
        method: "meta/decisions/decide",
        params: { decision_id: DECISION_ID, approve: true, presented_digest: DIGEST },
      },
    ]);
  });

  it("decide carries a typed attestation", async () => {
    const { handle, calls } = mockHandle();
    await handle.decisionsDecide(DECISION_ID, {
      approve: false,
      presentedDigest: DIGEST,
      reason: "not now",
      attestation: {
        ingress: "chat",
        subject: "U123",
        scope: "chat-session-1",
        eventRef: "chat:event-1",
        observedAtMs: 1_700_000_000_000,
      },
    });
    assert.deepEqual(calls, [
      {
        method: "meta/decisions/decide",
        params: {
          decision_id: DECISION_ID,
          approve: false,
          presented_digest: DIGEST,
          reason: "not now",
          attestation: {
            ingress: "chat",
            subject: "U123",
            scope: "chat-session-1",
            event_ref: "chat:event-1",
            observed_at_ms: 1_700_000_000_000,
          },
        },
      },
    ]);
  });

  it("decide inside a tool callback is refused", async () => {
    const { handle, calls } = mockHandle();
    let caught: unknown = undefined;
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerBuilder({
      async buildAgent(options: SessionBuildOptions): Promise<void> {
        options.profileName = "test";
        options.registerTool("approve_it", async () => {
          try {
            await handle.decisionsDecide(DECISION_ID, {
              approve: true,
              presentedDigest: DIGEST,
            });
          } catch (error) {
            caught = error;
          }
          return "done";
        });
      },
    });
    await dispatcher.handleCallback("callback/build_agent", { options: { scope_id: "s1" } });
    await dispatcher.handleCallback("callback/call_tool", {
      scope_id: "s1",
      tool: "approve_it",
      arguments: {},
    });
    assert.equal((caught as Error | undefined)?.name, "DecideInsideToolCallbackError");
    assert.equal((caught as { kind?: string } | undefined)?.kind, "decide_inside_tool_callback");
    assert.equal(calls.length, 0, "nothing is sent from inside a tool callback");

    // Outside a callback the host decides as before.
    await handle.decisionsDecide(DECISION_ID, { approve: true, presentedDigest: DIGEST });
    assert.deepEqual(
      calls.map((call) => call.method),
      ["meta/decisions/decide"],
    );
  });
});
