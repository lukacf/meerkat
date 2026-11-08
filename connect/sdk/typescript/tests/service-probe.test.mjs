import test from 'node:test';
import assert from 'node:assert/strict';
import { inspect } from 'node:util';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, unlinkSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { ServiceProbeClient, ServiceSetupEvent, ServiceSetupLaunchError, ServiceSetupTimeout } from '../dist/service-probe.js';

// Artificial protocol peer, real OS pipes and process. No native credentials,
// provider, approval authority, grant or resource access is exercised here.
const PEER = String.raw`
import {createInterface} from 'node:readline';
import {readFileSync,writeFileSync,appendFileSync,existsSync} from 'node:fs';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
const lines=createInterface({input:process.stdin})[Symbol.asyncIterator]();
setTimeout(()=>process.exit(74),10000).unref();
const first=await lines.next();if(first.done)process.exit(0);
const init=JSON.parse(first.value),root=init.params.realm.context_root,mode=readFileSync(join(root,'mode'),'utf8');
writeFileSync(join(root,'launch.json'),JSON.stringify({argv:process.argv.slice(2),env:process.env,initial:init,pid:process.pid}));
const emit=v=>process.stdout.write(JSON.stringify(v)+'\n');
const reply=(id,result)=>emit({jsonrpc:'2.0',id,result});
const target={scope:'scope'};
const event=(kind,fields={})=>emit({jsonrpc:'2.0',method:'connect/event',params:{target,kind,...fields}});
const state={available:true,can_send:true,can_stop:false,outcome_uncertain:false,active_send:null,last_completion:null,
 private_notice_pending:false,commissioning:false,can_probe:false,active_probe:null,probe_profiles:['read','second']};
const update=fields=>{Object.assign(state,fields);event('state',{state});};
const kinds=['approval','provider_target','provider_action','provider_api_key','provider_browser','provider_device','service_secret'];
const initialized=()=>reply(init.id,{protocol_version:mode==='downgrade'?5:6,scope:'scope',session_id:'session',
 capabilities:{prompt_kinds:kinds,service_preparation:true,service_probes:{selection:'trusted_process_profiles',
 requires:'successful_finalization_and_governed_reopen',result:'native_tool_completion_or_profile_verified_resource',
 slack_inbound:mode==='bad-capabilities'?'supported':'unsupported'}},state});
const prompt=(id,value,where=target)=>emit({jsonrpc:'2.0',method:'connect/prompt',id,params:{target:where,prompt:value}});
let pending,send;
const commission=()=>({realm:'realm',session_id:'session',job_id:'job',agent:'agent',state:'completed',governance:'already_governed',
 grants:[{state:'committed',issued:'controller',candidate_grant_id:null,superseded:null,cleanup:[]},
 {state:'committed',issued:'catalog',candidate_grant_id:null,superseded:null,cleanup:[]}],
 controller_disposal:'drained',issuance:'committed',custody_released:true,capability_verified:false});
const ready=()=>{
 const observed=commission();if(mode==='bad-commission')observed.capability_verified=true;
 event('service_commissioning',{state:'observed',observation:observed,capability_verified:false});
 event('service_finalization',{state:'observed',observation:{kind:'synthetic_fixture'}});
 event('service_probe_state',{state:'available',job_id:mode==='bad-job'?'foreign':'job',session_id:'session',capability_verified:false});
 update({commissioning:false,can_probe:true});
};
const report=(request,status='capability_verified')=>({profile_id:request.params.profile_id,
 application_id:'app',instance_id:'instance',login_scope_id:'login',viewer_id:'viewer',origin:'https://host.invalid',
 realm:'realm',session_id:'session',job_id:'job',server:'selected-server',agent:'agent',operation:'repository_read',
 resource:{repository:'PRIVATE-RESOURCE',nested:[{id:1}]},status,input_id:'input',run_id:'run',tool_call_id:'call',
 result_sha256:'a'.repeat(64),installed_connection_sha256:'b'.repeat(64),dispatch_outcome:'committed',
 native_terminal_committed:true,native_close_joined:true});
const finish=(request,status)=>{
 const value=report(request,status);
 if(mode==='unknown') {value.status='outcome_unknown';value.dispatch_outcome='unknown';}
 if(mode==='tool-only')value.status='tool_completed';
 if(mode==='wrong-session')value.session_id='other';
 if(mode==='wrong-viewer')value.viewer_id='other';
if(mode==='wrong-agent')value.agent='other';
 if(mode==='wrong-profile')value.profile_id='other';
 if(mode==='wrong-job')value.job_id='other';
 if(mode==='unknown-success')value.dispatch_outcome='unknown';
 if(mode==='missing-close')value.native_close_joined=false;
 if(mode==='missing-terminal')value.native_terminal_committed=false;
 if(mode==='missing-result')value.result_sha256=null;
 if(mode==='extra-report')value.raw_result='PRIVATE-RAW';
 if(mode==='bad-hash')value.result_sha256='A'.repeat(64);
 if(mode==='tool-failed'){value.status='tool_failed';value.dispatch_outcome='failed';}
 Object.assign(state,{active_probe:null,can_probe:true});
 reply(request.id,{scope:'scope',probe_request_id:mode==='wrong-request'?'foreign':request.id,report:value,state});
 event('state',{state});pending=null;
};
emit({jsonrpc:'2.0',method:'connect/event',params:{target:{initialize_request_id:init.id},kind:'state',
 state:{...state,available:false,can_send:false}}});
if(mode==='preinit')prompt('p:initial',{kind:'provider_action',actions:['use']},{initialize_request_id:init.id});
else initialized();
for await(const line of lines){
 const request=JSON.parse(line);appendFileSync(join(root,'requests.jsonl'),JSON.stringify(request)+'\n');
 if(request.method==='connect/send'){
  send=request.id;Object.assign(state,{last_completion:{send_request_id:send,completion:'completed'},can_send:false});
  reply(send,{scope:'scope',send_request_id:send,completion:'completed',state});
  event('unavailable',{reason:'finalizing'});
  update({available:false,can_send:false});
  event('service_finalization',{state:'native_closed',grant_issued:false});
  if(mode==='external-retired')event('unavailable',{reason:'expired'});
  update({commissioning:true});
  prompt('p:commission',mode==='bad-prompt'?{kind:'service_secret',job_id:'job',service:'server',slot:'slot'}:
   {kind:'approval',summary:'Review actual native commissioning',body:{lifetime:'Until explicitly revoked'}});
 }else if(request.result?.answer){
  if(request.id==='p:initial'){
   emit({jsonrpc:'2.0',method:'connect/event',params:{target:{initialize_request_id:init.id},kind:'prompt_closed',prompt_id:request.id}});initialized();
  }else{
   event('prompt_closed',{prompt_id:request.id});
   if(request.result.answer.kind==='cancel')event('unavailable',{reason:'cancelled'});else ready();
  }
 }else if(request.method==='connect/probe'){
  pending=request;update({active_probe:request.id,can_probe:false});
  if(mode==='refused') {emit({jsonrpc:'2.0',id:request.id,error:{code:-32000,message:'PRIVATE-ERROR',data:{code:'probe_unavailable'}}});pending=null;update({active_probe:null,can_probe:true});}
  else if(mode==='held') {while(existsSync(join(root,'hold')))await delay(5);finish(request);}
  else if(mode!=='pending')finish(request);
 }else if(request.method==='connect/cancel'){
  const active=!!pending;
  event('unavailable',{reason:'cancelled'});
  if(pending)finish(pending,'cancelled');
  reply(request.id,{scope:'scope',send_request_id:request.params.send_request_id,status:active?'cancellation_requested':'not_current'});
 }else if(request.method==='connect/close'){
  reply(request.id,{target:request.params.target,resources_closed:true});
  await new Promise(resolve=>process.stdout.write('',resolve));process.exit(0);
 }
}
process.exit(0);
`;

