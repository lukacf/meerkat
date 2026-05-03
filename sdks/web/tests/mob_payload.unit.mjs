import assert from 'node:assert/strict';
import test from 'node:test';

import { Mob } from '../dist/mob.js';
import { MeerkatRuntime } from '../dist/runtime.js';

function makeSubscriptionRuntime(overrides = {}) {
  return {
    default: async () => undefined,
    runtime_version: () => '0.6.0',
    init_runtime_from_config: () => JSON.stringify({ status: 'initialized' }),
    destroy_runtime: () => undefined,
    async mob_create(definitionJson) {
      return JSON.parse(definitionJson).id;
    },
    async mob_subscribe_events() {
      return 1;
    },
    poll_subscription() {
      return '[]';
    },
    close_subscription: () => undefined,
    ...overrides,
  };
}

async function makeRuntimeMob(wasm) {
  const runtime = await MeerkatRuntime.init(wasm, {
    anthropicApiKey: 'sk-test',
    model: 'claude-sonnet-4-5',
  });
  const mob = await runtime.createMob({
    id: 'mob-web-unit',
    profiles: {
      worker: {
        model: 'claude-sonnet-4-5',
      },
    },
  });
  return { runtime, mob };
}

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

test('MeerkatRuntime keeps a clean empty subscription poll as empty success', async () => {
  const wasm = makeSubscriptionRuntime();
  const { runtime, mob } = await makeRuntimeMob(wasm);
  try {
    const subscription = await mob.subscribeEvents();
    assert.deepEqual(subscription.poll(), []);
  } finally {
    runtime.destroy();
  }
});

test('MeerkatRuntime propagates subscription serialization failures', async () => {
  const wasm = makeSubscriptionRuntime({
    poll_subscription() {
      throw new Error('serialize_error: failed to serialize subscription attributed event');
    },
  });
  const { runtime, mob } = await makeRuntimeMob(wasm);
  try {
    const subscription = await mob.subscribeEvents();
    assert.throws(() => subscription.poll(), /serialize_error/);
  } finally {
    runtime.destroy();
  }
});

test('MeerkatRuntime rejects malformed subscription poll output', async () => {
  const wasm = makeSubscriptionRuntime({
    poll_subscription() {
      return JSON.stringify({ events: [] });
    },
  });
  const { runtime, mob } = await makeRuntimeMob(wasm);
  try {
    const subscription = await mob.subscribeEvents();
    assert.throws(() => subscription.poll(), /expected event array/);
  } finally {
    runtime.destroy();
  }
});
