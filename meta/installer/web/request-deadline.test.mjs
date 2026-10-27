import { test } from 'node:test';
import assert from 'node:assert/strict';
import { request, requestBlob } from './state.mjs';

test('package download deadlines cover both headers and blob-body completion', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const phase of ['headers', 'body']) {
    let signal, finish, bodyStarted = false;
    const stalled = new Promise(resolve => { finish = resolve; });
    t.mock.method(globalThis, 'fetch', async (_url, options) => {
      signal = options.signal;
      assert.equal(Object.hasOwn(options, 'timeoutMs'), false);
      if (phase === 'headers') return stalled;
      return { ok: true, blob: () => { bodyStarted = true; return stalled; } };
    });
    const pending = requestBlob('/application.zip', { timeoutMs: 60000 });
    const refused = assert.rejects(pending, error => error.code === 'read_timeout' && /Try again/.test(error.message));
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(bodyStarted, phase === 'body');
    t.mock.timers.tick(60000);
    await refused;
    assert.equal(signal.aborted, true);
    finish(phase === 'headers' ? { ok: true, blob: async () => new Blob(['late']) } : new Blob(['late']));
    await new Promise(resolve => setImmediate(resolve));
  }
});

test('package reads return the literal blob and preserve checked HTTP refusal details', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let signal;
  const artifact = new Blob([new Uint8Array([0, 1, 255, 13, 10])], { type: 'application/zip' });
  t.mock.method(globalThis, 'fetch', async (_url, options) => {
    signal = options.signal;
    return { ok: true, blob: async () => artifact, json: () => assert.fail('A package cannot be decoded as JSON') };
  });
  assert.equal(await requestBlob('/application.zip', { timeoutMs: 60000 }), artifact);
  t.mock.timers.tick(60000);
  assert.equal(signal.aborted, false);
  t.mock.method(globalThis, 'fetch', async () => ({ ok: false, status: 409,
    json: async () => ({ detail: 'The current build changed. Review it before downloading.', code: 'build_changed' }),
    blob: () => assert.fail('A refused response cannot become an application package') }));
  await assert.rejects(requestBlob('/application.zip', { timeoutMs: 60000 }), error =>
    error.status === 409 && error.code === 'build_changed' && /Review it/.test(error.message));
});

test('request deadlines preserve ordinary error details and caller cancellation', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let signal;
  t.mock.method(globalThis, 'fetch', async (_url, options) => {
    signal = options.signal;
    assert.equal(Object.hasOwn(options, 'timeoutMs'), false);
    return { ok: false, status: 409, json: async () => ({ detail: 'Keep the saved selection.', code: 'selection_changed' }) };
  });
  await assert.rejects(request('/status', { timeoutMs: 30000 }), error =>
    error.status === 409 && error.code === 'selection_changed' && error.message === 'Keep the saved selection.');
  t.mock.timers.tick(30000);
  assert.equal(signal.aborted, false, 'A settled response must clear its deadline');

  const caller = new AbortController();
  t.mock.method(globalThis, 'fetch', (_url, options) => {
    signal = options.signal;
    return new Promise((_, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true }));
  });
  const cancelled = request('/status', { timeoutMs: 30000, signal: caller.signal });
  const reason = new Error('Caller changed the selected setup.');
  caller.abort(reason);
  await assert.rejects(cancelled, error => error === reason);
  t.mock.timers.tick(30000);
  assert.equal(signal.reason, reason);
});

test('mutation requests without a deadline keep their body, headers and caller-owned lifetime', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const caller = new AbortController();
  let finish, settled = false;
  t.mock.method(globalThis, 'fetch', (_url, options) => {
    assert.equal(options.signal, caller.signal);
    assert.equal(options.method, 'POST');
    assert.deepEqual(options.headers, { 'Content-Type': 'application/json', 'X-Request-Owner': 'retained' });
    assert.equal(options.body, '{"draft":"Keep this exact idea."}');
    return new Promise(resolve => { finish = resolve; });
  });
  const pending = request('/build', { method: 'POST', signal: caller.signal,
    headers: { 'X-Request-Owner': 'retained' }, body: { draft: 'Keep this exact idea.' } })
    .then(result => { settled = true; return result; });
  t.mock.timers.tick(30000);
  await Promise.resolve();
  assert.equal(settled, false, 'A read deadline must not become an automatic build timeout');
  assert.equal(caller.signal.aborted, false);
  finish({ ok: true, json: async () => ({ accepted: true }) });
  assert.deepEqual(await pending, { accepted: true });
});

