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

test("authErrorDetail reads the typed sub-kind from error details", async () => {
  const { authErrorDetail, WIRE_AUTH_ERROR_DETAILS } = await import("../dist/index.js");
  assert.deepEqual([...WIRE_AUTH_ERROR_DETAILS], ["remote_custody_required"]);
  const refused = new MeerkatError("-32602", "text", {
    reason: "invalid_target",
    detail: "remote_custody_required",
  });
  assert.equal(authErrorReason(refused), "invalid_target");
  assert.equal(authErrorDetail(refused), "remote_custody_required");
  assert.equal(authErrorDetail(new MeerkatError("-32602", "text", { reason: "invalid_target" })), undefined);
  assert.equal(authErrorDetail(new MeerkatError("-32602", "text", { detail: "other" })), undefined);
  assert.equal(authErrorDetail(null), undefined);
});
