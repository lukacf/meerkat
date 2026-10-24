import assert from 'node:assert/strict';
import test from 'node:test';
import { MeerkatError } from '../dist/session.js';

// A typed runtime error whose data names a reason this SDK has no accessor
// for, such as a queued input a graceful stop kept for the next start, keeps
// its code and its whole data.
test('a typed error with an unknown data reason keeps its code and whole data', () => {
  const data = {
    kind: 'requeued_across_stop',
    reason: 'requeued_across_stop',
    session_id: 's1',
    input_id: 'i1',
    stopped_at: '2026-10-22T22:00:00Z',
    resumable: true,
  };
  for (const reason of [
    { code: 'SESSION_NOT_RUNNING', message: 'kept queued', data },
    JSON.stringify({ code: 'SESSION_NOT_RUNNING', message: 'kept queued', data }),
  ]) {
    const error = MeerkatError.fromWasm(reason);
    assert.equal(error.code, 'SESSION_NOT_RUNNING');
    assert.equal(error.message, 'kept queued');
    assert.deepEqual(error.data, data);
  }
});
