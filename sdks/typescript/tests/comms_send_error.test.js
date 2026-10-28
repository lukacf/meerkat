/**
 * Typed comms/send error data: `CommsSendErrorData` and `commsSendError`.
 *
 * The codes come from `CommsSendErrorData` in
 * artifacts/schemas/wire-types.json, emitted from the Rust contract
 * (crates/meerkat-contracts/src/wire/comms.rs).
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { COMMS_SEND_ERROR_CODES, MeerkatError, commsSendError } from "../dist/index.js";

const SCHEMA = JSON.parse(
  readFileSync(new URL("../../../artifacts/schemas/wire-types.json", import.meta.url), "utf8"),
).CommsSendErrorData;
const WIRE_CODES = SCHEMA.oneOf.map((arm) => arm.properties.code.const).sort();

describe("commsSendError", () => {
  it("knows exactly the wire codes", () => {
    assert.deepEqual([...COMMS_SEND_ERROR_CODES].sort(), WIRE_CODES);
  });

  for (const code of WIRE_CODES) {
    it(`returns the typed data for ${code}`, () => {
      const data = { code, message: "comms/send failed" };
      assert.deepEqual(commsSendError(new MeerkatError("INTERNAL_ERROR", "failed", data)), data);
    });
  }

  it("is undefined without a known code", () => {
    for (const details of [undefined, null, "send_failed", { message: "no code" }, { code: 1 }, { code: "not_a_code" }]) {
      assert.equal(commsSendError(new MeerkatError("INTERNAL_ERROR", "failed", details)), undefined);
    }
    assert.equal(commsSendError(new Error("not a meerkat error")), undefined);
    assert.equal(commsSendError(undefined), undefined);
  });

  const ENVELOPE_ID = "00000000-0000-4000-8000-000000000001";
  const AMBIGUOUS_MESSAGE =
    `delivery outcome for envelope ${ENVELOPE_ID} is unknown; it may already be ` +
    "delivered. Reconcile against durable work truth before any new attempt - a " +
    "retry can duplicate. (ack timed out)";

  // The JSON-RPC error responses `comms/send` returns
  // (crates/meerkat-rpc/src/handlers/comms.rs): the typed data is `error.data`.
  const WIRE_FAILURES = [
    {
      name: "peer_unreachable (transport_error)",
      rpcCode: -32603,
      rpcMessage: "peer 'alice' is unreachable: transport_error",
      data: {
        code: "peer_unreachable",
        peer: "alice",
        reason: "transport_error",
        message: "peer 'alice' is unreachable: transport_error",
        details: "connection refused",
      },
    },
    {
      name: "peer_unreachable (offline_or_no_ack)",
      rpcCode: -32603,
      rpcMessage: "peer 'alice' is unreachable: offline_or_no_ack",
      data: {
        code: "peer_unreachable",
        peer: "alice",
        reason: "offline_or_no_ack",
        message: "peer 'alice' is unreachable: offline_or_no_ack",
      },
    },
    {
      name: "peer_admission_dropped",
      rpcCode: -32603,
      rpcMessage: "peer 'alice' rejected envelope at ingress: inbox_full",
      data: {
        code: "peer_admission_dropped",
        peer: "alice",
        reason: "inbox_full",
        message: "peer 'alice' rejected envelope at ingress: inbox_full",
      },
    },
    {
      name: "peer_wake_refused (mob_not_running)",
      rpcCode: -32603,
      rpcMessage: `peer 'alice' was not woken for envelope ${ENVELOPE_ID} (mob_not_running)`,
      data: {
        code: "peer_wake_refused",
        peer: "alice",
        envelope_id: ENVELOPE_ID,
        refusal: { kind: "mob_not_running" },
        retryable: true,
        message: `peer 'alice' was not woken for envelope ${ENVELOPE_ID} (mob_not_running)`,
      },
    },
    {
      name: "peer_wake_refused (member_broken)",
      rpcCode: -32603,
      rpcMessage: `peer 'alice' was not woken for envelope ${ENVELOPE_ID} (member_broken)`,
      data: {
        code: "peer_wake_refused",
        peer: "alice",
        envelope_id: ENVELOPE_ID,
        refusal: {
          kind: "member_broken",
          reason: "missing durable session snapshot",
          hold: "tail_held_for_recovery",
        },
        retryable: false,
        message: `peer 'alice' was not woken for envelope ${ENVELOPE_ID} (member_broken)`,
      },
    },
    {
      name: "send_ambiguous",
      rpcCode: -32603,
      rpcMessage: AMBIGUOUS_MESSAGE,
      data: {
        code: "send_ambiguous",
        envelope_id: ENVELOPE_ID,
        message: AMBIGUOUS_MESSAGE,
        retry_safe: false,
        required_action: "reconcile",
      },
    },
    {
      name: "invalid_command",
      rpcCode: -32602,
      rpcMessage: "Command validation failed",
      data: { code: "invalid_command", message: "body must not be empty" },
    },
  ];

  for (const [index, failure] of WIRE_FAILURES.entries()) {
    it(`keeps the whole typed data through the RPC transport: ${failure.name}`, async () => {
      const { MeerkatClient } = await import("../dist/client.js");
      const client = new MeerkatClient();
      const requestId = 400 + index;
      const response = client.registerRequest(requestId);
      client.handleLine(
        JSON.stringify({
          jsonrpc: "2.0",
          id: requestId,
          error: { code: failure.rpcCode, message: failure.rpcMessage, data: failure.data },
        }),
      );
      await assert.rejects(response, (error) => {
        assert.equal(error.code, failure.data.code);
        assert.deepEqual(commsSendError(error), failure.data);
        return true;
      });
    });
  }
});
