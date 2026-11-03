import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough } from 'node:stream';
import { MockAgent } from 'undici';
import { StdioServerTransport } from '@modelcontextprotocol/server/stdio';
import { PROTOCOL_VERSION_META_KEY, CLIENT_INFO_META_KEY, CLIENT_CAPABILITIES_META_KEY } from '@modelcontextprotocol/server';
import { createOwnedHttpTransport, serveMcpStdio } from '../dist/mcp.js';
import { KEY, platformProfiles } from './fixtures/platform-profiles.mjs';

const metadata = { [PROTOCOL_VERSION_META_KEY]: '2026-07-28', [CLIENT_INFO_META_KEY]: { name: 'platform-fixture', version: '1' }, [CLIENT_CAPABILITIES_META_KEY]: {} };
const CANARY = 'unselected-private-fixture-token';
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
async function eventually(predicate) {
  const until = Date.now() + 4000;
  while (!predicate()) {
    if (Date.now() > until) throw new Error('MCP fixture timed out');
    await new Promise(resolve => setTimeout(resolve, 2));
  }
}
function harness(d, era, token, { status = 200, httpFactory } = {}) {
  const stdin = new PassThrough(), stdout = new PassThrough(), replies = [], diagnostics = [];
  const dispatcher = new MockAgent(); dispatcher.disableNetConnect();
  if (!httpFactory) {
    const url = new URL(d.url(token));
    dispatcher.get(url.origin).intercept({ method: 'GET', path: url.pathname + url.search, headers: d.headers(token) })
      .reply(status, status === 200 ? d.body : `${d.url(token)} ${token} ${CANARY}`, { headers: { location: 'https://unselected.invalid' } });
  }
  let pending = '', id = 0;
  stdout.on('data', data => {
    pending += data;
    let index;
    while ((index = pending.indexOf('\n')) >= 0) {
      replies.push(JSON.parse(pending.slice(0, index))); pending = pending.slice(index + 1);
    }
  });
  const environment = { SELECTED_TOKEN: token, AMBIENT_TOKEN: CANARY }, wire = new StdioServerTransport(stdin, stdout);
  const owner = serveMcpStdio({ profile: d.profile, token_env: 'SELECTED_TOKEN', ...d.config }, {
    environment, httpFactory: httpFactory ?? (() => createOwnedHttpTransport(dispatcher)), transport: wire,
    onerror: (...args) => diagnostics.push(args),
  });
  environment.SELECTED_TOKEN = CANARY;
  const send = (method, params = {}) => {
    const sent = ++id;
    stdin.write(JSON.stringify({ jsonrpc: '2.0', id: sent, method, params: { ...params, ...(era === 'modern' ? { _meta: metadata } : {}) } }) + '\n');
    return sent;
  };
  const request = async (method, params = {}) => {
    const sent = send(method, params);
    await eventually(() => replies.some(item => item.id === sent));
    return replies.find(item => item.id === sent);
  };
  return { owner, dispatcher, wire, replies, diagnostics, send, request, async close() { try { await owner.close(); } finally { if (httpFactory) await dispatcher.close(); } }, async start() {
    if (era === 'legacy') {
      const initialized = await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'platform-fixture', version: '1' } });
      assert.equal(initialized.result.protocolVersion, '2024-11-05');
      stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    }
  } };
}
function redacted(value, d, token) {
  const encoded = typeof value === 'string' ? value : JSON.stringify(value);
  for (const secret of [token, encodeURIComponent(token), CANARY, d.url(token)]) assert.ok(!encoded.includes(secret));
}

