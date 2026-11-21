/** Request-shape checks only; completeness and authentication belong to the gateway. */
import assert from "node:assert/strict";
import { it } from "node:test";

import * as sdk from "../src/index.js";
import * as decisions from "../src/decisions.js";
import { MetaRuntime } from "../src/runtime.js";
import { runtimeConfig } from "./runtime-config.js";

const DECISION_ID = `decision:${"a".repeat(64)}`;
const DIGEST = `sha256:${"b".repeat(64)}`;

type PresentationValue = { toDict(): Record<string, unknown> };

function presentationType(): new (options: Record<string, unknown>) => PresentationValue {
  const value = (sdk as unknown as Record<string, unknown>).DecisionPresentation;
  assert.equal(typeof value, "function", "missing public typed presentation export: DecisionPresentation");
  assert.equal(value, (decisions as unknown as Record<string, unknown>).DecisionPresentation);
  return value as new (options: Record<string, unknown>) => PresentationValue;
}

function input(audience = "approver", options = false): Record<string, unknown> {
  return {
    audience: audience === "approver"
      ? { kind: "approver", approverId: "team:owners" }
      : { kind: "reviewer", reviewer: "principal:reviewer" },
    subjectDigest: DIGEST,
    partsShown: [{ index: 0, count: 1, partDigest: DIGEST }],
    channelRef: "chat:owners", messageRef: "message-1", atMs: 0,
    ...(options ? { maxPartBytes: 0, link: { reference: "document:1", servedDigest: DIGEST } } : {}),
  };
}

function expectedWire(audience = "approver", options = false): Record<string, unknown> {
  return {
    audience: audience === "approver"
      ? { kind: "approver", approver_id: "team:owners" }
      : { kind: "reviewer", reviewer: "principal:reviewer" },
    subject_digest: DIGEST,
    parts_shown: [{ index: 0, count: 1, part_digest: DIGEST }],
    channel_ref: "chat:owners", message_ref: "message-1", at_ms: 0,
    ...(options ? { max_part_bytes: 0, link: { reference: "document:1", served_digest: DIGEST } } : {}),
  };
}

function mockHandle() {
  const runtime = new MetaRuntime(runtimeConfig());
  const calls: { method: string; params?: Record<string, unknown> }[] = [];
  (runtime as any)._rpc = async (method: string, params?: Record<string, unknown>) => {
    calls.push({ method, params });
    return { decision: { decision_id: DECISION_ID, phase: "open" } };
  };
  return { handle: runtime.mobHandle(), calls };
}

function attestation(presentation: unknown) {
  return {
    ingress: "chat", subject: "U123", scope: "session-1",
    eventRef: "event-1", observedAtMs: 1, presentation,
  };
}

it("raw presentations remain a deliberate unchanged compatibility path", async () => {
  const raw = { audience: { kind: "future" }, extra: "keep", link: null };
  const { handle, calls } = mockHandle();
  await handle.decisionsPresent(DECISION_ID, raw);
  await handle.decisionsDecide(DECISION_ID, {
    approve: true, presentedDigest: DIGEST,
    attestation: { ...attestation(raw), presentation: raw },
  });
  assert.deepEqual(calls[0].params?.presentation, raw);
  assert.deepEqual((calls[1].params?.attestation as any).presentation, raw);
  assert.deepEqual(raw, { audience: { kind: "future" }, extra: "keep", link: null });
});

it("the typed presentation constructor is exported from package and module", () => {
  presentationType();
});

for (const audience of ["approver", "reviewer"]) {
  for (const options of [false, true]) {
    it(`both nested forms send exact ${audience} presentation, options=${options}`, async () => {
      const Presentation = presentationType();
      const value = new Presentation(input(audience, options));
      const wire = expectedWire(audience, options);
      assert.deepEqual(value.toDict(), wire);
      const { handle, calls } = mockHandle();
      // Runtime assertions intentionally survive the feature's missing type
      // exports on the baseline. The separate type fixture checks typed calls.
      await handle.decisionsPresent(DECISION_ID, value as any);
      await handle.decisionsDecide(DECISION_ID, {
        approve: false, presentedDigest: DIGEST, reason: "reviewed",
        attestation: attestation(value) as any,
      });
      assert.deepEqual(calls, [
        { method: "meta/decisions/present", params: { decision_id: DECISION_ID, presentation: wire } },
        { method: "meta/decisions/decide", params: {
          decision_id: DECISION_ID, approve: false, presented_digest: DIGEST, reason: "reviewed",
          attestation: { ingress: "chat", subject: "U123", scope: "session-1",
            event_ref: "event-1", observed_at_ms: 1, presentation: wire },
        } },
      ]);
      // Compare the same valid legacy wire object through both wrappers,
      // for each audience and absent/present optional-field combination.
      const legacy = mockHandle();
      await legacy.handle.decisionsPresent(DECISION_ID, wire);
      await legacy.handle.decisionsDecide(DECISION_ID, {
        approve: false, presentedDigest: DIGEST, reason: "reviewed",
        attestation: { ...attestation(wire), presentation: wire },
      });
      assert.deepEqual(legacy.calls, calls);
    });
  }
}

