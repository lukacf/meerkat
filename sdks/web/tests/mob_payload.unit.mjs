import assert from 'node:assert/strict';
import test from 'node:test';

import { Mob } from '../dist/mob.js';
import { MeerkatRuntime } from '../dist/runtime.js';
import { Session } from '../dist/session.js';
import { isKnownEvent } from '../dist/types.js';

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
    source: {
      type: 'runtime',
      runtime_id: 'worker-runtime',
    },
    source_id: 'runtime:worker-runtime',
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

async function runtimeWithMobList(payload) {
  return MeerkatRuntime.init(
    {
      async default() {},
      runtime_version() {
        return '0.6.0';
      },
      init_runtime_from_config() {
        return JSON.stringify({ status: 'initialized' });
      },
      async mob_list() {
        return JSON.stringify(payload);
      },
    },
    {},
  );
}

test('isKnownEvent requires typed skills_resolved identities', () => {
  assert.equal(
    isKnownEvent({
      type: 'skills_resolved',
      skills: [
        {
          source_uuid: '00000000-0000-4b11-8111-000000000001',
          skill_name: 'email-extractor',
        },
      ],
      injection_bytes: 128,
    }),
    true,
  );
  assert.equal(
    isKnownEvent({
      type: 'skills_resolved',
      skills: ['legacy/ref'],
      injection_bytes: 128,
    }),
    false,
  );
  assert.equal(
    isKnownEvent({
      type: 'skills_resolved',
      skills: [{ source_uuid: '00000000-0000-4b11-8111-000000000001' }],
      injection_bytes: 128,
    }),
    false,
  );
});

test('isKnownEvent fails closed for unknown skill resolution statuses', () => {
  assert.equal(
    isKnownEvent({
      type: 'skill_resolution_failed',
      reason: { reason_type: 'future_status', message: 'future details' },
      reference: 'legacy/ref',
      error: 'missing',
    }),
    false,
  );
  assert.equal(
    isKnownEvent({
      type: 'skill_resolution_failed',
      reason: { reason_type: 'unknown', message: 'future details' },
      reference: 'legacy/ref',
      error: 'missing',
    }),
    true,
  );
});

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
  assert.deepEqual(items[0].source, { type: 'runtime', runtime_id: 'worker-runtime' });
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
  assert.deepEqual(items[0].envelope.source, { type: 'runtime', runtime_id: 'worker-runtime' });
  assert.equal(items[0].envelope.payload.type, 'text_delta');
  assert.equal('event' in items[0].envelope, false);
});

test('Mob subscriptions reject source-id-only EventEnvelope payloads', async () => {
  const sourceIdOnly = {
    event_id: '00000000-0000-0000-0000-000000000001',
    source_id: 'session:00000000-0000-4000-8000-000000000001',
    seq: 7,
    timestamp_ms: 1710000000000,
    payload: {
      type: 'text_delta',
      delta: 'legacy',
    },
  };
  const mob = makeSubscriptionMob((handle) => {
    if (handle === 1) {
      return JSON.stringify([sourceIdOnly]);
    }
    return JSON.stringify([
      {
        source: {
          identity: 'worker-runtime',
          generation: 0,
        },
        role: 'worker',
        envelope: sourceIdOnly,
      },
    ]);
  });

  const memberSubscription = await mob.subscribeMemberEvents('worker-1');
  assert.throws(() => memberSubscription.poll(), /missing source/);

  const mobSubscription = await mob.subscribeEvents();
  assert.throws(() => mobSubscription.poll(), /missing source/);
});

test('Mob subscriptions reject malformed typed EventEnvelope source instead of trusting source_id', async () => {
  const malformedSource = canonicalEnvelope({
    source: {
      type: 'session',
    },
    source_id: 'session:00000000-0000-4000-8000-000000000001',
  });
  const mob = makeSubscriptionMob((handle) => {
    assert.equal(handle, 1);
    return JSON.stringify([malformedSource]);
  });

  const subscription = await mob.subscribeMemberEvents('worker-1');
  assert.throws(() => subscription.poll(), /source missing session_id/);
});

