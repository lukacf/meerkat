import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough } from 'node:stream';
import { MockAgent } from 'undici';
import { StdioServerTransport } from '@modelcontextprotocol/server/stdio';
import { PROTOCOL_VERSION_META_KEY, CLIENT_INFO_META_KEY, CLIENT_CAPABILITIES_META_KEY } from '@modelcontextprotocol/server';
import { createMcpServer, createOwnedHttpTransport, describeMcpConfiguration, parseConfiguration, serveMcpStdio } from '../dist/mcp.js';
import { oauthProfiles as nextProfiles } from './fixtures/oauth-profiles.mjs';

const metadata = { [PROTOCOL_VERSION_META_KEY]: '2026-07-28', [CLIENT_INFO_META_KEY]: { name: 'oauth-fixture', version: '1' }, [CLIENT_CAPABILITIES_META_KEY]: {} };
const configuration = definition => ({ profile: definition.profile, token_env: 'SELECTED_TOKEN', ...definition.config });
const CANARY = 'unselected-private-fixture-token';
async function eventually(predicate) {
  const until = Date.now() + 4000;
  while (!predicate()) {
    if (Date.now() > until) throw new Error('MCP fixture timed out');
    await new Promise(resolve => setTimeout(resolve, 2));
  }
}
function harness(definition, era, token, status = 200) {
  const stdin = new PassThrough(), stdout = new PassThrough(), replies = [];
  const dispatcher = new MockAgent(); dispatcher.disableNetConnect();
  const url = new URL(definition.url);
  dispatcher.get(url.origin).intercept({ method: 'GET', path: url.pathname + url.search, headers: definition.headers(token) })
    .reply(status, status === 200 ? definition.body : CANARY);
  let pending = '', id = 0;
  stdout.on('data', data => {
    pending += data;
    let index;
    while ((index = pending.indexOf('\n')) >= 0) {
      replies.push(JSON.parse(pending.slice(0, index))); pending = pending.slice(index + 1);
    }
  });
  const env = { SELECTED_TOKEN: token, AMBIENT_TOKEN: CANARY };
  const owner = serveMcpStdio(configuration(definition), {
    environment: env, httpFactory: () => createOwnedHttpTransport(dispatcher), transport: new StdioServerTransport(stdin, stdout),
  });
  // Serving snapshots the selected credential before lazy protocol construction.
  env.SELECTED_TOKEN = CANARY;
  const request = async (method, params = {}) => {
    const sent = ++id;
    stdin.write(JSON.stringify({ jsonrpc: '2.0', id: sent, method, params: { ...params, ...(era === 'modern' ? { _meta: metadata } : {}) } }) + '\n');
    await eventually(() => replies.some(item => item.id === sent));
    return replies.find(item => item.id === sent);
  };
  return { owner, dispatcher, request, async start() {
    if (era === 'legacy') {
      const initialized = await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'oauth-fixture', version: '1' } });
      assert.equal(initialized.result.protocolVersion, '2024-11-05');
      stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    }
  } };
}

for (const definition of nextProfiles) {
  for (const era of ['legacy', 'modern']) {
    test(`${era} MCP ${definition.profile} uses real donor and isolated explicit credentials`, async () => {
      const first = harness(definition, era, 'account-a'), second = harness(definition, era, 'account-b');
      try {
        await Promise.all([first.start(), second.start()]);
        const listed = await first.request('tools/list');
        const name = definition.profile.replaceAll('.', '_');
        assert.deepEqual(listed.result.tools.map(tool => tool.name), [name]);
        assert.equal(listed.result.tools[0].annotations.readOnlyHint, true);
        assert.equal(listed.result.tools[0].inputSchema.additionalProperties, false);
        assert.ok(!JSON.stringify(listed).includes(CANARY));
        const invalid = await first.request('tools/call', { name, arguments: { ...definition.args, token: CANARY } });
        assert.equal(invalid.result.isError, true);
        assert.equal(invalid.result.structuredContent.error.entered, false);
        assert.ok(!JSON.stringify(invalid).includes(CANARY));
        const calls = await Promise.all([first, second].map(h => h.request('tools/call', { name, arguments: definition.args })));
        for (const call of calls) {
          assert.equal(call.error, undefined);
          assert.equal(call.result.isError, undefined);
          assert.deepEqual(call.result.structuredContent.value, definition.result);
        }
        first.dispatcher.assertNoPendingInterceptors(); second.dispatcher.assertNoPendingInterceptors();
      } finally { await Promise.all([first.owner.close(), second.owner.close()]); }
    });
  }
  test(`MCP ${definition.profile} preserves safe HTTP error metadata`, async () => {
    for (const status of [401, 403, 429, 503]) {
      const h = harness(definition, 'modern', 'selected-account', status);
      try {
        const result = await h.request('tools/call', { name: definition.profile.replaceAll('.', '_'), arguments: definition.args });
        assert.equal(result.result.isError, true);
        assert.equal(result.result.structuredContent.error.kind, 'http_error');
        assert.equal(result.result.structuredContent.error.status, status);
        assert.equal(result.result.structuredContent.error.dispatched, true);
        assert.ok(!JSON.stringify(result).includes(CANARY));
        h.dispatcher.assertNoPendingInterceptors();
      } finally { await h.owner.close(); }
    }
  });
}

