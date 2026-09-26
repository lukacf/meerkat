import assert from "node:assert/strict";
import test from "node:test";
import { MeerkatClient, parseEvent } from "../dist/index.js";

const sessionId = "00000000-0000-4000-8000-000000000001";
const runId = "00000000-0000-4000-8000-000000000002";
const inputId = "00000000-0000-4000-8000-000000000003";
const notice = {
  kind: "background_job",
  body: "Image generation finished.",
  blocks: [{
    type: "background_job",
    job_id: "job-image-1",
    status: "completed",
    persisted: true,
  }],
  created_at: "2026-09-26T00:00:00Z",
  runtime_origin: {
    session_id: sessionId,
    run_id: runId,
    input_id: inputId,
    append_ordinal: 1,
  },
};
const events = [
  {
    type: "boundary_append_applied",
    run_id: runId,
    input_id: inputId,
    content: [{ type: "text", text: "Image generation finished." }],
    append_count: 2,
    notices: [notice],
    transcript_start: 7,
  },
  {
    type: "boundary_append_applied",
    run_id: runId,
    input_id: inputId,
    content: "Legacy append without notice metadata.",
    append_count: 1,
  },
  {
    type: "boundary_appends_discarded",
    session_id: sessionId,
    run_id: runId,
    input_ids: [inputId, "00000000-0000-4000-8000-000000000004"],
  },
];

for (const raw of events) {
  const caseName = raw.type === "boundary_appends_discarded"
    ? "discarded exact inputs"
    : raw.notices ? "applied notice provenance" : "applied legacy fields";
  test(`boundary append ${caseName} preserves runtime fields through parsing`, () => {
    assert.deepEqual(parseEvent(structuredClone(raw)), raw);
  });

  test(`boundary append ${caseName} survives envelope and scoped decoding`, () => {
    const envelope = MeerkatClient.parseAgentEventEnvelope({
      event_id: "00000000-0000-4000-8000-000000000010",
      source: { type: "session", session_id: sessionId },
      seq: 7,
      timestamp_ms: 1,
      payload: structuredClone(raw),
    });
    assert.deepEqual(envelope.payload, raw);

    const scoped = parseEvent({
      scope_id: "primary",
      scope_path: [{ scope: "primary", session_id: sessionId }],
      event: structuredClone(raw),
    });
    assert.deepEqual(scoped.event, raw);
  });
}
