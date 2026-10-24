// The typed mob destroy report on the Web SDK: `mobDestroyIncompleteReport`
// and `Mob.lifecycle('destroy')`.
//
// The report is `WireMobDestroyReport` in artifacts/schemas/wire-types.json,
// emitted from the Rust contract (crates/meerkat-contracts/src/wire/mob.rs).
// The rejections below are the runtime's incomplete-destroy envelope
// (mob_destroy_error_value in crates/meerkat-web-runtime/src/lib.rs).

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import * as webSdk from '../dist/index.js';
import { Mob } from '../dist/mob.js';
import { mobDestroyIncompleteReport } from '../dist/mob-destroy-report.js';
import { MeerkatRuntime } from '../dist/runtime.js';
import { MeerkatError } from '../dist/session.js';

const { version: CURRENT_WASM_VERSION } = JSON.parse(
  await readFile(new URL('../package.json', import.meta.url), 'utf8'),
);
const SCHEMA = JSON.parse(
  await readFile(new URL('../../../artifacts/schemas/wire-types.json', import.meta.url), 'utf8'),
).WireMobDestroyReport;

// Every field set, as the runtime reports it.
const FULL_REPORT = {
  force_destroyed_members: ['forced'],
  orphaned_remote_members: ['orphaned'],
  remote_cleanup_deadline_exceeded: true,
  metadata_scrubbed: true,
  events_cleared: true,
  namespace_cleaned: true,
  errors: ['refused: the runtime refused the reload'],
  reloaded_members: ['reloaded'],
  member_reload_refusals: [
    {
      member: 'refused',
      refusal: { kind: 'repair_blocked', reason: 'commit outcome not reported', evidence_digest: 'digest-1' },
    },
  ],
};

function incompleteData(report) {
  return { code: 'mob_destroy_incomplete', destroy_report: report, retryable: true };
}

/** The runtime's rejection of a destroy that did not finish. */
function incompleteRejection(report) {
  return JSON.stringify({
    code: 'mob_destroy_incomplete',
    message: 'mob destroy incomplete: refused: the runtime refused the reload',
    destroy_report: report,
    retryable: true,
    data: incompleteData(report),
  });
}

function rejectingLifecycleMob(report) {
  return new Mob('mob-web-unit', {
    async mob_lifecycle() {
      throw incompleteRejection(report);
    },
  });
}

async function rejection(promise) {
  return promise.then(
    () => assert.fail('the call must reject'),
    (error) => error,
  );
}

test('the package root exports mobDestroyIncompleteReport', () => {
  assert.equal(webSdk.mobDestroyIncompleteReport, mobDestroyIncompleteReport);
});

test('mobDestroyIncompleteReport reads the typed report of a rejected lifecycle destroy', async () => {
  const error = await rejection(rejectingLifecycleMob(FULL_REPORT).lifecycle('destroy'));
  assert.ok(error instanceof MeerkatError);
  assert.equal(error.code, 'mob_destroy_incomplete');
  const report = mobDestroyIncompleteReport(error);
  assert.equal(report?.member_reload_refusals?.[0]?.refusal.kind, 'repair_blocked');
  assert.deepEqual(report, FULL_REPORT);
  assert.deepEqual(error.data, incompleteData(FULL_REPORT), 'data stays as sent');
});

test('mobDestroyIncompleteReport reads every field the schema names', () => {
  const report = mobDestroyIncompleteReport(
    new MeerkatError('mob_destroy_incomplete', 'incomplete', incompleteData(FULL_REPORT)),
  );
  assert.deepEqual(Object.keys(report).sort(), Object.keys(SCHEMA.properties).sort());
});

test('mobDestroyIncompleteReport reads a refusal kind it does not know as unknown, without an error', async () => {
  const error = await rejection(
    rejectingLifecycleMob({
      errors: ['m: refused'],
      member_reload_refusals: [{ member: 'm', refusal: { kind: 'a_later_kind', detail: 1 } }],
    }).lifecycle('destroy'),
  );
  assert.deepEqual(mobDestroyIncompleteReport(error), {
    errors: ['m: refused'],
    member_reload_refusals: [{ member: 'm', refusal: { kind: 'unknown' } }],
  });
});

test('MeerkatRuntime.destroy keeps the typed report of an incomplete mob destroy', async () => {
  const runtime = await MeerkatRuntime.init(
    {
      default: async () => undefined,
      runtime_version: () => CURRENT_WASM_VERSION,
      init_runtime_from_config: () =>
        JSON.stringify({
          status: 'initialized',
          runtime_handle: 'mock-runtime-handle',
          model: 'claude-sonnet-4-5',
          providers: ['anthropic'],
        }),
      async destroy_runtime() {
        throw incompleteRejection(FULL_REPORT);
      },
    },
    { anthropicApiKey: 'sk-test', model: 'claude-sonnet-4-5' },
  );
  const error = await rejection(runtime.destroy());
  assert.ok(error instanceof MeerkatError);
  assert.deepEqual(mobDestroyIncompleteReport(error), FULL_REPORT);
});

test('mobDestroyIncompleteReport is undefined for another code or a report that does not parse', () => {
  const refusal = (value) => incompleteData({ member_reload_refusals: [value] });
  for (const data of [
    undefined,
    null,
    'mob_destroy_incomplete',
    { code: 'mob_error', destroy_report: FULL_REPORT },
    { code: 'mob_destroy_incomplete' },
    incompleteData(null),
    incompleteData({ errors: 'x' }),
    incompleteData({ metadata_scrubbed: 'yes' }),
    refusal({ member: 'm' }),
    refusal({ member: 'm', refusal: { kind: 'repair_blocked' } }),
    refusal({ member: 'm', refusal: { kind: 'repair_blocked', reason: 'r', evidence_digest: 1 } }),
  ]) {
    assert.equal(
      mobDestroyIncompleteReport(new MeerkatError('mob_destroy_incomplete', 'x', data)),
      undefined,
      JSON.stringify(data),
    );
  }
  assert.equal(mobDestroyIncompleteReport(new Error('not a meerkat error')), undefined);
  assert.equal(mobDestroyIncompleteReport(incompleteRejection(FULL_REPORT)), undefined);
});

test('Mob.lifecycle reads a destroy report refusal kind it does not know as unknown', async () => {
  const mob = new Mob('mob-web-unit', {
    async mob_lifecycle(mobId, action) {
      return JSON.stringify({
        mob_id: mobId,
        action,
        ok: true,
        destroy_report: {
          ...FULL_REPORT,
          member_reload_refusals: [{ member: 'm', refusal: { kind: 'a_later_kind' } }],
        },
      });
    },
  });
  const result = await mob.lifecycle('destroy');
  assert.deepEqual(result.destroy_report, {
    ...FULL_REPORT,
    member_reload_refusals: [{ member: 'm', refusal: { kind: 'unknown' } }],
  });
});

test('Mob.lifecycle keeps a result without a destroy report and rejects a malformed one', async () => {
  const lifecycleMob = (destroyReport) =>
    new Mob('mob-web-unit', {
      async mob_lifecycle(mobId, action) {
        return JSON.stringify({ mob_id: mobId, action, ok: true, destroy_report: destroyReport });
      },
    });
  assert.deepEqual(await lifecycleMob(undefined).lifecycle('stop'), {
    mob_id: 'mob-web-unit',
    action: 'stop',
    ok: true,
  });
  await assert.rejects(
    () => lifecycleMob({ errors: 'x' }).lifecycle('destroy'),
    /Invalid mob\/lifecycle response: malformed destroy_report/,
  );
});
