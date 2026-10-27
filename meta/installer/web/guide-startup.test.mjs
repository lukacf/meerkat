import test from 'node:test';
import assert from 'node:assert/strict';
import { createGuideStartupRecovery, guideStartupObservation, guideStartupPresentation } from './guide-startup.mjs';

const failed = { status: 'failed', startup_retry_required: true, available: true };
const ready = { status: 'ready', startup_retry_required: false, available: true };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
function fixture(read) {
  let selected = 'a';
  const updates = [], calls = [], wakes = [];
  const controller = createGuideStartupRecovery({ guideId: 'a', currentGuideId: () => selected,
    request: async (url, options) => { calls.push({ url, options }); return read(url, options); },
    onUpdate: patch => updates.push(structuredClone(patch)), onRecovered: () => wakes.push(true) });
  return { controller, updates, calls, wakes, select: id => { selected = id; } };
}

test('only the typed failed startup latch exposes recovery for the selected guide', () => {
  for (const status of [{ status: 'failed' }, { status: 'failed', startup_retry_required: 'true' },
    { status: 'ready', startup_retry_required: true }, { status: 'failed', startup_retry_required: false }]) {
    const observation = guideStartupObservation(status);
    assert.notEqual(observation?.retryRequired, true);
  }
  const startup = guideStartupObservation({ ...failed, detail: 'private detail', proposal: { secret: true } });
  assert.deepEqual(startup, { status: 'failed', retryRequired: true, available: true });
  assert.equal(guideStartupPresentation({ id: 'a' }, { guideId: 'a', startup, error: '502' }), 'failed');
  assert.equal(guideStartupPresentation({ id: 'b' }, { guideId: 'a', startup, error: '502' }), null);
  assert.equal(guideStartupPresentation({ id: 'a' }, { guideId: 'a', error: 'Setup team failed' }), 'reconnecting');
});

test('observing a timeline failure performs only a status GET, and one deliberate retry sends only a body-less start POST', async () => {
  const pending = deferred();
  const f = fixture((url, options) => options?.method === 'POST' ? pending.promise : failed);
  assert.equal(await f.controller.retry(), false);
  await f.controller.observeFailure();
  assert.deepEqual(f.calls, [{ url: '/api/architect/status', options: { timeoutMs: 30000 } }]);
  const first = f.controller.retry();
  assert.equal(await f.controller.retry(), false);
  assert.deepEqual(f.calls[1], { url: '/api/architect/start', options: { method: 'POST' } });
  assert.equal(f.calls.filter(call => call.options?.method === 'POST').length, 1);
  assert.deepEqual(f.updates.at(-1), { startup: null, retrying: true, retryPending: true, retryError: '' });
  pending.resolve(ready);
  assert.equal(await first, true);
  assert.equal(f.updates.at(-1).retrying, false);
  assert.equal(f.wakes.length, 1);
});

test('a pre-retry delayed status read cannot restore a stale failed latch after a successful retry', async () => {
  const late = deferred(); let gets = 0;
  const f = fixture((url, options) => options?.method === 'POST' ? ready : ++gets === 1 ? failed : late.promise);
  await f.controller.observeFailure();
  const read = f.controller.observeFailure();
  await f.controller.retry();
  const accepted = structuredClone(f.updates);
  late.resolve(failed); await read;
  assert.deepEqual(f.updates, accepted);
  assert.equal(await f.controller.retry(), false);
});

test('lost POST response consults current host status before authorizing another retry', async () => {
  for (const observed of [ready, { status: 'starting', startup_retry_required: false }, null]) {
    let gets = 0;
    const f = fixture((url, options) => { if (options?.method === 'POST') throw new Error('response lost'); return ++gets === 1 ? failed : observed; });
    await f.controller.observeFailure();
    await f.controller.retry();
    assert.equal(f.calls.filter(call => call.url === '/api/architect/status').length, 2);
    assert.equal(await f.controller.retry(), false);
    assert.equal(f.calls.filter(call => call.options?.method === 'POST').length, 1);
    assert.notEqual(f.updates.at(-2).startup?.retryRequired, true);
  }
});

