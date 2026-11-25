"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { assertScopeCancellation, heldReadsOfLastStart } = require("./recovery-scope-oracle.cjs");

function validEvidence() {
  return {
    phase: "repair", host: "shared", oldScope: "scope-a", replacementScope: "scope-b", switchedAt: 30,
    oldMarker: "Private old owner record", replacementMarker: "Replacement owner record",
    held: [{ scope: "scope-a", status: 200, heldAt: 20, abortedAt: 31, releasedAt: 40,
      response: JSON.stringify({ result: { frames: [{ id: "old:frame", payload: { text: "Private old owner record" } }], latest_cursor: "old:cursor" } }) }],
    observations: [
      { scope: "scope-a", path: "/console/timeline/stream", status: 409, startedAt: 15 },
      { scope: "scope-b", path: "/console/rpc", method: "POST", startedAt: 32, endedAt: 33, status: 200,
        request: JSON.stringify({ method: "meta/console/query_timeline", params: { mode: "recent", limit: 200, identity: "router:main" } }),
        response: JSON.stringify({ result: { frames: [{ id: "new:frame", payload: { text: "Replacement owner record" } }], latest_cursor: "new:cursor" } }) },
      { scope: "scope-b", path: "/console/timeline/stream?identity=router%3Amain", method: "GET", startedAt: 34, status: 200, cursor: "new:cursor" },
    ],
    replacementFrames: [{ id: "new:frame", payload: { text: "Replacement owner record" } }], replacementCursor: "new:cursor",
    transcript: "Replacement owner record", rows: ["new:frame"], draft: "", contextCount: 0,
    phaseAfter: "live", errors: [], modelRequestCounts: { before: [2, 2], after: [2, 2] },
  };
}

test("accepts actual old-socket cancellation with isolated replacement history", () => {
  assert.doesNotThrow(() => assertScopeCancellation(validEvidence()));
});

for (const [name, corrupt] of [
  ["uncancelled held query", value => { delete value.held[0].abortedAt; }],
  ["query cancelled before the scope switch", value => { value.held[0].abortedAt = 29; }],
  ["response released before cancellation", value => { value.held[0].releasedAt = 30; }],
  ["fabricated or failed history response", value => { value.held[0].status = 503; }],
  ["old transcript injected after replacement", value => { value.transcript += value.oldMarker; }],
  ["replacement history absent", value => { value.transcript = "Loading"; }],
  ["old cursor transferred to new stream", value => { value.observations[2].cursor = "old:cursor"; }],
  ["replacement stream skips its owner frontier", value => { value.observations[2].cursor = "invented:cursor"; }],
  ["old scope opens another subscription", value => { value.observations.push({ scope: "scope-a", path: "/console/timeline/stream", startedAt: 41 }); }],
  ["scope switch submits a message", value => { value.observations.push({ scope: "scope-b", path: "/console/rpc", method: "POST", startedAt: 35, request: JSON.stringify({ method: "meta/console/send" }) }); }],
  ["owner receives an unintended model request", value => { value.modelRequestCounts.after[1]++; }],
  ["old draft remains", value => { value.draft = "old draft"; }],
  ["old quote remains", value => { value.contextCount = 1; }],
  ["hidden replacement error", value => { value.errors = ["ERR_FAILED"]; }],
  ["replacement did not resume", value => { value.phaseAfter = "retrying"; }],
  ["repair was not caused by real expired replay", value => { value.observations[0].status = 200; }],
  ["duplicate replacement rows", value => { value.rows.push(value.rows[0]); }],
]) {
  test(`rejects ${name}`, () => {
    const evidence = validEvidence(); corrupt(evidence);
    assert.throws(() => assertScopeCancellation(evidence));
  });
}

test("initial seed needs no fabricated replay gap", () => {
  const evidence = validEvidence(); evidence.phase = "seed";
  evidence.observations = evidence.observations.slice(1);
  assert.doesNotThrow(() => assertScopeCancellation(evidence));
});

test("uses the browser's completed owner snapshot when history advances after the prebrowser query", () => {
  const evidence = validEvidence();
  evidence.replacementCursor = "new:earlier-frontier";
  assert.doesNotThrow(() => assertScopeCancellation(evidence));
});