const malformed: [string, unknown][] = [
  ["audience", {}], ["audience", { kind: "approver" }],
  ["audience", { kind: "approver", approverId: 7 }],
  ["audience", { kind: "approver", approverId: "a", reviewer: "r" }],
  ["audience", { kind: "reviewer" }],
  ["audience", { kind: "reviewer", reviewer: false }],
  ["audience", { kind: "reviewer", reviewer: "r", approverId: "a" }],
  ["audience", { kind: "future", reviewer: "r" }], ["audience", null],
  ["partsShown", [{ index: 0, count: 1 }]],
  ["partsShown", [{ index: 0, count: 1, partDigest: 7 }]],
  ["partsShown", [{ index: 0, count: 1, partDigest: "d", extra: 1 }]],
  ["partsShown", "parts"], ["partsShown", [null]],
  ["link", { reference: "r" }], ["link", { servedDigest: "d" }],
  ["link", { reference: "r", servedDigest: [] }],
  ["link", { reference: "r", servedDigest: "d", extra: 1 }],
  ["subjectDigest", 4], ["channelRef", null], ["messageRef", []],
  ["atMs", true], ["atMs", -1], ["atMs", 1.5], ["atMs", Number.MAX_SAFE_INTEGER + 1],
  ["atMs", NaN], ["atMs", Infinity],
  ["maxPartBytes", true], ["maxPartBytes", -1], ["maxPartBytes", 2 ** 32],
  ["extra", "closed request"],
];
for (const [index, [field, value]] of malformed.entries()) {
  it(`typed nested structure rejects malformed case ${index}: ${field}`, () => {
    const Presentation = presentationType();
    assert.throws(() => new Presentation({ ...input(), [field]: value }), TypeError);
  });
}

for (const field of ["audience", "subjectDigest", "partsShown", "channelRef", "messageRef", "atMs"]) {
  it(`typed presentation requires ${field}`, () => {
    const Presentation = presentationType();
    const missing = input();
    delete missing[field];
    assert.throws(() => new Presentation(missing), TypeError);
  });
}

for (const field of ["index", "count"]) {
  for (const value of [true, -1, 1.5, 2 ** 32]) {
    it(`canonical part ${field} rejects ${String(value)}`, () => {
      const Presentation = presentationType();
      assert.throws(() => new Presentation({
        ...input(), partsShown: [{ index: 0, count: 1, partDigest: DIGEST, [field]: value }],
      }), TypeError);
    });
  }
}

it("structural typing does not invent gateway semantic validation", () => {
  const Presentation = presentationType();
  const wire = new Presentation({
    ...input(), subjectDigest: "gateway-validates-digests", atMs: Number.MAX_SAFE_INTEGER,
    maxPartBytes: 2 ** 32 - 1,
    partsShown: [{ index: 2 ** 32 - 1, count: 0, partDigest: "d" }],
  }).toDict();
  assert.equal(wire.at_ms, Number.MAX_SAFE_INTEGER);
  assert.equal(wire.max_part_bytes, 2 ** 32 - 1);
  assert.deepEqual(wire.parts_shown, [{ index: 2 ** 32 - 1, count: 0, part_digest: "d" }]);
  assert.equal("link" in wire, false);
});

it("omitted and null options serialize identically without mutation", () => {
  const Presentation = presentationType();
  const original = input();
  const value = new Presentation(original);
  const nulls = new Presentation({ ...original, maxPartBytes: null, link: null });
  const first = value.toDict();
  (first.parts_shown as any[])[0].part_digest = "changed-output";
  assert.deepEqual(value.toDict(), expectedWire());
  assert.deepEqual(nulls.toDict(), expectedWire());
  assert.deepEqual(original, input());
});
