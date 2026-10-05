import { test } from "node:test";
import assert from "node:assert/strict";
import { authErrorReason, MeerkatError, WIRE_AUTH_ERROR_REASONS } from "../dist/index.js";

test("authErrorReason reads the typed reason from error details", () => {
  for (const reason of WIRE_AUTH_ERROR_REASONS) {
    assert.equal(authErrorReason(new MeerkatError("-32602", "text", { reason })), reason);
  }
  assert.equal(authErrorReason(new MeerkatError("-32602", "slot_occupied")), undefined);
  assert.equal(authErrorReason(new MeerkatError("-32602", "text", { reason: "other" })), undefined);
  assert.equal(authErrorReason(null), undefined);
});