function fixture(t, mode = 'normal') {
  const root = mkdtempSync(join(tmpdir(), 'probe-pipe-')), script = join(root, 'peer.mjs');
  writeFileSync(script, PEER); writeFileSync(join(root, 'mode'), mode);
  const binding = { application_id: 'app', instance_id: 'instance', origin: 'https://host.invalid',
    principal_id: 'viewer', login_scope_id: 'login', expires_at_ms: Date.now() + 120000,
    realm_id: 'realm', context_root: root, state_root: join(root, 'state'), user_config_root: join(root, 'config'),
    resource_manifest_sha256: 'a'.repeat(64), credential_scope: { kind: 'file', root: join(root, 'vault') } };
  const launcher = { protocol_version: 6, executable: process.execPath, args: [script, '--host-selected'],
    credential_scope: { ...binding.credential_scope }, environment: { PATH: '/usr/bin:/bin', LANG: 'C' } };
  const clients = [];
  t.after(async () => {
    if (existsSync(join(root, 'hold'))) unlinkSync(join(root, 'hold'));
    let failure;
    for (const client of clients) {
      try { const result = await client.close(12000); assert.equal(result.process_joined, true); assert.notEqual(result.exit_code, 74); }
      catch (error) { failure ??= error; }
    }
    if (!failure) rmSync(root, { recursive: true, force: true });
    if (failure) throw failure;
  });
  return { root, binding, launcher, clients,
    launch() { try { const owner = new ServiceProbeClient(launcher, binding); clients.push(owner); return owner; }
      catch (error) { if (error instanceof ServiceSetupLaunchError) clients.push(error.owner); throw error; } },
    requests() { const file = join(root, 'requests.jsonl'); return existsSync(file) ? readFileSync(file, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse) : []; } };
}
async function until(client, predicate) {
  for (let i = 0; i < 100; i++) { const e = await client.nextEvent(5000); if (predicate(e)) return e; }
  assert.fail('missing expected native observation');
}
async function commission(client) {
  await until(client, e => e.payload.method === 'connect/initialize');
  client.submit('Prepare the host-selected service');
  const prompt = await until(client, e => e.kind === 'prompt');
  assert.equal(prompt.payload.kind, 'approval');
  client.answer(prompt.request_id, { kind: 'approval', decision: 'approve' });
  const observed = await until(client, e => e.kind === 'service_commissioning');
  await until(client, e => e.kind === 'state' && client.state?.can_probe);
  return observed;
}
const selection = (profile = 'read') => ({ job_id: 'job', profile_id: profile });

