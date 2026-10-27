/**
 * A keyed send: `send(identity, content, { idempotencyKey })` and
 * `sendStatus`. The gateway commits a keyed send as a member continuation
 * under the key and answers the continuation's receipt with its delivery
 * status; a replay answers the first receipt. A gateway that predates keyed
 * sends would ignore the field and send unkeyed, so the SDK refuses a key,
 * typed, unless the gateway advertises `meta/send_status`, the read that
 * ships with keyed sends. A keyed steer (`handlingMode: "steer"`) also needs
 * the `keyed_send_handling_mode` feature capability: a gateway without it
 * would ignore the mode and queue.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { CapabilityUnavailableError } from "../src/errors.js";
import {
  keyedSendResultToDict,
  parseContinuationStatus,
  parseKeyedSendResult,
} from "../src/types.js";
import { runtimeConfig } from "./runtime-config.js";

const RECEIPT = {
  delivery_id: "delivery-1",
  committed_at_ms: 1_761_000_000_000,
  status: { state: "admitted", session_id: "session-1", input_id: "input-1" },
};

async function makeRuntime(keyed: boolean, steer = false) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params: Record<string, unknown> = {},
  ) => {
    calls.push({ method, params });
    switch (method) {
      case "meta/capabilities":
        return {
          contract_version: "0.5.0",
          methods: keyed
            ? ["meta/send", "meta/status_identity", "meta/send_status"]
            : ["meta/send", "meta/status_identity"],
          feature_capabilities: steer ? ["keyed_send_handling_mode"] : [],
          loaded_modules: [],
        };
      case "meta/send":
        return "idempotency_key" in params ? { continuation: RECEIPT } : { fencing_token: 1 };
      case "meta/send_status":
        return params.input_id === "unknown"
          ? {}
          : {
              state: {
                input_id: params.input_id,
                current_state: "consumed",
                terminal_outcome: "completed",
                attempt_count: 1,
                recovery_count: 0,
                created_at: "2026-10-23T03:00:00Z",
                updated_at: "2026-10-23T03:00:01Z",
              },
            };
      default:
        return {};
    }
  };
  const sent = (method: string) =>
    calls.filter((call) => call.method === method).map((call) => call.params);
  return { rt, calls, sent };
}

describe("keyed send", () => {
  it("forwards the key and returns the receipt", async () => {
    const { rt, sent } = await makeRuntime(true);
    const result = await rt.send("keeper", "Ingest item 1.", { idempotencyKey: "item-1" });
    assert.equal(result.deliveryId, "delivery-1");
    assert.equal(result.committedAtMs, 1_761_000_000_000);
    assert.deepEqual(result.status, {
      state: "admitted",
      sessionId: "session-1",
      inputId: "input-1",
    });
    assert.deepEqual(sent("meta/send"), [
      { identity: "keeper", content: "Ingest item 1.", idempotency_key: "item-1" },
    ]);
  });

  it("refuses a key, typed, on a gateway that does not serve keyed sends", async () => {
    const { rt, sent } = await makeRuntime(false);
    await assert.rejects(
      rt.send("keeper", "Ingest item 2.", { idempotencyKey: "item-2" }),
      (error: unknown) =>
        error instanceof CapabilityUnavailableError && error.method === "meta/send",
    );
    assert.deepEqual(sent("meta/send"), [], "nothing is sent unkeyed");
    await rt.send("keeper", "Ingest item 2.");
    assert.deepEqual(sent("meta/send"), [{ identity: "keeper", content: "Ingest item 2." }]);
  });

  it("refuses a key with a tracked turn or an authored row before any call", async () => {
    const { rt, calls } = await makeRuntime(true);
    for (const extra of [{ trackTurn: true }, { expectedSpecDigest: "sha256:aaaa" }]) {
      await assert.rejects(
        rt.send("keeper", "Ingest item 3.", {
          idempotencyKey: "item-3",
          ...extra,
        } as unknown as { idempotencyKey: string }),
        /idempotencyKey/,
      );
    }
    assert.deepEqual(calls, []);
  });

  it("reads an input's state, and an unknown input as null", async () => {
    const { rt, sent } = await makeRuntime(true);
    const read = await rt.sendStatus("keeper", "session-1", "input-1");
    const unknown = await rt.sendStatus("keeper", "session-1", "unknown");
    assert.equal(read.state?.inputId, "input-1");
    assert.equal(read.state?.currentState, "consumed");
    assert.equal(read.state?.terminalOutcome, "completed");
    assert.equal(read.state?.raw.attempt_count, 1);
    assert.equal(unknown.state, null);
    assert.deepEqual(sent("meta/send_status")[0], {
      identity: "keeper",
      session_id: "session-1",
      input_id: "input-1",
    });
  });

  it("forwards a keyed steer's handling mode", async () => {
    const { rt, sent } = await makeRuntime(true, true);
    const result = await rt.send("keeper", "Check the ledger too.", {
      idempotencyKey: "steer-1",
      handlingMode: "steer",
    });
    assert.equal(result.deliveryId, "delivery-1");
    assert.deepEqual(sent("meta/send"), [
      {
        identity: "keeper",
        content: "Check the ledger too.",
        idempotency_key: "steer-1",
        handling_mode: "steer",
      },
    ]);
  });

  it("refuses a keyed steer, typed, on a gateway without the keyed mode, and still queues", async () => {
    const { rt, sent } = await makeRuntime(true, false);
    await assert.rejects(
      rt.send("keeper", "Check the ledger too.", {
        idempotencyKey: "steer-2",
        handlingMode: "steer",
      }),
      (error: unknown) =>
        error instanceof CapabilityUnavailableError &&
        error.method === "meta/send" &&
        JSON.stringify(error.data) === JSON.stringify({ capability: "keyed_send_handling_mode" }),
    );
    assert.deepEqual(sent("meta/send"), [], "nothing is sent as a queued send");
    await rt.send("keeper", "Check the ledger too.", {
      idempotencyKey: "queue-2",
      handlingMode: "queue",
    });
    assert.deepEqual(sent("meta/send"), [
      {
        identity: "keeper",
        content: "Check the ledger too.",
        idempotency_key: "queue-2",
        handling_mode: "queue",
      },
    ]);
  });

  it("refuses an unkeyed steer, or an unknown mode, before any call", async () => {
    const { rt, calls } = await makeRuntime(true, true);
    await assert.rejects(
      rt.send("keeper", "Check the ledger too.", {
        handlingMode: "steer",
      } as unknown as { idempotencyKey: string }),
      /idempotencyKey/,
    );
    for (const extra of [{}, { idempotencyKey: "steer-3" }]) {
      await assert.rejects(
        rt.send("keeper", "Check the ledger too.", {
          handlingMode: "sideways",
          ...extra,
        } as unknown as { idempotencyKey: string }),
        /handlingMode/,
      );
    }
    assert.deepEqual(calls, []);
  });

  it("takes an unkeyed queue as the default send", async () => {
    const { rt, sent } = await makeRuntime(true, false);
    await rt.send("keeper", "File the report.", { handlingMode: "queue" });
    assert.deepEqual(
      sent("meta/send"),
      [{ identity: "keeper", content: "File the report." }],
      "an unkeyed queue is the default send",
    );
    assert.deepEqual(sent("meta/capabilities"), [], "an unkeyed send needs no capability");
  });

  it("parses every continuation state and tolerates new ones", () => {
    assert.deepEqual(parseContinuationStatus({ state: "pending" }), { state: "pending" });
    assert.equal(parseContinuationStatus({ state: "stranded" }).state, "stranded");
    assert.equal(
      parseContinuationStatus({ state: "refused", reason: "member_retired" }).reason,
      "member_retired",
    );
    assert.equal(parseContinuationStatus({ state: "later" }).state, "later");
    assert.deepEqual(keyedSendResultToDict(parseKeyedSendResult({ continuation: RECEIPT })), {
      continuation: RECEIPT,
    });
  });
});
