import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { select, inputs } from '../dist/profiles.js';
import { Invocation } from '../dist/transport.js';

const CANARY = 'fixture-private-value-never-in-errors';
const definitions = [
  { profile: 'tally.get-form', auth: { apiKey: 'tally-key' }, args: { form_id: 'form_1' }, path: 'https://api.tally.so/forms/form_1', body: { id: 'form_1', enabled: false, name: null } },
  { profile: 'asana.get-task', auth: { accessToken: 'asana-token' }, args: { task: '123456' }, path: 'https://app.asana.com/api/1.0/tasks/123456', body: { data: { gid: '123456', completed: false } } },
  { profile: 'nocodb.get-record', auth: { origin: 'https://noco.example.invalid', apiToken: 'noco-token', version: 4 }, args: { baseId: 'base_1', tableId: 'table_1', recordId: 42 }, path: 'https://noco.example.invalid/api/v3/data/base_1/table_1/records/42', body: { id: 42, fields: { value: null } } },
];
const response = value => new Response(JSON.stringify(value));
const make = (definition, transport, extra = {}) => createReader({ profile: definition.profile, auth: definition.auth, transport, ...extra });
const safeError = (kind, extra = {}) => error => {
  assert.ok(error instanceof ActivepiecesError);
  assert.equal(error.kind, kind);
  assert.equal(error.writeOutcome, 'not_applicable');
  for (const [key, value] of Object.entries(extra)) assert.equal(error[key], value);
  assert.ok(!String(error).includes(CANARY));
  assert.ok(!JSON.stringify(error).includes(CANARY));
  assert.equal(error.cause, undefined);
  return true;
};
async function eventually(predicate) {
  const until = Date.now() + 3000;
  while (!predicate()) {
    if (Date.now() > until) throw new Error('Fixture condition timed out');
    await new Promise(resolve => setTimeout(resolve, 2));
  }
}

for (const definition of definitions) {
  test(`real donor action ${definition.profile} uses expected request and value`, async () => {
    let calls = 0;
    let borrowed;
    const reader = make(definition, async (url, init) => {
      calls++;
      const parsed = new URL(url);
      assert.equal(parsed.origin + parsed.pathname, definition.path);
      assert.equal(init.method, 'GET');
      assert.equal(init.redirect, 'error');
      assert.ok(init.signal instanceof AbortSignal);
      const headers = new Headers(init.headers);
      if (definition.profile === 'nocodb.get-record') assert.equal(headers.get('xc-token'), definition.auth.apiToken);
      else assert.equal(headers.get('authorization'), `Bearer ${definition.auth.apiKey ?? definition.auth.accessToken}`);
      if (definition.profile === 'asana.get-task') assert.match(parsed.searchParams.get('opt_fields'), /name/);
      else assert.equal(parsed.search, '');
      borrowed = response(definition.body);
      return borrowed;
    });
    const description = reader.describe();
    assert.equal(description.effect, 'read');
    assert.equal(description.inputSchema.additionalProperties, false);
    assert.ok(!JSON.stringify(description).includes('token'));
    description.inputSchema.properties = {};
    assert.notDeepEqual(reader.describe().inputSchema.properties, {});
    assert.deepEqual(await reader.invoke(definition.args), definition.profile === 'asana.get-task' ? definition.body.data : definition.body);
    assert.equal(calls, 1);
    assert.equal(borrowed.body.locked, false);
    assert.equal(borrowed.bodyUsed, true);
  });

  test(`real donor ${definition.profile} HTTP failures remain safe and are not retried`, async () => {
    for (const status of [401, 403, 429, 503]) {
      let calls = 0;
      let cancelled = false;
      const body = new ReadableStream({ cancel() { cancelled = true; } });
      const reader = make(definition, async () => { calls++; return new Response(body, { status }); });
      await assert.rejects(reader.invoke(definition.args), safeError('http_error', { entered: true, dispatched: true, status }));
      assert.equal(calls, 1);
      assert.equal(cancelled, true);
      assert.equal(body.locked, false);
    }
  });
}

test('configuration is explicit, bounded and rejects unsupported shapes', () => {
  const base = { profile: 'tally.get-form', auth: { apiKey: 'key' }, transport: async () => response({}) };
  for (const change of [
    { profile: 'anything' }, { profile: CANARY }, { extra: true }, { timeoutMs: null }, { timeoutMs: 0 },
    { timeoutMs: 60001 }, { maxResponseBytes: 0 }, { maxResponseBytes: 4194305 }, { transport: null },
    { auth: { apiKey: '' } }, { auth: { apiKey: 'key\nheader' } }, { auth: { apiKey: 'key', extra: CANARY } },
  ]) assert.throws(() => createReader({ ...base, ...change }), safeError('invalid_configuration', { entered: false, dispatched: false }));
  for (const origin of ['http://host.invalid', 'https://user:pass@host.invalid', 'https://host.invalid/',
    'https://host.invalid/path', 'https://host.invalid?x=1', 'https://host.invalid#x', 'https://HOST.invalid', 'https://host.invalid:443']) {
    assert.throws(() => createReader({ ...base, profile: 'nocodb.get-record', auth: { origin, apiToken: 'key', version: 3 } }), safeError('invalid_configuration'));
  }
});

