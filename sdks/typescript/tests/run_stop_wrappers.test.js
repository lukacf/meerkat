/**
 * Run-fenced Stop wrappers (`turn/stop_run`, `mob/stop_member_run`) send the
 * exact RPC literals with snake_case params and validate the typed receipt
 * union on its `outcome` discriminator.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";
import { Mob } from "../dist/mob.js";
import { Session } from "../dist/session.js";

const RUN_ID = "01936f8b-0000-7000-8000-000000000042";

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

describe("run-fenced stop wrappers", () => {
  it("Session.stopRun issues turn/stop_run and returns the stopped receipt", async () => {
    const receipt = {
      outcome: "stopped",
      run_id: RUN_ID,
      contributors: [
        { input_id: "i-1", completion: "cancelled", terminal: "cancelled" },
        { input_id: "i-2", completion: "runtime_terminated", terminal: "cancelled" },
      ],
    };
    const { client, calls } = fakeClient({
      "turn/stop_run": { session_id: "s-1", receipt },
    });
    const session = new Session(client, { sessionId: "s-1", sessionRef: undefined });

    const result = await session.stopRun(RUN_ID, "user pressed stop");

    assert.deepEqual(calls, [
      {
        method: "turn/stop_run",
        params: { session_id: "s-1", run_id: RUN_ID, reason: "user pressed stop" },
      },
    ]);
    assert.equal(result.session_id, "s-1");
    assert.deepEqual(result.receipt, receipt);
  });

  it("a stale stop is a not_current receipt, not an error", async () => {
    const receipt = { outcome: "not_current", run_id: RUN_ID, current_run_id: "r-2" };
    const { client } = fakeClient({ "turn/stop_run": { session_id: "s-1", receipt } });
    const result = await client._stopRun("s-1", RUN_ID, "late");
    assert.deepEqual(result.receipt, receipt);
  });

  it("a not_stoppable receipt carries the runtime state", async () => {
    const receipt = { outcome: "not_stoppable", run_id: RUN_ID, state: "stopped" };
    const { client } = fakeClient({ "turn/stop_run": { session_id: "s-1", receipt } });
    const result = await client._stopRun("s-1", RUN_ID, "stop");
    assert.deepEqual(result.receipt, receipt);
  });

  for (const receipt of [
    { outcome: "stopped", run_id: RUN_ID },
    { outcome: "not_stoppable", run_id: RUN_ID },
    { outcome: "mystery", run_id: RUN_ID },
    { outcome: "not_current" },
  ]) {
    it(`fails closed on a malformed receipt ${JSON.stringify(receipt)}`, async () => {
      const { client } = fakeClient({ "turn/stop_run": { session_id: "s-1", receipt } });
      await assert.rejects(client._stopRun("s-1", RUN_ID, "stop"), /turn\/stop_run/);
    });
  }

  it("Mob.stopMemberRun issues mob/stop_member_run with identity and run", async () => {
    const receipt = { outcome: "not_current", run_id: RUN_ID };
    const { client, calls } = fakeClient({
      "mob/stop_member_run": { mob_id: "mob-1", agent_identity: "worker-1", receipt },
    });
    const result = await new Mob(client, "mob-1").stopMemberRun("worker-1", RUN_ID, "stop");
    assert.deepEqual(calls, [
      {
        method: "mob/stop_member_run",
        params: {
          mob_id: "mob-1",
          agent_identity: "worker-1",
          run_id: RUN_ID,
          reason: "stop",
        },
      },
    ]);
    assert.equal(result.mob_id, "mob-1");
    assert.equal(result.agent_identity, "worker-1");
    assert.deepEqual(result.receipt, receipt);
  });
});
