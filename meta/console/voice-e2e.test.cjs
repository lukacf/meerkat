"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { verifyVoiceTranscriptHandoff } = require("./voice-e2e.cjs");

test("voice handoff publishes the canonical nested TranscriptMessageIdentity carrier", async () => {
  // Stop at the first publish boundary. This exercises the browser scenario's
  // real fixture construction without launching a browser or asserting UI work.
  const captured = new Error("captured canonical rows");
  let rows;
  const page = {
    waitForFunction: async () => {},
    evaluate: async () => {},
    locator: () => ({
      waitFor: async () => {},
      count: async () => 0,
      locator: () => ({ textContent: async () => "The first provisional fragment" }),
    }),
    getByTestId: () => ({ fill: async () => {}, evaluate: async () => {} }),
  };
  await assert.rejects(verifyVoiceTranscriptHandoff(page, {
    channels: new Map([["active-channel", { identity: "identity:alpha", closed: false }]]),
    caption: () => {},
    setTimeline: async frames => { rows = frames; throw captured; },
  }), error => error === captured);
  assert.equal(rows.length, 3);
  const spoken = rows[2];
  assert.equal(spoken.source.kind, "session_history");
  assert.equal(spoken.identity, "identity:alpha");
  assert.equal(spoken.session_id, "session-alpha");
  assert.equal(spoken.payload.message.role, "block_assistant");
  assert.deepEqual(spoken.payload.message.identity, { realtime_origin: {
    session_id: "session-alpha", channel_id: "active-channel", canonical_row_sequence: 3,
    provider_item_ids: ["spoken-first", "spoken-second"],
  } });
  assert.equal(Object.hasOwn(spoken.payload.message, "realtime_origin"), false,
    "the proof must not fall back to the legacy flat compatibility carrier");
  assert.equal(Object.hasOwn(spoken.payload, "realtime_origin"), false);
  const source = "## Spoken review\n\nKeep A\u030a, å and 🚀 exactly.\n\n| Check | Result |\n| --- | --- |\n| WorkGraph | Ready |";
  for (const field of ["text", "result", "content"]) assert.equal(spoken.payload[field], source);
  assert.deepEqual(spoken.payload.message.blocks, [{ block_type: "transcript", data: { text: source, source: "spoken" } }]);
  for (const row of rows.slice(0, 2)) {
    assert.equal(row.payload.message.identity?.realtime_origin, undefined,
      "ordinary text rows cannot authorize a live speech join");
  }
});
