import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { parseConfiguration, readerConfiguration } from '../dist/mcp-config.js';
import { Invocation } from '../dist/transport.js';
import { select, inputs } from '../dist/profiles.js';
import { KEY, serviceProfiles } from './fixtures/service-profiles.mjs';

const reader = (d, transport, key = KEY) => createReader({ profile: d.profile, auth: d.auth(key), transport });
const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const failure = (kind, entered = true, dispatched = true) => error => {
  assert.ok(error instanceof ActivepiecesError);
  assert.equal(error.kind, kind); assert.equal(error.entered, entered); assert.equal(error.dispatched, dispatched);
  assert.equal(error.writeOutcome, 'not_applicable'); assert.equal(error.cause, undefined);
  for (const secret of [KEY, encodeURIComponent(KEY), serviceProfiles[0].url(KEY)]) {
    assert.ok(!`${String(error)} ${JSON.stringify(error)} ${error.stack}`.includes(secret));
  }
  return true;
};

for (const d of serviceProfiles) {
  test(`${d.profile} executes its real action with exact account, request and useful result`, async () => {
    let calls = 0, response;
    const run = reader(d, async (url, init) => {
      calls++; assert.equal(url, d.url(KEY)); assert.equal(init.method, 'GET'); assert.equal(init.redirect, 'error');
      assert.ok(init.signal instanceof AbortSignal); assert.equal(init.body, undefined);
      assert.deepEqual(Object.fromEntries(init.headers), d.headers(KEY));
      return response = Response.json(d.body);
    });
    assert.equal(run.describe().action, d.action); assert.equal(run.describe().effect, 'read');
    assert.deepEqual(await run.invoke(d.args), d.result); assert.equal(calls, 1);
    assert.equal(response.body.locked, false); assert.equal(response.bodyUsed, true);
  });

  test(`${d.profile} detaches arguments and credentials while two accounts overlap`, async () => {
    const gate = deferred(), active = deferred(), calls = [];
    const firstConfig = { profile: d.profile, auth: d.auth('first-account'), transport: async (url, init) => {
      calls.push([url, Object.fromEntries(init.headers)]); active.resolve(); await gate.promise;
      return Response.json({ [d.envelope]: { ...d.result, account: 'first' } });
    } };
    const first = createReader(firstConfig);
    for (const key of Object.keys(firstConfig.auth)) firstConfig.auth[key] = 'changed-account';
    const args = { ...d.args }, pending = first.invoke(args);
    await active.promise; for (const key of Object.keys(args)) args[key] = 'changed-input';
    const second = reader(d, async (url, init) => {
      calls.push([url, Object.fromEntries(init.headers)]); return Response.json({ [d.envelope]: { ...d.result, account: 'second' } });
    }, 'second-account');
    assert.deepEqual(await second.invoke(d.args), { ...d.result, account: 'second' });
    gate.resolve(); assert.deepEqual(await pending, { ...d.result, account: 'first' });
    assert.deepEqual(calls, ['first-account', 'second-account'].map(key => [d.url(key), d.headers(key)]));
  });

  test(`${d.profile} joins held body cancellation and leaves another account usable`, async () => {
    const controller = new AbortController(), entered = deferred(), cancelling = deferred(), release = deferred();
    let settled = false, cancellations = 0, stream;
    const run = reader(d, async () => {
      stream = new ReadableStream({ cancel() { cancellations++; cancelling.resolve(); return release.promise; } });
      entered.resolve(); return new Response(stream);
    });
    const pending = run.invoke(d.args, { signal: controller.signal }).finally(() => { settled = true; });
    const rejected = assert.rejects(pending, failure('cancelled'));
    await entered.promise; controller.abort(); await cancelling.promise; await tick(); assert.equal(settled, false);
    assert.deepEqual(await reader(d, async () => Response.json(d.body), 'other-account').invoke(d.args), d.result);
    release.resolve(); await rejected; assert.equal(cancellations, 1); assert.equal(stream.locked, false);
  });

  test(`${d.profile} refuses redirects and error statuses without replay or leaked error bodies`, async () => {
    for (const status of [301, 302, 307, 401, 403, 404, 429, 503]) {
      let calls = 0, cancellations = 0;
      const stream = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode(d.url(KEY))); }, cancel() { cancellations++; } });
      const run = reader(d, async () => {
        calls++; return new Response(stream, { status, headers: { location: d.url('other-account') } });
      });
      await assert.rejects(run.invoke(d.args), error => {
        failure(status < 400 ? 'redirect_refused' : 'http_error')(error);
        assert.equal(error.status, status < 400 ? undefined : status); return true;
      });
      assert.equal(calls, 1); assert.equal(cancellations, 1); assert.equal(stream.locked, false);
    }
  });

  test(`${d.profile} requires the provider envelope and a record result`, async () => {
    for (const body of [null, [], {}, { arbitrary: 'value' }, { [d.envelope]: null }, { [d.envelope]: [] }, { [d.envelope]: false }]) {
      await assert.rejects(reader(d, async () => Response.json(body)).invoke(d.args), failure('invalid_response'));
    }
    await assert.rejects(reader(d, async () => new Response('not-json')).invoke(d.args), failure('invalid_response'));
    assert.deepEqual(await reader(d, async () => Response.json({ [d.envelope]: {} })).invoke(d.args), {});
  });

  test(`${d.profile} pre-abort and invalid arguments never reach the donor transport`, async () => {
    let calls = 0;
    const run = reader(d, async () => { calls++; return Response.json(d.body); });
    const controller = new AbortController(); controller.abort();
    await assert.rejects(run.invoke(d.args, { signal: controller.signal }), failure('cancelled', false, false));
    const key = Object.keys(d.args)[0];
    for (const value of [{}, { ...d.args, token: KEY }, { ...d.args, api_secret: KEY }, { ...d.args, [key]: 123 },
      { ...d.args, [key]: `${d.args[key]}\n` }, { ...d.args, [key]: '\ud800' }]) {
      await assert.rejects(run.invoke(value), failure('invalid_arguments', false, false));
    }
    assert.equal(calls, 0);
  });
}

