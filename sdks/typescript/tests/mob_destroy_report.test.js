/**
 * The typed mob destroy report: `mobDestroyIncompleteReport` and `mobDestroy`.
 *
 * The report is `WireMobDestroyReport` in artifacts/schemas/wire-types.json,
 * emitted from the Rust contract (crates/meerkat-contracts/src/wire/mob.rs).
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { MeerkatClient, MeerkatError, mobDestroyIncompleteReport } from "../dist/index.js";

const SCHEMA = JSON.parse(
  readFileSync(new URL("../../../artifacts/schemas/wire-types.json", import.meta.url), "utf8"),
).WireMobDestroyReport;

// Every field set, as a server reports it.
const FULL_REPORT = {
  force_destroyed_members: ["forced"],
  orphaned_remote_members: ["orphaned"],
  remote_cleanup_deadline_exceeded: true,
  metadata_scrubbed: true,
  events_cleared: true,
  namespace_cleaned: true,
  errors: ["refused: the runtime refused the reload"],
  reloaded_members: ["reloaded"],
  member_reload_refusals: [
    {
      member: "refused",
      refusal: {
        kind: "repair_blocked",
        reason: "commit outcome not reported",
        evidence_digest: "digest-1",
      },
    },
  ],
};

function incompleteData(report) {
  return { code: "mob_destroy_incomplete", destroy_report: report, retryable: true };
}

/** The client's rejection of a JSON-RPC error response carrying `data`. */
async function rpcError(requestId, data) {
  const client = new MeerkatClient();
  const response = client.registerRequest(requestId);
  client.handleLine(
    JSON.stringify({
      jsonrpc: "2.0",
      id: requestId,
      error: { code: -32603, message: "mob destroy incomplete: refused", data },
    }),
  );
  return response.then(
    () => assert.fail("the request must reject"),
    (error) => error,
  );
}

describe("mobDestroyIncompleteReport", () => {
  it("reads the typed report of an incomplete destroy through the RPC transport", async () => {
    const error = await rpcError(700, incompleteData(FULL_REPORT));
    assert.ok(error instanceof MeerkatError);
    assert.equal(error.code, "mob_destroy_incomplete");
    const report = mobDestroyIncompleteReport(error);
    assert.equal(report?.member_reload_refusals?.[0]?.refusal.kind, "repair_blocked");
    assert.deepEqual(report, FULL_REPORT);
    assert.deepEqual(error.details, incompleteData(FULL_REPORT), "details stay as sent");
  });

  it("reads every field the schema names", () => {
    const report = mobDestroyIncompleteReport(
      new MeerkatError("mob_destroy_incomplete", "incomplete", incompleteData(FULL_REPORT)),
    );
    assert.deepEqual(Object.keys(report).sort(), Object.keys(SCHEMA.properties).sort());
  });

  it("reads a refusal kind it does not know as unknown, without an error", async () => {
    const error = await rpcError(
      701,
      incompleteData({
        errors: ["m: refused"],
        member_reload_refusals: [{ member: "m", refusal: { kind: "a_later_kind", detail: 1 } }],
      }),
    );
    assert.deepEqual(mobDestroyIncompleteReport(error), {
      errors: ["m: refused"],
      member_reload_refusals: [{ member: "m", refusal: { kind: "unknown" } }],
    });
  });

  it("leaves absent fields absent", () => {
    const report = mobDestroyIncompleteReport(
      new MeerkatError(
        "mob_destroy_incomplete",
        "incomplete",
        incompleteData({
          metadata_scrubbed: false,
          member_reload_refusals: [
            { member: "m", refusal: { kind: "repair_blocked", reason: "r" } },
            { member: "n", refusal: { kind: "repair_blocked", reason: "r", evidence_digest: null } },
          ],
        }),
      ),
    );
    assert.deepEqual(report, {
      metadata_scrubbed: false,
      member_reload_refusals: [
        { member: "m", refusal: { kind: "repair_blocked", reason: "r" } },
        { member: "n", refusal: { kind: "repair_blocked", reason: "r" } },
      ],
    });
  });

  it("is undefined for another code or a report that does not parse", () => {
    const refusal = (value) => incompleteData({ member_reload_refusals: [value] });
    for (const details of [
      undefined,
      null,
      "mob_destroy_incomplete",
      { code: "send_failed", destroy_report: FULL_REPORT },
      { code: "mob_destroy_incomplete" },
      incompleteData(null),
      incompleteData([]),
      incompleteData({ errors: "x" }),
      incompleteData({ errors: [1] }),
      incompleteData({ reloaded_members: null }),
      incompleteData({ metadata_scrubbed: "yes" }),
      incompleteData({ member_reload_refusals: {} }),
      refusal({ refusal: { kind: "repair_blocked", reason: "r" } }),
      refusal({ member: "m" }),
      refusal({ member: "m", refusal: { reason: "r" } }),
      refusal({ member: "m", refusal: { kind: "repair_blocked" } }),
      refusal({ member: "m", refusal: { kind: "repair_blocked", reason: "r", evidence_digest: 1 } }),
    ]) {
      assert.equal(
        mobDestroyIncompleteReport(new MeerkatError("mob_destroy_incomplete", "x", details)),
        undefined,
        JSON.stringify(details),
      );
    }
    assert.equal(mobDestroyIncompleteReport(new Error("not a meerkat error")), undefined);
    assert.equal(mobDestroyIncompleteReport(undefined), undefined);
  });
});

describe("mobDestroy", () => {
  function destroyClient(response) {
    const client = new MeerkatClient();
    client.request = async () => response;
    return client;
  }

  it("returns the typed report", async () => {
    const result = await destroyClient({
      mob_id: "mob-1",
      ok: true,
      destroy_report: FULL_REPORT,
    }).mobDestroy("mob-1");
    assert.deepEqual(result, { mobId: "mob-1", ok: true, destroyReport: FULL_REPORT });
  });

  it("reads a refusal kind it does not know as unknown, without an error", async () => {
    const result = await destroyClient({
      mob_id: "mob-1",
      ok: true,
      destroy_report: {
        member_reload_refusals: [{ member: "m", refusal: { kind: "a_later_kind" } }],
      },
    }).mobDestroy("mob-1");
    assert.equal(result.destroyReport.member_reload_refusals[0].refusal.kind, "unknown");
  });

  it("rejects a malformed report", async () => {
    await assert.rejects(
      () =>
        destroyClient({ mob_id: "mob-1", ok: true, destroy_report: { errors: "x" } }).mobDestroy(
          "mob-1",
        ),
      (error) =>
        error instanceof MeerkatError &&
        error.code === "INVALID_RESPONSE" &&
        /malformed destroy_report/.test(error.message),
    );
  });
});