test('invalid manual IDs, numbers and extra fields never enter transport', async () => {
  let calls = 0;
  const transport = async () => { calls++; return response({}); };
  const cases = [
    [definitions[0], [{ form_id: '../x' }, { form_id: 'a?b' }, { form_id: '%2f' }, { form_id: 'a'.repeat(129) }, { form_id: 'a', extra: CANARY }, {}]],
    [definitions[1], [{ task: '../x' }, { task: '123/4' }, { task: 123 }, { task: '1'.repeat(65) }]],
    [definitions[2], [{ ...definitions[2].args, baseId: '..' }, { ...definitions[2].args, tableId: 'x#y' },
      ...[0, -1, 1.5, '1', true, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1].map(recordId => ({ ...definitions[2].args, recordId }))]],
  ];
  for (const [definition, cases_] of cases) for (const args of cases_) {
    await assert.rejects(make(definition, transport).invoke(args), safeError('invalid_arguments', { entered: false, dispatched: false }));
  }
  const hostile = Object.defineProperty({}, 'form_id', { enumerable: true, get() { throw new Error(CANARY); } });
  await assert.rejects(make(definitions[0], transport).invoke(hostile), safeError('invalid_arguments'));
  assert.equal(calls, 0);
});

test('NocoDB version 3 keeps its actual donor endpoint', async () => {
  const definition = definitions[2];
  const reader = createReader({ profile: definition.profile, auth: { ...definition.auth, version: 3 }, transport: async url => {
    assert.equal(url, 'https://noco.example.invalid/api/v2/tables/table_1/records/42');
    return response({ Id: 42 });
  } });
  assert.deepEqual(await reader.invoke(definition.args), { Id: 42 });
});

test('per-invocation donor factories preserve simultaneous account transport and auth', async () => {
  const entered = [];
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const readers = ['account-a', 'account-b'].map(apiKey => createReader({
    profile: 'tally.get-form', auth: { apiKey }, transport: async (_url, init) => {
      assert.equal(new Headers(init.headers).get('authorization'), `Bearer ${apiKey}`);
      entered.push(apiKey);
      await gate;
      return response({ account: apiKey });
    },
  }));
  const results = readers.map(reader => reader.invoke({ form_id: 'form' }));
  await eventually(() => entered.length === 2);
  release();
  assert.deepEqual(await Promise.all(results), [{ account: 'account-a' }, { account: 'account-b' }]);
});

test('redirect status, followed redirect and changed response URL are refused and closed', async () => {
  for (const variant of ['status', 'followed', 'url']) {
    let closed = false;
    const stream = new ReadableStream({ cancel() { closed = true; } });
    const value = new Response(stream, { status: variant === 'status' ? 302 : 200 });
    if (variant === 'followed') Object.defineProperty(value, 'redirected', { value: true });
    if (variant === 'url') Object.defineProperty(value, 'url', { value: 'https://other.invalid/' });
    await assert.rejects(make(definitions[0], async () => value).invoke(definitions[0].args), safeError('redirect_refused', { dispatched: true }));
    assert.equal(closed, true);
    assert.equal(stream.locked, false);
  }
});

test('malformed JSON, invalid UTF-8, invalid donor envelope and transport exceptions are redacted', async () => {
  for (const value of [new Response(CANARY), new Response(new Uint8Array([0xff]))]) {
    await assert.rejects(make(definitions[0], async () => value).invoke(definitions[0].args), safeError('invalid_response'));
    assert.equal(value.body.locked, false);
  }
  await assert.rejects(make(definitions[1], async () => response({ wrong: CANARY })).invoke(definitions[1].args), safeError('invalid_response'));
  await assert.rejects(make(definitions[0], async () => { throw new Error(CANARY); }).invoke(definitions[0].args), safeError('transport_error'));
});

test('oversized response joins asynchronous cancellation before releasing ownership', async () => {
  let cancelStarted = false;
  let finishCancel;
  const stream = new ReadableStream({
    start(controller) { controller.enqueue(new TextEncoder().encode('x'.repeat(128))); },
    cancel() { cancelStarted = true; return new Promise(resolve => { finishCancel = resolve; }); },
  });
  let settled = false;
  const operation = make(definitions[0], async () => new Response(stream), { maxResponseBytes: 32 }).invoke(definitions[0].args);
  const assertion = assert.rejects(operation, safeError('response_too_large'));
  operation.then(() => { settled = true; }, () => { settled = true; });
  await eventually(() => cancelStarted);
  assert.equal(settled, false);
  finishCancel();
  await assertion;
  assert.equal(stream.locked, false);
});