test("binds each stock stream to its own preceding global seed while ignoring per-pane history", () => {
  const evidence = validEvidence();
  evidence.host = "stock";
  evidence.observations[2].path = "/console/timeline/stream";
  const seed = evidence.observations[1];
  seed.request = JSON.stringify({ method: "meta/console/query_timeline", params: { mode: "recent", limit: 200 } });
  evidence.observations.push({ ...seed, startedAt: 35, endedAt: 36,
    response: seed.response.replace("new:cursor", "new:advanced-cursor") });
  evidence.observations.push({ ...seed, startedAt: 36, endedAt: 37,
    request: JSON.stringify({ method: "meta/console/query_timeline", params: { mode: "recent", limit: 200, identity: "router:main" } }),
    response: seed.response.replace("new:cursor", "new:unrelated-pane-cursor") });
  evidence.observations.push({ ...evidence.observations[2], startedAt: 38, cursor: "new:advanced-cursor" });
  assert.doesNotThrow(() => assertScopeCancellation(evidence));
});

for (const [name, corrupt] of [
  ["wrong replacement stream identity", value => { value.observations[2].path = "/console/timeline/stream?identity=domain%3Adelivery"; }],
  ["shared replacement stream lacks identity", value => { value.observations[2].path = "/console/timeline/stream"; }],
  ["replacement stream repeats its identity", value => { value.observations[2].path += "&identity=router%3Amain"; }],
  ["replacement stream narrows conversation", value => { value.observations[2].path += "&conversation_id=other"; }],
  ["replacement uses another stream path", value => { value.observations[2].path = "/console/timeline/stream/other?identity=router%3Amain"; }],
  ["replacement uses another HTTP method", value => { value.observations[2].method = "POST"; }],
  ["identity-filtered stock replacement stream", value => { value.host = "stock"; const body = JSON.parse(value.observations[1].request); delete body.params.identity; value.observations[1].request = JSON.stringify(body); }],
  ["nonfinite scope switch time", value => { value.switchedAt = NaN; }],
  ["nonfinite held release time", value => { value.held[0].releasedAt = Infinity; }],
  ["missing old-scope stream start", value => { delete value.observations[0].startedAt; }],
  ["missing stream start", value => { delete value.observations[2].startedAt; }],
  ["nonfinite stream start", value => { value.observations[2].startedAt = Infinity; }],
  ["missing browser seed", value => { value.observations.splice(1, 1); }],
  ["incomplete browser seed", value => { delete value.observations[1].endedAt; }],
  ["seed ends after the stream opens", value => { value.observations[1].endedAt = 35; }],
  ["seed belongs to old scope", value => { value.observations[1].scope = "scope-a"; }],
  ["seed belongs to another identity", value => { value.observations[1].request = value.observations[1].request.replace("router:main", "domain:delivery"); }],
  ["shared seed is not identity scoped", value => { const body = JSON.parse(value.observations[1].request); delete body.params.identity; value.observations[1].request = JSON.stringify(body); }],
  ["seed uses another page size", value => { value.observations[1].request = value.observations[1].request.replace('"limit":200', '"limit":1000'); }],
  ["seed is an older page", value => { const body = JSON.parse(value.observations[1].request); body.params.before = "new:older"; value.observations[1].request = JSON.stringify(body); }],
  ["seed is a replay page", value => { const body = JSON.parse(value.observations[1].request); body.params.after = "new:after"; value.observations[1].request = JSON.stringify(body); }],
  ["seed is unavailable", value => { const body = JSON.parse(value.observations[1].response); body.result.available = false; value.observations[1].response = JSON.stringify(body); }],
  ["seed has wrong owner marker", value => { value.observations[1].response = value.observations[1].response.replace(value.replacementMarker, "Unrelated history"); }],
  ["seed contains old owner bytes", value => { const body = JSON.parse(value.observations[1].response); body.result.frames.push({ payload: { text: value.oldMarker } }); value.observations[1].response = JSON.stringify(body); }],
  ["seed frontier differs from stream", value => { value.observations[1].response = value.observations[1].response.replace("new:cursor", "new:actual-frontier"); }],
  ["stream reuses an older completed seed frontier", value => { const seed = value.observations[1]; value.observations.splice(2, 0, { ...seed, startedAt: 33, endedAt: 33.5, response: seed.response.replace("new:cursor", "new:advanced-frontier") }); }],
]) {
  test(`rejects ${name} as replacement cursor authority`, () => {
    const evidence = validEvidence(); corrupt(evidence);
    assert.throws(() => assertScopeCancellation(evidence));
  });
}

