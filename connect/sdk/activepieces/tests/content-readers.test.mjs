import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { parseConfiguration, readerConfiguration } from '../dist/mcp-config.js';
import { Invocation } from '../dist/transport.js';
import { select, inputs } from '../dist/profiles.js';
import { KEY, FIXTURES } from './fixtures/content-profiles.mjs';

const reader = (f, transport, key = KEY) => createReader({ profile: f.profile, auth: f.auth(key), transport });
const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const failure = (kind, entered = true, dispatched = true) => error => {
  assert.ok(error instanceof ActivepiecesError);
  assert.equal(error.kind, kind); assert.equal(error.entered, entered); assert.equal(error.dispatched, dispatched);
  assert.ok(!String(error).includes(KEY)); return true;
};

for (const f of FIXTURES) {
  test(`${f.profile} executes its pinned action and preserves useful content`, async () => {
    let calls = 0;
    const run = reader(f, async (url, init) => {
      calls++; assert.equal(url, f.url); assert.equal(init.method, 'GET'); assert.equal(init.redirect, 'error');
      assert.ok(init.signal instanceof AbortSignal);
      const [name, value] = f.header(KEY); assert.equal(init.headers.get(name), value);
      assert.equal(init.body, undefined);
      return Response.json(f.wire);
    });
    assert.equal(run.describe().action, f.action);
    assert.equal(run.describe().effect, 'read');
    assert.deepEqual(await run.invoke(f.args), f.result); assert.equal(calls, 1);
  });

  test(`${f.profile} snapshots the configured account and does not borrow another invocation`, async () => {
    const gate = deferred(), active = deferred(), calls = [];
    const config = { profile: f.profile, auth: f.auth('first-account'), transport: async (url, init) => {
      calls.push([url, Object.fromEntries(init.headers)]); active.resolve(); await gate.promise; return Response.json(f.wire);
    } };
    const first = createReader(config);
    const second = reader(f, async (url, init) => { calls.push([url, Object.fromEntries(init.headers)]); return Response.json(f.wire); }, 'second-account');
    for (const key of ['apiKey', 'apiToken']) if (key in config.auth) config.auth[key] = 'changed-account';
    const pending = first.invoke(f.args); await active.promise;
    assert.deepEqual(await second.invoke(f.args), f.result); gate.resolve(); assert.deepEqual(await pending, f.result);
    for (const [index, key] of ['first-account', 'second-account'].entries()) {
      const [name, value] = f.header(key); assert.equal(calls[index][1][name], value);
    }
  });

  test(`${f.profile} rejects status/redirect errors, cancels error bodies and never retries`, async () => {
    for (const status of [301, 401, 403, 429, 503]) {
      let calls = 0, cancellations = 0;
      const run = reader(f, async () => {
        calls++;
        return new Response(new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode(KEY)); }, cancel() { cancellations++; } }),
          { status, headers: { location: 'https://unselected.invalid' } });
      });
      await assert.rejects(run.invoke(f.args), failure(status === 301 ? 'redirect_refused' : 'http_error'));
      assert.equal(calls, 1); assert.equal(cancellations, 1);
    }
  });
}

test('content profile setup accepts only its explicit region, origin and account fields', () => {
  for (const f of FIXTURES) {
    const config = { profile: f.profile, token_env: 'CONTENT_TOKEN', ...f.mcp };
    assert.equal(parseConfiguration(config).profile, f.profile);
    assert.equal(createReader(readerConfiguration(config, KEY, async () => { throw 0; })).describe().profile, f.profile);
    for (const value of [{ ...config, api_key: KEY }, { ...config, token_env: 'CONTENT_TOKEN\n' }, { ...config, region: 'arbitrary' }]) {
      assert.throws(() => parseConfiguration(value));
    }
  }
  const f = FIXTURES[0];
  for (const auth of [{ ...f.auth(KEY), origin: 'http://fixture.atlassian.net' },
    { ...f.auth(KEY), origin: 'https://fixture.atlassian.net/wiki' }, { ...f.auth(KEY), username: 'wrong:name' },
    { ...f.auth(KEY), username: 'someone\n' }]) {
    assert.throws(() => createReader({ profile: f.profile, auth, transport: async () => { throw 0; } }));
  }
  assert.throws(() => parseConfiguration({ profile: 'jotform.get-submission', token_env: 'CONTENT_TOKEN' }));
});

