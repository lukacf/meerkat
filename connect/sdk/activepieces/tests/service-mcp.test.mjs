import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough } from 'node:stream';
import { MockAgent } from 'undici';
import { StdioServerTransport } from '@modelcontextprotocol/server/stdio';
import { PROTOCOL_VERSION_META_KEY, CLIENT_INFO_META_KEY, CLIENT_CAPABILITIES_META_KEY } from '@modelcontextprotocol/server';
import { createOwnedHttpTransport, serveMcpStdio } from '../dist/mcp.js';
import { KEY, serviceProfiles } from './fixtures/service-profiles.mjs';

const metadata = { [PROTOCOL_VERSION_META_KEY]: '2026-07-28', [CLIENT_INFO_META_KEY]: { name: 'service-fixture', version: '1' }, [CLIENT_CAPABILITIES_META_KEY]: {} };
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
  const owner = serveMcpStdio({ profile: d.profile, token_env: 'SELECTED_TOKEN' }, {
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
      const initialized = await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'service-fixture', version: '1' } });
      assert.equal(initialized.result.protocolVersion, '2024-11-05');
      stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    }
  } };
}
function redacted(value, d, token) {
  const encoded = typeof value === 'string' ? value : JSON.stringify(value);
  for (const secret of [token, encodeURIComponent(token), CANARY, d.url(token)]) assert.ok(!encoded.includes(secret));
}

for (const d of serviceProfiles) {
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
    for (const status of [302, 401, 403, 404, 429, 503]) {
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

for (const era of ['legacy', 'modern']) {
  test(`${era} MCP Kit strips URL-bearing transport errors and diagnostic payloads`, async () => {
    const d = serviceProfiles[0]; let calls = 0, closed = 0;
    const h = harness(d, era, KEY, { httpFactory: () => ({
      send: async url => { calls++; assert.equal(url, d.url(KEY)); throw new Error(`Cannot read ${url}`, { cause: KEY }); },
      close: async () => { closed++; },
    }) });
    try {
      await h.start();
      const call = await h.request('tools/call', { name: d.profile.replaceAll('.', '_'), arguments: d.args });
      assert.equal(call.result.isError, true); assert.equal(call.result.structuredContent.error.kind, 'transport_error');
      assert.equal(call.result.structuredContent.error.dispatched, true); assert.equal(calls, 1);
      redacted(call, d, KEY);
      // The official transport's error event must not escape through the host diagnostic callback.
      h.wire.onerror(new Error(`Transport diagnostic ${d.url(KEY)}`));
      await assert.rejects(h.owner.closed, error => {
        assert.equal(error.message, 'Activepieces MCP cleanup failed'); redacted(`${error} ${error.stack}`, d, KEY); return true;
      });
      assert.ok(h.diagnostics.length > 0); assert.ok(h.diagnostics.every(args => args.length === 0));
      redacted(h.diagnostics, d, KEY); redacted(h.replies, d, KEY); assert.equal(closed, 1);
    } finally { await h.close().catch(() => {}); }
  });
}