test('default JSON GET deadlines release held headers and bodies without rewriting caller options', async t => {
  for (const phase of ['headers', 'body']) for (const method of [undefined, 'GET', 'get'])
    await t.test(`${phase}:${method === undefined ? 'default' : method}`, async t => {
      t.mock.timers.enable({ apis: ['setTimeout'] });
      let finish, signal, failure, settled = false;
      const stalled = new Promise(resolve => { finish = resolve; });
      const headers = Object.freeze({ 'X-Read-Owner': 'retained' });
      const options = Object.freeze({ ...(method === undefined ? {} : { method }), headers });
      t.mock.method(globalThis, 'fetch', async (_url, actual) => {
        signal = actual.signal;
        assert.equal(actual.method, method, 'Classification must not rewrite the fetch method');
        assert.equal(actual.headers, headers);
        assert.equal(Object.hasOwn(actual, 'timeoutMs'), false);
        return phase === 'headers' ? stalled : { ok: true, json: () => stalled };
      });
      const pending = request('/ordinary-guide-read', options).then(value => {
        settled = true; return value;
      }, error => { settled = true; failure = error; });
      try {
        await new Promise(resolve => setImmediate(resolve));
        t.mock.timers.tick(29999);
        await Promise.resolve();
        assert.equal(settled, false);
        t.mock.timers.tick(1);
        await new Promise(resolve => setImmediate(resolve));
        assert.equal(failure?.code, 'read_timeout', 'An ordinary GET must release at its default deadline');
        assert.equal(signal?.aborted, true);
        assert.equal(Object.hasOwn(options, 'timeoutMs'), false, 'The caller object stays unchanged');
      } finally {
        finish(phase === 'headers' ? { ok: true, json: async () => ({ stale: true }) } : { stale: true });
        await pending;
      }
      assert.equal(failure?.code, 'read_timeout', 'A late header/body cannot replace the settled timeout');
    });
});

test('default JSON GET deadline respects explicit opt-outs and a caller-selected longer deadline', async t => {
  for (const selected of [0, undefined, 60000]) await t.test(String(selected), async t => {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const caller = new AbortController();
    let finish, signal, failure, settled = false;
    const options = Object.freeze({ method: 'get', timeoutMs: selected, signal: caller.signal });
    t.mock.method(globalThis, 'fetch', (_url, actual) => {
      signal = actual.signal;
      assert.equal(actual.method, 'get');
      assert.equal(Object.hasOwn(actual, 'timeoutMs'), false);
      return new Promise(resolve => { finish = resolve; });
    });
    const pending = request('/explicit-read', options).then(value => { settled = true; return value; },
      error => { settled = true; failure = error; });
    try {
      await Promise.resolve();
      t.mock.timers.tick(30000);
      await new Promise(resolve => setImmediate(resolve));
      assert.equal(settled, false, 'The GET default must not replace an explicit override');
      assert.equal(signal.aborted, false);
      if (selected !== 60000) assert.equal(signal, caller.signal, 'An explicit no-deadline read retains its caller signal');
    } finally {
      finish({ ok: true, json: async () => ({ retained: true }) });
      await pending;
    }
    assert.equal(failure, undefined);
    assert.deepEqual(await pending, { retained: true });
    t.mock.timers.tick(60000);
    assert.equal(signal.aborted, false, 'Settled explicit reads clear any selected deadline');
    assert.equal(options.timeoutMs, selected);
    assert.equal(options.signal, caller.signal);
  });
});

test('response-body admission retains exact current responses and refuses stale or cancelled reads', async t => {
  for (const wire of ['json-success', 'blob-success', 'refusal'])
    for (const admission of ['current', 'stale', 'deadline', 'caller-abort'])
      await t.test(`${wire}:${admission}`, async t => {
        t.mock.timers.enable({ apis: ['setTimeout'] });
        const caller = new AbortController();
        const literal = wire === 'blob-success' ? new Blob(['exact current package']) : { selected: true };
        let release, signal, admitted = 0, bodyReads = 0;
        const held = new Promise(resolve => { release = resolve; });
        const stillCurrent = async () => { admitted++; return held; };
        const options = Object.freeze({ timeoutMs: 60000, signal: caller.signal, stillCurrent });
        t.mock.method(globalThis, 'fetch', async (_url, actual) => {
          signal = actual.signal;
          assert.equal(Object.hasOwn(actual, 'stillCurrent'), false, 'Ownership is not a native fetch option');
          assert.equal(Object.hasOwn(actual, 'timeoutMs'), false);
          return { ok: wire !== 'refusal', status: wire === 'refusal' ? 409 : 200,
            json: async () => { bodyReads++; return wire === 'refusal'
              ? { detail: 'Keep the selected package.', code: 'selection_changed' } : literal; },
            blob: async () => { bodyReads++; return literal; } };
        });
        const read = wire === 'blob-success' ? requestBlob : request;
        const pending = read('/selected-package', options).then(value => ({ value }), error => ({ error }));
        try {
          await new Promise(resolve => setImmediate(resolve));
          assert.equal(admitted, 1);
          assert.equal(bodyReads, 0, 'No body starts while ownership admission is awaiting');
          let result;
          if (admission === 'deadline') {
            t.mock.timers.tick(60000);
            result = await pending;
            assert.equal(result.error?.code, 'read_timeout');
            assert.equal(signal.aborted, true);
            release(true);
          } else {
            const reason = new Error('The caller retired this selected package read.');
            if (admission === 'caller-abort') caller.abort(reason);
            release(admission !== 'stale');
            result = await pending;
            if (admission === 'caller-abort') assert.equal(result.error, reason);
            else if (admission === 'stale') assert.deepEqual(result, { value: undefined });
            else if (wire === 'refusal') {
              assert.equal(result.error?.status, 409);
              assert.equal(result.error?.code, 'selection_changed');
              assert.equal(result.error?.message, 'Keep the selected package.');
            } else assert.equal(result.value, literal);
          }
          await new Promise(resolve => setImmediate(resolve));
          assert.equal(bodyReads, admission === 'current' ? 1 : 0,
            'Late admission cannot start a body after its deadline or caller cancellation');
          assert.equal(options.stillCurrent, stillCurrent);
          assert.equal(options.signal, caller.signal);
        } finally {
          release(true);
          await pending;
        }
      });
});
