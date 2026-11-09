import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { parseConfiguration, readerConfiguration } from '../dist/mcp-config.js';
import { Invocation } from '../dist/transport.js';
import { inputs, select } from '../dist/profiles.js';
import { KEY, businessProfiles } from './fixtures/business-profiles.mjs';
const reader = (d, transport, key = KEY, options = {}) => createReader({ profile: d.profile, auth: d.auth(key), transport, ...options });
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const tick = () => new Promise(resolve => setImmediate(resolve));
const failure = (kind, entered = true, dispatched = true) => error => {
  assert.ok(error instanceof ActivepiecesError); assert.equal(error.kind, kind);
  assert.equal(error.entered, entered); assert.equal(error.dispatched, dispatched);
  assert.equal(error.writeOutcome, 'not_applicable'); assert.equal(error.cause, undefined);
  for (const secret of [KEY, encodeURIComponent(KEY), Buffer.from(`${KEY}:x`).toString('base64')]) {
    assert.ok(!`${error} ${JSON.stringify(error)} ${error.stack}`.includes(secret));
  }
  return true;
};
for (const d of businessProfiles) {
  test(`${d.profile} executes unchanged action with exact request and useful output`, async () => {
    let calls = 0, response;
    const run = reader(d, async (url, init) => {
      calls++; assert.equal(url, d.url(KEY)); assert.equal(init.method, 'GET'); assert.equal(init.redirect, 'error');
      assert.equal(init.body, undefined); assert.ok(init.signal instanceof AbortSignal);
      assert.deepEqual(Object.fromEntries(init.headers), d.headers(KEY)); return response = Response.json(d.body);
    });
    assert.equal(run.describe().action, d.action); assert.equal(run.describe().effect, 'read');
    assert.deepEqual(await run.invoke(d.args), d.result); assert.equal(calls, 1);
    assert.equal(response.body.locked, false); assert.equal(response.bodyUsed, true);
  });
  test(`${d.profile} keeps concurrent accounts and detached inputs independent`, async () => {
    const gate = deferred(), entered = deferred(), calls = [];
    const config = { profile: d.profile, auth: d.auth('account-a'), transport: async (url, init) => {
      calls.push([url, Object.fromEntries(init.headers)]); entered.resolve(); await gate.promise; return Response.json(d.accountBody('a'));
    } };
    const a = createReader(config); Object.assign(config.auth, d.auth('mutated-token'));
    const args = structuredClone(d.args), pending = a.invoke(args);
    await entered.promise; for (const key of Object.keys(args)) args[key] = 'mutated-input';
    const b = reader(d, async (url, init) => { calls.push([url, Object.fromEntries(init.headers)]); return Response.json(d.accountBody('b')); }, 'account-b');
    assert.deepEqual(await b.invoke(d.args), d.accountResult('b')); gate.resolve(); assert.deepEqual(await pending, d.accountResult('a'));
    assert.deepEqual(calls, ['account-a', 'account-b'].map(key => [d.url(key), d.headers(key)]));
  });
  test(`${d.profile} joins body cancellation before same reader recovery`, async () => {
    const arrived = deferred(), cancelling = deferred(), release = deferred(), controller = new AbortController();
    let calls = 0, settled = false, cancellations = 0, body;
    const run = reader(d, async () => {
      if (++calls > 1) return Response.json(d.body);
      body = new ReadableStream({ cancel() { cancellations++; cancelling.resolve(); return release.promise; } });
      arrived.resolve(); return new Response(body);
    });
    const pending = run.invoke(d.args, { signal: controller.signal }).finally(() => { settled = true; });
    const rejected = assert.rejects(pending, failure('cancelled'));
    await arrived.promise; controller.abort(); await cancelling.promise; await tick(); assert.equal(settled, false);
    release.resolve(); await rejected; assert.equal(body.locked, false); assert.equal(cancellations, 1);
    assert.deepEqual(await run.invoke(d.args), d.result);
  });
  test(`${d.profile} refuses redirect and provider errors and releases unread bodies`, async () => {
    for (const status of [301, 302, 307, 401, 403, 404, 429, 503]) {
      let calls = 0, cancellations = 0;
      const body = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode(KEY)); }, cancel() { cancellations++; } });
      const run = reader(d, async () => { calls++; return new Response(body, { status, headers: { location: 'https://unselected.invalid' } }); });
      if (d.profile === 'campaign-monitor.find-subscriber' && status === 404) {
        assert.deepEqual(await run.invoke(d.args), { found: false, result: null });
      } else await assert.rejects(run.invoke(d.args), error => {
        failure(status < 400 ? 'redirect_refused' : 'http_error')(error);
        assert.equal(error.status, status < 400 ? undefined : status); return true;
      });
      assert.equal(calls, 1); assert.equal(cancellations, 1); assert.equal(body.locked, false);
    }
  });
  test(`${d.profile} rejects malformed provider shape without a false miss`, async () => {
    for (const body of d.invalidBodies) await assert.rejects(reader(d, async () => Response.json(body)).invoke(d.args), failure('invalid_response'));
    await assert.rejects(reader(d, async () => new Response('bad-json')).invoke(d.args), failure('invalid_response'));
    await assert.rejects(reader(d, async () => new Response(new Uint8Array([255]))).invoke(d.args), failure('invalid_response'));
  });
  test(`${d.profile} pre-abort and unknown arguments never enter a donor`, async () => {
    let calls = 0; const run = reader(d, async () => { calls++; return Response.json(d.body); });
    const controller = new AbortController(); controller.abort();
    await assert.rejects(run.invoke(d.args, { signal: controller.signal }), failure('cancelled', false, false));
    for (const args of [{}, { ...d.args, token: KEY }, { ...d.args, origin: 'https://unselected.invalid' }]) {
      await assert.rejects(run.invoke(args), failure('invalid_arguments', false, false));
    }
    assert.equal(calls, 0);
  });
  test(`${d.profile} bounds response bytes and owns deadline cleanup`, async t => {
    const arrived = deferred();
    let cancellations = 0;
    const body = new ReadableStream({ cancel() { cancellations++; } });
    t.mock.timers.enable({ apis: ['setTimeout'] });
    try {
      // This case targets response cleanup, so expire only after dispatch.
      // A short wall-clock budget can instead expire during cold donor setup.
      const pending = reader(d, async () => {
        arrived.resolve();
        return new Response(body);
      }, KEY, { timeoutMs: 60_000 }).invoke(d.args);
      const rejected = assert.rejects(pending, failure('deadline_exceeded'));
      await arrived.promise;
      await tick();
      assert.equal(body.locked, true);
      t.mock.timers.tick(60_000);
      await rejected;
      assert.equal(cancellations, 1); assert.equal(body.locked, false);
    } finally { t.mock.timers.reset(); }
    await assert.rejects(reader(d, async () => new Response('x'.repeat(1025)), KEY, { maxResponseBytes: 1024 }).invoke(d.args), failure('response_too_large'));
    await assert.rejects(reader(d, async () => { throw new Error(`request failed ${KEY}`); }).invoke(d.args), failure('transport_error'));
  });
}

