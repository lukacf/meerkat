import assert from 'node:assert/strict';
import test from 'node:test';
import { Session, MeerkatError } from '../dist/session.js';
import { Mob } from '../dist/mob.js';

const RUN_ID = '01936f8b-0000-7000-8000-000000000042';

function sessionWithStop(stopRun) {
  return new Session(
    7,
    async () => '{}',
    async () => '{}',
    async () => {},
    () => '[]',
    async () => '{}',
    async () => {},
    async () => {},
    stopRun,
  );
}

test('Session.stopRun calls the wasm stop export and returns the typed receipt', async () => {
  const calls = [];
  const receipt = {
    outcome: 'stopped',
    run_id: RUN_ID,
    contributors: [
      { input_id: 'i-1', completion: 'cancelled', terminal: 'cancelled' },
      { input_id: 'i-2', completion: 'runtime_terminated', terminal: 'cancelled' },
    ],
  };
  const session = sessionWithStop(async (handle, runId, reason) => {
    calls.push({ handle, runId, reason });
    return JSON.stringify(receipt);
  });
  const result = await session.stopRun(RUN_ID, 'user pressed stop');
  assert.deepEqual(calls, [{ handle: 7, runId: RUN_ID, reason: 'user pressed stop' }]);
  assert.deepEqual(result, receipt);
});

test('a stale stop resolves to a not_current receipt', async () => {
  const receipt = { outcome: 'not_current', run_id: RUN_ID, current_run_id: 'r-2' };
  const session = sessionWithStop(async () => JSON.stringify(receipt));
  assert.deepEqual(await session.stopRun(RUN_ID, 'late'), receipt);
});

test('malformed receipts fail closed', async () => {
  for (const receipt of [
    { outcome: 'stopped', run_id: RUN_ID },
    { outcome: 'not_stoppable', run_id: RUN_ID },
    { outcome: 'mystery', run_id: RUN_ID },
  ]) {
    const session = sessionWithStop(async () => JSON.stringify(receipt));
    await assert.rejects(session.stopRun(RUN_ID, 'stop'), MeerkatError);
  }
});

test('a session without the stop binding reports capability unavailable', async () => {
  const session = sessionWithStop(undefined);
  await assert.rejects(session.stopRun(RUN_ID, 'stop'), (error) => {
    assert.ok(error instanceof MeerkatError);
    assert.equal(error.code, 'CAPABILITY_UNAVAILABLE');
    return true;
  });
});

test('Mob.stopMemberRun calls the wasm mob stop export', async () => {
  const calls = [];
  const receipt = { outcome: 'not_current', run_id: RUN_ID };
  const mob = new Mob('mob-web-unit', {
    async mob_stop_member_run(mobId, agentIdentity, runId, reason) {
      calls.push({ mobId, agentIdentity, runId, reason });
      return JSON.stringify(receipt);
    },
  });
  const result = await mob.stopMemberRun('worker-1', RUN_ID, 'stop');
  assert.deepEqual(calls, [
    { mobId: 'mob-web-unit', agentIdentity: 'worker-1', runId: RUN_ID, reason: 'stop' },
  ]);
  assert.deepEqual(result, receipt);
});
