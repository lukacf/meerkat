/**
 * Compaction request wrappers (`session/compact`, `mob/compact_member`) send
 * the exact RPC literals with snake_case params and validate the closed
 * `outcome` vocabulary.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";
import { Mob } from "../dist/mob.js";
import { Session } from "../dist/session.js";

function fakeClient(results) {
  const client = new MeerkatClient();
  const calls = [];
  client.request = async (method, params) => {
    calls.push({ method, params });
    const result = results[method];
    if (!result) {
      throw new Error(`no canned result for ${method}`);
    }
    return structuredClone(result);
  };
  return { client, calls };
}

describe("compaction request wrappers", () => {
  for (const outcome of ["requested", "already_requested"]) {
    it(`Session.compact issues session/compact and returns ${outcome}`, async () => {
      const { client, calls } = fakeClient({
        "session/compact": { session_id: "s-1", outcome },
      });
      const session = new Session(client, { sessionId: "s-1", sessionRef: undefined });

      const result = await session.compact();

      assert.deepEqual(calls, [
        { method: "session/compact", params: { session_id: "s-1" } },
      ]);
      assert.deepEqual(result, { session_id: "s-1", outcome });
    });
  }

  for (const malformed of [
    { session_id: "s-1", outcome: "compacted" },
    { session_id: "s-1" },
    { outcome: "requested" },
  ]) {
    it(`a malformed session/compact result fails closed: ${JSON.stringify(malformed)}`, async () => {
      const { client } = fakeClient({ "session/compact": malformed });
      await assert.rejects(client._compact("s-1"), { code: "INVALID_RESPONSE" });
    });
  }

  it("Mob.compactMember issues mob/compact_member with the identity", async () => {
    const { client, calls } = fakeClient({
      "mob/compact_member": {
        mob_id: "mob-1",
        agent_identity: "worker-1",
        outcome: "requested",
      },
    });
    const result = await new Mob(client, "mob-1").compactMember("worker-1");
    assert.deepEqual(calls, [
      {
        method: "mob/compact_member",
        params: { mob_id: "mob-1", agent_identity: "worker-1" },
      },
    ]);
    assert.deepEqual(result, {
      mob_id: "mob-1",
      agent_identity: "worker-1",
      outcome: "requested",
    });
  });

  it("mob/compact_member refuses an unknown outcome", async () => {
    const { client } = fakeClient({
      "mob/compact_member": {
        mob_id: "mob-1",
        agent_identity: "worker-1",
        outcome: "woken",
      },
    });
    await assert.rejects(client.compactMobMember("mob-1", "worker-1"), {
      code: "INVALID_RESPONSE",
    });
  });
});
