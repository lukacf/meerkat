import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough } from 'node:stream';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { Dispatcher, MockAgent } from 'undici';
import { StdioServerTransport } from '@modelcontextprotocol/server/stdio';
import { PROTOCOL_VERSION_META_KEY, CLIENT_INFO_META_KEY, CLIENT_CAPABILITIES_META_KEY, InMemoryTransport } from '@modelcontextprotocol/server';
import { parseConfiguration, loadConfiguration, describeMcpConfiguration, createMcpServer, serveMcpStdio, createOwnedHttpTransport } from '../dist/mcp.js';

const CANARY = 'private-fixture-must-not-be-returned';
const config = { profile: 'tally.get-form', token_env: 'FIXTURE_TOKEN' };
const environment = { FIXTURE_TOKEN: 'fixture-token' };
const metadata = { [PROTOCOL_VERSION_META_KEY]: '2026-07-28', [CLIENT_INFO_META_KEY]: { name: 'fixture', version: '1' }, [CLIENT_CAPABILITIES_META_KEY]: {} };
const definitions = [
  { config, origin: 'https://api.tally.so', path: '/forms/form_1', args: { form_id: 'form_1' }, body: { id: 'form_1', enabled: false }, result: { id: 'form_1', enabled: false } },
  { config: { profile: 'asana.get-task', token_env: 'FIXTURE_TOKEN' }, origin: 'https://app.asana.com', path: /^\/api\/1\.0\/tasks\/123\?opt_fields=/, args: { task: '123' }, body: { data: { gid: '123' } }, result: { gid: '123' } },
  { config: { profile: 'nocodb.get-record', token_env: 'FIXTURE_TOKEN', origin: 'https://noco.example.invalid', version: 4 }, origin: 'https://noco.example.invalid', path: '/api/v3/data/base/table/records/42', args: { baseId: 'base', tableId: 'table', recordId: 42 }, body: { fields: { value: null } }, result: { fields: { value: null } } },
];
async function eventually(predicate) {
  const deadline = Date.now() + 4000;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error('Fixture timed out');
    await new Promise(resolve => setTimeout(resolve, 2));
  }
}
function harness(configuration = config, httpFactory = () => ({ send: async () => new Response('{"ok":true}'), close: async () => {} }), era = 'legacy', selectedEnvironment = environment) {
  const stdin = new PassThrough(), stdout = new PassThrough(), replies = [];
  let pending = '';
  stdout.on('data', chunk => {
    pending += chunk;
    let index;
    while ((index = pending.indexOf('\n')) >= 0) {
      const line = pending.slice(0, index); pending = pending.slice(index + 1);
      replies.push(JSON.parse(line));
    }
  });
  const owner = serveMcpStdio(configuration, { environment: selectedEnvironment, httpFactory, transport: new StdioServerTransport(stdin, stdout) });
  let id = 0;
  const send = (method, params = {}, requestId = ++id) => {
    stdin.write(JSON.stringify({ jsonrpc: '2.0', id: requestId, method, params: { ...params, ...(era === 'modern' ? { _meta: metadata } : {}) } }) + '\n');
    return requestId;
  };
  const reply = async requestId => { await eventually(() => replies.some(item => item.id === requestId)); return replies.find(item => item.id === requestId); };
  return { stdin, replies, owner, send, reply, async start() {
    if (era === 'legacy') {
      const value = await reply(send('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'fixture', version: '1' } }));
      assert.equal(value.result.protocolVersion, '2024-11-05');
      stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    }
  } };
}

for (const era of ['legacy', 'modern']) for (const definition of definitions) {
  test(`${era} official MCP wire executes real ${definition.config.profile} donor through owned HTTP`, async () => {
    const dispatcher = new MockAgent(); dispatcher.disableNetConnect();
    dispatcher.get(definition.origin).intercept({ method: 'GET', path: definition.path }).reply(200, definition.body);
    const h = harness(definition.config, () => createOwnedHttpTransport(dispatcher), era);
    try {
      await h.start();
      const list = await h.reply(h.send('tools/list'));
      assert.equal(list.error, undefined);
      const descriptor = list.result.tools[0];
      assert.equal(descriptor.annotations.readOnlyHint, true);
      assert.equal(descriptor.inputSchema.additionalProperties, false);
      assert.ok(!JSON.stringify(descriptor).includes('fixture-token'));
      const result = await h.reply(h.send('tools/call', { name: descriptor.name, arguments: definition.args }));
      assert.equal(result.error, undefined);
      assert.deepEqual(result.result.structuredContent.value, definition.result);
      dispatcher.assertNoPendingInterceptors();
    } finally { await h.owner.close(); }
    await assert.rejects(dispatcher.request({ origin: definition.origin, path: '/', method: 'GET' }), { code: 'UND_ERR_DESTROYED' });
  });
}

test('strict nonsecret configuration, exact env selection and describe need no provider setup', async () => {
  for (const value of [{ ...config, token: CANARY }, { ...config, token_env: '' }, { ...config, token_env: 'A,B' },
    { ...config, token_env: 'A\nB' }, { ...config, profile: CANARY }, { ...config, origin: 'https://extra.invalid' },
    { ...config, timeout_ms: null }, { ...config, max_response_bytes: 0 },
    { profile: 'nocodb.get-record', token_env: 'X', origin: 'https://user@x.invalid', version: 4 },
    { profile: 'nocodb.get-record', token_env: 'X', origin: 'https://x.invalid', version: '4' }]) {
    assert.throws(() => parseConfiguration(value), error => !String(error).includes(CANARY));
  }
  const description = describeMcpConfiguration(config);
  assert.equal(description.tools[0].name, 'tally_get-form');
  let owners = 0;
  const dependencies = { environment: { OTHER_TOKEN: 'fixture', TALLY_API_KEY: 'ambient' }, httpFactory() { owners++; throw 0; } };
  await assert.rejects(createMcpServer(config, dependencies), /configuration is invalid/);
  assert.equal(owners, 0);
  const dir = await mkdtemp(join(tmpdir(), 'toolkit-ap-mcp-config-'));
  for (const [name, contents] of [['oversized', ' '.repeat(8193)], ['bad-json', '{"token":"'+CANARY+'"'], ['bad-utf8', Buffer.from([0xff])]]) {
    const path = join(dir, name); await writeFile(path, contents);
    await assert.rejects(loadConfiguration(path), error => !String(error).includes(CANARY));
  }
});

test('unknown tools, task params and secret/extra input fields refuse donor dispatch', async () => {
  let calls = 0;
  const h = harness(config, () => ({ send: async () => { calls++; throw 0; }, close: async () => {} }));
  try {
    await h.start();
    for (const params of [
      { name: 'other', arguments: { secret: CANARY } },
      { name: 'tally_get-form', arguments: { form_id: 'a', token: CANARY } },
      { name: 'tally_get-form', arguments: { form_id: '../x' } },
      { name: 'tally_get-form', arguments: { form_id: 'a' }, task: { ttl: 1000 } },
    ]) {
      const result = await h.reply(h.send('tools/call', params));
      assert.ok(result.error || result.result.isError);
      assert.ok(!JSON.stringify(result).includes(CANARY));
    }
    assert.equal(calls, 0);
  } finally { await h.owner.close(); }
});

test('owned HTTP refuses redirects and does not retry 401, 429 or server failures', async () => {
  for (const status of [302, 401, 429, 503]) {
    const dispatcher = new MockAgent(); dispatcher.disableNetConnect();
    dispatcher.get('https://api.tally.so').intercept({ method: 'GET', path: '/forms/a' }).reply(status, CANARY, { headers: { location: 'https://other.invalid/' } });
    const h = harness(config, () => createOwnedHttpTransport(dispatcher));
    try {
      await h.start();
      const result = await h.reply(h.send('tools/call', { name: 'tally_get-form', arguments: { form_id: 'a' } }));
      assert.equal(result.result.isError, true);
      assert.equal(result.result.structuredContent.error.kind, status === 302 ? 'redirect_refused' : 'http_error');
      assert.ok(!JSON.stringify(result).includes(CANARY));
      dispatcher.assertNoPendingInterceptors();
    } finally { await h.owner.close(); }
    await assert.rejects(dispatcher.request({ origin: 'https://api.tally.so', path: '/', method: 'GET' }), { code: 'UND_ERR_DESTROYED' });
  }
});

test('owned HTTP handles malformed, oversized and pre-header cancellation without leaking upstream text', async () => {
  for (const [body, expected] of [[CANARY, 'invalid_response'], ['x'.repeat(2048), 'response_too_large']]) {
    const dispatcher = new MockAgent(); dispatcher.disableNetConnect();
    dispatcher.get('https://api.tally.so').intercept({ method: 'GET', path: '/forms/a' }).reply(200, body);
    const h = harness({ ...config, max_response_bytes: 1024 }, () => createOwnedHttpTransport(dispatcher));
    try {
      await h.start();
      const result = await h.reply(h.send('tools/call', { name: 'tally_get-form', arguments: { form_id: 'a' } }));
      assert.equal(result.result.structuredContent.error.kind, expected);
      assert.ok(!JSON.stringify(result).includes(CANARY));
    } finally { await h.owner.close(); }
  }
  const dispatcher = new MockAgent(); dispatcher.disableNetConnect();
  dispatcher.get('https://api.tally.so').intercept({ method: 'GET', path: '/forms/a' }).reply(200, '{}').delay(500);
  const http = createOwnedHttpTransport(dispatcher), abort = new AbortController();
  const pending = http.send('https://api.tally.so/forms/a', { method: 'GET', redirect: 'error', signal: abort.signal });
  abort.abort(); await assert.rejects(pending); await http.close();
});

test('SDK startup, factory and wire failures terminate owned serving with fixed diagnostics', { timeout: 10000 }, async () => {
  let wireClosed = false;
  const owner = serveMcpStdio(config, { environment, transport: {
    async start() { throw new Error(CANARY); }, async send() {}, async close() { wireClosed = true; },
  }, onerror() { throw new Error(CANARY); } });
  await assert.rejects(owner.closed, error => String(error).includes('cleanup failed') && !String(error).includes(CANARY));
  assert.equal(wireClosed, true);
  const h = harness(config, () => { throw new Error(CANARY); });
  h.send('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'fixture', version: '1' } });
  await assert.rejects(h.owner.closed, error => !String(error).includes(CANARY));
  assert.ok(!JSON.stringify(h.replies).includes(CANARY));
  const badWire = harness();
  badWire.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 44, private: CANARY }) + '\n');
  await assert.rejects(badWire.owner.closed, error => !String(error).includes(CANARY));
});

