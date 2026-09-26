import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { parseEvent, MeerkatClient } from "../dist/index.js";
const identity = {
  interaction_id: "interaction-1",
  run_id: "run-1",
  objective_id: "objective-1",
  realtime_origin: {
    canonical_row_sequence: 7,
    channel_id: "channel-1",
    session_id: "realtime-session",
    provider_item_ids: ["item-1", "item-2"],
    context_observation_id: {
      channel_id: "channel-1",
      namespace: "test",
      nonce: "nonce-1",
    },
  },
};
const events = [
  { type: "run_started", session_id: "session-1", prompt: "hi" },
  {
    type: "run_completed",
    session_id: "session-1",
    result: "done",
    usage: { input_tokens: 1, output_tokens: 1 },
  },
  {
    type: "run_failed",
    session_id: "session-1",
    error_class: "internal",
    error: "failure",
  },
];
describe("owner lifecycle identity", () => {
  for (const event of events) {
    it(`${event.type} keeps old identity-free events valid`, () => {
      const parsed = parseEvent(event);
      assert.equal(parsed.type, event.type);
      assert.equal(Object.hasOwn(parsed, "identity"), false);
    });
    for (const value of [
      {},
      identity,
      {
        interaction_id: null,
        run_id: null,
        objective_id: null,
        realtime_origin: null,
      },
      {
        realtime_origin: {
          canonical_row_sequence: 0,
          channel_id: "c",
          session_id: "s",
          context_observation_id: null,
        },
      },
    ]) {
      it(`${event.type} preserves ${JSON.stringify(value)}`, () => {
        const raw = { ...event, identity: structuredClone(value) },
          parsed = parseEvent(raw);
        assert.equal(parsed.type, event.type);
        assert.deepEqual(parsed.identity, value);
        const envelope = MeerkatClient.parseAgentEventEnvelope({
          event_id: "00000000-0000-4000-8000-000000000010",
          source: { type: "callback" },
          seq: 0,
          timestamp_ms: 1,
          payload: raw,
        });
        assert.deepEqual(envelope.payload.identity, value);
      });
    }
    const invalid = [
      null,
      "guessed",
      [],
      { interaction_id: 7 },
      { run_id: false },
      { objective_id: [] },
      { realtime_origin: {} },
      { realtime_origin: "channel" },
    ];
    for (const [field, value] of [
      ["canonical_row_sequence", -1],
      ["canonical_row_sequence", 1.5],
      ["canonical_row_sequence", true],
      ["channel_id", null],
      ["session_id", 7],
      ["provider_item_ids", null],
      ["provider_item_ids", [7]],
      ["context_observation_id", {}],
      ["context_observation_id", { channel_id: "c", namespace: "n", nonce: 3 }],
    ])
      invalid.push({
        ...identity,
        realtime_origin: { ...identity.realtime_origin, [field]: value },
      });
    for (const value of invalid)
      it(`${event.type} rejects malformed identity ${JSON.stringify(value)}`, () => {
        const raw = { ...event, identity: value },
          parsed = parseEvent(raw);
        assert.equal(parsed.type, "malformed_event");
        assert.deepEqual(parsed.raw, raw);
      });
  }
});
