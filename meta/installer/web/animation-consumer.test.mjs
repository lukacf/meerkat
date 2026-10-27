import test from 'node:test';
import assert from 'node:assert/strict';
import {createAnimationConsumer, checkedAnimationIntentAck, withAnimationIntent, isStaleAnimationIntent} from './animation-consumer.mjs';

const token = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const scope = {setup_id: 'owned-guide', stream_id: 'owned-guide:overview'};
const ack = body => ({...body, intent_token: token, status: 'live', lease_seconds: 90, expires_at: '2030-01-01T00:00:00Z'});
function fixture(handler = async body => ack(body), initialRevision = 0) {
  const calls = [], timers = new Map(); let id = 0, unavailable = 0;
  const consumer = createAnimationConsumer({initialRevision, fetcher: async (url, options) => {
    const body = JSON.parse(options.body); calls.push({url, options, body});
    return {ok: true, json: async () => handler(body, url)};
  }, setTimer: (callback, delay) => {const timer = ++id; timers.set(timer, {callback, delay}); return timer;}, clearTimer: timer => timers.delete(timer)});
  return {consumer, calls, timers, declare: (next = scope) => consumer.declare(next, {onUnavailable: () => unavailable++}),
    unavailable: () => unavailable, tick: async delay => {
      const entry = [...timers].find(([, item]) => item.delay === delay); assert.ok(entry, `Existing ${delay}ms timer`);
      timers.delete(entry[0]); await entry[1].callback(); await new Promise(resolve => setImmediate(resolve));
    }};
}

test('a fresh viewer declares immediately and acknowledges only exact scope, without persisted browser identity', async () => {
  const a = fixture(), b = fixture();
  const first = a.declare(), second = b.declare();
  assert.equal(a.calls.length, 1);
  assert.notEqual(a.consumer.consumerId, b.consumer.consumerId);
  assert.deepEqual(Object.keys(a.calls[0].body).sort(), ['consumer_id', 'intent_revision', 'setup_id', 'stream_id']);
  const binding = await first.binding();
  assert.deepEqual(Object.keys(binding).sort(), ['consumer_id', 'intent_revision', 'intent_token', 'setup_id', 'stream_id']);
  assert.equal(binding.intent_revision, 1);
  for (const field of ['consumer_id', 'intent_revision', 'setup_id', 'stream_id', 'intent_token', 'status']) {
    assert.equal(checkedAnimationIntentAck(first.scope, {...ack(first.scope), [field]: 'foreign'}), false, field);
  }
  await second.binding(); first.release(); second.release();
  assert.equal(a.calls.at(-1).options.keepalive, true);
  assert.equal(Object.hasOwn(a.calls.at(-1).options, 'signal'), false);
});

test('cleanup before ack tombstones exact scope and a delayed old ack cannot control a newer revision', async () => {
  let resolveOld;
  const ui = fixture((body, url) => url.endsWith('/declare') && body.intent_revision === 1
    ? new Promise(resolve => {resolveOld = () => resolve(ack(body));}) : ack(body));
  const old = ui.declare(); old.release();
  const current = ui.declare({...scope, stream_id: 'owned-guide:workflow'});
  assert.equal(current.scope.intent_revision, 2);
  const release = ui.calls.find(call => call.url.endsWith('/release'));
  assert.deepEqual(release.body, old.scope);
  assert.equal(Object.hasOwn(release.body, 'intent_token'), false);
  await current.binding(); resolveOld(); await new Promise(resolve => setImmediate(resolve));
  await assert.rejects(old.binding());
  assert.equal(old.active, false);
  assert.equal(current.status, 'live');
  assert.equal(ui.unavailable(), 0);
  old.release(); assert.equal(ui.calls.filter(call => call.url.endsWith('/release')).length, 1);
  current.release();
});

test('unknown declaration retries reuse the identical scope and never mint another intent', async () => {
  let attempts = 0;
  const ui = fixture((body, url) => {if (url.endsWith('/declare') && !attempts++) throw new Error('Lost transport'); return ack(body);});
  const lease = ui.declare(); await new Promise(resolve => setImmediate(resolve));
  await ui.tick(2000);
  await lease.binding();
  assert.deepEqual(ui.calls[0].body, ui.calls[1].body);
  assert.equal(ui.calls[0].body.intent_revision, 1);
  lease.release();
});