test('business profiles select explicit credentials and only Okta permits a selected origin', () => {
  for (const d of businessProfiles) {
    const config = { profile: d.profile, token_env: 'SELECTED_TOKEN', ...d.config };
    assert.deepEqual(parseConfiguration(config), config);
    assert.equal(createReader(readerConfiguration(config, KEY, async () => { throw 0; })).describe().profile, d.profile);
    for (const invalid of [{ ...config, access_token: KEY }, { ...config, region: 'us' }, { ...config, username: 'unselected' }]) assert.throws(() => parseConfiguration(invalid));
    for (const auth of [d.auth(''), d.auth('token\n'), { ...d.auth(KEY), otherToken: KEY }]) assert.throws(() => reader(d, async () => { throw 0; }, KEY, { auth }));
    if (d.profile !== 'okta.find-user-by-email') assert.throws(() => parseConfiguration({ ...config, origin: 'https://unselected.invalid' }));
  }
  const okta = businessProfiles[2], campaign = businessProfiles[1];
  for (const origin of [undefined, 'http://selected.invalid', 'https://selected.invalid/', 'https://selected.invalid/path', 'https://a@selected.invalid', 'https://selected.invalid?x=1']) {
    assert.throws(() => reader(okta, async () => { throw 0; }, KEY, { auth: { ...okta.auth(KEY), origin } }));
  }
  assert.throws(() => reader(campaign, async () => { throw 0; }, 'api:key'));
});

