import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { parseConfiguration, readerConfiguration } from '../dist/mcp-config.js';
import { Invocation } from '../dist/transport.js';
import { inputs, select } from '../dist/profiles.js';
import { selectedToken } from '../dist/mcp-config.js';
import { createDonorBundle } from '../dist/donor-factory.js';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import http from 'node:http';
import https from 'node:https';
import childProcess from 'node:child_process';
import { KEY, platformProfiles } from './fixtures/platform-profiles.mjs';
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
for (const d of platformProfiles) {
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

test('platform configuration binds auth independently of read arguments', () => {
  for (const d of platformProfiles) {
    const config = { profile: d.profile, token_env: 'SELECTED_TOKEN', ...d.config };
    assert.deepEqual(parseConfiguration(config), config);
    assert.equal(createReader(readerConfiguration(config, KEY, async () => { throw 0; })).describe().profile, d.profile);
    for (const extra of [{ access_token: KEY }, { refresh_token: KEY }, { page_id: 'mutation-only-string' }]) assert.throws(() => parseConfiguration({ ...config, ...extra }));
    for (const auth of [d.auth(''), d.auth('token\n'), { ...d.auth(KEY), otherToken: KEY }]) assert.throws(() => reader(d, async () => { throw 0; }, KEY, { auth }));
    if (!Object.hasOwn(d.config, 'origin')) assert.throws(() => parseConfiguration({ ...config, origin: 'https://unselected.invalid' }));
    if (!Object.hasOwn(d.config, 'region')) assert.throws(() => parseConfiguration({ ...config, region: 'us' }));
  }
  for (const d of platformProfiles.filter(d => d.config.origin)) for (const origin of [undefined, 'http://selected.invalid', 'https://selected.invalid/', 'https://selected.invalid/path', 'https://a@selected.invalid', 'https://selected.invalid?x=1']) {
    assert.throws(() => reader(d, async () => { throw 0; }, KEY, { auth: { ...d.auth(KEY), origin } }));
  }
  assert.throws(() => reader(platformProfiles[1], async () => { throw 0; }, 'api:key'));
});

test('Airtable preserves the donor non-200 success wrapper after checking record identity', async () => {
  const d = platformProfiles[0];
  assert.deepEqual(await reader(d, async () => Response.json(d.body, { status: 201 })).invoke(d.args), { status: 201, headers: {}, body: d.body });
  await assert.rejects(reader(d, async () => Response.json({ ...d.body, id: 'other' }, { status: 201 })).invoke(d.args), failure('invalid_response'));
  await assert.rejects(reader(d, async () => new Response(null, { status: 204 })).invoke(d.args), failure('invalid_response'));
});

test('WordPress preserves application-password spaces and selected username', async () => {
  const d = platformProfiles[4], key = 'abcd efgh ijkl mnop qrst uvwx';
  const config = { profile: d.profile, token_env: 'WP_PASSWORD', ...d.config };
  assert.equal(selectedToken(config, { WP_PASSWORD: key }), key);
  const run = reader(d, async (url, init) => { assert.deepEqual(Object.fromEntries(init.headers), d.headers(key)); return Response.json(d.body); }, key);
  assert.deepEqual(await run.invoke(d.args), d.body);
  for (const bad of ['', ' ', '\n', 'abcd\nefgh']) {
    assert.throws(() => selectedToken(config, { WP_PASSWORD: bad }));
    assert.throws(() => reader(d, async () => { throw 0; }, bad));
  }
  for (const username of ['reader:other', '', 'reader\n']) assert.throws(() => reader(d, async () => { throw 0; }, key, { auth: { ...d.auth(key), username } }));
});

test('Toggl optional filters preserve false, omitted active and explicit null or empty name', async () => {
  const d = platformProfiles[1];
  for (const extra of [{}, { name: null }, { name: '' }, { name: '   ' }, { billable: false, only_me: false, only_templates: false }]) {
    const run = reader(d, async url => {
      const query = Object.fromEntries(new URL(url).searchParams);
      assert.deepEqual(query, { ...(extra.name ? { name: extra.name } : {}), ...(Object.hasOwn(extra, 'billable') ? { billable: 'false', only_me: 'false', only_templates: 'false' } : {}), page: '1', per_page: '100' });
      return Response.json([]);
    });
    assert.deepEqual(await run.invoke({ workspace_id: 123, ...extra }), []);
  }
  for (const extra of [{ active: null }, { billable: null }, { only_me: '' }, { page: 0 }, { page: null }, { page: 10_001 }, { per_page: 201 }, { name: '\ud800' }]) {
    await assert.rejects(reader(d, async () => { throw 0; }).invoke({ workspace_id: 123, ...extra }), failure('invalid_arguments', false, false));
  }
  await assert.rejects(reader(d, async () => Response.json(new Array(21).fill(d.body[0]))).invoke(d.args), failure('invalid_response'));
  await assert.rejects(reader(d, async () => Response.json([{ ...d.body[0], id: Number.MAX_SAFE_INTEGER + 1 }])).invoke(d.args), failure('invalid_response'));
});

test('Google Calendar keeps date metadata and encoded calendar selection while optional zone omissions remain absent', async () => {
  const d = platformProfiles[5];
  for (const extra of [{}, { time_zone: null }, { time_zone: '' }, { time_zone: '   ' }]) {
    const run = reader(d, async url => {
      assert.equal(url, 'https://www.googleapis.com/calendar/v3/calendars/a%2Bb%2F%26%25%23%F0%9F%98%80/events/event123');
      return Response.json(d.body);
    });
    assert.deepEqual(await run.invoke({ calendar_id: ' a+b/&%#\u{1f600} ', event_id: 'event123', ...extra }), d.body);
  }
  for (const extra of [{ calendar_id: ' . ' }, { calendar_id: '..' }, { calendar_id: '   ' }, { calendar_id: '\ud800' }, { event_id: 'a/bcd' }, { event_id: 'x'.repeat(1025) }, { max_attendees: null }, { max_attendees: 0 }, { max_attendees: 201 }, { time_zone: 'Mars/Imaginary' }, { time_zone: '\ud800' }]) {
    await assert.rejects(reader(d, async () => { throw 0; }).invoke({ ...d.args, ...extra }), failure('invalid_arguments', false, false));
  }
});

test('manual identifiers refuse traversal, terminal newlines and malformed Unicode before donor entry', async () => {
  for (const d of platformProfiles) {
    const key = d.profile === 'google-calendar.get-event' ? 'event_id' : Object.keys(d.args)[0], original = d.args[key];
    const invalid = typeof original === 'number' ? [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '123'] :
      [null, '', '../selected', original + '\n', original + '\u2028', '\ud800'];
    for (const item of invalid) await assert.rejects(reader(d, async () => { throw 0; }).invoke({ ...d.args, [key]: item }), failure('invalid_arguments', false, false));
  }
  const d = platformProfiles[0];
  await assert.rejects(reader(d, async () => { throw 0; }).invoke({ ...d.args, table_id_or_name: 'Table Name' }), failure('invalid_arguments', false, false));
  const n = platformProfiles[6];
  for (const page_id of [n.args.page_id.replaceAll('-', ''), n.args.page_id.toUpperCase(), n.args.page_id + '/']) {
    await assert.rejects(reader(n, async () => { throw 0; }).invoke({ page_id }), failure('invalid_arguments', false, false));
  }
});

test('Intercom uses explicit region and never falls back to ambient token', async () => {
  const d = platformProfiles[7], prior = process.env.INTERCOM_API_KEY;
  process.env.INTERCOM_API_KEY = 'unselected-canary';
  try {
    for (const region of ['us', 'eu', 'au']) {
      const run = reader(d, async (url, init) => {
        assert.equal(new URL(url).origin, `https://api.${region === 'us' ? '' : region + '.'}intercom.io`);
        assert.equal(init.headers.get('authorization'), `Bearer ${KEY}`); return Response.json(d.body);
      }, KEY, { auth: { accessToken: KEY, region } });
      assert.deepEqual(await run.invoke(d.args), d.body);
    }
    for (const region of [undefined, 'hipaa', 'eu.intercom', 'unselected.invalid']) assert.throws(() => reader(d, async () => { throw 0; }, KEY, { auth: { accessToken: KEY, region } }));
  } finally { if (prior === undefined) delete process.env.INTERCOM_API_KEY; else process.env.INTERCOM_API_KEY = prior; }
});

test('Notion SDK timeout cannot abandon the owned request or release the invocation early', async t => {
  const d = platformProfiles[6], arrived = deferred(), late = deferred(), cancelled = deferred(), release = deferred();
  const unhandled = []; const onUnhandled = error => unhandled.push(error);
  process.on('unhandledRejection', onUnhandled);
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let settled = false, directRejected = false, body;
  try {
    // A direct unchanged SDK operation demonstrates its timer really rejects
    // while the fetch promise is still pending in this same controlled run.
    const donor = createDonorBundle(async () => { await late.promise; throw new Error('retired fixture failure'); });
    const direct = assert.rejects(donor.notionGetPage.run({ auth: { type: 'CUSTOM_AUTH', props: { accessToken: KEY } }, propsValue: d.args }), error => {
      assert.equal(error.code, 'notionhq_client_request_timeout'); directRejected = true; return true;
    });
    const run = reader(d, async (url, init) => {
      arrived.resolve(init.signal); await late.promise;
      body = new ReadableStream({ cancel() { cancelled.resolve(); return release.promise; } });
      return new Response(body);
    }, KEY, { timeoutMs: 60_000 });
    const pending = run.invoke(d.args).finally(() => { settled = true; });
    const rejected = assert.rejects(pending, failure('deadline_exceeded'));
    const signal = await arrived.promise;
    t.mock.timers.tick(60_000); await direct; await tick();
    assert.equal(directRejected, true); assert.equal(signal.aborted, true); assert.equal(settled, false);
    late.resolve(); await cancelled.promise; await tick(); assert.equal(settled, false);
    release.resolve(); await rejected; await tick();
    assert.equal(body.locked, false); assert.deepEqual(unhandled, []);
  } finally { late.resolve(); release.resolve(); t.mock.timers.reset(); process.off('unhandledRejection', onUnhandled); }
});

test('each new profile holds a late response until its owned cleanup finishes after transport abort', async () => {
  for (const d of platformProfiles) {
    const entered = deferred(), late = deferred(), cancelling = deferred(), release = deferred(), controller = new AbortController();
    let settled = false, body, calls = 0;
    const run = reader(d, async (url, init) => {
      if (++calls > 1) return Response.json(d.body);
      entered.resolve(init.signal); await late.promise;
      body = new ReadableStream({ cancel() { cancelling.resolve(); return release.promise; } });
      return new Response(body);
    });
    const pending = run.invoke(d.args, { signal: controller.signal }).finally(() => { settled = true; });
    const rejected = assert.rejects(pending, failure('cancelled'));
    const signal = await entered.promise; controller.abort(); await tick(); assert.equal(signal.aborted, true); assert.equal(settled, false);
    late.resolve(); await cancelling.promise; await tick(); assert.equal(settled, false);
    release.resolve(); await rejected; assert.equal(body.locked, false);
    assert.deepEqual(await run.invoke(d.args), d.body);
  }
});

test('projected statements are exact source slices and unused piece registrations have no executed module closure', async () => {
  const root = new URL('../', import.meta.url), read = rel => readFile(new URL(rel, root), 'utf8');
  const hash = text => createHash('sha256').update(text).digest('hex');
  const proof = JSON.parse(await read('dist/SOURCE-PROJECTIONS.json'));
  assert.equal(proof.records.length, 10);
  for (const item of proof.records) {
    const original = await read(`vendor/activepieces/${item.source}`), projected = await read(`dist/SOURCE-PROJECTIONS/${item.source}`);
    assert.equal(hash(original), item.source_sha256); assert.equal(hash(projected), item.projected_sha256);
    assert.equal(projected, item.slices.map(slice => {
      const text = original.slice(slice.start_utf16, slice.end_utf16); assert.equal(hash(text), slice.sha256); return text;
    }).join('\n') + '\n');
    assert.ok(!projected.includes('= createPiece('));
  }
  const build = JSON.parse(await read('dist/BUILD-MANIFEST.json'));
  for (const path of Object.keys(build.metafile.inputs)) {
    assert.ok(!/community\/(stripe|wordpress|toggl-track)\/src\/lib\/(trigger|triggers)\//.test(path), path);
    if (/community\/(stripe|wordpress|toggl-track)\/src\/lib\/actions\//.test(path)) assert.match(path, /\/(get-invoice|get-page\.action|find-project)\.ts$/);
  }
});

test('real SDKs and metadata initialization make no ambient HTTP, fetch or process calls', async t => {
  let ambient = 0;
  const refuse = () => { ambient++; throw new Error('Ambient operation refused'); };
  t.mock.method(http, 'request', refuse); t.mock.method(https, 'request', refuse);
  t.mock.method(globalThis, 'fetch', refuse); t.mock.method(childProcess, 'spawn', refuse); t.mock.method(childProcess, 'exec', refuse);
  for (const d of platformProfiles) assert.deepEqual(await reader(d, async () => Response.json(d.body)).invoke(d.args), d.body);
  assert.equal(ambient, 0);
});