test('owned HTTP joins an actual Undici body with a pending read when MCP cancels after headers', async () => {
  let headersSent = false, aborted = false, closed = false;
  class HeldDispatcher extends Dispatcher {
    dispatch(_options, handler) {
      const controller = { paused: false, pause() {}, resume() {}, abort() {
        if (aborted) return;
        aborted = true;
        queueMicrotask(() => {
          handler.onResponseError(controller, new Error(CANARY));
          handler.onResponseData(controller, Buffer.from(CANARY));
        });
      } };
      handler.onRequestStart(controller, null);
      handler.onResponseStart(controller, 200, { 'content-type': 'application/json' }, 'OK');
      headersSent = true;
      return true;
    }
    async close() { assert.equal(aborted, true); closed = true; }
  }
  const h = harness(config, () => createOwnedHttpTransport(new HeldDispatcher()));
  await h.start();
  const id = h.send('tools/call', { name: 'tally_get-form', arguments: { form_id: 'a' } });
  await eventually(() => headersSent);
  h.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: id } }) + '\n');
  await eventually(() => aborted);
  await h.owner.close();
  assert.equal(closed, true);
  assert.ok(!h.replies.some(item => item.id === id));
});

test('MCP cancellation joins late body cleanup before dispatcher closure and preserves another call', async () => {
  let started = false, cancelStarted = false, closed = false, releaseCancel;
  const stream = new ReadableStream({ cancel() { cancelStarted = true; return new Promise(resolve => { releaseCancel = resolve; }); } });
  let calls = 0;
  const h = harness(config, () => ({ send: async () => { calls++; if (calls === 1) { started = true; return new Response(stream); } return new Response('{"other":true}'); }, close: async () => { assert.equal(stream.locked, false); closed = true; } }));
  await h.start();
  const first = h.send('tools/call', { name: 'tally_get-form', arguments: { form_id: 'a' } });
  await eventually(() => started);
  h.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: first, reason: CANARY } }) + '\n');
  await eventually(() => cancelStarted);
  const second = await h.reply(h.send('tools/call', { name: 'tally_get-form', arguments: { form_id: 'b' } }));
  assert.equal(second.result.structuredContent.value.other, true);
  h.stdin.end();
  let settled = false; h.owner.closed.then(() => { settled = true; });
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(closed, false); assert.equal(settled, false);
  releaseCancel();
  await h.owner.closed;
  assert.equal(closed, true);
  assert.ok(!h.replies.some(item => item.id === first));
});