test('only a fresh failed latch after a rejected retry permits another deliberate attempt', async () => {
  let posts = 0;
  const f = fixture((url, options) => { if (options?.method === 'POST' && ++posts === 1) throw new Error('retry rejected'); return options?.method === 'POST' ? ready : failed; });
  await f.controller.observeFailure();
  assert.equal(await f.controller.retry(), false);
  assert.equal(f.updates.at(-2).startup.retryRequired, true);
  assert.equal(await f.controller.retry(), true);
  assert.equal(posts, 2);
});

test('disposing and returning to the same guide makes old status and retry results inert', async () => {
  for (const result of ['resolve', 'reject']) {
    const pending = deferred();
    const f = fixture((url, options) => options?.method === 'POST' ? pending.promise : failed);
    await f.controller.observeFailure();
    const retry = f.controller.retry();
    f.select('b'); f.controller.dispose(); f.select('a');
    const before = structuredClone(f.updates);
    result === 'resolve' ? pending.resolve(ready) : pending.reject(new Error('late failure'));
    assert.equal(await retry, false);
    assert.deepEqual(f.updates, before);
    assert.equal(f.wakes.length, 0);
    assert.equal(f.calls.length, 2);
  }
  const pending = deferred(), f = fixture(() => pending.promise);
  const read = f.controller.observeFailure();
  f.controller.dispose(); pending.resolve(failed); await read;
  assert.deepEqual(f.updates, []);
});

test('a successful actual timeline read consumes the recovery latch without touching source state or drafts', async () => {
  const f = fixture(() => failed);
  await f.controller.observeFailure();
  f.controller.timelineRecovered();
  assert.equal(await f.controller.retry(), false);
  assert.equal(f.calls.length, 1);
});

test('a checked failed status does not replay a still-entered original Start, and a later ready check fences its late failure', async () => {
  const pending = deferred(); let status = failed;
  const f = fixture((url, options) => options?.method === 'POST' ? pending.promise : status);
  await f.controller.observeFailure();
  const original = f.controller.retry();
  assert.equal(await f.controller.checkStatus(), false);
  assert.equal(f.updates.at(-1).retryPending, true);
  assert.equal(f.updates.at(-1).startup.retryRequired, true);
  assert.equal(await f.controller.retry(), false, 'A fresh failed observation cannot replace the original entered call.');
  assert.equal(f.calls.filter(call => call.options?.method === 'POST').length, 1);
  status = ready;
  assert.equal(await f.controller.checkStatus(), true);
  assert.equal(f.controller.isRetrying(), false);
  pending.reject(new Error('late original body failure'));
  assert.equal(await original, false);
  assert.equal(f.updates.filter(patch => patch.startup).at(-1).startup.status, 'ready');
  assert.equal(await f.controller.retry(), false);
  assert.equal(f.calls.filter(call => call.options?.method === 'POST').length, 1);
});

test('an unavailable status read leaves the original Start joined and another read reachable without any POST replay', async () => {
  const pending = deferred(); let reads = 0;
  const f = fixture((url, options) => {
    if (options?.method === 'POST') return pending.promise;
    if (++reads === 2) throw new Error('status response lost');
    return reads === 1 ? failed : ready;
  });
  await f.controller.observeFailure();
  const original = f.controller.retry();
  assert.equal(await f.controller.checkStatus(), false);
  assert.equal(f.controller.isRetrying(), true);
  assert.match(f.updates.at(-1).retryError, /could not be confirmed/);
  assert.equal(await f.controller.retry(), false);
  assert.equal(await f.controller.checkStatus(), true);
  pending.resolve(failed); await original;
  assert.equal(f.updates.filter(patch => patch.startup).at(-1).startup.status, 'ready');
  assert.equal(f.calls.filter(call => call.options?.method === 'POST').length, 1);
});

test('a disposed original guide rejects its late independent startup check without waking or changing the replacement guide', async () => {
  const pending = deferred();
  const f = fixture(() => pending.promise);
  const check = f.controller.checkStatus();
  assert.equal(await f.controller.checkStatus(), false, 'One status read owns the selected recovery at a time.');
  assert.equal(f.controller.isRetrying(), true, 'Timeline polling is paused while this newer read owns presentation.');
  f.select('b'); f.controller.dispose(); f.select('a');
  const updates = structuredClone(f.updates);
  pending.resolve(ready);
  assert.equal(await check, false);
  assert.deepEqual(f.updates, updates);
  assert.deepEqual(f.wakes, []);
  assert.equal(f.calls.length, 1);
});
