import assert from "node:assert/strict";
import { test } from "node:test";
import { parseWorkGraphItem, type WorkGraphItem } from "../dist/index.js";

// A consumer-created item using every pre-finish-by required property. This
// must compile without a cast, assertion, or a fabricated finishBy value.
const priorShape: WorkGraphItem = {
  id: "work-prior-consumer",
  realmId: "consumer-realm",
  namespace: "default",
  title: "An item written by a pre-finish-by consumer",
  description: null,
  status: "open",
  completionPolicy: { type: "manual" },
  priority: "normal",
  labels: [],
  owner: null,
  claim: null,
  machineState: {},
  revision: 1,
  dueAt: null,
  notBefore: null,
  snoozedUntil: null,
  createdAt: "2026-11-18T00:00:00Z",
  updatedAt: "2026-11-18T00:00:00Z",
  terminalAt: null,
  externalRefs: [],
  evidenceRefs: [],
};

const timestamp = "2026-11-19T12:30:00.123456789Z";
const explicitTarget: WorkGraphItem = { ...priorShape, finishBy: timestamp };
const explicitClear: WorkGraphItem = { ...priorShape, finishBy: null };

test("consumer-created WorkGraphItem may omit finishBy", () => {
  assert.equal(Object.hasOwn(priorShape, "finishBy"), false);
  assert.equal(priorShape.finishBy, undefined);
  assert.equal(explicitTarget.finishBy, timestamp);
  assert.equal(explicitClear.finishBy, null);
});

test("WorkGraphItem decoding still supplies the gateway finish-by value", () => {
  assert.equal(parseWorkGraphItem({ finish_by: timestamp }).finishBy, timestamp);
  assert.equal(parseWorkGraphItem({ finish_by: null }).finishBy, null);
  const older = parseWorkGraphItem({});
  assert.equal(Object.hasOwn(older, "finishBy"), true);
  assert.equal(older.finishBy, null);
});