test('EOF aborts pre-header call, waits late response and joins owner close rejection safely', async () => {
  let entered = false, giveResponse, bodyClosed = false;
  const stream = new ReadableStream({ cancel() { bodyClosed = true; } });
  const h = harness(config, () => ({ send: async () => { entered = true; return new Promise(resolve => { giveResponse = resolve; }); }, close: async () => { assert.equal(bodyClosed, true); throw new Error(CANARY); } }));
  await h.start(); h.send('tools/call', { name: 'tally_get-form', arguments: { form_id: 'a' } });
  await eventually(() => entered); h.stdin.end();
  let settled = false; h.owner.closed.then(() => { settled = true; }, () => { settled = true; });
  await new Promise(resolve => setTimeout(resolve, 10)); assert.equal(settled, false);
  giveResponse(new Response(stream));
  await assert.rejects(h.owner.closed, error => String(error).includes('cleanup failed') && !String(error).includes(CANARY));
  assert.equal(stream.locked, false);
});

test('per-server credentials stay isolated and explicit concurrency cap refuses excess calls', async () => {
  const gates = [], observed = [];
  function factory(label) { return () => ({ send: async (_url, init) => {
    observed.push([label, new Headers(init.headers).get('authorization')]);
    return new Promise(resolve => gates.push(() => resolve(new Response('{}'))));
  }, close: async () => {} }); }
  const first = harness(config, factory('a'));
  const second = harness(config, factory('b'), 'legacy', { FIXTURE_TOKEN: 'another-account-token' });
  await first.start(); await second.start();
  for (let i=0;i<4;i++) first.send('tools/call', { name:'tally_get-form', arguments:{form_id:'a'} });
  await eventually(() => observed.length === 4);
  const excess = await first.reply(first.send('tools/call', { name:'tally_get-form', arguments:{form_id:'a'} }));
  assert.equal(excess.result.structuredContent.error.kind, 'server_busy');
  second.send('tools/call', { name:'tally_get-form', arguments:{form_id:'b'} });
  await eventually(() => observed.length === 5);
  assert.deepEqual(observed.map(x=>x[0]), ['a','a','a','a','b']);
  assert.deepEqual(observed.map(x=>x[1]), [...Array(4).fill('Bearer fixture-token'), 'Bearer another-account-token']);
  gates.forEach(release=>release());
  await first.owner.close(); await second.owner.close();
});

