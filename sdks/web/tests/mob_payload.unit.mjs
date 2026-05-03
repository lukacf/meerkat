import assert from 'node:assert/strict';
import test from 'node:test';

import { Mob } from '../dist/mob.js';
import { MeerkatRuntime } from '../dist/runtime.js';
import { Session } from '../dist/session.js';

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

function canonicalEnvelope(overrides = {}) {
  return {
    event_id: '00000000-0000-0000-0000-000000000001',
    source_id: 'worker-runtime',
    seq: 7,
    timestamp_ms: 1710000000000,
    payload: {
      type: 'text_delta',
      delta: 'hello',
    },
    ...overrides,
  };
}

function makeSubscriptionMob(pollSubscription) {
  return new Mob('mob-web-unit', {
    async mob_member_subscribe() {
      return 1;
    },
    async mob_subscribe_events() {
      return 2;
    },
    poll_subscription: pollSubscription,
    close_subscription: () => undefined,
  });
}

function makeDirectSession(pollEvents) {
  return new Session(
    7,
    async () => '{}',
    () => JSON.stringify({ session_id: 'session-web-unit', phase: 'idle' }),
    () => undefined,
    pollEvents,
    async () => '{}',
  );
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

test('Mob.subscribeMemberEvents projects canonical WASM EventEnvelope payloads', async () => {
  const envelope = canonicalEnvelope();
  const mob = makeSubscriptionMob((handle) => {
    assert.equal(handle, 1);
    return JSON.stringify([envelope]);
  });

  const subscription = await mob.subscribeMemberEvents('worker-1');
  const items = subscription.poll();

  assert.deepEqual(items, [envelope]);
  assert.equal(items[0].payload.type, 'text_delta');
  assert.equal('event' in items[0], false);
});

test('Mob.subscribeEvents projects canonical attributed WASM EventEnvelope payloads', async () => {
  const envelope = canonicalEnvelope({
    mob_id: 'mob-web-unit',
  });
  const attributed = {
    source: {
      identity: 'worker-runtime',
      generation: 0,
    },
    source_fence_token: 3,
    role: 'worker',
    envelope,
  };
  const mob = makeSubscriptionMob((handle) => {
    assert.equal(handle, 2);
    return JSON.stringify([attributed]);
  });

  const subscription = await mob.subscribeEvents();
  const items = subscription.poll();

  assert.deepEqual(items, [attributed]);
  assert.deepEqual(items[0].source, { identity: 'worker-runtime', generation: 0 });
  assert.equal(items[0].envelope.payload.type, 'text_delta');
  assert.equal('event' in items[0].envelope, false);
});

test('Mob subscriptions reject unrecognized event envelopes instead of fabricating unknown events', async () => {
  const legacyMemberEnvelope = {
    event_id: '00000000-0000-0000-0000-000000000001',
    source_id: 'worker-runtime',
    seq: 7,
    timestamp_ms: 1710000000000,
    event: {
      type: 'text_delta',
      delta: 'legacy',
    },
  };
  const mob = makeSubscriptionMob((handle) => {
    if (handle === 1) {
      return JSON.stringify([legacyMemberEnvelope]);
    }
    return JSON.stringify([
      {
        source: {
          identity: 'worker-runtime',
          generation: 0,
        },
        role: 'worker',
        envelope: legacyMemberEnvelope,
      },
    ]);
  });

  const memberSubscription = await mob.subscribeMemberEvents('worker-1');
  assert.throws(() => memberSubscription.poll(), /missing payload/);

  const mobSubscription = await mob.subscribeEvents();
  assert.throws(() => mobSubscription.poll(), /missing payload/);
});

test('Mob.subscribeEvents rejects legacy string sources instead of hiding runtime generation', async () => {
  const mob = makeSubscriptionMob((handle) => {
    assert.equal(handle, 2);
    return JSON.stringify([
      {
        source: 'worker-runtime',
        role: 'worker',
        envelope: canonicalEnvelope({ mob_id: 'mob-web-unit' }),
      },
    ]);
  });

  const subscription = await mob.subscribeEvents();
  assert.throws(() => subscription.poll(), /missing source/);
});

test('Session direct polling projects valid agent events', () => {
  const event = {
    type: 'text_delta',
    delta: 'hello',
  };
  const session = makeDirectSession(() => JSON.stringify([event]));

  assert.deepEqual(session.pollEvents(), [event]);
  assert.deepEqual(session.subscribe().poll(), [event]);
});

test('Session direct polling rejects malformed output instead of clean empty success', () => {
  const session = makeDirectSession(() => JSON.stringify({ events: [] }));

  assert.throws(() => session.pollEvents(), /expected event array/);
});

test('Session direct polling rejects malformed event items', () => {
  const session = makeDirectSession(() => JSON.stringify([{}]));

  assert.throws(() => session.pollEvents(), /missing type/);
  assert.throws(() => session.subscribe().poll(), /missing type/);
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
