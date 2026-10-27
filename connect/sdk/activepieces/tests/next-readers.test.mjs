import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { select, inputs } from '../dist/profiles.js';
import { Invocation } from '../dist/transport.js';
import { nextProfiles } from './fixtures/next-profiles.mjs';

const TOKEN = 'selected-fixture-token', CANARY = 'private-provider-body';
const safe = (kind, metadata = {}) => error => {
  assert.ok(error instanceof ActivepiecesError);
  assert.equal(error.kind, kind);
  assert.equal(error.writeOutcome, 'not_applicable');
  for (const [key, value] of Object.entries(metadata)) assert.equal(error[key], value);
  assert.ok(!String(error).includes(CANARY));
  assert.ok(!JSON.stringify(error).includes(TOKEN));
  assert.equal(error.cause, undefined);
  return true;
};
const reader = (definition, transport, token = TOKEN) => createReader({ profile: definition.profile, auth: definition.auth(token), transport });
const response = body => new Response(JSON.stringify(body));
async function eventually(predicate) {
  const until = Date.now() + 4000;
  while (!predicate()) {
    if (Date.now() > until) throw new Error('Fixture timed out');
    await new Promise(resolve => setTimeout(resolve, 2));
  }
}

for (const definition of nextProfiles) {
  test(`real ${definition.profile} donor uses exact credential/path/query and preserves data`, async () => {
    let calls = 0, borrowed;
    const selected = reader(definition, async (url, init) => {
      calls++;
      assert.equal(url, definition.url);
      assert.equal(init.method, 'GET');
      assert.equal(init.redirect, 'error');
      assert.ok(init.signal instanceof AbortSignal);
      assert.equal(init.body, undefined);
      const expected = { accept: 'application/json', ...definition.headers(TOKEN) };
      assert.deepEqual(Object.fromEntries(new Headers(init.headers)), expected);
      return borrowed = response(definition.body);
    });
    assert.equal(selected.describe().effect, 'read');
    assert.deepEqual(await selected.invoke(definition.args), definition.result);
    assert.equal(calls, 1);
    assert.equal(borrowed.body.locked, false);
    assert.equal(borrowed.bodyUsed, true);
  });

  test(`${definition.profile} HTTP and redirect failures survive donor wrappers without replay`, async () => {
    for (const status of [302, 401, 403, 404, 429, 503]) {
      let calls = 0, closed = false;
      const body = new ReadableStream({ cancel() { closed = true; } });
      await assert.rejects(reader(definition, async () => {
        calls++; return new Response(body, { status });
      }).invoke(definition.args), safe(status === 302 ? 'redirect_refused' : 'http_error', {
        entered: true, dispatched: true, ...(status === 302 ? {} : { status }),
      }));
      assert.equal(calls, 1); assert.equal(closed, true); assert.equal(body.locked, false);
    }
    await assert.rejects(reader(definition, async () => new Response(CANARY)).invoke(definition.args), safe('invalid_response'));
    await assert.rejects(reader(definition, async () => { throw new Error(CANARY); }).invoke(definition.args), safe('transport_error'));
  });

  test(`${definition.profile} overlapping selected accounts keep their credentials and donor closures`, async () => {
    let release;
    const gate = new Promise(resolve => { release = resolve; }), entered = [];
    const calls = ['account-a', 'account-b'].map(token => reader(definition, async (url, init) => {
      assert.equal(url, definition.url);
      const headers = new Headers(init.headers);
      for (const [key, value] of Object.entries(definition.headers(token))) assert.equal(headers.get(key), value);
      entered.push(token); await gate;
      return response(definition.profile === 'pagerduty.get-incident' ? { incident: { account: token } } : { account: token });
    }, token).invoke(definition.args));
    await eventually(() => entered.length === 2); release();
    assert.deepEqual(await Promise.all(calls), [{ account: 'account-a' }, { account: 'account-b' }]);
  });

  test(`${definition.profile} cancellation joins borrowed body cleanup through donor error wrappers`, async () => {
    let entered = false, cancelStarted = false, release, settled = false;
    const body = new ReadableStream({ cancel() { cancelStarted = true; return new Promise(resolve => { release = resolve; }); } });
    const abort = new AbortController();
    const operation = reader(definition, async () => { entered = true; return new Response(body); }).invoke(definition.args, { signal: abort.signal });
    const assertion = assert.rejects(operation, safe('cancelled', { entered: true, dispatched: true }));
    operation.then(() => { settled = true; }, () => { settled = true; });
    await eventually(() => entered); abort.abort(CANARY);
    await eventually(() => cancelStarted); assert.equal(settled, false);
    release(); await assertion; assert.equal(body.locked, false);
  });

  test(`${definition.profile} refuses unexpected donor headers, auth, query, method and follow-up endpoint`, async () => {
    let calls = 0;
    const selection = select({ profile: definition.profile, auth: definition.auth(TOKEN), transport: async () => { calls++; return response({}); } });
    const prepared = inputs(selection, definition.args), parsed = new URL(definition.url);
    const valid = { method: 'GET', url: parsed.origin + parsed.pathname,
      ...(definition.donorHeaders ? { headers: definition.donorHeaders(TOKEN) } : { authentication: { type: 'BEARER_TOKEN', token: TOKEN } }),
      ...(definition.query === undefined ? {} : { queryParams: definition.query }),
    };
    for (const request of [
      { ...valid, url: valid.url + '/more' }, { ...valid, url: 'https://other.invalid/' },
      { ...valid, method: 'POST' }, { ...valid, body: {} },
      { ...valid, headers: { ...valid.headers, 'X-Extra': CANARY } },
      { ...valid, authentication: { type: 'BEARER_TOKEN', token: CANARY } },
      { ...valid, queryParams: { ...valid.queryParams, unexpected: CANARY } },
      { ...valid, queryParams: { ...valid.queryParams, user_field_names: 'false' } },
    ]) {
      const invocation = new Invocation(selection, prepared.pathname);
      try { await assert.rejects(invocation.request(request), safe('upstream_error', { dispatched: false })); }
      finally { invocation.finish(); }
    }
    assert.equal(calls, 0);
  });
}

