/**
 * A quarantined persisted mob: `mobQuarantined`, `listQuarantinedMobs` and
 * `mobRestoreRetry`.
 *
 * A persisted mob whose store did not restore is quarantined: `mob/list`
 * names it under `quarantined`, and every call that addresses its id rejects
 * with `error.data.kind` `"mob_quarantined"` (`MobError::MobQuarantined` in
 * crates/meerkat-mob/src/error.rs). `mob/restore_retry` re-attempts its
 * restore.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  MOB_QUARANTINED_KIND,
  MOB_QUARANTINE_CLASSES,
  MeerkatClient,
  MeerkatError,
  mobQuarantined,
} from "../dist/index.js";

// The refusal's data, as a server sends it for a store that could not serve.
const REFUSAL = {
  kind: "mob_quarantined",
  code: "SESSION_RUNTIME_UNAVAILABLE",
  mob_id: "broken",
  class: "store_unavailable",
  retryable: true,
  content_retained: true,
  cause: "session catalog unavailable",
};

// One `mob/list` quarantined row, as a server sends it.
const ROW = {
  mob_id: "broken",
  class: "store_unreadable",
  retryable: false,
  retrying: false,
  attempts: 1,
  message: "mob 'broken' is quarantined (store_unreadable): file is not a database",
  code: "INTERNAL_ERROR",
  structured_data: { kind: "mob_quarantined", class: "store_unreadable" },
};

/** The client's rejection of a JSON-RPC error response carrying `data`. */
async function rpcError(requestId, data) {
  const client = new MeerkatClient();
  const response = client.registerRequest(requestId);
  client.handleLine(
    JSON.stringify({
      jsonrpc: "2.0",
      id: requestId,
      error: {
        code: -32603,
        message: "mob 'broken' is quarantined (store_unavailable): session catalog unavailable",
        data,
      },
    }),
  );
  return response.then(
    () => assert.fail("the request must reject"),
    (error) => error,
  );
}

/** A client whose requests answer `answer(method, params)`. */
function answeringClient(answer) {
  const client = new MeerkatClient();
  const sent = [];
  client.requireCapability = () => {};
  client.request = async (method, params) => {
    sent.push([method, params]);
    return answer(method, params);
  };
  return { client, sent };
}

describe("mobQuarantined", () => {
  it("reads the typed refusal through the RPC transport", async () => {
    const error = await rpcError(710, REFUSAL);
    assert.ok(error instanceof MeerkatError);
    assert.equal(MOB_QUARANTINED_KIND, "mob_quarantined");
    assert.deepEqual(mobQuarantined(error), {
      mobId: "broken",
      quarantineClass: "store_unavailable",
      retryable: true,
      cause: "session catalog unavailable",
      code: "SESSION_RUNTIME_UNAVAILABLE",
    });
    assert.deepEqual(error.details, REFUSAL, "details stay as sent");
  });

  it("keeps a class this SDK does not know as sent", () => {
    assert.deepEqual([...MOB_QUARANTINE_CLASSES].sort(), [
      "resume_refused",
      "store_unavailable",
      "store_unreadable",
    ]);
    const refusal = mobQuarantined(
      new MeerkatError("INTERNAL_ERROR", "quarantined", {
        ...REFUSAL,
        class: "store_migrating",
        retryable: false,
      }),
    );
    assert.equal(refusal?.quarantineClass, "store_migrating");
    assert.equal(refusal?.retryable, false);
  });

  it("answers undefined for any other error", () => {
    assert.equal(mobQuarantined(new Error("not a meerkat error")), undefined);
    assert.equal(
      mobQuarantined(
        new MeerkatError("SESSION_RUNTIME_UNAVAILABLE", "held", {
          kind: "mob_held_before_activation",
          mob_id: "child",
        }),
      ),
      undefined,
    );
    assert.equal(
      mobQuarantined(new MeerkatError("INTERNAL_ERROR", "x", { ...REFUSAL, retryable: "yes" })),
      undefined,
    );
  });
});

describe("listQuarantinedMobs", () => {
  it("reads the quarantined rows beside the managed ones", async () => {
    const { client, sent } = answeringClient(() => ({
      mobs: [{ mob_id: "mob-1", status: "Running" }],
      quarantined: [ROW],
    }));
    assert.deepEqual(await client.listQuarantinedMobs(), [
      {
        mobId: "broken",
        quarantineClass: "store_unreadable",
        retryable: false,
        retrying: false,
        attempts: 1,
        message: ROW.message,
        code: "INTERNAL_ERROR",
        structuredData: ROW.structured_data,
      },
    ]);
    assert.deepEqual(await client.listMobs(), [{ mobId: "mob-1", status: "Running" }]);
    assert.deepEqual(
      sent.map(([method]) => method),
      ["mob/list", "mob/list"],
    );
  });

  it("is empty when the answer names none", async () => {
    const { client } = answeringClient(() => ({ mobs: [] }));
    assert.deepEqual(await client.listQuarantinedMobs(), []);
  });

  it("rejects a malformed row", async () => {
    const { client } = answeringClient(() => ({ mobs: [], quarantined: [{ mob_id: "broken" }] }));
    await assert.rejects(client.listQuarantinedMobs(), (error) => {
      assert.ok(error instanceof MeerkatError);
      assert.equal(error.code, "INVALID_RESPONSE");
      return true;
    });
  });
});

describe("mobRestoreRetry", () => {
  it("sends the mob id and answers the outcome", async () => {
    const { client, sent } = answeringClient(() => ({ mob_id: "broken", outcome: "restored" }));
    assert.deepEqual(await client.mobRestoreRetry("broken"), {
      mob_id: "broken",
      outcome: "restored",
    });
    assert.deepEqual(sent, [["mob/restore_retry", { mob_id: "broken" }]]);
  });
});