test('cancellation after headers joins body cleanup, and another call remains independent', async () => {
  let headersReceived = false;
  let cancelStarted = false;
  let finishCancel;
  const stream = new ReadableStream({ cancel() { cancelStarted = true; return new Promise(resolve => { finishCancel = resolve; }); } });
  const controller = new AbortController();
  let count = 0;
  const reader = make(definitions[0], async (_url, init) => {
    count++;
    if (count === 1) { headersReceived = true; return new Response(stream); }
    assert.equal(init.signal.aborted, false);
    return response({ independent: true });
  });
  let settled = false;
  const first = reader.invoke(definitions[0].args, { signal: controller.signal });
  const assertion = assert.rejects(first, safeError('cancelled', { entered: true, dispatched: true }));
  first.then(() => { settled = true; }, () => { settled = true; });
  await eventually(() => headersReceived);
  controller.abort(CANARY);
  await eventually(() => cancelStarted);
  assert.equal(settled, false);
  assert.deepEqual(await reader.invoke(definitions[0].args), { independent: true });
  finishCancel();
  await assertion;
  assert.equal(stream.locked, false);
});

test('pre-cancel refuses dispatch; cancellation before headers joins a late borrowed response', async () => {
  const controller = new AbortController();
  controller.abort(CANARY);
  let entered = false;
  await assert.rejects(make(definitions[0], async () => { entered = true; return response({}); }).invoke(definitions[0].args, { signal: controller.signal }), safeError('cancelled', { entered: false, dispatched: false }));
  assert.equal(entered, false);
  const active = new AbortController();
  let giveResponse;
  let closed = false;
  let settled = false;
  const stream = new ReadableStream({ cancel() { closed = true; } });
  const call = make(definitions[0], async () => { entered = true; return new Promise(resolve => { giveResponse = resolve; }); }).invoke(definitions[0].args, { signal: active.signal });
  const assertion = assert.rejects(call, safeError('cancelled', { dispatched: true }));
  call.then(() => { settled = true; }, () => { settled = true; });
  await eventually(() => entered);
  active.abort();
  await new Promise(resolve => setTimeout(resolve, 5));
  assert.equal(settled, false);
  giveResponse(new Response(stream));
  await assertion;
  assert.equal(closed, true);
  assert.equal(stream.locked, false);
});

test('whole-call deadline propagates to the host without replay', async t => {
  let calls = 0;
  let entered;
  const arrival = new Promise(resolve => { entered = resolve; });
  const reader = make(definitions[0], async (_url, init) => {
    calls++;
    entered();
    return new Promise((_resolve, reject) => init.signal.addEventListener('abort', () => reject(new Error(CANARY)), { once: true }));
  }, { timeoutMs: 60_000 });
  t.mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const pending = reader.invoke(definitions[0].args);
    const rejected = assert.rejects(pending, safeError('deadline_exceeded', { dispatched: true }));
    await arrival;
    t.mock.timers.tick(60_000);
    await rejected;
    assert.equal(calls, 1);
  } finally { t.mock.timers.reset(); }
});

test('unexpected donor URLs, headers, methods and request fields refuse host dispatch', async () => {
  let calls = 0;
  for (const definition of definitions) {
    const selection = select({ profile: definition.profile, auth: definition.auth,
      transport: async () => { calls++; return response({}); } });
    const prepared = inputs(selection, definition.args);
    const request = { method: 'GET', url: definition.path,
      ...(definition.profile === 'nocodb.get-record' ? { headers: { 'xc-token': definition.auth.apiToken } } :
        { authentication: { type: 'BEARER_TOKEN', token: definition.auth.apiKey ?? definition.auth.accessToken } }) };
    const variants = [
      { ...request, method: 'POST' }, { ...request, url: 'https://other.invalid/' },
      { ...request, url: `${definition.path}?secret=${CANARY}` }, { ...request, body: CANARY },
      { ...request, timeout: 123 }, { ...request, headers: { 'xc-token': CANARY } },
      { ...request, authentication: { type: 'BEARER_TOKEN', token: CANARY } },
      { ...request, queryParams: { unknown: CANARY } },
      definition.profile === 'nocodb.get-record' ?
        { ...request, headers: { ...request.headers, extra: CANARY } } :
        { ...request, authentication: { ...request.authentication, extra: CANARY } },
    ];
    for (const value of variants) {
      const invocation = new Invocation(selection, prepared.pathname);
      invocation.entered = true;
      try { await assert.rejects(invocation.request(value), safeError('upstream_error', { entered: true, dispatched: false })); }
      finally { invocation.finish(); }
    }
  }
  assert.equal(calls, 0);
});

test('rejected body cancellation remains a safe failure and releases its reader', async () => {
  let started = false;
  let rejected = false;
  const stream = new ReadableStream({
    start(controller) { controller.enqueue(new Uint8Array(128)); },
    async cancel() {
      started = true;
      await new Promise(resolve => setTimeout(resolve, 5));
      rejected = true;
      throw new Error(CANARY);
    },
  });
  await assert.rejects(make(definitions[0], async () => new Response(stream), { maxResponseBytes: 32 })
    .invoke(definitions[0].args), safeError('response_too_large', { entered: true, dispatched: true }));
  assert.equal(started, true);
  assert.equal(rejected, true);
  assert.equal(stream.locked, false);
});