test('a declaration that never responds is withdrawn before a delayed ack can activate it', async () => {
  let resolveDeclare;
  const ui = fixture((body, url) => url.endsWith('/declare')
    ? new Promise(resolve => {resolveDeclare = () => resolve(ack(body));}) : ack(body));
  const lease = ui.declare();
  await ui.tick(90000);
  await assert.rejects(lease.binding());
  assert.equal(lease.status, 'unavailable'); assert.equal(lease.active, false);
  assert.equal(ui.unavailable(), 1); assert.equal(ui.timers.size, 0);
  const release = ui.calls.find(call => call.url.endsWith('/release'));
  assert.deepEqual(release.body, lease.scope);
  assert.equal(release.options.keepalive, true);
  assert.equal(Object.hasOwn(release.options, 'signal'), false);
  resolveDeclare(); await new Promise(resolve => setImmediate(resolve));
  assert.equal(lease.active, false); assert.equal(ui.timers.size, 0);
  lease.release();
  assert.equal(ui.calls.filter(call => call.url.endsWith('/release')).length, 1);
});

test('renewal keeps the exact token and a repeated unknown renewal cannot resurrect the intent', async () => {
  let fail = false;
  const ui = fixture((body, url) => {if (fail && url.endsWith('/renew')) throw new Error('Lost renewal'); return ack(body);});
  const lease = ui.declare(), binding = await lease.binding();
  await ui.tick(30000);
  assert.deepEqual(ui.calls.find(call => call.url.endsWith('/renew')).body, binding);
  fail = true; await ui.tick(30000); await ui.tick(2000);
  assert.equal(lease.status, 'unavailable');
  assert.equal(ui.unavailable(), 1);
  await assert.rejects(lease.binding());
  assert.equal(ui.calls.filter(call => call.url.endsWith('/declare')).length, 1);
  assert.equal(ui.timers.size, 0);
  assert.deepEqual(ui.calls.find(call => call.url.endsWith('/release')).body, binding);
  lease.release();
  assert.equal(ui.calls.filter(call => call.url.endsWith('/release')).length, 1);
});

test('expiry wins over a delayed renewal acknowledgment and fresh interest requires an explicit new revision', async () => {
  let resolveRenew;
  const ui = fixture((body, url) => url.endsWith('/renew') ? new Promise(resolve => {resolveRenew = () => resolve(ack(body));}) : ack(body));
  const old = ui.declare(); await old.binding();
  const renewal = ui.tick(30000); await new Promise(resolve => setImmediate(resolve));
  await ui.tick(90000); resolveRenew(); await renewal;
  assert.equal(old.status, 'stale'); assert.equal(old.active, false); assert.equal(ui.unavailable(), 1);
  await assert.rejects(old.binding());
  const current = ui.declare(); await current.binding();
  assert.equal(current.scope.intent_revision, 2);
  assert.equal(current.active, true);
  current.release();
});

test('a declaration ack already expired on arrival cannot extend its lease from response time', async () => {
  const ui = fixture(body => ({...ack(body), expires_at: '2000-01-01T00:00:00Z'}));
  const lease = ui.declare(); await assert.rejects(lease.binding());
  assert.equal(lease.status, 'stale'); assert.equal(lease.active, false);
  assert.equal(ui.unavailable(), 1); assert.equal(ui.timers.size, 0);
  assert.equal(ui.calls.filter(call => call.url.endsWith('/declare')).length, 1);
  lease.release();
});

test('the last JavaScript-safe revision is usable but exhaustion cannot wrap or send a new declaration', async () => {
  const ui = fixture(undefined, Number.MAX_SAFE_INTEGER - 1), last = ui.declare(); await last.binding();
  assert.equal(last.scope.intent_revision, Number.MAX_SAFE_INTEGER);
  const before = ui.calls.length;
  assert.throws(() => ui.declare());
  assert.equal(ui.calls.length, before);
  last.release();
});

test('attaching a new token preserves the exact captured factual and correction body and excludes ack metadata', () => {
  const factual = {blueprint: {domain: 'exact'}, model: {nodes: []}, feedback: 'Fine links', stream_id: 'owned-guide:overview', setup_id: 'owned-guide',
    expected_parent: {operation_id: '1'.repeat(64)}, review_id: '2'.repeat(64), consumer_intent: {intent_token: 'old'}};
  const before = structuredClone(factual), core = {...scope, consumer_id: crypto.randomUUID(), intent_revision: 2, intent_token: token};
  const body = withAnimationIntent(factual, {...core, status: 'live', lease_seconds: 90, expires_at: 'metadata'});
  assert.deepEqual(body.consumer_intent, core);
  const {consumer_intent: ignored, ...saved} = before;
  const {consumer_intent: attached, ...actual} = body;
  assert.deepEqual(actual, saved);
  assert.deepEqual(factual, before);
  body.expected_parent.operation_id = 'changed'; assert.deepEqual(factual, before);
  assert.equal(isStaleAnimationIntent({status: 409}, {code: 'stale_visualization_intent'}), true);
  assert.equal(isStaleAnimationIntent({status: 409}, {detail: 'Correction conflict'}), false);
  assert.equal(isStaleAnimationIntent({status: 404}, {code: 'stale_visualization_intent'}), false);
});