test('Okta serializes literal dot-atom email symbols without admitting quoted SCIM expressions', async () => {
  const d = businessProfiles[2]; let calls = 0;
  for (const email of ['a+tag@example.com', "a!#$%&'*+/=?^_`{|}~-@Example.COM", 'a.b@example.com']) {
    const run = reader(d, async url => {
      calls++; const parsed = new URL(url); assert.deepEqual([...parsed.searchParams], [['search', `profile.email eq "${email}"`]]);
      assert.equal(parsed.hash, ''); return Response.json([{ profile: { email } }]);
    });
    assert.deepEqual(await run.invoke({ email }), { profile: { email } });
  }
  for (const email of ['a" or status eq "ACTIVE@example.com', 'a\\b@example.com', '"alice"@example.com', '\ud800@example.com',
    'ä@example.com', 'a..b@example.com', '.a@example.com', 'a.@example.com', 'a@example.com\n', 'a@example.com\u2028', 'a@localhost']) {
    await assert.rejects(reader(d, async () => { calls++; throw 0; }).invoke({ email }), failure('invalid_arguments', false, false));
  }
  assert.equal(calls, 3);
});

test('Okta empty, multiple matches, mismatched emails and malformed lists have distinct outcomes', async () => {
  const d = businessProfiles[2];
  assert.deepEqual(await reader(d, async () => Response.json([])).invoke(d.args), { success: false, message: `No user found with email: ${d.args.email}`, data: [] });
  const first = { ...d.result, profile: { ...d.result.profile, email: d.args.email.toUpperCase() } };
  const second = { ...d.result, id: 'second-match' };
  assert.deepEqual(await reader(d, async () => Response.json([first, second])).invoke(d.args), first);
  for (const body of [[first, { ...second, profile: { email: 'unrelated@example.com' } }], new Array(201).fill(first), [first, 1], { length: 1, 0: first }]) {
    await assert.rejects(reader(d, async () => Response.json(body)).invoke(d.args), failure('invalid_response'));
  }
});

test('Campaign Monitor retains repeated and empty field transformation but rejects inherited keys before donor catch-all', async () => {
  const d = businessProfiles[1];
  assert.deepEqual(await reader(d, async () => Response.json(d.body)).invoke(d.args), d.result);
  for (const key of Object.getOwnPropertyNames(Object.prototype)) {
    const body = { ...d.body, CustomFields: [{ Key: key, Value: 'value' }] };
    await assert.rejects(reader(d, async () => Response.json(body)).invoke(d.args), failure('invalid_response'));
  }
  const body = { ...d.body, CustomFields: new Array(101).fill({ Key: 'field', Value: 'value' }) };
  await assert.rejects(reader(d, async () => Response.json(body)).invoke(d.args), failure('invalid_response'));
  assert.equal({}.polluted, undefined);
});

test('Campaign Monitor 404 cleanup failure remains an error and the reader can recover', async () => {
  const d = businessProfiles[1]; let calls = 0;
  const body = new ReadableStream({ cancel() { return Promise.reject(new Error(KEY)); } });
  const run = reader(d, async () => ++calls === 1 ? new Response(body, { status: 404 }) : Response.json(d.body));
  await assert.rejects(run.invoke(d.args), failure('transport_error'));
  assert.deepEqual(await run.invoke(d.args), d.result); assert.equal(body.locked, false);
});