// The held history reads that a scope switch must cancel are those of the
// console's last start. `at` is a time on the proxy's clock.
function startEvidence({ features, everyMobRead = null, reads }) {
  const capabilities = features === undefined ? [] : [{
    scope: "scope-a", path: "/console/rpc", method: "POST", status: 200, startedAt: 1, endedAt: 4,
    request: JSON.stringify({ method: "meta/capabilities", params: {} }),
    response: JSON.stringify({ result: features === null ? {} : { feature_capabilities: features } }),
  }];
  const experience = everyMobRead ? [{ scope: "scope-a", path: "/console/experience?mobs=*", method: "GET", status: 200, ...everyMobRead }] : [];
  const held = reads.map(([startedAt, abortedAt]) => ({
    scope: "scope-a", path: "/console/rpc", method: "POST", status: 200, startedAt, heldAt: startedAt + 1,
    ...(abortedAt === undefined ? {} : { abortedAt }),
  }));
  return { observations: [...capabilities, ...experience, ...held], held, scope: "scope-a" };
}

test("on a gateway that lists mob addressing only a live read held after the every-mob experience read is of the last start", () => {
  // Before the capabilities answer, and before the every-mob experience read ends: nothing yet.
  assert.deepEqual(heldReadsOfLastStart(startEvidence({ reads: [[2]] })), []);
  assert.deepEqual(heldReadsOfLastStart(startEvidence({ features: ["mob_addressing"], reads: [[2, 6], [7]] })), []);
  assert.deepEqual(heldReadsOfLastStart(startEvidence({ features: ["mob_addressing"], everyMobRead: { startedAt: 5 }, reads: [[2, 6], [7]] })), []);
  // An every-mob read made before the capabilities answer is not the one the last start follows.
  assert.deepEqual(heldReadsOfLastStart(startEvidence({ features: ["mob_addressing"], everyMobRead: { startedAt: 3, endedAt: 6 }, reads: [[7]] })), []);
  // The read of the second start is still live when the every-mob read ends: the console cancels it itself.
  assert.deepEqual(heldReadsOfLastStart(startEvidence({ features: ["mob_addressing"], everyMobRead: { startedAt: 5, endedAt: 9 }, reads: [[2, 6], [7]] })), []);
  const settled = startEvidence({ features: ["mob_addressing"], everyMobRead: { startedAt: 5, endedAt: 9 }, reads: [[2, 6], [7, 10], [11]] });
  assert.deepEqual(heldReadsOfLastStart(settled), [settled.held[2]]);
});

test("on a gateway that hosts one mob every live held read is of the last start", () => {
  for (const features of [null, [], ["other"]]) {
    const evidence = startEvidence({ features, reads: [[2, 6], [7]] });
    assert.deepEqual(heldReadsOfLastStart(evidence), [evidence.held[1]]);
  }
  const otherScope = startEvidence({ features: [], reads: [[7]] });
  otherScope.held[0].scope = "scope-b";
  assert.deepEqual(heldReadsOfLastStart(otherScope), []);
});

test("a held read that a newer unscoped read of the scope followed is of a start the console replaces", () => {
  const read = (startedAt, extra = {}) => ({
    scope: "scope-a", path: "/console/rpc", method: "POST", status: 200, startedAt, heldAt: startedAt + 1,
    request: JSON.stringify({ method: "meta/console/query_timeline", params: { mode: "recent", limit: 200, mobs: "*" } }), ...extra,
  });
  const evidence = startEvidence({ features: ["mob_addressing"], everyMobRead: { startedAt: 5, endedAt: 6 }, reads: [] });
  // Both began after the every-mob read ended; the proxy has not yet seen the first one cancelled.
  const second = read(7), third = read(11);
  const memberRead = read(12, { request: JSON.stringify({ method: "meta/console/query_timeline", params: { mode: "recent", limit: 200, identity: "router:main" } }) });
  const held = [second, third];
  assert.deepEqual(heldReadsOfLastStart({ observations: [...evidence.observations, second, third, memberRead], held, scope: "scope-a" }), [third]);
  assert.deepEqual(heldReadsOfLastStart({ observations: [...evidence.observations, second], held: [second], scope: "scope-a" }), [second]);
});
