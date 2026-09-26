import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient, parseCoreEvent, parseEvent } from "../dist/index.js";

const identity = { interaction_id: "interaction-1", run_id: "run-1" };
const skillKey = {
  source_uuid: "00000000-0000-4000-8000-000000000001",
  skill_name: "test-skill",
};
const contentBlocks = [
  { type: "text", text: " exact text\n" },
  { type: "image", media_type: "image/png", source: "inline", data: "aGVsbG8=" },
  { type: "image", media_type: "image/png", source: "blob", blob_id: "blob-1" },
  { type: "video", media_type: "video/mp4", duration_ms: 12, source: "inline", data: "aGVsbG8=" },
  { type: "video", media_type: "video/mp4", duration_ms: 12, source: "uri", uri: "https://example.test/video.mp4" },
  { type: "structured", data: { ready: true } },
  { type: "structured", data: null },
  { type: "skill_context", skill_key: skillKey, text: "skill body" },
];

function parseEnvelope(raw) {
  return MeerkatClient.parseAgentEventEnvelope({
    event_id: "00000000-0000-4000-8000-000000000010",
    source: { type: "callback" },
    seq: 0,
    timestamp_ms: 1,
    payload: raw,
  }).payload;
}

describe("run_started runtime input", () => {
  for (const input of [
    { kind: "content", content: " exact prompt\n" },
    { kind: "content", content: "" },
    { kind: "content", content: [] },
    ...contentBlocks.map((block) => ({ kind: "content", content: [block] })),
    { kind: "content", content: contentBlocks },
    { kind: "pending_tool_results" },
  ]) {
    it(`preserves the wire input ${JSON.stringify(input)}`, () => {
      const raw = {
        type: "run_started",
        session_id: "session-1",
        input: structuredClone(input),
        identity: structuredClone(identity),
      };
      for (const parse of [parseCoreEvent, parseEvent, parseEnvelope]) {
        const event = parse(raw);
        assert.equal(event.type, "run_started");
        assert.equal(event.sessionId, raw.session_id);
        assert.deepEqual(event.input, input);
        assert.deepEqual(event.identity, identity);
        assert.equal(Object.hasOwn(event, "prompt"), false);
      }
    });
  }

  it("distinguishes an empty content input from a pending-tool continuation", () => {
    const empty = parseEvent({
      type: "run_started", session_id: "s", input: { kind: "content", content: "" },
    });
    const pending = parseEvent({
      type: "run_started", session_id: "s", input: { kind: "pending_tool_results" },
    });
    assert.equal(empty.input.kind, "content");
    assert.equal(pending.input.kind, "pending_tool_results");
    assert.equal(Object.hasOwn(pending.input, "content"), false);
  });

  const invalidInputs = [
    {},
    { prompt: "obsolete prompt-only frame" },
    { input: null },
    { input: "hello" },
    { input: [] },
    { input: {} },
    { input: { kind: "unknown" } },
    { input: { kind: "content" } },
    { input: { kind: "content", content: null } },
    { input: { kind: "content", content: 7 } },
    { input: { kind: "content", content: false } },
    { input: { kind: "content", content: ["invalid block"] } },
    ...[
      {},
      null,
      [],
      { type: "unknown" },
      { type: "text" },
      { type: "text", text: 7 },
      { type: "image", media_type: "image/png", source: "inline" },
      { type: "image", media_type: null, source: "inline", data: "aGVsbG8=" },
      { type: "image", media_type: "image/png", source: "blob", blob_id: 7 },
      { type: "image", media_type: "image/png", source: "uri", uri: "https://example.test/image.png" },
      { type: "video", media_type: "video/mp4", duration_ms: -1, source: "inline", data: "aGVsbG8=" },
      { type: "video", media_type: "video/mp4", duration_ms: 1.5, source: "inline", data: "aGVsbG8=" },
      { type: "video", media_type: "video/mp4", duration_ms: true, source: "uri", uri: "https://example.test/video.mp4" },
      { type: "video", media_type: "video/mp4", duration_ms: 2 ** 64, source: "inline", data: "aGVsbG8=" },
      { type: "video", media_type: "video/mp4", source: "inline", data: "aGVsbG8=" },
      { type: "video", media_type: "video/mp4", duration_ms: 1, source: "uri", uri: null },
      { type: "video", media_type: "video/mp4", duration_ms: 1, source: "blob", blob_id: "blob-1" },
      { type: "structured" },
      { type: "skill_context", skill_key: {}, text: "skill body" },
      { type: "skill_context", skill_key: { sourceUuid: skillKey.source_uuid, skillName: skillKey.skill_name }, text: "skill body" },
      { type: "skill_context", skill_key: { ...skillKey, source_uuid: "invalid" }, text: "skill body" },
      { type: "skill_context", skill_key: { ...skillKey, skill_name: "Invalid--skill" }, text: "skill body" },
      { type: "skill_context", skill_key: skillKey, text: null },
    ].map((block) => ({ input: { kind: "content", content: [block] } })),
  ];
  for (const fields of invalidInputs) {
    it(`preserves malformed input explicitly ${JSON.stringify(fields)}`, () => {
      const raw = { type: "run_started", session_id: "session-1", ...fields };
      for (const parse of [parseCoreEvent, parseEvent, parseEnvelope]) {
        const event = parse(raw);
        assert.equal(event.type, "malformed_event");
        assert.equal(event.rawType, "run_started");
        assert.deepEqual(event.raw, raw);
        assert.ok(event.error.length > 0);
      }
    });
  }

  for (const session of [{}, { session_id: null }, { session_id: 7 }, { session_id: [] }]) {
    it(`requires a string session_id ${JSON.stringify(session)}`, () => {
      const raw = { type: "run_started", input: { kind: "pending_tool_results" }, ...session };
      const event = parseEvent(raw);
      assert.equal(event.type, "malformed_event");
      assert.deepEqual(event.raw, raw);
    });
  }
});
