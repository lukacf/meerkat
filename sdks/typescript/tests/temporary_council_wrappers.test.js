/**
 * Issue #159: the temporary-council wrappers issue the exact RPC method
 * literals with snake_case params, keep the one-time host bootstrap OUT of
 * the request object, treat an unknown council as a typed absence, and fail
 * closed on a malformed envelope.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";

const REQUEST = {
  council_id: "design-review-42",
  definition_template: { id: "ignored", profiles: {} },
  participants: [
    {
      order: 0,
      role: "critic",
      source_mob_id: "source",
      source_identity: "alice",
      target_identity: "alice-branch",
      target_profile: "council",
      scope: "invoke_and_observe",
    },
  ],
  topic: "should we ship?",
  bounds: {
    deadline: { kind: "relative", after_millis: 60000 },
    max_rounds: 1,
    max_exchanges: 2,
    max_result_bytes: 4096,
  },
  merge_back: { policy: "no_merge" },
  durability: "durable",
};

const RUN_RESULT = {
  result: {
    council_id: "design-review-42",
    request_fingerprint: "sha256:abc",
    temporary_mob_id: "council--design-review-42",
    exit_reason: { reason: "completed" },
    rounds_completed: 1,
    exchanges: [],
    merge: { kind: "no_merge", confirmed_participants: ["alice-branch"] },
    participants: [],
    truncated_exchange_count: 0,
    merge_truncated: false,
    durability: "durable",
    concluded_at: "2026-08-28T10:00:00.000Z",
  },
  cleanup: {
    status: "settled",
    attempted_at: "2026-08-28T10:00:01.000Z",
    attempts: 1,
    temporary_mob_destroyed: true,
    released_participants: [0],
    revoked_participants: [],
    debts: [],
    budget_exhausted: false,
  },
  replayed: false,
};

function cannedClient(result) {
  const client = new MeerkatClient();
  const calls = [];
  client.request = async (method, params) => {
    calls.push({ method, params });
    return structuredClone(result);
  };
  return { client, calls };
}

describe("temporary council wrappers", () => {
  it("runTemporaryCouncil issues mob/temporary_council_run without a bootstrap", async () => {
    const { client, calls } = cannedClient(RUN_RESULT);
    const outcome = await client.runTemporaryCouncil(REQUEST);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].method, "mob/temporary_council_run");
    assert.deepEqual(calls[0].params, { request: REQUEST });
    assert.equal(outcome.replayed, false);
    assert.equal(outcome.result.exit_reason.reason, "completed");
    assert.equal(outcome.cleanup.status, "settled");
  });

  it("keeps the one-time host bootstrap outside the fingerprinted request", async () => {
    const { client, calls } = cannedClient(RUN_RESULT);
    const descriptor = {
      kind: "host",
      address: "tcp://10.0.0.2:7100",
      identity: { public_key: "AAAA" },
      bootstrap_token: "one-time",
    };
    await client.runTemporaryCouncil(REQUEST, [descriptor]);
    assert.deepEqual(calls[0].params, {
      request: REQUEST,
      host_bindings: [descriptor],
    });
    assert.equal(
      Object.hasOwn(calls[0].params.request, "host_bindings"),
      false,
      "a ceremony token must never be folded into the council request",
    );
  });

  it("getTemporaryCouncil treats an unknown council as a typed absence", async () => {
    const { client, calls } = cannedClient({});
    const result = await client.getTemporaryCouncil("never-created");
    assert.equal(calls[0].method, "mob/temporary_council_get");
    assert.deepEqual(calls[0].params, { council_id: "never-created" });
    assert.equal(result.council, undefined);
  });

  it("recoverTemporaryCouncils issues the maintenance sweep with no params", async () => {
    const { client, calls } = cannedClient({ reports: [] });
    const result = await client.recoverTemporaryCouncils();
    assert.equal(calls[0].method, "mob/temporary_council_recover");
    assert.deepEqual(calls[0].params, {});
    assert.deepEqual(result.reports, []);
  });

  it("fails closed on a malformed run envelope", async () => {
    for (const malformed of [
      { cleanup: RUN_RESULT.cleanup, replayed: false },
      { result: RUN_RESULT.result, replayed: false },
      { result: RUN_RESULT.result, cleanup: RUN_RESULT.cleanup },
    ]) {
      const { client } = cannedClient(malformed);
      await assert.rejects(
        () => client.runTemporaryCouncil(REQUEST),
        /Invalid mob\/temporary_council_run response/,
      );
    }
  });

  it("fails closed on a non-object council projection", async () => {
    const { client } = cannedClient({ council: "not-a-record" });
    await assert.rejects(
      () => client.getTemporaryCouncil("design-review-42"),
      /Invalid mob\/temporary_council_get response/,
    );
  });
});
