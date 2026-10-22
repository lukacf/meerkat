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
  assert.deepEqual([...WIRE_AUTH_ERROR_DETAILS], [
    "remote_custody_required",
    "upstream_timeout",
    "upstream_timeout_after_code",
  ]);
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

test("an MCP network timeout says whether the code was sent; an unsettled commit is its own reason", async () => {
  const { authErrorDetail } = await import("../dist/index.js");
  assert.ok(WIRE_AUTH_ERROR_REASONS.includes("commit_unsettled"));
  for (const detail of ["upstream_timeout", "upstream_timeout_after_code"]) {
    const timedOut = new MeerkatError("-32603", "text", { reason: "upstream_failure", detail });
    assert.equal(authErrorReason(timedOut), "upstream_failure");
    assert.equal(authErrorDetail(timedOut), detail);
  }
  const unsettled = new MeerkatError("-32603", "text", { reason: "commit_unsettled" });
  assert.equal(authErrorReason(unsettled), "commit_unsettled");
  assert.equal(authErrorDetail(unsettled), undefined);
});
