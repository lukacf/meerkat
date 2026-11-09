// The typed refusal of a call that addressed a quarantined persisted mob on
// the Web SDK: `mobQuarantined`.
//
// The refusal is `MobError::MobQuarantined` (crates/meerkat-mob/src/error.rs);
// the runtime carries its structured data under the envelope's `data`
// (mob_error_value in crates/meerkat-web-runtime/src/lib.rs).

import assert from 'node:assert/strict';
import test from 'node:test';

import * as webSdk from '../dist/index.js';
import {
  MOB_QUARANTINED_KIND,
  MOB_QUARANTINE_CLASSES,
  mobQuarantined,
} from '../dist/mob-errors.js';
import { MeerkatError } from '../dist/session.js';

// The refusal's data, as the runtime sends it for a store that could not serve.
const REFUSAL = {
  kind: 'mob_quarantined',
  code: 'SESSION_RUNTIME_UNAVAILABLE',
  mob_id: 'broken',
  class: 'store_unavailable',
  retryable: true,
  content_retained: true,
  cause: 'session catalog unavailable',
};

test('mobQuarantined reads the typed refusal of a runtime rejection', () => {
  const error = MeerkatError.fromWasm(
    JSON.stringify({
      code: 'SESSION_RUNTIME_UNAVAILABLE',
      message: "mob 'broken' is quarantined (store_unavailable): session catalog unavailable",
      data: REFUSAL,
    }),
  );
  assert.equal(MOB_QUARANTINED_KIND, 'mob_quarantined');
  assert.equal(webSdk.mobQuarantined, mobQuarantined, 'the package root exports it');
  assert.deepEqual(mobQuarantined(error), {
    mobId: 'broken',
    quarantineClass: 'store_unavailable',
    retryable: true,
    cause: 'session catalog unavailable',
    code: 'SESSION_RUNTIME_UNAVAILABLE',
  });
});

test('mobQuarantined keeps a class this SDK does not know as sent', () => {
  assert.deepEqual([...MOB_QUARANTINE_CLASSES].sort(), [
    'resume_refused',
    'store_unavailable',
    'store_unreadable',
  ]);
  const refusal = mobQuarantined(
    new MeerkatError('INTERNAL_ERROR', 'quarantined', {
      ...REFUSAL,
      class: 'store_migrating',
      retryable: false,
    }),
  );
  assert.equal(refusal?.quarantineClass, 'store_migrating');
  assert.equal(refusal?.retryable, false);
});

test('mobQuarantined answers undefined for any other error', () => {
  assert.equal(mobQuarantined(new Error('not a meerkat error')), undefined);
  assert.equal(
    mobQuarantined(
      new MeerkatError('SESSION_RUNTIME_UNAVAILABLE', 'held', {
        kind: 'mob_held_before_activation',
        mob_id: 'child',
      }),
    ),
    undefined,
  );
  assert.equal(
    mobQuarantined(new MeerkatError('INTERNAL_ERROR', 'x', { ...REFUSAL, retryable: 'yes' })),
    undefined,
  );
});
