import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import { Mob } from '../dist/mob.js';
import { MeerkatRuntime } from '../dist/runtime.js';
import { Session, serializePromptContentInput } from '../dist/session.js';
import { isKnownEvent } from '../dist/types.js';

const WEB_PACKAGE_JSON_URL = new URL('../package.json', import.meta.url);
const { version: CURRENT_WASM_VERSION } = JSON.parse(
  await readFile(WEB_PACKAGE_JSON_URL, 'utf8'),
);

function makeSubscriptionRuntime(overrides = {}) {
  return {
    default: async () => undefined,
    runtime_version: () => CURRENT_WASM_VERSION,
    init_runtime_from_config: () => JSON.stringify({ status: 'initialized', model: 'claude-sonnet-4-5', providers: ['anthropic'] }),
    destroy_runtime: () => undefined,
    async mob_create(definitionJson) {
      return JSON.parse(definitionJson).id;
    },
    async mob_subscribe_events() {
      return 'stream-1';
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
      return 'stream-1';
    },
    async mob_subscribe_events() {
      return 'stream-2';
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
        return CURRENT_WASM_VERSION;
      },
      init_runtime_from_config() {
        return JSON.stringify({ status: 'initialized', model: 'claude-sonnet-4-5', providers: ['anthropic'] });
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
    assert.equal(handle, 'stream-1');
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
    assert.equal(handle, 'stream-2');
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

test('Mob.subscribeMemberEvents rejects envelopes with unknown payload event types', async () => {
  // parseEventPayload must classify the payload discriminant against the
  // generated AgentEvent inventory (isKnownEvent) — an unknown type is a
  // malformed/forward-incompatible wire shape, never blindly cast through.
  const envelope = canonicalEnvelope();
  envelope.payload = { type: 'totally_unknown_event', x: 1 };
  const mob = makeSubscriptionMob(() => JSON.stringify([envelope]));

  const subscription = await mob.subscribeMemberEvents('worker-1');
  assert.throws(
    () => subscription.poll(),
    /unknown event type "totally_unknown_event"/,
  );
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
    if (handle === 'stream-1') {
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
    assert.equal(handle, 'stream-1');
    return JSON.stringify([malformedSource]);
  });

  const subscription = await mob.subscribeMemberEvents('worker-1');
  assert.throws(() => subscription.poll(), /source missing session_id/);
});

test('Mob subscriptions drop legacy source_id strings instead of surfacing them', async () => {
  const envelope = canonicalEnvelope({
    source: {
      type: 'session',
      session_id: '00000000-0000-4000-8000-000000000001',
    },
    source_id: 'session:not-a-uuid',
  });
  const mob = makeSubscriptionMob((handle) => {
    assert.equal(handle, 'stream-1');
    return JSON.stringify([envelope]);
  });

  const subscription = await mob.subscribeMemberEvents('worker-1');
  const [item] = subscription.poll();

  assert.deepEqual(item.source, {
    type: 'session',
    session_id: '00000000-0000-4000-8000-000000000001',
  });
  // The legacy envelope-level string is not part of the typed envelope.
  assert.equal(item.source_id, undefined);
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
    if (handle === 'stream-1') {
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
    assert.equal(handle, 'stream-2');
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

test('Session direct polling fails closed on an unknown event type instead of casting blindly', () => {
  const session = makeDirectSession(() =>
    JSON.stringify([{ type: 'totally_unknown_event', payload: { x: 1 } }]),
  );

  assert.throws(() => session.pollEvents(), /unknown event type "totally_unknown_event"/);
  assert.throws(
    () => session.subscribe().poll(),
    /unknown event type "totally_unknown_event"/,
  );
});

test('Session direct polling surfaces a lag gap as the generated stream_truncated event', () => {
  // K19: the lag contract is the generated `stream_truncated` AgentEvent
  // (reason `stream_lagged`) — the hand `{type:'lagged'}` sentinel is gone,
  // and the legacy sentinel now fails event-type validation like any other
  // unknown discriminant.
  const truncated = {
    type: 'stream_truncated',
    reason: { kind: 'stream_lagged', dropped: 3 },
  };
  const session = makeDirectSession(() => JSON.stringify([truncated]));

  assert.deepEqual(session.pollEvents(), [truncated]);

  const legacy = makeDirectSession(() => JSON.stringify([{ type: 'lagged', skipped: 3 }]));
  assert.throws(() => legacy.pollEvents(), /unknown event type "lagged"/);
});

test('Session direct polling fails closed on a structurally invalid skills_resolved event', () => {
  // `skills_resolved` is a known discriminant, but the structural skill
  // guards inside isKnownEvent must still reject a payload missing the typed
  // `skills` array — proof the full record (not a synthetic `{ type }`) is
  // validated.
  const session = makeDirectSession(() => JSON.stringify([{ type: 'skills_resolved' }]));

  assert.throws(() => session.pollEvents(), /unknown event type "skills_resolved"/);
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
  assert.equal(
    Object.hasOwn(status, 'state'),
    false,
    'the deleted legacy state projection must not reappear',
  );
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

  const missingMemberRef = new Mob('mob-web-unit', {
    async mob_member_status() {
      // status present but member_ref absent: the runtime-owned handle is
      // required, so the SDK must fail closed rather than fabricate it.
      return JSON.stringify({ status: 'active', tokens_used: 0, is_final: false });
    },
  });
  await assert.rejects(
    () => missingMemberRef.memberStatus('worker-1'),
    /Invalid mob member_status response: missing member_ref/,
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
      call: (mob) => mob.spawnHelper('summarize', { agentIdentity: 'helper-1' }),
      binding: 'mob_spawn_helper',
      response: { agent_identity: 'helper-1', member_ref: 'ref-helper-1' },
      pattern: /Invalid mob spawn_helper response: tokens_used must be number/,
    },
    {
      method: 'forkHelper',
      call: (mob) => mob.forkHelper('worker-1', 'review', { agentIdentity: 'fork-1' }),
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
        // member_ref is runtime-owned and arrives in the payload; the SDK no
        // longer fabricates it client-side from {mobId, agentIdentity}.
        member_ref: 'runtime-owned-ref-worker-1',
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
  // member_ref must originate from the runtime payload, not be synthesized
  // client-side from {mobId, agentIdentity}.
  assert.equal(snapshot.member_ref, 'runtime-owned-ref-worker-1');
  // J58: realtime_attachment_status assertion removed; field gone from the
  // wire shape with the live-adapter-mvp sweep.
  assert.equal(snapshot.resolved_capabilities.realtime, true);
  assert.deepEqual(snapshot.external_member, { provider: 'external' });

  const flowStatus = await deliveryMob.flowStatus('run-1');
  assert.equal(flowStatus.run_id, 'run-1');
  assert.equal(flowStatus.status, 'running');
  assert.deepEqual(flowStatus.step_ledger, []);
});

test('memberStatus rejects malformed resolved_capabilities booleans instead of coercing', async () => {
  // A non-boolean capability flag must fail closed; the SDK must NOT coerce it
  // (e.g. Boolean("false") === true would silently flip the meaning).
  const malformedMob = new Mob('mob-web-unit', {
    async mob_member_status() {
      return JSON.stringify({
        status: 'active',
        member_ref: 'runtime-owned-ref-worker-1',
        tokens_used: 1,
        is_final: false,
        resolved_capabilities: {
          vision: false,
          image_input: false,
          image_tool_results: false,
          inline_video: false,
          realtime: 'false', // string, not boolean
          web_search: false,
          image_generation: false,
        },
      });
    },
  });
  await assert.rejects(
    () => malformedMob.memberStatus('worker-1'),
    /resolved_capabilities\.realtime must be boolean/,
  );

  // An absent capability flag is equally malformed against the domain shape
  // (all 7 flags are required) and must throw rather than default to false.
  const missingFlagMob = new Mob('mob-web-unit', {
    async mob_member_status() {
      return JSON.stringify({
        status: 'active',
        member_ref: 'runtime-owned-ref-worker-1',
        tokens_used: 1,
        is_final: false,
        resolved_capabilities: {
          vision: true,
          image_input: true,
          image_tool_results: true,
          inline_video: true,
          realtime: true,
          web_search: true,
          // image_generation omitted
        },
      });
    },
  });
  await assert.rejects(
    () => missingFlagMob.memberStatus('worker-1'),
    /resolved_capabilities\.image_generation must be boolean/,
  );

  // Sanity: a fully-typed capabilities block still parses and preserves truth.
  const validMob = new Mob('mob-web-unit', {
    async mob_member_status() {
      return JSON.stringify({
        status: 'active',
        member_ref: 'runtime-owned-ref-worker-1',
        tokens_used: 1,
        is_final: false,
        resolved_capabilities: {
          vision: true,
          image_input: false,
          image_tool_results: true,
          inline_video: false,
          realtime: true,
          web_search: false,
          image_generation: true,
        },
      });
    },
  });
  const snapshot = await validMob.memberStatus('worker-1');
  assert.deepEqual(snapshot.resolved_capabilities, {
    vision: true,
    image_input: false,
    image_tool_results: true,
    inline_video: false,
    realtime: true,
    web_search: false,
    image_generation: true,
  });
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
      return 'stream-1';
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
      return 'stream-3';
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
      return 'stream-2';
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
      return 'stream-4';
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
      return 'stream-1';
    },
    poll_subscription() {
      return JSON.stringify([
        {
          event_id: 'evt-1',
          source: { type: 'session', session_id: 'session-typed-1' },
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
  assert.equal(memberEvent.seq, 1);

  const mobWide = new Mob('mob-web-unit', {
    async mob_subscribe_events() {
      return 'stream-2';
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
  assert.equal(mobEvent.envelope.payload.type, 'turn_completed');
});

// ── Row #39 / K19: prompt content is emitted as the tagged content-input
// wire shape ({"text": ...} | {"blocks": [...]}) — the tag is the
// discriminator, never JSON shape-sniffing of a raw string. ──────────────────

test('serializePromptContentInput emits plain text as the tagged {text} shape', () => {
  assert.deepEqual(JSON.parse(serializePromptContentInput('hello world')), {
    text: 'hello world',
  });
});

test('serializePromptContentInput emits block content as the tagged {blocks} shape', () => {
  const blocks = [{ type: 'text', text: 'hi' }];
  assert.deepEqual(JSON.parse(serializePromptContentInput(blocks)), { blocks });
});

test('serializePromptContentInput keeps text that looks like block JSON as TEXT', () => {
  // K19 regression: a plain-text prompt whose body is itself valid
  // block-array JSON must stay tagged text — it can never be misread as
  // structured blocks by the WASM boundary.
  const sneaky = JSON.stringify([{ type: 'text', text: 'hi' }]);
  assert.deepEqual(JSON.parse(serializePromptContentInput(sneaky)), {
    text: sneaky,
  });
});

test('Session.turn forwards tagged block JSON to the WASM boundary', async () => {
  let forwardedPrompt;
  const session = new Session(
    7,
    async (_handle, prompt) => {
      forwardedPrompt = prompt;
      return JSON.stringify({ text: 'ok' });
    },
    () => JSON.stringify({ session_id: 'session-web-unit' }),
    () => undefined,
    () => '[]',
    async () => '{}',
  );

  const blocks = [
    { type: 'text', text: 'describe' },
    { type: 'image', media_type: 'image/png', data: 'QUJD' },
  ];
  await session.turn(blocks);
  // The Rust side deserializes the tagged shape strictly and fails closed.
  assert.deepEqual(JSON.parse(forwardedPrompt), { blocks });
});

// ── Row #215: destroyed/torn-down handles are classified by typed code ──────

test('Session.destroy is idempotent on a retired handle via the typed code', () => {
  let destroyAttempts = 0;
  // The runtime retires a destroyed handle and rejects a repeat destroy with
  // the typed invalid_session_handle envelope (a JSON string), like the real
  // WASM err_js output.
  const invalidHandleEnvelope = JSON.stringify({
    code: 'invalid_session_handle',
    message: 'unknown browser session handle: 7',
  });
  const session = new Session(
    7,
    async () => '{}',
    () => '{}',
    () => {
      destroyAttempts += 1;
      if (destroyAttempts > 1) {
        throw invalidHandleEnvelope;
      }
    },
    () => '[]',
    async () => '{}',
  );

  assert.doesNotThrow(() => session.destroy());
  // A repeat destroy of the retired handle is classified by the typed code
  // and treated as already-gone (idempotent), not re-thrown.
  assert.doesNotThrow(() => session.destroy());
  assert.equal(destroyAttempts, 2);
});

test('Session.destroy re-throws non-already-gone typed errors', () => {
  const busyEnvelope = JSON.stringify({
    code: 'SESSION_BUSY',
    message: 'session is busy',
  });
  const session = new Session(
    7,
    async () => '{}',
    () => '{}',
    () => {
      throw busyEnvelope;
    },
    () => '[]',
    async () => '{}',
  );
  assert.throws(() => session.destroy(), /SESSION_BUSY/);
});

test('Mob.listMembers rejects legacy profile aliases without canonical role', async () => {
  const mob = new Mob('mob-web-unit', {
    async mob_list_members() {
      return JSON.stringify([
        {
          agent_identity: 'worker-1',
          member_ref: 'ref-worker-1',
          profile_name: 'worker',
        },
      ]);
    },
  });

  await assert.rejects(() => mob.listMembers(), /missing role/);
});

test('Mob.listMembers rejects missing canonical agent_identity', async () => {
  const mob = new Mob('mob-web-unit', {
    async mob_list_members() {
      return JSON.stringify([
        {
          member_ref: 'ref-worker-1',
          role: 'worker',
        },
      ]);
    },
  });

  await assert.rejects(() => mob.listMembers(), /missing agent_identity/);
});

// Regression: the WASM helper deserializers (MobSpawnHelperOptions /
// MobForkHelperOptions) consume `role_name`, matching the canonical wire
// contract and the TS/Python SDKs. The web SDK previously serialized
// `profile_name`, which the WASM boundary silently dropped, so
// `spawnHelper`/`forkHelper` built helpers with the DEFAULT profile.
test('Mob.spawnHelper / forkHelper serialize canonical role_name and model_override', async () => {
  let spawnCaptured;
  let forkCaptured;
  const mob = new Mob('mob-web-unit', {
    async mob_spawn_helper(_mobId, json) {
      spawnCaptured = JSON.parse(json);
      return JSON.stringify({ agent_identity: 'h-1', member_ref: 'ref-h-1', tokens_used: 0 });
    },
    async mob_fork_helper(_mobId, json) {
      forkCaptured = JSON.parse(json);
      return JSON.stringify({ agent_identity: 'h-2', member_ref: 'ref-h-2', tokens_used: 0 });
    },
  });

  await mob.spawnHelper('do it', {
    agentIdentity: 'h-1',
    profileName: 'reviewer',
    modelOverride: 'gpt-5.6-sol',
  });
  await mob.forkHelper('src-1', 'do it', {
    agentIdentity: 'h-2',
    profileName: 'reviewer',
    modelOverride: 'claude-opus-4-8',
  });

  assert.equal(spawnCaptured.role_name, 'reviewer');
  assert.equal(spawnCaptured.profile_name, undefined);
  assert.equal(spawnCaptured.model_override, 'gpt-5.6-sol');
  assert.equal(forkCaptured.role_name, 'reviewer');
  assert.equal(forkCaptured.profile_name, undefined);
  assert.equal(forkCaptured.model_override, 'claude-opus-4-8');
});