test('Mob subscriptions keep legacy source_id inert when typed source disagrees', async () => {
  const envelope = canonicalEnvelope({
    source: {
      type: 'session',
      session_id: '00000000-0000-4000-8000-000000000001',
    },
    source_id: 'session:not-a-uuid',
  });
  const mob = makeSubscriptionMob((handle) => {
    assert.equal(handle, 1);
    return JSON.stringify([envelope]);
  });

  const subscription = await mob.subscribeMemberEvents('worker-1');
  const [item] = subscription.poll();

  assert.deepEqual(item.source, {
    type: 'session',
    session_id: '00000000-0000-4000-8000-000000000001',
  });
  assert.equal(item.source_id, 'session:not-a-uuid');
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

test('Session destroy does not cache lifecycle state in the browser handle', async () => {
  let destroyCalls = 0;
  let stateCalls = 0;
  let pollCalls = 0;
  let appendCalls = 0;
  const session = new Session(
    11,
    async () => {
      throw new Error('SESSION_NOT_FOUND: session not found');
    },
    () => {
      stateCalls += 1;
      return JSON.stringify({
        handle: 11,
        session_id: 'session-web-unit',
        mob_id: '',
        model: 'claude-sonnet-4-5',
        usage: { input_tokens: 0, output_tokens: 0 },
        message_count: 0,
        is_active: false,
        last_assistant_text: null,
      });
    },
    () => {
      destroyCalls += 1;
    },
    () => {
      pollCalls += 1;
      return JSON.stringify([{ type: 'text_complete', text: 'from wasm' }]);
    },
    async () => {
      appendCalls += 1;
      throw new Error('SESSION_NOT_FOUND: session not found');
    },
  );

  session.destroy();

  assert.equal(destroyCalls, 1);
  assert.equal(session.getState().session_id, 'session-web-unit');
  assert.equal(stateCalls, 1);
  assert.deepEqual(session.pollEvents(), [{ type: 'text_complete', text: 'from wasm' }]);
  assert.equal(pollCalls, 1);
  await assert.rejects(() => session.turn('after destroy'), /SESSION_NOT_FOUND|session not found/);
  await assert.rejects(
    () => session.appendSystemContext({ text: 'after destroy' }),
    /SESSION_NOT_FOUND|session not found/,
  );
  assert.equal(appendCalls, 1);
  assert.throws(() => session.isDestroyed, /deprecated/i);
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

test('Mob.status projects only generated status truth', async () => {
  const mob = new Mob('mob-web-unit', {
    async mob_status(mobId) {
      return JSON.stringify({ mob_id: mobId, status: 'Running' });
    },
  });

  const status = await mob.status();

  assert.equal(status.mob_id, 'mob-web-unit');
  assert.equal(status.status, 'Running');
  assert.equal(status.state, 'Running');
});

test('MeerkatRuntime.listMobs projects only generated mob list status truth', async () => {
  const runtime = await runtimeWithMobList({
    mobs: [{ mob_id: 'mob-web-unit', status: 'Running', state: 'Stale' }],
  });

  const statuses = await runtime.listMobs();

  assert.deepEqual(statuses, [
    {
      mob_id: 'mob-web-unit',
      status: 'Running',
      state: 'Running',
    },
  ]);
});

test('MeerkatRuntime.listMobs rejects malformed typed status rows instead of projecting state', async () => {
  const malformedPayloads = [
    [{ mob_id: 'mob-web-unit', state: 'Running' }],
    { mobs: [{ mob_id: 'mob-web-unit', state: 'Running' }] },
    { mobs: [{ mob_id: 'mob-web-unit', status: '' }] },
    { mobs: [{ status: 'Running' }] },
    { mobs: 'not-an-array' },
    {},
  ];

  for (const payload of malformedPayloads) {
    const runtime = await runtimeWithMobList(payload);
    await assert.rejects(() => runtime.listMobs(), /Invalid mob\/list response/);
  }
});

test('Mob decoders reject missing typed status instead of fabricating defaults', async () => {
  const missingMobStatus = new Mob('mob-web-unit', {
    async mob_status() {
      return JSON.stringify({ mob_id: 'mob-web-unit', state: 'Running' });
    },
  });
  await assert.rejects(
    () => missingMobStatus.status(),
    /Invalid mob\/status response: missing status/,
  );

  const missingMemberStatus = new Mob('mob-web-unit', {
    async mob_member_status() {
      return JSON.stringify({ tokens_used: 0, is_final: false });
    },
  });
  await assert.rejects(
    () => missingMemberStatus.memberStatus('worker-1'),
    /Invalid mob member_status response: missing status/,
  );

  const missingRespawnStatus = new Mob('mob-web-unit', {
    async mob_respawn() {
      return JSON.stringify({
        receipt: {
          identity: 'worker-1',
          member_ref: 'ref-worker-1',
        },
      });
    },
  });
  await assert.rejects(
    () => missingRespawnStatus.respawn('worker-1'),
    /Invalid mob respawn response: missing status/,
  );

  const missingAppendStatus = new Mob('mob-web-unit', {
    async mob_append_system_context() {
      return JSON.stringify({
        mob_id: 'mob-web-unit',
        agent_identity: 'worker-1',
      });
    },
  });
  await assert.rejects(
    () =>
      missingAppendStatus.appendSystemContext('worker-1', {
        text: 'remember this',
      }),
    /Invalid mob append_system_context response: missing status/,
  );
});

test('Mob result decoders reject missing generated truth instead of fabricating success', async () => {
  const missingHandlingMode = new Mob('mob-web-unit', {
    async mob_member_send() {
      return JSON.stringify({
        mob_id: 'mob-web-unit',
        agent_identity: 'worker-1',
        member_ref: 'ref-worker-1',
      });
    },
  });
  await assert.rejects(
    () => missingHandlingMode.member('worker-1').send('hello', 'steer'),
    /Invalid mob member delivery response: missing handling_mode/,
  );

  const missingDeliveryMobId = new Mob('mob-web-unit', {
    async mob_member_send() {
      return JSON.stringify({
        agent_identity: 'worker-1',
        member_ref: 'ref-worker-1',
        handling_mode: 'queue',
      });
    },
  });
  await assert.rejects(
    () => missingDeliveryMobId.member('worker-1').send('hello'),
    /Invalid mob member delivery response: missing mob_id/,
  );

  const malformedHelperResults = [
    {
      method: 'spawnHelper',
      call: (mob) => mob.spawnHelper('summarize'),
      binding: 'mob_spawn_helper',
      response: { agent_identity: 'helper-1', member_ref: 'ref-helper-1' },
      pattern: /Invalid mob spawn_helper response: tokens_used must be number/,
    },
    {
      method: 'forkHelper',
      call: (mob) => mob.forkHelper('worker-1', 'review'),
      binding: 'mob_fork_helper',
      response: { agent_identity: 'fork-1', member_ref: 'ref-fork-1' },
      pattern: /Invalid mob fork_helper response: tokens_used must be number/,
    },
  ];

  for (const fixture of malformedHelperResults) {
    const mob = new Mob('mob-web-unit', {
      async [fixture.binding]() {
        return JSON.stringify(fixture.response);
      },
    });
    await assert.rejects(
      () => fixture.call(mob),
      fixture.pattern,
      `${fixture.method} must not fabricate tokens_used`,
    );
  }

  const malformedFlowStatus = new Mob('mob-web-unit', {
    async mob_flow_status() {
      return JSON.stringify({ state: 'legacy-success' });
    },
  });
  await assert.rejects(
    () => malformedFlowStatus.flowStatus('run-1'),
    /Invalid mob flow_status response: missing run/,
  );
});

test('Mob result decoders preserve generated result truth after validation', async () => {
  const deliveryMob = new Mob('mob-web-unit', {
    async mob_member_send() {
      return JSON.stringify({
        mob_id: 'mob-web-unit',
        agent_identity: 'worker-1',
        member_ref: 'ref-worker-1',
        handling_mode: 'steer',
      });
    },
    async mob_member_status() {
      return JSON.stringify({
        status: 'active',
        tokens_used: 3,
        is_final: false,
        current_session_id: 'session-1',
        // J58: realtime_attachment_status field removed from mob_member_status
        // wire shape as part of the live-adapter-mvp sweep.
        resolved_capabilities: {
          vision: false,
          image_input: false,
          image_tool_results: false,
          inline_video: false,
          realtime: true,
          web_search: false,
          image_generation: false,
        },
        external_member: { provider: 'external' },
      });
    },
    async mob_flow_status() {
      return JSON.stringify({
        run: {
          run_id: 'run-1',
          status: 'running',
          step_ledger: [],
        },
      });
    },
  });

  const receipt = await deliveryMob.member('worker-1').send('hello', 'steer');
  assert.deepEqual(receipt, {
    agent_identity: 'worker-1',
    member_ref: 'ref-worker-1',
    handling_mode: 'steer',
  });

  const snapshot = await deliveryMob.memberStatus('worker-1');
  assert.equal(snapshot.current_session_id, 'session-1');
  // J58: realtime_attachment_status assertion removed; field gone from the
  // wire shape with the live-adapter-mvp sweep.
  assert.equal(snapshot.resolved_capabilities.realtime, true);
  assert.deepEqual(snapshot.external_member, { provider: 'external' });

  const flowStatus = await deliveryMob.flowStatus('run-1');
  assert.equal(flowStatus.run_id, 'run-1');
  assert.equal(flowStatus.status, 'running');
  assert.deepEqual(flowStatus.step_ledger, []);
});

test('Mob.respawn rejects legacy receipt carriers instead of projecting success', async () => {
  const mob = new Mob('mob-web-unit', {
    async mob_respawn() {
      return JSON.stringify({
        status: 'completed',
        receipt: {
          agent_identity: 'worker-1',
          member_ref: 'ref-worker-1',
        },
      });
    },
  });

  await assert.rejects(
    () => mob.respawn('worker-1'),
    /Invalid mob respawn response: receipt missing identity/,
  );
});

test('Mob event decoders reject malformed envelopes instead of synthesizing unknown events', async () => {
  const memberMob = new Mob('mob-web-unit', {
    async mob_member_subscribe() {
      return 1;
    },
    poll_subscription() {
      return JSON.stringify([{ event: { type: 'text_delta', delta: 'legacy' } }]);
    },
    close_subscription() {},
  });
  const memberSubscription = await memberMob.member('worker-1').subscribe();
  assert.throws(
    () => memberSubscription.poll(),
    /Invalid mob member subscription event\[0\]: missing payload/,
  );

  const metadataLightMember = new Mob('mob-web-unit', {
    async mob_member_subscribe() {
      return 3;
    },
    poll_subscription() {
      return JSON.stringify([{ payload: { type: 'turn_completed' } }]);
    },
    close_subscription() {},
  });
  const metadataLightMemberSubscription = await metadataLightMember
    .member('worker-1')
    .subscribe();
  assert.throws(
    () => metadataLightMemberSubscription.poll(),
    /Invalid mob member subscription event\[0\]: missing event_id/,
  );

  const mobWide = new Mob('mob-web-unit', {
    async mob_subscribe_events() {
      return 2;
    },
    poll_subscription() {
      return JSON.stringify([
        {
          source: { identity: 'worker-runtime', generation: 1 },
          role: 'worker',
          envelope: {},
        },
      ]);
    },
    close_subscription() {},
  });
  const mobSubscription = await mobWide.subscribeEvents();
  assert.throws(
    () => mobSubscription.poll(),
    /Invalid mob attributed subscription event\[0\]: envelope: missing payload/,
  );

  const metadataLightMobWide = new Mob('mob-web-unit', {
    async mob_subscribe_events() {
      return 4;
    },
    poll_subscription() {
      return JSON.stringify([
        {
          source: { identity: 'worker-runtime', generation: 1 },
          role: 'worker',
          envelope: { payload: { type: 'turn_completed' } },
        },
      ]);
    },
    close_subscription() {},
  });
  const metadataLightMobSubscription = await metadataLightMobWide.subscribeEvents();
  assert.throws(
    () => metadataLightMobSubscription.poll(),
    /Invalid mob attributed subscription event\[0\]: envelope: missing event_id/,
  );

  const eventLogMob = new Mob('mob-web-unit', {
    async mob_events() {
      return JSON.stringify([
        {
          cursor: 1,
          timestamp: '2026-05-03T00:00:00Z',
          mob_id: 'mob-web-unit',
        },
      ]);
    },
  });
  await assert.rejects(
    () => eventLogMob.events('', 10),
    /Invalid mob\/events response\[0\]: missing kind/,
  );
});

test('Mob event decoders preserve generated payload envelopes', async () => {
  const memberMob = new Mob('mob-web-unit', {
    async mob_member_subscribe() {
      return 1;
    },
    poll_subscription() {
      return JSON.stringify([
        {
          event_id: 'evt-1',
          source: { type: 'session', session_id: 'session-typed-1' },
          source_id: 'session-1',
          seq: 1,
          timestamp_ms: 123,
          payload: { type: 'text_delta', delta: 'hello' },
        },
      ]);
    },
    close_subscription() {},
  });

  const memberSubscription = await memberMob.member('worker-1').subscribe();
  const [memberEvent] = memberSubscription.poll();

  assert.equal(memberEvent.payload.type, 'text_delta');
  assert.deepEqual(memberEvent.source, { type: 'session', session_id: 'session-typed-1' });
  assert.equal(memberEvent.source_id, 'session-1');
  assert.equal(memberEvent.seq, 1);

  const mobWide = new Mob('mob-web-unit', {
    async mob_subscribe_events() {
      return 2;
    },
    poll_subscription() {
      return JSON.stringify([
        {
          source: { identity: 'worker-1', generation: 7 },
          source_fence_token: 7,
          role: 'worker',
          envelope: {
            event_id: 'evt-2',
            source: { type: 'session', session_id: 'session-typed-2' },
            source_id: 'session-2',
            seq: 2,
            timestamp_ms: 456,
            payload: { type: 'turn_completed' },
          },
        },
      ]);
    },
    close_subscription() {},
  });

  const mobSubscription = await mobWide.subscribeEvents();
  const [mobEvent] = mobSubscription.poll();

  assert.deepEqual(mobEvent.source, { identity: 'worker-1', generation: 7 });
  assert.equal('agent_identity' in mobEvent, false);
  assert.equal(mobEvent.source_fence_token, 7);
  assert.deepEqual(mobEvent.envelope.source, {
    type: 'session',
    session_id: 'session-typed-2',
  });
  assert.equal(mobEvent.envelope.source_id, 'session-2');
  assert.equal(mobEvent.envelope.payload.type, 'turn_completed');
});