test('service profiles accept only their named credential configuration', () => {
  for (const d of serviceProfiles) {
    const config = { profile: d.profile, token_env: 'SERVICE_TOKEN' };
    assert.deepEqual(parseConfiguration(config), config);
    assert.equal(createReader(readerConfiguration(config, KEY, async () => { throw 0; })).describe().profile, d.profile);
    for (const bad of [{ ...config, api_secret: KEY }, { ...config, origin: 'https://other.invalid' }, { ...config, region: 'us' }]) {
      assert.throws(() => parseConfiguration(bad));
    }
    for (const auth of [{ ...d.auth(KEY), accessToken: 'other' }, d.auth(''), d.auth('token\n')]) {
      assert.throws(() => createReader({ profile: d.profile, auth, transport: async () => { throw 0; } }));
    }
  }
});

test('Kit query secret cannot be replaced, moved into headers or retargeted by its donor seam', async () => {
  const d = serviceProfiles[0]; let calls = 0;
  const selection = select({ profile: d.profile, auth: d.auth(KEY), transport: async () => { calls++; return Response.json(d.body); } });
  const prepared = inputs(selection, d.args);
  const base = { method: 'GET', url: selection.origin + prepared.pathname, queryParams: { api_secret: KEY } };
  for (const changed of [
    { queryParams: { api_secret: 'other-account' } }, { queryParams: {} }, { queryParams: undefined },
    { queryParams: { api_secret: KEY, api_key: 'other-account' } }, { headers: { Authorization: `Bearer ${KEY}` } },
    { authentication: { type: 'BEARER_TOKEN', token: KEY } }, { url: d.url(KEY) },
    { url: `https://unselected.invalid${prepared.pathname}` }, { url: `${selection.origin}/v3/subscribers/456` },
  ]) {
    const inv = new Invocation(selection, prepared.pathname, undefined, prepared.query);
    try { await assert.rejects(inv.request({ ...base, ...changed }), failure('upstream_error', false, false)); }
    finally { inv.finish(); }
  }
  assert.equal(calls, 0);
});

test('Kit drops transport exceptions containing the complete credential-bearing URL', async () => {
  const d = serviceProfiles[0]; let calls = 0;
  const run = reader(d, async url => { calls++; assert.equal(url, d.url(KEY)); throw new Error(`Failed ${url}`, { cause: { secret: KEY } }); });
  await assert.rejects(run.invoke(d.args), failure('transport_error')); assert.equal(calls, 1);
});

