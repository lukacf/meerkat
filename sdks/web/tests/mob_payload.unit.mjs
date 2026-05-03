import assert from 'node:assert/strict';
import test from 'node:test';

import { Mob } from '../dist/mob.js';

test('Mob.spawn strips legacy generation and projects typed wasm spawn rows', async () => {
  let captured;
  const mob = new Mob('mob-web-unit', {
    async mob_spawn(mobId, specsJson) {
      captured = { mobId, specs: JSON.parse(specsJson) };
      return JSON.stringify([
        {
          status: 'spawned',
          result: {
            agent_identity: 'worker-1',
            member_ref: 'ref-worker-1',
          },
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

test('Mob.spawn rejects malformed typed wasm result envelopes', async () => {
  const malformedPayloads = [
    [
      {
        ok: true,
        agent_identity: 'worker-1',
        member_ref: 'ref-worker-1',
      },
    ],
    [
      {
        agent_identity: 'worker-1',
        member_ref: 'ref-worker-1',
      },
    ],
    [
      {
        status: 'ok',
        agent_identity: 'worker-1',
        member_ref: 'ref-worker-1',
      },
    ],
    [
      {
        status: 'ok',
        result: {
          agent_identity: 'worker-1',
          member_ref: 'ref-worker-1',
        },
      },
    ],
    [
      {
        status: 'error',
        error: 'profile missing',
      },
    ],
    [
      {
        status: 'spawned',
      },
    ],
    [
      {
        status: 'spawned',
        result: {
          agent_identity: 'worker-1',
        },
      },
    ],
    [
      {
        status: 'failed',
        result: {
          message: '',
        },
      },
    ],
    {
      results: [],
    },
  ];

  for (const payload of malformedPayloads) {
    const mob = new Mob('mob-web-unit', {
      async mob_spawn() {
        return JSON.stringify(payload);
      },
    });

    await assert.rejects(
      () => mob.spawn([{ profile: 'worker', agent_identity: 'worker-1' }]),
      /Invalid mob spawn response/,
    );
  }
});

test('Mob.spawn rejects typed failed rows instead of projecting success', async () => {
  const mob = new Mob('mob-web-unit', {
    async mob_spawn() {
      return JSON.stringify([
        {
          status: 'failed',
          result: {
            message: 'profile missing',
          },
        },
      ]);
    },
  });

  await assert.rejects(
    () => mob.spawn([{ profile: 'worker', agent_identity: 'worker-1' }]),
    /Mob spawn failed: profile missing/,
  );
});