test('content input boundaries refuse before provider entry', async () => {
  for (const f of FIXTURES) {
    let calls = 0;
    const run = reader(f, async () => { calls++; return Response.json(f.wire); });
    const field = Object.keys(f.args)[0];
    const bad = [{}, { ...f.args, unknown: true }, { ...f.args, [field]: 123 },
      { ...f.args, [field]: '\ud800' }, { ...f.args, [field]: '123\n' }];
    if (f.profile === 'brevo.find-contact') bad.push({ identifier: '.' }, { identifier: '..' }, { identifier: 'a', identifier_type: 'unknown' });
    else bad.push({ ...f.args, [field]: '123/456' }, { ...f.args, [field]: '1'.repeat(65) });
    if (f.profile === 'confluence.get-page-content') bad.push({ ...f.args, includeDescendants: true });
    for (const value of bad) await assert.rejects(run.invoke(value), failure('invalid_arguments', false, false));
    assert.equal(calls, 0);
  }
});

test('Brevo preserves its donor-defined missing-contact outcome and joins body cleanup', async () => {
  const f = FIXTURES[1], cancelEntered = deferred(), release = deferred();
  let settled = false, cancelled = 0, calls = 0;
  const run = reader(f, async () => {
    calls++;
    return new Response(new ReadableStream({ cancel() { cancelled++; cancelEntered.resolve(); return release.promise; } }), { status: 404 });
  });
  const pending = run.invoke(f.args).finally(() => { settled = true; });
  await cancelEntered.promise; await tick(); assert.equal(settled, false);
  release.resolve(); assert.deepEqual(await pending, { found: false, data: {} });
  assert.equal(calls, 1); assert.equal(cancelled, 1);
  for (const other of [FIXTURES[0], FIXTURES[2]]) {
    await assert.rejects(reader(other, async () => new Response(null, { status: 404 })).invoke(other.args), failure('http_error'));
  }
});

test('cancellation during Brevo missing-contact cleanup cannot become successful completion', async () => {
  const f = FIXTURES[1], entered = deferred(), release = deferred(), controller = new AbortController();
  let settled = false;
  const run = reader(f, async () => new Response(new ReadableStream({ cancel() { entered.resolve(); return release.promise; } }), { status: 404 }));
  const pending = run.invoke(f.args, { signal: controller.signal }).finally(() => { settled = true; });
  const rejected = assert.rejects(pending, failure('cancelled'));
  await entered.promise; controller.abort(); await tick(); assert.equal(settled, false);
  assert.deepEqual(await reader(f, async () => Response.json(f.wire), 'other-account').invoke(f.args), f.result);
  release.resolve(); await rejected;
});

test('Brevo identifiers encode one Unicode path segment and exact declared type', async () => {
  const f = FIXTURES[1], identifier = 'x/🦊?%# +';
  let seen;
  const run = reader(f, async url => { seen = new URL(url); return Response.json(f.wire); });
  await run.invoke({ identifier, identifier_type: 'ext_id' });
  assert.equal(seen.pathname, `/v3/contacts/${encodeURIComponent(identifier)}`);
  assert.equal(seen.searchParams.get('identifierType'), 'ext_id');
  await assert.rejects(run.invoke({ identifier: '🦊'.repeat(257) }), failure('invalid_arguments', false, false));
  assert.deepEqual(await run.invoke({ identifier: '🦊'.repeat(256) }), f.result);
});

test('Jotform uses only the selected regional endpoint and refuses malformed envelopes', async () => {
  const f = FIXTURES[2];
  for (const [region, origin] of [['us', 'https://api.jotform.com'], ['eu', 'https://eu-api.jotform.com'], ['hipaa', 'https://hipaa-api.jotform.com']]) {
    const run = createReader({ profile: f.profile, auth: { ...f.auth(KEY), region }, transport: async url => {
      assert.equal(new URL(url).origin, origin); return Response.json(f.wire);
    } });
    assert.deepEqual(await run.invoke(f.args), f.result);
  }
  for (const body of [{ content: {} }, { responseCode: '200', content: {} }, { responseCode: 0, content: {} },
    { responseCode: 200, content: [] }, { responseCode: 200, content: null }]) {
    await assert.rejects(reader(f, async () => Response.json(body)).invoke(f.args), failure('invalid_response'));
  }
  await assert.rejects(reader(f, async () => Response.json({ responseCode: 403, message: KEY, content: {} })).invoke(f.args), failure('upstream_error'));
});

test('content donor seam refuses altered account, path and query before transport', async () => {
  for (const f of FIXTURES) {
    let calls = 0;
    const selection = select({ profile: f.profile, auth: f.auth(KEY), transport: async () => { calls++; return Response.json(f.wire); } });
    const prepared = inputs(selection, f.args);
    const inv = new Invocation(selection, prepared.pathname, undefined, prepared.query);
    try {
      await assert.rejects(inv.request({ method: 'GET', url: `${selection.origin}${prepared.pathname}`,
        authentication: { type: 'BEARER_TOKEN', token: 'other-account' }, queryParams: prepared.query }), failure('upstream_error', false, false));
      assert.equal(calls, 0);
    } finally { inv.finish(); }
  }
});