test('service string schemas and runtime boundaries agree in both regular expression modes', () => {
  for (const d of serviceProfiles) {
    const config = { profile: d.profile, auth: d.auth(KEY), transport: async () => { throw 0; } };
    const property = Object.keys(d.args)[0], schema = createReader(config).describe().inputSchema.properties[property];
    const selected = select(config);
    for (const suffix of ['', '\n', '\r', '\r\n', '\u2028', '\u2029', '\u0000', '\\', '/', '%2F']) {
      const value = d.args[property] + suffix;
      for (const flags of ['', 'u']) assert.equal(new RegExp(schema.pattern, flags).test(value), suffix === '');
      if (suffix === '') assert.doesNotThrow(() => inputs(selected, { [property]: value }));
      else assert.throws(() => inputs(selected, { [property]: value }));
    }
    const length = d.profile === 'kit.get-subscriber' ? 64 : 128;
    const token = '1'.repeat(length);
    assert.doesNotThrow(() => inputs(selected, { [property]: token }));
    assert.throws(() => inputs(selected, { [property]: token + '1' }));
  }
});

test('Calendly accepts only a UUID token or the exact provider scheduled-event URI', async () => {
  const d = serviceProfiles[2]; let calls = 0;
  const run = reader(d, async url => { calls++; assert.equal(url, d.url(KEY)); return Response.json(d.body); });
  for (const value of ['event_123', d.args.scheduledEvent]) assert.deepEqual(await run.invoke({ scheduledEvent: value }), d.result);
  assert.equal(calls, 2);
  const invalid = [
    ' https://api.calendly.com/scheduled_events/event_123', 'https://api.calendly.com/scheduled_events/event_123 ',
    'https://api.calendly.com.evil.invalid/scheduled_events/event_123', 'https://api.calendly.com@evil.invalid/scheduled_events/event_123',
    'https://evil.invalid@api.calendly.com/scheduled_events/event_123', 'https://api.calendly.com:443/scheduled_events/event_123',
    'https://api.calendly.com./scheduled_events/event_123', 'https://API.CALENDLY.COM/scheduled_events/event_123',
    'http://api.calendly.com/scheduled_events/event_123', 'https://api.calendly.com/scheduled_events/event_123/',
    'https://api.calendly.com/scheduled_events/event_123?token=other', 'https://api.calendly.com/scheduled_events/event_123#fragment',
    'https://api.calendly.com//scheduled_events/event_123', 'https://api.calendly.com/other/../scheduled_events/event_123',
    'https://api.calendly.com/%73cheduled_events/event_123', 'https://api.calendly.com/scheduled_events/%65vent_123',
    'https://api.calendly.com/scheduled_events/event_123%2Fother', 'https://api.calendly.com/scheduled_events/..',
    'https://api.calendly.com/scheduled_events/event_123/other', 'https://api.calendly.com/scheduled_events/',
    'api.calendly.com/scheduled_events/event_123', '//api.calendly.com/scheduled_events/event_123', 'event_123/other',
    'https://api.calendly.com/scheduled_events/event_123\n',
  ];
  const schema = run.describe().inputSchema.properties.scheduledEvent;
  for (const value of invalid) {
    assert.equal(new RegExp(schema.pattern).test(value), false, value);
    await assert.rejects(run.invoke({ scheduledEvent: value }), failure('invalid_arguments', false, false));
  }
  assert.equal(calls, 2);
});

test('new readers retain bounded responses and join deadline-driven body cleanup', async () => {
  for (const d of serviceProfiles) {
    let cancellations = 0;
    const stream = new ReadableStream({ cancel() { cancellations++; } });
    const run = createReader({ profile: d.profile, auth: d.auth(KEY), timeoutMs: 250, transport: async () => new Response(stream) });
    await assert.rejects(run.invoke(d.args), failure('deadline_exceeded'));
    assert.equal(cancellations, 1); assert.equal(stream.locked, false);
    const bounded = createReader({ profile: d.profile, auth: d.auth(KEY), maxResponseBytes: 1024,
      transport: async () => Response.json({ [d.envelope]: { text: 'x'.repeat(1025) } }) });
    await assert.rejects(bounded.invoke(d.args), failure('response_too_large'));
  }
});
