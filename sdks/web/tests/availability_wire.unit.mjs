import assert from 'node:assert/strict';
import test from 'node:test';
import { MeerkatError, Session } from '../dist/session.js';

function sessionWith(events) {
  return new Session(7, async () => '{}',
    () => JSON.stringify({ session_id: 's1', phase: 'idle' }),
    () => undefined, () => JSON.stringify(events),
    async () => '{}', async () => {}, async () => {});
}

test('runtime readiness details survive a rejected session destroy', async () => {
  const details = {
    code: 'SESSION_RUNTIME_UNAVAILABLE',
    reason: { kind: 'authority_changed' },
  };
  let destroyCalls = 0;
  const session = new Session(7, async () => '{}',
    async () => JSON.stringify({ session_id: 's1', phase: 'idle' }),
    async () => {
      destroyCalls += 1;
      throw JSON.stringify({
        code: 'SESSION_RUNTIME_UNAVAILABLE',
        message: 'session runtime unavailable',
        details,
      });
    },
    () => '[]', async () => '{}', async () => {}, async () => {});

  await assert.rejects(session.destroy(), error => {
    assert.ok(error instanceof MeerkatError);
    assert.equal(error.code, 'SESSION_RUNTIME_UNAVAILABLE');
    assert.equal(error.message, 'session runtime unavailable');
    assert.deepEqual(error.data, details);
    return true;
  });
  assert.equal(destroyCalls, 1);
  assert.equal(await session.sessionId, 's1');
});

test('typed WASM errors preserve explicit data when details are also present', () => {
  for (const data of [{ existing: 'data' }, null]) {
    const error = MeerkatError.fromWasm({
      code: 'EXISTING_ERROR', message: 'existing error', data,
      details: { other: 'details' },
    });
    assert.deepEqual(error.data, data);
  }
});

for (const [kind, retryability] of [
  ['operation_authorization_unavailable', 'non_retryable'],
  ['operation_observation_unavailable', 'non_retryable'],
  ['operation_refused', 'non_retryable'],
  ['server_overloaded', 'retryable'],
]) {
  test(`availability wire preserves ${kind} through session polling`, () => {
    const event = {
      type: 'run_failed', session_id: 's1', terminal_cause_kind: 'llm_failure',
      error_report: { class: 'llm', message: 'safe diagnostic', reason: {
        reason_type: 'llm_provider_error', provider_error_kind: kind,
        provider_error_retryability: retryability, provider_error: null,
      } },
    };
    const control = { type: 'text_delta', delta: 'ordinary text' };
    const session = sessionWith([event, control]);
    assert.deepEqual(session.pollEvents(), [event, control]);
    assert.deepEqual(session.subscribe().poll(), [event, control]);
  });
}

test('availability wire keeps callback settlement order and separate audit diagnostic', () => {
  const settlements = [
    { admission_source: 'configured_gate', effect_kind: 'tool_dispatch', physical_outcome: 'failed', failure_kind: 'operation_authorization_unavailable' },
    { admission_source: 'authorization_audit', effect_kind: 'tool_dispatch', physical_outcome: 'committed', failure_kind: 'operation_observation_unavailable' },
    { admission_source: 'context_gate', effect_kind: 'tool_dispatch', physical_outcome: 'unknown', failure_kind: 'authorization_refused' },
  ];
  const pending = {
    type: 'interaction_callback_pending', interaction_id: 'interaction-1', tool_name: 'read_record', args: {},
    pending_tool_calls: [{ tool_use_id: 'ordered-wire', tool_name: 'read_record', args: {}, settlement_failures: settlements }],
  };
  const audit = { type: 'operation_observation_failed', operation_id: 'operation-1', phase: 'outcome' };
  const session = sessionWith([pending, audit]);
  const [actual, actualAudit] = session.pollEvents();
  assert.deepEqual(actual.pending_tool_calls[0].settlement_failures, settlements);
  assert.deepEqual(actualAudit, audit);
  assert.deepEqual(session.subscribe().poll(), [pending, audit]);
});