test('explicit launcher freezes exact argv/environment/binding without stdio flags', async t => {
  const f = fixture(t), client = f.launch();
  f.launcher.args.push('--mutated'); f.launcher.environment.LANG = 'changed'; f.binding.principal_id = 'changed';
  await until(client, e => e.payload.method === 'connect/initialize');
  const actual = JSON.parse(readFileSync(join(f.root, 'launch.json'), 'utf8'));
  assert.deepEqual(actual.argv, ['--host-selected']); assert.equal(actual.env.LANG, 'C');
  assert.equal(actual.initial.params.protocol_version, 6); assert.equal(actual.initial.params.viewer.principal_id, 'viewer');
  for (const key of ['HOME', 'OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GITHUB_PERSONAL_ACCESS_TOKEN']) assert.ok(!(key in actual.env));
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('invalid launcher/profile/environment refuses before process creation', async t => {
  const f = fixture(t);
  for (const changes of [{ protocol_version: 5 }, { executable: 'relative' }, { args: ['\ud800'] },
    { credential_scope: { kind: 'file', root: '/wrong' } }, { environment: { OPENAI_API_KEY: 'PRIVATE' } },
    { profiles: [] }, { args: ['x'.repeat(65537)] }]) {
    assert.throws(() => new ServiceProbeClient({ ...f.launcher, ...changes }, f.binding), /trusted protocol6/);
  }
  let accessed = false;
  assert.throws(() => new ServiceProbeClient({ ...f.launcher, get environment() { accessed = true; return {}; } }, f.binding), /trusted protocol6/);
  assert.equal(accessed, false); assert.equal(existsSync(join(f.root, 'launch.json')), false);
});

for (const mode of ['downgrade', 'bad-capabilities']) test(`no protocol/capability fallback: ${mode}`, async t => {
  const f = fixture(t, mode), client = f.launch();
  await assert.rejects(until(client, e => e.payload.method === 'connect/initialize'), /transport failed/);
  assert.equal((await client.close(5000)).native_resources_closed, false);
  assert.equal(f.clients.length, 1);
});

test('provider prompt before initialization uses retained initial target', async t => {
  const f = fixture(t, 'preinit'), client = f.launch(), initialState = await client.nextEvent(5000);
  assert.equal(initialState.kind, 'state'); assert.equal(initialState.payload.target.initialize_request_id, 'h:initialize');
  const prompt = await until(client, e => e.kind === 'prompt');
  assert.equal(prompt.kind, 'prompt'); client.answer(prompt.request_id, { kind: 'provider_action', action: 'use' });
  await commission(client); assert.equal(client.state.can_probe, true);
});

test('finalizing closes setup input but preserves distinct commissioning approval and typed facts', async t => {
  const f = fixture(t), client = f.launch(); await until(client, e => e.payload.method === 'connect/initialize'); client.submit('prepare');
  const prompt = await until(client, e => e.kind === 'prompt');
  assert.equal(client.state.commissioning, true); assert.equal(client.viewerCurrent, true);
  assert.throws(() => client.submit('new input'), /setup input owner/);
  assert.throws(() => client.connectorCancel(), /setup input owner/);
  assert.throws(() => client.probe(selection()), /unavailable/);
  client.answer(prompt.request_id, { kind: 'approval', decision: 'approve' });
  const observed = await until(client, e => e.kind === 'service_commissioning');
  const facts = client.commissioning(observed);
  assert.equal(facts.capability_verified, false); assert.equal(facts.custody_released, true);
  assert.equal(facts.grants[0].issued, 'controller');
  assert.throws(() => { facts.grants[0].state = 'other'; }, TypeError);
  await until(client, e => e.kind === 'state' && client.state.can_probe);
  assert.equal(f.requests().filter(r => r.method === 'connect/send').length, 1);
});

test('external retirement cannot be reversed by finalization or commissioning state', async t => {
  const f = fixture(t, 'external-retired'), client = f.launch(); await until(client, e => e.payload.method === 'connect/initialize'); client.submit('prepare');
  const prompt = await until(client, e => e.kind === 'retired_prompt');
  assert.equal(client.viewerCurrent, false); assert.equal(client.viewerCurrentAtObservation(prompt), false);
  assert.throws(() => client.answer(prompt.request_id, { kind: 'approval', decision: 'approve' }), /unavailable/);
  assert.throws(() => client.probe(selection()), /unavailable/);
});

test('only opaque observed selectors cross the probe request boundary', async t => {
  const f = fixture(t), client = f.launch(); await commission(client);
  for (const bad of [{ ...selection(), operation: 'different' }, { ...selection(), resource: {} },
    { ...selection(), arguments: {} }, { job_id: 'foreign', profile_id: 'read' }, selection('unknown')]) {
    assert.throws(() => client.probe(bad), /selection required|unavailable/);
  }
  const id = client.probe(selection());
  assert.throws(() => client.probe(selection('second')), /unavailable/);
  const event = await until(client, e => e.request_id === id);
  const report = client.probeReport(event);
  assert.equal(report.status, 'capability_verified'); assert.equal(report.native_terminal_committed, true);
  assert.equal(report.native_close_joined, true); assert.equal(report.profile_id, 'read');
  assert.equal(client.viewerCurrentAtObservation(event), true);
  assert.throws(() => { report.resource.nested[0].id = 2; }, TypeError);
  assert.equal(inspect(event).includes('PRIVATE-RESOURCE'), false);
  assert.deepEqual(JSON.parse(JSON.stringify(event)), { kind: 'result', request_id: id });
  assert.equal(client.probeReport(new ServiceSetupEvent('result', id, event.payload)), undefined);
  assert.throws(() => client.probe(selection()), /already attempted/);
  assert.deepEqual(f.requests().find(r => r.method === 'connect/probe').params, { scope: 'scope', job_id: 'job', profile_id: 'read' });
});

for (const [mode, expected, dispatch] of [['unknown', 'outcome_unknown', 'unknown'],
  ['tool-only', 'tool_completed', 'committed'], ['tool-failed', 'tool_failed', 'failed']]) {
  test(`actual report preserves ${mode} without readiness promotion`, async t => {
    const f = fixture(t, mode), client = f.launch(); await commission(client);
    const id = client.probe(selection()), event = await until(client, e => e.request_id === id);
    assert.equal(client.probeReport(event).status, expected); assert.equal(client.probeReport(event).dispatch_outcome, dispatch);
    assert.equal('ready' in client, false);
  });
}

test('malformed or foreign native reports never become typed observations', async t => {
  for (const mode of ['wrong-session', 'wrong-viewer', 'wrong-agent', 'wrong-profile', 'wrong-job', 'wrong-request', 'unknown-success',
    'missing-close', 'missing-terminal', 'missing-result', 'extra-report', 'bad-hash']) {
    await t.test(mode, async t => {
      const f = fixture(t, mode), client = f.launch(); await commission(client); const id = client.probe(selection());
      await assert.rejects(until(client, e => e.request_id === id), /transport failed/);
      assert.equal(client.viewerCurrent, false); assert.equal((await client.close(5000)).native_resources_closed, false);
    });
  }
});

for (const mode of ['bad-commission', 'bad-job', 'bad-prompt']) test(`commissioning refuses corrupt native attribution: ${mode}`, async t => {
  const f = fixture(t, mode), client = f.launch();
  await assert.rejects(commission(client), /transport failed/);
  assert.equal((await client.close(5000)).native_resources_closed, false);
});

test('active probe cancellation retires the binding and preserves native facts', async t => {
  const f = fixture(t, 'pending'), client = f.launch(); await commission(client);
  const probe = client.probe(selection()); await until(client, e => e.kind === 'state' && client.state.active_probe === probe);
  const cancel = client.cancel(probe); assert.equal(client.viewerCurrent, false);
  assert.throws(() => client.probe(selection('second')), /unavailable/);
  const event = await until(client, e => e.request_id === probe);
  assert.equal(client.probeReport(event).status, 'cancelled'); assert.equal(client.probeReport(event).native_close_joined, true);
  assert.equal(client.viewerCurrentAtObservation(event), false);
  assert.equal((await until(client, e => e.request_id === cancel)).payload.result.status, 'cancellation_requested');
});

test('only latest completed probe can retire; old correlation cannot cancel a newer attempt', async t => {
  const f = fixture(t), client = f.launch(); await commission(client);
  const first = client.probe(selection()); const event = await until(client, e => e.request_id === first);
  const second = client.probe(selection('second')); assert.throws(() => client.cancel(first), /not current/);
  await until(client, e => e.request_id === second);
  const cancel = client.cancel(second); assert.equal(client.viewerCurrent, false);
  assert.equal(client.probeReport(event).status, 'capability_verified'); // Historical fact stays intact.
  assert.equal((await until(client, e => e.request_id === cancel)).payload.result.status, 'not_current');
  assert.equal((await client.close(5000)).native_resources_closed, true);
});

test('native refusal burns the attempt and exposes no error text', async t => {
  const f = fixture(t, 'refused'), client = f.launch(); await commission(client);
  const id = client.probe(selection()), event = await until(client, e => e.request_id === id);
  assert.equal(event.kind, 'refused'); assert.equal(client.probeReport(event), undefined);
  assert.equal(JSON.stringify(event.payload).includes('PRIVATE-ERROR'), false);
  assert.throws(() => client.probe(selection()), /already attempted/);
});

test('observation and close timeout retain exactly the same process without probe replay', async t => {
  const f = fixture(t, 'held'); writeFileSync(join(f.root, 'hold'), '');
  const client = f.launch(); await commission(client); const id = client.probe(selection()), pid = client.pid;
  await until(client, e => e.kind === 'state' && client.state.active_probe === id);
  await assert.rejects(client.nextEvent(20), ServiceSetupTimeout);
  await assert.rejects(client.close(20), ServiceSetupTimeout);
  process.kill(pid, 0); assert.equal(client.pid, pid);
  assert.throws(() => client.probe(selection()), /unavailable/);
  unlinkSync(join(f.root, 'hold'));
  const event = await until(client, e => e.request_id === id);
  assert.equal(client.viewerCurrentAtObservation(event), false);
  assert.equal((await client.close(5000)).native_resources_closed, true);
  assert.equal(f.requests().filter(r => r.method === 'connect/probe').length, 1);
  assert.equal(f.requests().filter(r => r.method === 'connect/close').length, 1);
});

test('expiry refuses new probes without altering earlier native facts', async t => {
  const f = fixture(t), original = Date.now, client = f.launch(); await commission(client);
  const id = client.probe(selection()), event = await until(client, e => e.request_id === id);
  try {
    Date.now = () => f.binding.expires_at_ms + 1;
    assert.equal(client.viewerCurrent, false); assert.throws(() => client.probe(selection('second')), /unavailable/);
    assert.equal(client.probeReport(event).status, 'capability_verified');
  } finally { Date.now = original; }
});
