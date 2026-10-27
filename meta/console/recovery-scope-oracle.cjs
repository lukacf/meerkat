"use strict";
const assert = require("node:assert/strict");

function assertScopeCancellation(value) {
  assert(["seed", "repair"].includes(value.phase));
  assert(["stock", "shared"].includes(value.host));
  assert.notEqual(value.oldScope, value.replacementScope);
  assert(Number.isFinite(value.switchedAt), "scope switch has a recorded time");
  assert(value.held.length > 0, "an actual successful old history response was held");
  const oldCursors = new Set();
  for (const held of value.held) {
    assert.equal(held.scope, value.oldScope);
    assert.equal(held.status, 200, "hold real successful owner history, not a fabricated fault");
    assert([held.heldAt, held.abortedAt, held.releasedAt].every(Number.isFinite), "held response has recorded hold, cancellation and release times");
    assert(held.heldAt <= value.switchedAt);
    assert(held.abortedAt >= value.switchedAt, "the scope switch cancels the old socket");
    assert(held.releasedAt >= held.abortedAt, "release happens after observed cancellation");
    const page = JSON.parse(held.response).result;
    assert(page?.frames?.some(frame => JSON.stringify(frame.payload).includes(value.oldMarker)), "held bytes contain actual old owner history");
    if (page.latest_cursor) oldCursors.add(page.latest_cursor);
  }
  assert(value.replacementFrames.some(frame => JSON.stringify(frame.payload).includes(value.replacementMarker)));
  assert(!value.replacementFrames.some(frame => JSON.stringify(frame.payload).includes(value.oldMarker)));
  assert(value.transcript.includes(value.replacementMarker), "replacement authorized history is visible");
  assert(!value.transcript.includes(value.oldMarker), "old history cannot enter the replacement");
  assert.equal(new Set(value.rows).size, value.rows.length, "replacement rows are not duplicated");
  assert.equal(value.draft, "", "old draft is cleared on authority change");
  assert.equal(value.contextCount, 0, "old quote snapshots are not visible in replacement scope");
  assert.equal(value.phaseAfter, "live");
  assert.deepEqual(value.errors, []);
  assert.deepEqual(value.modelRequestCounts.after, value.modelRequestCounts.before, "read-only scope switch does not dispatch model work");
  const streams = value.observations.filter(item => item.path.startsWith("/console/timeline/stream"));
  assert(streams.every(item => Number.isFinite(item.startedAt)), "every observed stream has a recorded start time");
  const replacement = streams.filter(item => item.scope === value.replacementScope);
  assert(replacement.some(item => item.status === 200), "new authority establishes an actual stream");
  for (const stream of replacement) {
    const target = new URL(stream.path, "http://scope-proxy.invalid");
    assert.equal(stream.method, "GET", "replacement opens the actual timeline stream");
    assert.equal(target.pathname, "/console/timeline/stream");
    assert.deepEqual(target.searchParams.getAll("identity"), value.host === "shared" ? ["router:main"] : [], "replacement stream uses the host's exact identity scope");
    assert(!target.searchParams.has("conversation_id"), "replacement stream cannot silently narrow its conversation scope");
    assert(!oldCursors.has(stream.cursor), "old accepted cursor cannot transfer to replacement stream");
    // The owner can project canonical history after the prebrowser query.
    // Bind each stream to the completed seed the browser actually received.
    const seeds = value.observations.filter(item => {
      if (item.scope !== value.replacementScope || item.path !== "/console/rpc" || item.method !== "POST" || item.status !== 200
        || !Number.isFinite(item.startedAt) || !Number.isFinite(item.endedAt)
        || item.startedAt < value.switchedAt || item.endedAt < item.startedAt || item.endedAt > stream.startedAt
        || (item.abortedAt !== undefined && item.abortedAt <= stream.startedAt)) return false;
      let request;
      try { request = JSON.parse(item.request); } catch { return false; }
      const params = request.params;
      return request.method === "meta/console/query_timeline" && params?.mode === "recent" && params.limit === 200
        && params.before === undefined && params.after === undefined
        && (value.host === "shared" ? params.identity === "router:main" : params.identity === undefined);
    }).sort((left, right) => right.endedAt - left.endedAt);
    assert(seeds.length, "replacement stream follows a completed successful seed for its own scope and identity");
    const snapshot = JSON.parse(seeds[0].response).result;
    assert(snapshot && snapshot.available !== false && Array.isArray(snapshot.frames), "replacement seed contains available owner history");
    assert(snapshot.frames.some(frame => JSON.stringify(frame.payload).includes(value.replacementMarker)), "replacement seed bytes contain its own owner marker");
    assert(!snapshot.frames.some(frame => JSON.stringify(frame.payload).includes(value.oldMarker)), "replacement seed bytes exclude old owner history");
    assert(typeof snapshot.latest_cursor === "string" && snapshot.latest_cursor.length > 0, "replacement seed supplies its exact owner frontier");
    assert.equal(stream.cursor, snapshot.latest_cursor, "replacement stream resumes at its actual completed owner seed frontier");
  }
  assert(!streams.some(item => item.scope === value.oldScope && item.startedAt > value.switchedAt), "disposed authority does not open another stream");
  if (value.phase === "repair") {
    assert(streams.some(item => item.scope === value.oldScope && item.status === 409 && item.startedAt < value.switchedAt), "repair follows a genuine expired replay response");
  }
  for (const item of value.observations) {
    let body;
    try { body = JSON.parse(item.request || "{}"); } catch { continue; }
    assert(body.method !== "meta/console/send" && !item.path.endsWith("/send"), "scope switch cannot automatically send");
  }
}

module.exports = { assertScopeCancellation };
