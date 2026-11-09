import test from "node:test";
import assert from "node:assert/strict";
import { startingAudiences, startingKinds, startingContextDetails, startingContextText } from "./forge-starting-context.mjs";

test("starting context resolves the chosen audience and kind without changing the hint", () => {
  const hint = Object.freeze({ version: 2, audience_id: "household", kind_id: "repetitive" });
  const details = startingContextDetails(hint);
  assert.equal(details.audience, startingAudiences.find(option => option.id === "household"));
  assert.equal(details.kind, startingKinds.find(option => option.id === "repetitive"));
  assert.equal(details.kind.prompt, "What's one task you keep doing by hand?");
  assert.equal(startingContextText(hint), "For my household, I would like help with repetitive work.");
  assert.deepEqual(hint, { version: 2, audience_id: "household", kind_id: "repetitive" });
});

test("starting context accepts all known combinations, including exploratory choices", () => {
  assert.equal(new Set(startingAudiences.map(option => option.id)).size, startingAudiences.length);
  assert.equal(new Set(startingKinds.map(option => option.id)).size, startingKinds.length);
  for (const audience of startingAudiences) for (const kind of startingKinds) {
    const hint = { version: 2, audience_id: audience.id, kind_id: kind.id };
    assert.deepEqual(startingContextDetails(hint), { audience, kind });
    assert.equal(startingContextText(hint), audience.context + kind.context);
  }
  assert.equal(startingContextText({ version: 2, audience_id: "exploring", kind_id: "exploring" }),
    "I am still exploring who this could help. I am still exploring what kind of help would be useful.");
});

test("starting context refuses malformed, unknown and legacy metadata", () => {
  const valid = { version: 2, audience_id: "team", kind_id: "organizing" };
  for (const hint of [null, undefined, [], "team", {},
    { ...valid, version: "2" }, { ...valid, version: 1 },
    { ...valid, audience_id: "unknown" }, { ...valid, kind_id: "unknown" },
    { ...valid, audience_id: 2 }, { ...valid, kind_id: 2 },
    Object.assign(Object.create({ version: 2 }), { audience_id: "team", kind_id: "organizing", extra: true }),
    { ...valid, inserted: "Stock prose" }, { audience_id: "team", kind_id: "organizing", prefix: "", inserted: "Stock prose" }]) {
    assert.equal(startingContextDetails(hint), null);
    assert.equal(startingContextText(hint), "");
  }
});