test('Help Scout separates literal structured fields and explicit search expression', async () => {
  const d = businessProfiles[3]; let calls = 0;
  const run = reader(d, async url => {
    calls++; const query = new URL(url).searchParams;
    assert.equal(query.get('query'), '(subject:"Export + billing & \u{1f600}" AND status:"active")');
    assert.equal(query.get('tags'), 'one,two'); return Response.json({ _embedded: { conversations: [] } });
  });
  assert.deepEqual(await run.invoke({ subject: 'Export + billing & \u{1f600}', query: 'status:"active"', tags: ['one', 'two'] }), { found: false, data: {} });
  for (const args of [{ subject: '" OR status:open' }, { subject: 'x\\y' }, { subject: '' }, { subject: '   ' }, { subject: '\ud800' },
    { query: '' }, { query: ' \u2003 ' }, { tags: [] }, { tags: ['one', 'one'] }, { tags: ['one,two'] }, { tags: [''] }, { tags: new Array(21).fill('a') },
    { mailboxId: '0' }, { mailboxId: '001' }, { assignTo: 1 }, { status: 'unknown' }]) {
    await assert.rejects(run.invoke(args), failure('invalid_arguments', false, false));
  }
  assert.equal(calls, 1);
  await assert.rejects(reader(d, async () => Response.json({ _embedded: { conversations: new Array(201).fill({}) } })).invoke(d.args), failure('invalid_response'));
});

test('inline donor query seam refuses modified literals, query overrides, account changes and origin retargeting', async () => {
  for (const d of businessProfiles.slice(1, 3)) {
    let calls = 0; const selection = select({ profile: d.profile, auth: d.auth(KEY), transport: async () => { calls++; return Response.json(d.body); } });
    const prepared = inputs(selection, d.args);
    const base = d.profile.startsWith('campaign-') ? {
      method: 'GET', url: selection.origin + prepared.pathname + prepared.inlineQuery,
      headers: { 'Content-Type': 'application/json' }, authentication: { type: 'BASIC', username: KEY, password: 'x' }, body: undefined,
    } : { method: 'GET', url: selection.origin + prepared.pathname + prepared.inlineQuery,
      headers: { Authorization: `SSWS ${KEY}`, 'Content-Type': 'application/json', Accept: 'application/json' }, body: undefined };
    for (const changed of [{ url: base.url + '&other=true' }, { url: base.url.replace(selection.origin, 'https://unselected.invalid') },
      { url: selection.origin + prepared.pathname }, { queryParams: prepared.query }, { body: {} }, { method: 'POST' },
      { headers: { Authorization: 'other-account' } }]) {
      const invocation = new Invocation(selection, prepared.pathname, undefined, prepared.query, prepared.inlineQuery);
      try { await assert.rejects(invocation.request({ ...base, ...changed }), failure('upstream_error', false, false)); }
      finally { invocation.finish(); }
    }
    assert.equal(calls, 0);
  }
});

test('business schema string patterns have full input boundaries and agree with runtime', () => {
  for (const d of businessProfiles) {
    const configuration = { profile: d.profile, auth: d.auth(KEY), transport: async () => { throw 0; } };
    const selection = select(configuration), schema = createReader(configuration).describe().inputSchema;
    for (const [key, property] of Object.entries(schema.properties)) {
      if (!property.pattern) continue;
      for (const suffix of ['', '\n', '\r', '\u0000', '\ud800']) {
        const value = d.args[key] + suffix;
        for (const flags of ['', 'u']) assert.equal(new RegExp(property.pattern, flags).test(value), suffix === '', `${d.profile}/${key}/${JSON.stringify(suffix)}`);
        const args = { ...d.args, [key]: value };
        if (suffix === '') assert.doesNotThrow(() => inputs(selection, args)); else assert.throws(() => inputs(selection, args));
      }
    }
  }
});
