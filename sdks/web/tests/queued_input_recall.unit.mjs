import assert from 'node:assert/strict';
import test from 'node:test';
import { Session, MeerkatError } from '../dist/session.js';

const INPUT_ID = '01936f8b-0000-7000-8000-0000000000a1';
const RUN_ID = '01936f8b-0000-7000-8000-000000000042';

function sessionWithRecall(recallInput) {
  return new Session(
    7,
    async () => '{}',
    async () => '{}',
    async () => {},
    () => '[]',
    async () => '{}',
    async () => {},
    async () => {},
    undefined,
    recallInput,
  );
}

test('Session.recallInput calls the wasm recall export and returns the typed answer', async () => {
  const calls = [];
  const recall = { outcome: 'abandoned', input_id: INPUT_ID, attempt_count: 0 };
  const session = sessionWithRecall(async (handle, inputId) => {
    calls.push({ handle, inputId });
    return JSON.stringify(recall);
  });
  const result = await session.recallInput(INPUT_ID);
  assert.deepEqual(calls, [{ handle: 7, inputId: INPUT_ID }]);
  assert.deepEqual(result, recall);
});

test('an input a run holds resolves to not_queued with its phase and run', async () => {
  const recall = { outcome: 'not_queued', input_id: INPUT_ID, phase: 'staged', run_id: RUN_ID };
  const session = sessionWithRecall(async () => JSON.stringify(recall));
  assert.deepEqual(await session.recallInput(INPUT_ID), recall);
});

test('a second recall resolves to already_terminal', async () => {
  const recall = { outcome: 'already_terminal', input_id: INPUT_ID, terminal: 'cancelled' };
  const session = sessionWithRecall(async () => JSON.stringify(recall));
  assert.deepEqual(await session.recallInput(INPUT_ID), recall);
});

test('an abandoned answer without an attempt count reads zero', async () => {
  const session = sessionWithRecall(async () =>
    JSON.stringify({ outcome: 'abandoned', input_id: INPUT_ID }),
  );
  assert.equal((await session.recallInput(INPUT_ID)).attempt_count, 0);
});

test('malformed recall answers fail closed', async () => {
  for (const recall of [
    'abandoned',
    { outcome: 'abandoned' },
    { outcome: 7, input_id: INPUT_ID },
    { outcome: '', input_id: INPUT_ID },
    { outcome: 'not_queued', input_id: INPUT_ID },
    { outcome: 'not_queued', input_id: INPUT_ID, phase: 'staged', run_id: 7 },
    { outcome: 'already_terminal', input_id: INPUT_ID },
    { outcome: 'abandoned', input_id: INPUT_ID, attempt_count: '0' },
  ]) {
    const session = sessionWithRecall(async () => JSON.stringify(recall));
    await assert.rejects(session.recallInput(INPUT_ID), MeerkatError);
  }
});

test('an outcome this SDK does not know is kept as it is', async () => {
  // The list of outcomes can grow: an outcome this SDK does not know is
  // returned unchanged, with every field the runtime sent, and rejects nothing.
  const recall = { outcome: 'kept_for_the_next_start', input_id: INPUT_ID, until: 'later' };
  const session = sessionWithRecall(async () => JSON.stringify(recall));
  assert.deepEqual(await session.recallInput(INPUT_ID), recall);
});

test('a refusal from the runtime surfaces as a MeerkatError', async () => {
  const session = sessionWithRecall(async () => {
    throw { code: 'invalid_params', message: 'session holds no input' };
  });
  await assert.rejects(session.recallInput(INPUT_ID), MeerkatError);
});

test('a session without the recall binding reports capability unavailable', async () => {
  const session = sessionWithRecall(undefined);
  await assert.rejects(session.recallInput(INPUT_ID), (error) => {
    assert.ok(error instanceof MeerkatError);
    assert.equal(error.code, 'CAPABILITY_UNAVAILABLE');
    return true;
  });
});