test('direct imported server closes its owned HTTP transport on SDK transport EOF', async () => {
  let closed = false;
  const server = await createMcpServer(config, { environment, httpFactory: () => ({ send: async () => new Response('{}'), close: async () => { closed = true; } }) });
  const [client, transport] = InMemoryTransport.createLinkedPair();
  await server.connect(transport); await client.start();
  await client.close(); await server.close();
  assert.equal(closed, true);
});

async function childResult(args, input = '', env = {}) {
  const child = spawn(process.execPath, args, { cwd: new URL('..', import.meta.url), env, stdio: ['pipe','pipe','pipe'] });
  let stdout='',stderr=''; child.stdout.on('data',d=>stdout+=d); child.stderr.on('data',d=>stderr+=d);
  const exited=once(child,'exit'); child.stdin.end(input); const [code,signal]=await exited;
  return {stdout,stderr,code,signal};
}
test('CLI describe, invalid config and raw legacy stdio keep stdout clean without provider calls', async () => {
  const dir=await mkdtemp(join(tmpdir(),'toolkit-ap-mcp-cli-')),path=join(dir,'config.json');
  await writeFile(path,JSON.stringify(config));
  const description=await childResult(['dist/mcp-cli.js','describe','--config',path]);
  assert.equal(description.code,0);assert.equal(description.stderr,'');assert.equal(JSON.parse(description.stdout).tools[0].name,'tally_get-form');
  const failed=await childResult(['dist/mcp-cli.js','serve','--config',path], '', {OTHER_TOKEN:CANARY});
  assert.equal(failed.code,1);assert.equal(failed.stdout,'');assert.ok(!failed.stderr.includes(CANARY));
  const oversized=await childResult(['dist/mcp-cli.js','serve','--config',path], 'x'.repeat(65537), environment);
  assert.equal(oversized.code,1);assert.equal(oversized.stdout,'');assert.ok(oversized.stderr.includes('protocol error'));
  const child=spawn(process.execPath,['dist/mcp-cli.js','serve','--config',path],{cwd:new URL('..',import.meta.url),env:environment,stdio:['pipe','pipe','pipe']});
  let output='',diagnostics='';child.stdout.on('data',d=>output+=d);child.stderr.on('data',d=>diagnostics+=d);
  const exited=once(child,'exit');
  child.stdin.write(JSON.stringify({jsonrpc:'2.0',id:1,method:'initialize',params:{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'fixture',version:'1'}}})+'\n');
  await eventually(()=>output.includes('"id":1'));
  child.stdin.write(JSON.stringify({jsonrpc:'2.0',id:2,method:'tools/list',params:{}})+'\n');
  await eventually(()=>output.includes('"id":2'));child.stdin.end();
  const [code]=await exited;assert.equal(code,0);assert.equal(diagnostics,'');
  const lines=output.trim().split('\n').map(x=>JSON.parse(x));assert.equal(lines.length,2);assert.equal(lines[1].result.tools[0].annotations.readOnlyHint,true);
});

test('raw subprocess fixture uses real donor and waits for body then owner on EOF', async () => {
  const child=spawn(process.execPath,['tests/fixtures/mcp-child.mjs'],{cwd:new URL('..',import.meta.url),env:{},stdio:['pipe','pipe','pipe']});
  let output='',diagnostics='';child.stdout.on('data',d=>output+=d);child.stderr.on('data',d=>diagnostics+=d);
  const exited=once(child,'exit');
  const send=(id,method,params)=>child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params:{...params,_meta:metadata}})+'\n');
  send(1,'tools/call',{name:'tally_get-form',arguments:{form_id:'ready'}});
  await eventually(()=>output.includes('"id":1'));assert.equal(JSON.parse(output.trim()).result.structuredContent.value.fixture,true);
  send(2,'tools/call',{name:'tally_get-form',arguments:{form_id:'held'}});
  await eventually(()=>diagnostics.includes('fixture request entered'));child.stdin.end();
  const [code]=await exited;assert.equal(code,0);assert.equal(output.trim().split('\n').length,1);
  assert.match(diagnostics,/fixture request entered\nfixture body closed\nfixture owner closed\n/);
});
