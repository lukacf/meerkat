import test from 'node:test';
import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { inspect } from 'node:util';
import { mkdtempSync, writeFileSync, chmodSync, readFileSync, existsSync, unlinkSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { setTimeout as delay } from 'node:timers/promises';
import { ServiceSetupClient, ServiceSetupLaunchError, ServiceSetupTimeout } from '../dist/service-setup.js';

// Artificial peer with actual OS pipes/process custody. No native/runtime,
// provider credentials, OAuth, network or resource capability is exercised.
const PEER = String.raw`
import {createInterface} from 'node:readline';
import {writeFileSync,readFileSync,appendFileSync,existsSync,renameSync} from 'node:fs';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
const rl=createInterface({input:process.stdin}),lines=rl[Symbol.asyncIterator]();
setTimeout(()=>process.exit(74),8000).unref();
const first=await lines.next();if(first.done)process.exit(0);
const init=JSON.parse(first.value),root=init.params.realm.context_root,mode=readFileSync(join(root,'mode'),'utf8');
process.on('exit',code=>writeFileSync(join(root,'exited.json'),JSON.stringify({pid:process.pid,code})));
writeFileSync(join(root,'observed.json'),JSON.stringify({pid:process.pid,argv:process.argv.slice(2),env:process.env,initial:init}));
const emit=v=>process.stdout.write(JSON.stringify(v)+'\n');
const reply=(id,result)=>emit({jsonrpc:'2.0',id,result});
const target={scope:'actual'},early={initialize_request_id:init.id};
const kinds=['approval','provider_target','provider_action','provider_api_key','provider_browser','provider_device','service_secret'];
const initialized=()=>reply(init.id,{protocol_version:5,scope:'actual',session_id:'native-session',capabilities:{prompt_kinds:kinds,service_preparation:true,forge_configuration:false,paired_host:false,tools:[]},state:{}});
const event=(kind,rest={},where=target)=>emit({jsonrpc:'2.0',method:'connect/event',params:{target:where,kind,...rest}});
let prompt=null,send=null;
const offer=(id,p,where=target)=>{prompt=id;emit({jsonrpc:'2.0',method:'connect/prompt',id,params:{target:where,prompt:p}});};
const finishPrompt=()=>{if(prompt){event('prompt_closed',{prompt_id:prompt});prompt=null;}};
const finishSend=(id,completion='completed',overrides={})=>reply(id,{scope:'actual',send_request_id:id,completion,state:{},...overrides});
const exit=async code=>{await new Promise(resolve=>process.stdout.write('',resolve));process.exit(code);};
if(mode==='preinit')offer('p:early',{kind:'provider_action',actions:['use']},early);
else if(mode==='bad-version')reply(init.id,{protocol_version:4,scope:'actual',session_id:'native-session',capabilities:{service_preparation:true,prompt_kinds:kinds},state:{}});
else initialized();
if(mode==='backpressure'){
 rl.close();process.stdin.pause();event('state',{value:'paused'});
 while(existsSync(join(root,'hold')))await delay(5);
 offer('p:late',{kind:'provider_device',user_code:'PRIVATE-LATE',verification_uri:'https://secret.invalid'});
 finishPrompt();await exit(0);
}
if(mode==='foreign')event('state',{}, {scope:'foreign'});
if(mode==='duplicate')process.stdout.write('{"jsonrpc":"2.0","method":"connect/event","params":{"target":{"scope":"actual"},"kind":"state","x":1,"x":2}}\n');
if(mode==='nonfinite')process.stdout.write('{"jsonrpc":"2.0","method":"connect/event","params":{"target":{"scope":"actual"},"kind":"state","x":1e999}}\n');
if(mode==='invalid-utf8')process.stdout.write(Buffer.from([0xff,0x0a]));
if(mode==='partial') {process.stdout.write('{"jsonrpc":');await exit(0);}
const floodMark=name=>{
 const path=join(root,name);writeFileSync(path+'.tmp',JSON.stringify({pid:process.pid}));renameSync(path+'.tmp',path);
};
if(mode==='flood'){
 floodMark('flood-ready.json');
 while(existsSync(join(root,'hold')))await delay(5);
 for(let i=0;i<140;i++)event('state',{index:i});
}
if(mode==='fragmented'){
 const frame=JSON.stringify({jsonrpc:'2.0',method:'connect/event',params:{target,kind:'state',blob:'x'.repeat(2*1024*1024)}})+'\n';
 for(let i=0;i<frame.length;i+=4096)await new Promise(resolve=>process.stdout.write(frame.slice(i,i+4096),resolve));
}
for await(const line of lines){
 const request=JSON.parse(line);appendFileSync(join(root,'requests.jsonl'),JSON.stringify(request)+'\n');
 if(request.method==='connect/send'){
  send=request.id;
  if(mode==='error')emit({jsonrpc:'2.0',id:send,error:{code:-32000,message:'PRIVATE-PROVIDER-BODY',data:{code:'PRIVATE-PROVIDER-CODE',nested:'PRIVATE-NESTED'}}});
  else if(mode==='bad-send')finishSend(send,'completed',{send_request_id:'wrong'});
  else if(mode==='unknown-outcome')finishSend(send,'outcome_unknown');
  else if(mode==='cancel'||mode==='late-cancel'||mode==='bad-cancel')event('state',{value:'running'});
  else if(mode==='approval')offer('p:approval',{kind:'approval'});
  else if(mode==='provider-browser')offer('p:browser',{kind:'provider_browser',url:'https://private.invalid/token'});
  else offer('p:secret',{kind:'service_secret',job_id:'job',service:'service',slot:'slot',label:'Private key'});
 }else if(request.method==='connect/cancel'){
  if(mode==='late-cancel'){
   offer('p:late',{kind:'provider_device',user_code:'PRIVATE-LATE'});finishPrompt();
  }
  finishSend(send,'cancelled');reply(request.id,{scope:'actual',send_request_id:request.params.send_request_id,status:mode==='bad-cancel'?['not_current']:'not_current'});
 }else if(request.method==='connect/connector/login/start')reply(request.id,{scope:'actual',started:{state:'native-state'}});
 else if(request.method==='connect/connector/login/complete')reply(request.id,{scope:'actual',completed:{state:'native-completion'}});
 else if(request.method==='connect/connector/login/cancel')reply(request.id,{scope:'actual',cancelled:true});
 else if(request.method==='connect/close'){
  while(existsSync(join(root,'hold')))await delay(5);
  if(mode==='preinit'&&prompt){event('prompt_closed',{prompt_id:prompt},early);prompt=null;}
  else finishPrompt();
  if(mode!=='no-receipt')reply(request.id,{target:request.params.target,resources_closed:true});
  await exit(mode==='exit-failure'?9:0);
 }else if(request.result?.answer){
  if(mode==='preinit'){
   event('prompt_closed',{prompt_id:prompt},early);prompt=null;initialized();
  }else{finishPrompt();finishSend(send,request.result.answer.kind==='cancel'?'cancelled':'completed');}
 }
}
if(mode==='flood')floodMark('flood-input-ended.json');
await exit(0);
`;

function fixture(t, mode = 'normal') {
  const root = mkdtempSync(join(tmpdir(), 'service-pipe-')), executable = join(root, 'peer.mjs');
  writeFileSync(executable, '#!' + process.execPath + '\n' + PEER); chmodSync(executable, 0o700);
  writeFileSync(join(root, 'mode'), mode);
  const binding = { application_id: 'app', instance_id: 'instance', origin: 'https://host.invalid',
    principal_id: 'principal', login_scope_id: 'login', expires_at_ms: Date.now() + 120000,
    realm_id: 'realm', context_root: root, state_root: join(root, 'state'), user_config_root: join(root, 'config'),
    resource_manifest_sha256: 'a'.repeat(64), credential_scope: { kind: 'file', root: join(root, 'credentials') } };
  const clients = [];
  t.after(async () => {
    if (existsSync(join(root, 'hold'))) unlinkSync(join(root, 'hold'));
    let failure, settled = true;
    for (const client of clients) {
      try {
        const closed = await client.close(10000);
        assert.equal(closed.process_joined, true);
        assert.notEqual(closed.exit_code, 74, 'fixture watchdog must never count as success');
      } catch (error) { failure ??= error; settled = false; }
    }
    if (settled) rmSync(root, { recursive: true, force: true });
    if (failure) throw failure;
  });
  const launch = (environment) => {
    try { const client = new ServiceSetupClient(executable, binding, environment); clients.push(client); return client; }
    catch (error) { if (error instanceof ServiceSetupLaunchError) clients.push(error.owner); throw error; }
  };
  return { root, executable, binding, clients, launch,
    async open(environment) { const client = launch(environment); const initial = await client.nextEvent(5000);
      assert.equal(initial.kind, 'result'); assert.equal(initial.payload.method, 'connect/initialize'); return client; },
    observed() { return JSON.parse(readFileSync(join(root, 'observed.json'), 'utf8')); },
    requests() { const file = join(root, 'requests.jsonl'); return existsSync(file) ? readFileSync(file, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse) : []; } };
}
async function until(client, predicate) {
  for (let i = 0; i < 140; i++) { const event = await client.nextEvent(5000); if (predicate(event)) return event; }
  assert.fail('expected event was not observed');
}
function answer(value = 'SYNTHETIC-PRIVATE-KEY') { return { kind: 'service_secret', job_id: 'job', service: 'service', slot: 'slot', value }; }

test('protected answers bind exact selectors; frozen private events redact inspection', async t => {
  const f = fixture(t), client = await f.open(), send = client.submit('Prepare the selected service');
  const prompt = await client.nextEvent(5000);
  assert.equal(prompt.kind, 'prompt'); assert.equal(prompt.payload.kind, 'service_secret');
  for (const change of [{ job_id: 'wrong' }, { slot: 'wrong' }, { service: 'wrong' }, { value: '\ud800' }, { value: '\0' }, { value: 'x'.repeat(16385) }, { unknown: 'private' }]) {
    assert.throws(() => client.answer(prompt.request_id, { ...answer(), ...change }), /invalid protected answer/);
  }
  assert.throws(() => { prompt.payload.slot = 'mutated'; }, TypeError);
  assert.deepEqual(JSON.parse(JSON.stringify(prompt)), { kind: 'prompt', request_id: 'p:secret' });
  assert.ok(!inspect(prompt).includes('Private key'));
  client.answer(prompt.request_id, answer());
  assert.throws(() => client.answer(prompt.request_id, answer()), /not current/);
  const result = await until(client, e => e.request_id === send);
  assert.equal(result.payload.result.completion, 'completed');
  assert.equal((await client.close(5000)).native_resources_closed, true);
  assert.deepEqual(f.requests().filter(r => r.result).map(r => r.result.answer), [answer()]);
});

test('launch freezes binding and excludes ambient credentials from the real child', async t => {
  const f = fixture(t), launches = [], original = childProcess.spawn;
  childProcess.spawn = (...args) => { launches.push({ ...args[2].env }); return original(...args); }; syncBuiltinESMExports();
  let client;
  try { client = f.launch(); f.binding.realm_id = 'changed'; f.binding.credential_scope.root = '/changed'; }
  finally { childProcess.spawn = original; syncBuiltinESMExports(); }
  await client.nextEvent(5000);
  assert.deepEqual(launches, [{ PATH: '/usr/bin:/bin', LANG: 'C' }]);
  const observed = f.observed(); assert.equal(observed.initial.params.realm.realm_id, 'realm');
  assert.deepEqual(observed.argv, ['--stdio', '--provider-store-root', join(f.root, 'credentials')]);
  assert.equal(observed.initial.params.protocol_version, 5);
  assert.ok(!('connector_auth' in observed.initial.params));
  for (const key of ['HOME', 'OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'RKAT_CONFIG']) assert.ok(!(key in observed.env));
  const extras = Object.keys(observed.env).filter(k => !['PATH', 'LANG'].includes(k));
  assert.deepEqual(extras, process.platform === 'darwin' && '__CF_USER_TEXT_ENCODING' in observed.env ? ['__CF_USER_TEXT_ENCODING'] : []);
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('invalid binding and private initializer fields refuse before any child', async t => {
  const f = fixture(t);
  for (const changes of [{ auth_binding: { realm: 'r', binding: 'b', origin: 'configured' } },
    { connector_auth: { profiles: [] } }, { expires_at_ms: Infinity }, { expires_at_ms: 1 },
    { origin: 'https://host.invalid/path' }, { origin: 'https://secret@host.invalid' }, { context_root: 'relative' },
    { credential_scope: { kind: 'file', root: 'relative' } }, { adapter: 'forge_application' }]) {
    assert.throws(() => new ServiceSetupClient(f.executable, { ...f.binding, ...changes }), /invalid trusted service/);
  }
  for (const key of ['OPENAI_API_KEY', 'HOME', '__CF_USER_TEXT_ENCODING']) {
    assert.throws(() => new ServiceSetupClient(f.executable, f.binding, { [key]: 'PRIVATE' }), /invalid trusted service/);
  }
  await delay(25); assert.ok(!existsSync(join(f.root, 'observed.json')));
});

test('native-default credentials require a fixed explicit HOME and use only stdio', async t => {
  const f = fixture(t); f.binding.credential_scope = { kind: 'native_default' };
  assert.throws(() => f.launch(), /invalid trusted service/);
  const client = await f.open({ PATH: '/usr/bin:/bin', LANG: 'C', HOME: join(f.root, 'fixed') });
  assert.equal(f.observed().env.HOME, join(f.root, 'fixed')); assert.deepEqual(f.observed().argv, ['--stdio']);
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('provider prompts during initialization retain the original request target', async t => {
  const f = fixture(t, 'preinit'), client = f.launch();
  const prompt = await client.nextEvent(5000); assert.equal(prompt.kind, 'prompt');
  assert.throws(() => client.submit('not initialized'), /initialization pending/);
  assert.throws(() => client.answer(prompt.request_id, { kind: 'provider_action', action: ['use'] }), /invalid protected answer/);
  client.answer(prompt.request_id, { kind: 'provider_action', action: 'use' });
  assert.equal((await client.nextEvent(5000)).kind, 'prompt_closed');
  assert.equal((await client.nextEvent(5000)).payload.method, 'connect/initialize');
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('close before initialization retains its exact target even after init response', async t => {
  const f = fixture(t), client = f.launch();
  assert.equal((await client.close(5000)).native_resources_closed, true);
  assert.deepEqual(f.requests().find(r => r.method === 'connect/close').params.target, { initialize_request_id: 'h:initialize' });
});

test('close during provider initialization makes late prompts non-actionable', async t => {
  const f = fixture(t, 'preinit'), client = f.launch(), closing = client.close(5000);
  const prompt = await client.nextEvent(5000); assert.equal(prompt.kind, 'retired_prompt');
  assert.deepEqual(prompt.payload, { prompt_kind: 'provider_action' });
  assert.throws(() => client.answer(prompt.request_id, { kind: 'provider_action', action: 'use' }), /unavailable/);
  assert.equal((await closing).native_resources_closed, true);
});

test('send cancellation is permanent and preserves late prompt closure and not_current', async t => {
  const f = fixture(t, 'late-cancel'), client = await f.open(), send = client.submit('prepare');
  assert.equal((await client.nextEvent(5000)).kind, 'state');
  const cancel = client.cancel(send);
  assert.throws(() => client.submit('retry'), /unavailable/);
  assert.throws(() => client.connectorCancel(), /unavailable/);
  const late = await client.nextEvent(5000); assert.equal(late.kind, 'retired_prompt');
  assert.deepEqual(late.payload, { prompt_kind: 'provider_device' });
  assert.ok(!JSON.stringify(late.payload).includes('PRIVATE-LATE'));
  assert.equal((await client.nextEvent(5000)).kind, 'prompt_closed');
  const result = await until(client, e => e.request_id === cancel);
  assert.equal(result.payload.result.status, 'not_current');
  assert.throws(() => client.submit('still retired'), /unavailable/);
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('cancel answer retires the client permanently', async t => {
  const f = fixture(t), client = await f.open(), send = client.submit('prepare');
  const prompt = await client.nextEvent(5000); client.answer(prompt.request_id, { kind: 'cancel' });
  assert.throws(() => client.submit('retry'), /unavailable/);
  assert.equal((await until(client, e => e.request_id === send)).payload.result.completion, 'cancelled');
  assert.throws(() => client.answer(prompt.request_id, answer()), /unavailable/);
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('real pipe backpressure preserves bounded close and the same native owner', async t => {
  const f = fixture(t, 'backpressure'); writeFileSync(join(f.root, 'hold'), '');
  const client = await f.open(); assert.equal((await client.nextEvent(5000)).kind, 'state');
  const pid = client.pid; client.submit('x'.repeat(4 * 1024 * 1024));
  const before = performance.now(); await assert.rejects(client.close(30), ServiceSetupTimeout);
  assert.ok(performance.now() - before < 1000); process.kill(pid, 0); assert.equal(client.pid, pid);
  unlinkSync(join(f.root, 'hold'));
  const closed = await client.close(5000); assert.equal(closed.process_joined, true); assert.equal(closed.native_resources_closed, false);
  assert.equal(JSON.parse(readFileSync(join(f.root, 'exited.json'))).pid, pid);
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
});

test('cancellation retires even when the actual blocked writer queue is full', async t => {
  const f = fixture(t, 'backpressure'); writeFileSync(join(f.root, 'hold'), '');
  const client = await f.open(); await client.nextEvent(5000);
  const send = client.submit('x'.repeat(4 * 1024 * 1024)); let filled = false;
  for (let i = 0; i < 256; i++) {
    try { client.connectorCancel(); } catch (error) { assert.match(error.message, /queue unavailable/); filled = true; break; }
  }
  assert.equal(filled, true); assert.throws(() => client.cancel(send), /cancellation outcome unknown/);
  assert.throws(() => client.submit('cannot retry'), /unavailable/);
  unlinkSync(join(f.root, 'hold')); assert.equal((await client.close(5000)).native_resources_closed, false);
});

test('fragmented large native frames retain their exact payload without losing boundaries', async t => {
  const f = fixture(t, 'fragmented'), client = await f.open(), state = await client.nextEvent(5000);
  assert.equal(state.kind, 'state'); assert.equal(state.payload.blob, 'x'.repeat(2 * 1024 * 1024));
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('close observation timeout does not resend, replace or kill the child', async t => {
  const f = fixture(t); writeFileSync(join(f.root, 'hold'), ''); const client = await f.open(), pid = client.pid;
  await assert.rejects(client.close(20), ServiceSetupTimeout); process.kill(pid, 0);
  unlinkSync(join(f.root, 'hold')); assert.equal((await client.close(5000)).native_resources_closed, true);
  assert.equal(f.requests().filter(r => r.method === 'connect/close').length, 1); assert.equal(client.pid, pid);
});

test('invalid timeout values and expired observation do not retire preparation', async t => {
  const f = fixture(t), client = await f.open();
  for (const timeout of [0, -1, NaN, Infinity, 2147483648]) {
    await assert.rejects(client.close(timeout), /invalid observation timeout/);
    await assert.rejects(client.nextEvent(timeout), /invalid observation timeout/);
  }
  await assert.rejects(client.nextEvent(10), ServiceSetupTimeout);
  const send = client.submit('still available'); const prompt = await client.nextEvent(5000); client.answer(prompt.request_id, answer());
  await until(client, e => e.request_id === send); assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('native failures redact provider text and unrecognized codes', async t => {
  const f = fixture(t, 'error'), client = await f.open(), send = client.submit('prepare');
  const refusal = await client.nextEvent(5000);
  assert.equal(refusal.request_id, send); assert.equal(refusal.kind, 'refused');
  assert.deepEqual(refusal.payload, { method: 'connect/send', code: 'native_refused' });
  assert.ok(!inspect(refusal).includes('PRIVATE')); assert.ok(!JSON.stringify(refusal.payload).includes('PRIVATE'));
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('outcome_unknown remains an observed completion without readiness claims', async t => {
  const f = fixture(t, 'unknown-outcome'), client = await f.open(), send = client.submit('prepare');
  const result = await client.nextEvent(5000); assert.equal(result.request_id, send);
  assert.equal(result.payload.result.completion, 'outcome_unknown');
  assert.ok(!('ready' in result.payload)); assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('corrupt frames, foreign scopes and mismatched completions cannot confirm cleanup', async t => {
  for (const mode of ['foreign', 'duplicate', 'nonfinite', 'invalid-utf8', 'partial', 'bad-version', 'bad-send', 'bad-cancel']) {
    await t.test(mode, async t => {
      const f = fixture(t, mode), client = f.launch();
      if (mode === 'bad-send') { await client.nextEvent(5000); client.submit('prepare'); }
      if (mode === 'bad-cancel') { await client.nextEvent(5000); const send = client.submit('prepare'); await client.nextEvent(5000); client.cancel(send); }
      await assert.rejects(async () => { for (;;) await client.nextEvent(5000); }, /transport failed/);
      assert.equal((await client.close(5000)).native_resources_closed, false);
    });
  }
});

test('inbound event overflow retires input before the observer drains the queue', async t => {
  const f = fixture(t, 'flood'); writeFileSync(join(f.root, 'hold'), '');
  const client = f.launch(), pid = client.pid;
  async function marker(name) {
    const path = join(f.root, name), deadline = Date.now() + 5000;
    while (!existsSync(path)) {
      assert.ok(Date.now() < deadline, 'fixture did not reach ' + name);
      await delay(5);
    }
    assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), { pid });
  }
  // Wait for this actual peer, independent of process startup latency. Do not
  // consume the initialize response or any state event while it floods stdout.
  await marker('flood-ready.json');
  unlinkSync(join(f.root, 'hold'));
  // Overflow must itself close the original input. A close() call here would
  // hide a missing queue bound by creating the EOF that this marker observes.
  await marker('flood-input-ended.json');
  await assert.rejects(async () => { for (;;) await client.nextEvent(5000); }, /transport failed/);
  const closed = await client.close(5000);
  assert.equal(closed.process_joined, true);
  assert.equal(closed.exit_code, 0);
  assert.equal(closed.native_resources_closed, false);
  assert.equal(client.pid, pid);
  assert.deepEqual(f.requests(), []);
});

test('approval answers require actual string enum values before forwarding', async t => {
  const f = fixture(t, 'approval'), client = await f.open(), send = client.submit('prepare');
  const prompt = await client.nextEvent(5000);
  assert.throws(() => client.answer(prompt.request_id, { kind: 'approval', decision: ['approve'] }), /invalid protected answer/);
  client.answer(prompt.request_id, { kind: 'approval', decision: 'approve' });
  await until(client, e => e.request_id === send);
  assert.equal((await client.close(5000)).native_resources_closed, true);
  assert.deepEqual(f.requests().filter(r => r.result).map(r => r.result.answer), [{ kind: 'approval', decision: 'approve' }]);
});

test('a process exit or a receipt followed by unsuccessful exit is not cleanup proof', async t => {
  for (const mode of ['no-receipt', 'exit-failure']) await t.test(mode, async t => {
    const f = fixture(t, mode), client = await f.open(), closed = await client.close(5000);
    assert.equal(closed.process_joined, true); assert.equal(closed.native_resources_closed, false);
    assert.equal(closed.exit_code, mode === 'no-receipt' ? 0 : 9);
  });
});

test('connector inputs are closed and successful observations preserve native payloads', async t => {
  const f = fixture(t), client = await f.open();
  const selection = { job_id: 'job', connector: { slot_id: 'slot', issuer: 'issuer', client: 'client', resource: 'resource',
    scopes: ['read'], strategy_id: 'strategy', account_selection: { mode: 'unverified' } }, redirect_uri: 'https://host.invalid/callback',
    operations: { read: { destination_origin: 'https://service.invalid', required_scopes: ['read'], read_only: true } } };
  assert.throws(() => client.connectorStart({ ...selection, connector: { ...selection.connector, client_secret: 'PRIVATE' } }), /closed native connector/);
  assert.throws(() => client.connectorStart({ ...selection, operations: { read: { ...selection.operations.read, private: 'PRIVATE' } } }), /closed native connector/);
  assert.throws(() => client.connectorComplete({ state: 'state', code: 'code', client_secret: 'PRIVATE' }), /closed connector/);
  const start = client.connectorStart(selection); assert.equal((await until(client, e => e.request_id === start)).payload.result.started.state, 'native-state');
  const complete = client.connectorComplete({ state: 'state', code: 'synthetic-code' });
  assert.equal((await until(client, e => e.request_id === complete)).payload.result.completed.state, 'native-completion');
  const cancel = client.connectorCancel(); assert.equal((await until(client, e => e.request_id === cancel)).payload.result.cancelled, true);
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('post-spawn listener startup failure returns the actual retained owner', async t => {
  const f = fixture(t), original = childProcess.spawn; let actual;
  childProcess.spawn = (...args) => {
    actual = original(...args); const on = actual.stdout.on.bind(actual.stdout);
    actual.stdout.on = (name, ...rest) => { if (name === 'data') throw new Error('PRIVATE-STARTUP'); return on(name, ...rest); };
    return actual;
  }; syncBuiltinESMExports();
  let failure;
  try { f.launch(); } catch (error) { failure = error; }
  finally { childProcess.spawn = original; syncBuiltinESMExports(); }
  assert.ok(failure instanceof ServiceSetupLaunchError); assert.equal(failure.owner.pid, actual.pid);
  assert.ok(!String(failure).includes('PRIVATE-STARTUP'));
  const closed = await failure.owner.close(5000); assert.equal(closed.process_joined, true); assert.equal(closed.native_resources_closed, false);
});

test('failure before close-listener registration still joins the actual returned child', async t => {
  const f = fixture(t), original = childProcess.spawn; let actual;
  childProcess.spawn = (...args) => {
    actual = original(...args); const once = actual.once.bind(actual);
    actual.once = (name, ...rest) => { if (name === 'error') throw new Error('PRIVATE-EARLY-STARTUP'); return once(name, ...rest); };
    return actual;
  }; syncBuiltinESMExports();
  let failure;
  try { f.launch(); } catch (error) { failure = error; }
  finally { childProcess.spawn = original; syncBuiltinESMExports(); }
  assert.ok(failure instanceof ServiceSetupLaunchError); assert.equal(failure.owner.pid, actual.pid);
  const closed = await failure.owner.close(5000); assert.equal(closed.process_joined, true); assert.equal(closed.native_resources_closed, false);
  assert.throws(() => process.kill(actual.pid, 0), { code: 'ESRCH' });
});

test('protected input accessors and coercion objects cannot expose private exceptions', async t => {
  const f = fixture(t), client = await f.open(), send = client.submit('prepare');
  const prompt = await client.nextEvent(5000); let getterCalls = 0;
  const privateError = () => { getterCalls++; throw new Error('PRIVATE-CANARY'); };
  const secret = { ...answer(), get value() { return privateError(); } };
  const coercion = { toString: privateError };
  for (const action of [() => client.answer(prompt.request_id, secret),
    () => client.answer(prompt.request_id, { ...answer(), value: coercion }),
    () => client.connectorStart({ get job_id() { return privateError(); } }),
    () => client.connectorComplete({ state: 'state', get code() { return privateError(); } })]) {
    assert.throws(action, error => !String(error).includes('PRIVATE-CANARY') && error.name === 'Error');
  }
  assert.equal(getterCalls, 0);
  client.answer(prompt.request_id, answer()); await until(client, e => e.request_id === send);
  assert.equal((await client.close(5000)).native_resources_closed, true);
  assert.deepEqual(f.requests().filter(r => r.result).map(r => r.result.answer), [answer()]);
});
