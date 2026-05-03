import assert from 'node:assert/strict';
import test from 'node:test';

import { Mob } from '../dist/mob.js';

test('Mob.spawn strips legacy generation from wasm payloads', async () => {
  let captured;
  const mob = new Mob('mob-web-unit', {
    async mob_spawn(mobId, specsJson) {
      captured = { mobId, specs: JSON.parse(specsJson) };
      return JSON.stringify([
        {
          mob_id: mobId,
          agent_identity: 'worker-1',
          member_ref: 'ref-worker-1',
        },
      ]);
    },
  });

  const result = await mob.spawn([
    {
      profile: 'worker',
      agent_identity: 'worker-1',
      generation: 7,
    },
  ]);

  assert.equal(result[0].agent_identity, 'worker-1');
  assert.deepEqual(captured, {
    mobId: 'mob-web-unit',
    specs: [
      {
        profile: 'worker',
        agent_identity: 'worker-1',
      },
    ],
  });
});

test('Mob.spawn rejects legacy ok result rows from wasm payloads', async () => {
  const mob = new Mob('mob-web-unit', {
    async mob_spawn() {
      return JSON.stringify([
        {
          ok: true,
          agent_identity: 'worker-1',
          member_ref: 'ref-worker-1',
        },
      ]);
    },
  });

  await assert.rejects(
    () => mob.spawn([{ profile: 'worker', agent_identity: 'worker-1' }]),
    /Invalid mob spawn response/,
  );
});
