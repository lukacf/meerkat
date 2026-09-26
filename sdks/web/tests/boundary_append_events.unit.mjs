import assert from 'node:assert/strict';
import test from 'node:test';
import { Mob } from '../dist/mob.js';
import { Session } from '../dist/session.js';

const sessionId = '00000000-0000-4000-8000-000000000001';
const runId = '00000000-0000-4000-8000-000000000002';
const inputId = '00000000-0000-4000-8000-000000000003';
const notice = {
  kind: 'background_job',
  body: 'Image generation finished.',
  blocks: [{
    type: 'background_job',
    job_id: 'job-image-1',
    status: 'completed',
    persisted: true,
  }],
  created_at: '2026-09-26T00:00:00Z',
  runtime_origin: {
    session_id: sessionId,
    run_id: runId,
    input_id: inputId,
    append_ordinal: 1,
  },
};
const events = [
  {
    type: 'boundary_append_applied',
    run_id: runId,
    input_id: inputId,
    content: [{ type: 'text', text: 'Image generation finished.' }],
    append_count: 2,
    notices: [notice],
    transcript_start: 7,
  },
  {
    type: 'boundary_append_applied',
    run_id: runId,
    input_id: inputId,
    content: 'Legacy append without notice metadata.',
    append_count: 1,
  },
  {
    type: 'boundary_appends_discarded',
    session_id: sessionId,
    run_id: runId,
    input_ids: [inputId, '00000000-0000-4000-8000-000000000004'],
  },
];

for (const raw of events) {
  const caseName = raw.type === 'boundary_appends_discarded'
    ? 'discarded exact inputs'
    : raw.notices ? 'applied notice provenance' : 'applied legacy fields';
  test(`boundary append ${caseName} survives direct session polling`, () => {
    const session = new Session(
      7,
      async () => '{}',
      () => JSON.stringify({ session_id: sessionId, phase: 'idle' }),
      () => undefined,
      () => JSON.stringify([raw]),
      async () => '{}',
    );
    assert.deepEqual(session.pollEvents(), [raw]);
    assert.deepEqual(session.subscribe().poll(), [raw]);
  });

  test(`boundary append ${caseName} survives member and mob polling`, async () => {
    const envelope = {
      event_id: '00000000-0000-4000-8000-000000000010',
      source: { type: 'session', session_id: sessionId },
      seq: 7,
      timestamp_ms: 1,
      payload: raw,
    };
    const attributed = {
      source: { identity: 'worker-image', generation: 3 },
      source_fence_token: 3,
      role: 'worker',
      envelope,
    };
    const mob = new Mob('mob-boundary', {
      async mob_member_subscribe() { return 'member-stream'; },
      async mob_subscribe_events() { return 'mob-stream'; },
      poll_subscription(handle) {
        return JSON.stringify([handle === 'member-stream' ? envelope : attributed]);
      },
      close_subscription() {},
    });
    const memberSubscription = await mob.subscribeMemberEvents('worker-image');
    const mobSubscription = await mob.subscribeEvents();
    assert.deepEqual(memberSubscription.poll(), [envelope]);
    assert.deepEqual(mobSubscription.poll(), [attributed]);
  });
}