for (const d of platformProfiles) {
  for (const era of ['legacy', 'modern']) {
    test(`${era} MCP ${d.profile} invokes real donor on two isolated selected accounts`, async () => {
      const first = harness(d, era, 'account-a'), second = harness(d, era, 'account-b');
      try {
        await Promise.all([first.start(), second.start()]);
        const listed = await first.request('tools/list'), name = d.profile.replaceAll('.', '_');
        assert.deepEqual(listed.result.tools.map(tool => tool.name), [name]);
        assert.equal(listed.result.tools[0].annotations.readOnlyHint, true);
        assert.equal(listed.result.tools[0].inputSchema.additionalProperties, false);
        assert.ok(!JSON.stringify(listed).includes(CANARY));
        const invalid = await first.request('tools/call', { name, arguments: { ...d.args, api_secret: CANARY } });
        assert.equal(invalid.result.isError, true); assert.equal(invalid.result.structuredContent.error.entered, false);
        assert.ok(!JSON.stringify(invalid).includes(CANARY));
        const calls = await Promise.all([first, second].map(h => h.request('tools/call', { name, arguments: d.args })));
        for (const call of calls) {
          assert.equal(call.error, undefined); assert.equal(call.result.isError, undefined);
          assert.deepEqual(call.result.structuredContent.value, d.result);
        }
        first.dispatcher.assertNoPendingInterceptors(); second.dispatcher.assertNoPendingInterceptors();
      } finally { await Promise.all([first.close(), second.close()]); }
    });
  }
  test(`MCP ${d.profile} returns safe redirect and provider error metadata`, async () => {
    for (const status of [302, 401, 403, 429, 503, ...(d.profile === 'campaign-monitor.find-subscriber' ? [] : [404])]) {
      const h = harness(d, 'modern', KEY, { status });
      try {
        const call = await h.request('tools/call', { name: d.profile.replaceAll('.', '_'), arguments: d.args });
        assert.equal(call.result.isError, true);
        assert.equal(call.result.structuredContent.error.kind, status === 302 ? 'redirect_refused' : 'http_error');
        assert.equal(call.result.structuredContent.error.status, status === 302 ? undefined : status);
        assert.equal(call.result.structuredContent.error.dispatched, true);
        redacted(call, d, KEY); redacted(h.diagnostics, d, KEY);
        h.dispatcher.assertNoPendingInterceptors();
      } finally { await h.close(); }
    }
  });
  test(`MCP ${d.profile} shutdown joins its held body before closing the HTTP owner`, async () => {
    const arrived = deferred(), cancelling = deferred(), release = deferred();
    let settled = false, closed = 0, cancellations = 0, calls = 0;
    const body = new ReadableStream({ cancel() { cancellations++; cancelling.resolve(); return release.promise; } });
    const h = harness(d, 'modern', KEY, { httpFactory: () => ({
      send: async (url, init) => {
        calls++; assert.equal(url, d.url(KEY)); assert.deepEqual(Object.fromEntries(init.headers), d.headers(KEY));
        arrived.resolve(); return new Response(body);
      }, close: async () => { closed++; assert.equal(body.locked, false); },
    }) });
    try {
      h.send('tools/call', { name: d.profile.replaceAll('.', '_'), arguments: d.args });
      await arrived.promise;
      const closing = h.owner.close().then(() => { settled = true; });
      await cancelling.promise; await new Promise(resolve => setImmediate(resolve));
      assert.equal(settled, false); assert.equal(closed, 0);
      release.resolve(); await closing;
      assert.equal(calls, 1); assert.equal(cancellations, 1); assert.equal(closed, 1);
      redacted(h.replies, d, KEY); redacted(h.diagnostics, d, KEY);
    } finally { release.resolve(); await h.close(); }
  });
}

test('MCP Notion keeps all four slots owned after SDK timeouts until actual cleanup retires', async t => {
  const d = platformProfiles.find(item => item.profile === 'notion.get-page');
  const arrived = deferred(), headers = deferred(), cancelling = deferred(), release = deferred();
  let calls = 0, cancellations = 0;
  const h = harness(d, 'modern', KEY, { httpFactory: () => ({
    send: async () => {
      if (++calls > 4) return Response.json(d.body);
      if (calls === 4) arrived.resolve();
      await headers.promise;
      return new Response(new ReadableStream({ cancel() {
        if (++cancellations === 4) cancelling.resolve();
        return release.promise;
      } }));
    }, close: async () => { assert.equal(cancellations, 4); },
  }) });
  const turns = async predicate => {
    for (let n = 0; n < 1000 && !predicate(); n++) await new Promise(done => setImmediate(done));
    assert.ok(predicate(), 'Expected protocol result within fixture turns');
  };
  t.mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const name = d.profile.replaceAll('.', '_');
    const ids = Array.from({ length: 4 }, () => h.send('tools/call', { name, arguments: d.args }));
    await arrived.promise; t.mock.timers.tick(60_000);
    await new Promise(done => setImmediate(done));
    const extra = h.send('tools/call', { name, arguments: d.args });
    await turns(() => h.replies.some(item => item.id === extra));
    assert.equal(h.replies.find(item => item.id === extra).result.structuredContent.error.kind, 'server_busy');
    assert.equal(calls, 4); assert.ok(!h.replies.some(item => ids.includes(item.id)));
    headers.resolve(); await cancelling.promise;
    assert.ok(!h.replies.some(item => ids.includes(item.id)));
    release.resolve(); await turns(() => ids.every(id => h.replies.some(item => item.id === id)));
    for (const id of ids) assert.equal(h.replies.find(item => item.id === id).result.structuredContent.error.kind, 'deadline_exceeded');
    const recovered = h.send('tools/call', { name, arguments: d.args });
    await turns(() => h.replies.some(item => item.id === recovered));
    assert.deepEqual(h.replies.find(item => item.id === recovered).result.structuredContent.value, d.body);
    assert.equal(calls, 5);
  } finally { headers.resolve(); release.resolve(); t.mock.timers.reset(); await h.close(); }
});