test('new profile input boundaries refuse invalid manual IDs and added fields before donor entry', async () => {
  let calls = 0;
  const invalid = {
    'baserow.get-row': [{ table_id: 0, row_id: 1 }, { table_id: '1', row_id: 1 }, { table_id: 1, row_id: 1.5 }, { table_id: 1, row_id: Number.MAX_SAFE_INTEGER + 1 }],
    'bitly.get-bitlink-details': ['https://bit.ly/a', 'bit.ly/../x', 'bit.ly/%2e%2e', 'BIT.ly/a', '-bit.ly/a', 'bit..ly/a', 'bit.ly/a?x=1', 'bit.ly/a\n'].map(bitlink => ({ bitlink })),
    'clockify.find-running-timer': [{ workspaceId: '../x' }, { workspaceId: 'w\n' }],
    'coda.get-row': ['.', '..', '\u0000', '\ud800', 'x'.repeat(257), '\u{1f600}'.repeat(257)].flatMap(name => [
      { docId: 'doc', tableId: name, rowIdOrName: 'row' }, { docId: 'doc', tableId: 'table', rowIdOrName: name },
    ]).concat([{ docId: '../doc', tableId: 'table', rowIdOrName: 'row' }]),
    'pagerduty.get-incident': [{ incidentId: '..' }, { incidentId: 'a/b' }],
    'postmark.get-delivery-stats': [{ server: 'unselected' }],
    'typeform.get-form-messages': [{ form_id: '..' }, { form_id: 'a#fragment' }],
  };
  for (const definition of nextProfiles) {
    for (const args of [...invalid[definition.profile], { ...definition.args, token: TOKEN }]) {
      await assert.rejects(reader(definition, async () => { calls++; return response({}); }).invoke(args), safe('invalid_arguments', { entered: false, dispatched: false }));
    }
  }
  assert.equal(calls, 0);
});

test('Baserow binds only an explicit database token and canonical HTTPS origin', () => {
  const base = { profile: 'baserow.get-row', transport: async () => response({}) };
  for (const auth of [
    { origin: 'https://base.invalid', databaseToken: TOKEN, email: 'x', password: 'x' },
    { origin: 'https://base.invalid', authType: 'jwt', databaseToken: TOKEN },
    { origin: 'https://base.invalid', databaseToken: TOKEN + '\n' },
    ...['http://base.invalid', 'https://base.invalid/', 'https://base.invalid/api', 'https://user@base.invalid', 'https://BASE.invalid'].map(origin => ({ origin, databaseToken: TOKEN })),
  ]) assert.throws(() => createReader({ ...base, auth }), safe('invalid_configuration'));
});

test('PagerDuty keeps the donor nullish incident fallback rather than changing its result', async () => {
  const definition = nextProfiles.find(item => item.profile === 'pagerduty.get-incident');
  for (const body of [{ incident: null, other: false }, { incident: false }, { incident: 0 }, { other: null }]) {
    assert.deepEqual(await reader(definition, async () => response(body)).invoke(definition.args), body.incident ?? body);
  }
});

test('Coda name lengths follow JSON Schema Unicode code points and preserve encoded segments', async () => {
  const definition = nextProfiles.find(item => item.profile === 'coda.get-row');
  const name = '\u{1f600}'.repeat(256);
  let calls = 0;
  const selected = reader(definition, async url => {
    calls++;
    assert.equal(new URL(url).pathname, `/apis/v1/docs/doc/tables/${encodeURIComponent(name)}/rows/${encodeURIComponent(name)}`);
    return response(definition.body);
  });
  assert.equal(selected.describe().inputSchema.properties.tableId.maxLength, 256);
  assert.deepEqual(await selected.invoke({ docId: 'doc', tableId: name, rowIdOrName: name }), definition.result);
  assert.equal(calls, 1);
});
