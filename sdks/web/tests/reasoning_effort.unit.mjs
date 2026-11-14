// The provider-neutral reasoning effort option on the Web SDK: the session
// config's `reasoningEffort`, the typed refusal (`reasoningEffortRefusal`)
// and the report of what a run sent.
//
// The refusal is `SessionError`'s `reasoning_effort_refused` data
// (crates/meerkat-core/src/service/mod.rs); the runtime carries it under the
// rejection envelope's `data`.

import assert from 'node:assert/strict';
import test from 'node:test';

import * as webSdk from '../dist/index.js';
import {
  REASONING_EFFORT_LEVELS,
  REASONING_EFFORT_REFUSAL_REASONS,
  REASONING_EFFORT_REFUSED_KIND,
  parseReasoningEffortReport,
  parseReasoningEffortResolution,
  reasoningEffortRefusal,
  reasoningEffortReport,
} from '../dist/reasoning-effort.js';
import { MeerkatRuntime } from '../dist/runtime.js';
import { MeerkatError } from '../dist/session.js';

// The refusal's data for a level the model does not accept.
const UNSUPPORTED = {
  kind: 'reasoning_effort_refused',
  retryable: false,
  reasoning_effort_refusal: {
    requested: 'max',
    provider: 'anthropic',
    model: 'claude-opus-4-5',
    route: { backend: 'anthropic_api', api: 'messages' },
    reason: 'unsupported_level',
    supported: ['low', 'medium', 'high'],
    facts: 'catalog',
  },
};

const refused = (data) => new MeerkatError('INVALID_PARAMS', 'reasoning effort refused', data);

const withRefusal = (changes) => ({
  ...UNSUPPORTED,
  reasoning_effort_refusal: { ...UNSUPPORTED.reasoning_effort_refusal, ...changes },
});

test('the package exports the reasoning effort helpers', () => {
  assert.equal(webSdk.reasoningEffortRefusal, reasoningEffortRefusal);
  assert.equal(webSdk.reasoningEffortReport, reasoningEffortReport);
  assert.equal(REASONING_EFFORT_REFUSED_KIND, 'reasoning_effort_refused');
  assert.deepEqual(
    [...REASONING_EFFORT_LEVELS],
    ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
  );
});

test('createSession hands the session config level to the runtime', async () => {
  const configs = [];
  const wasm = {
    create_session_simple: async (configJson) => {
      configs.push(JSON.parse(configJson));
      return 7;
    },
  };
  // The constructor is private in the types only; `init` needs a wasm build.
  const runtime = new MeerkatRuntime(wasm, 'runtime-1');

  await runtime.createSession({ model: 'test', reasoningEffort: 'xhigh' });
  await runtime.createSession({ model: 'test' });

  assert.equal(configs[0].reasoning_effort, 'xhigh');
  assert.equal('reasoning_effort' in configs[1], false);
});

test('reasoningEffortRefusal reads the typed refusal of a runtime rejection', () => {
  assert.deepEqual(reasoningEffortRefusal(refused(UNSUPPORTED)), {
    requested: 'max',
    provider: 'anthropic',
    model: 'claude-opus-4-5',
    route: { backend: 'anthropic_api', api: 'messages' },
    reason: 'unsupported_level',
    supported: ['low', 'medium', 'high'],
    facts: 'catalog',
    transient: false,
    retryable: false,
  });
});

test('the eight reasons are closed and only one is transient', () => {
  assert.deepEqual(
    [...REASONING_EFFORT_REFUSAL_REASONS],
    [
      'unsupported_level',
      'supported_levels_unknown',
      'supported_levels_unavailable',
      'conflicting_provider_effort',
      'conflicting_thinking_budget',
      'conflicting_thinking_type',
      'conflicting_reasoning_body',
      'conflicting_reasoning_mode',
    ],
  );
  for (const reason of REASONING_EFFORT_REFUSAL_REASONS) {
    const transient = reason === 'supported_levels_unavailable';
    const refusal = reasoningEffortRefusal(
      refused({ ...withRefusal({ reason }), retryable: transient }),
    );
    assert.equal(refusal?.reason, reason);
    // The class is read from the reason; the runtime's mark agrees.
    assert.equal(refusal?.transient, transient, reason);
    assert.equal(refusal?.retryable, transient, reason);
  }
});

test('a refusal that names no route, or no supported set, parses', () => {
  const refusal = reasoningEffortRefusal(
    refused(
      withRefusal({
        route: null,
        reason: 'supported_levels_unknown',
        supported: null,
        facts: null,
      }),
    ),
  );
  assert.equal(refusal?.route, null);
  assert.equal(refusal?.supported, null);
  assert.equal(refusal?.facts, null);
});

test('advisory values are kept and an unknown reason keeps the raw data', () => {
  const advisory = reasoningEffortRefusal(
    refused(withRefusal({ route: { backend: 'new_backend', api: 'new_api' }, facts: 'new_source' })),
  );
  assert.deepEqual(advisory?.route, { backend: 'new_backend', api: 'new_api' });
  assert.equal(advisory?.facts, 'new_source');

  const unknownReason = withRefusal({ reason: 'a_reason_from_a_newer_runtime' });
  const error = refused(unknownReason);
  assert.equal(reasoningEffortRefusal(error), undefined);
  assert.deepEqual(error.data, unknownReason);
  assert.equal(reasoningEffortRefusal(refused({ kind: 'mob_quarantined' })), undefined);
  assert.equal(reasoningEffortRefusal(new Error('no data')), undefined);
});

test('the run report and a resolution keep unknown advisory values', () => {
  const block = {
    requested: 'high',
    sent: [
      {
        level: 'high',
        provider: 'openai',
        model: 'gpt-5.5',
        route: { backend: 'new_backend', api: 'new_api' },
        fields: ['reasoning.effort'],
        facts: 'new_source',
        decided_by: 'new_decider',
        requests: 3,
      },
    ],
  };
  const report = parseReasoningEffortReport(block);
  assert.deepEqual(report, {
    requested: 'high',
    sent: [
      {
        level: 'high',
        provider: 'openai',
        model: 'gpt-5.5',
        route: { backend: 'new_backend', api: 'new_api' },
        fields: ['reasoning.effort'],
        facts: 'new_source',
        decidedBy: 'new_decider',
        requests: 3,
      },
    ],
  });
  assert.deepEqual(reasoningEffortReport({ text: 'ok', reasoning_effort: block }), report);
  assert.equal(reasoningEffortReport({ text: 'ok' }), undefined);
  assert.equal(
    parseReasoningEffortResolution({
      requested: 'high',
      level: 'high',
      provider: 'openai',
      model: 'gpt-5.5',
      route: { backend: 'copilot', api: 'responses' },
      fields: ['reasoning.effort'],
      facts: 'account',
      decided_by: 'request',
    })?.route?.backend,
    'copilot',
  );
});
