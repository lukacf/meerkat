import test from 'node:test';
import assert from 'node:assert/strict';
import { inspect } from 'node:util';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, unlinkSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { ServiceCollectionClient, ServiceSetupEvent, ServiceSetupLaunchError, ServiceSetupTimeout } from '../dist/service-collection.js';

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
 private_notice_pending:false,commissioning:false,can_collect:false,active_collection:null,collection_sources:['read','second'],reusable_source_coverage:'unqualified_credential_continuity'};
if(mode==='bad-state-extra')state.program={};
if(mode==='bad-state-coverage')state.reusable_source_coverage='qualified';
if(mode==='bad-state-active')state.active_collection='foreign';
const update=fields=>{Object.assign(state,fields);event('state',{state});};
const kinds=['approval','provider_target','provider_action','provider_api_key','provider_browser','provider_device','service_secret'];
const initialized=()=>reply(init.id,{protocol_version:mode==='downgrade'?5:7,scope:'scope',session_id:'session',
 capabilities:{prompt_kinds:kinds,service_preparation:true,service_collections:{selection:'trusted_process_sources',
 requires:'successful_finalization_and_governed_reopen',result:'historical_required_operation_observations',
 reusable_source_coverage:'unqualified_credential_continuity',slack_inbound:mode==='bad-capabilities'?'supported':'unsupported'}},state});
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
 if(mode==='no-ready')return;
 event('service_collection_state',{state:'available',job_id:mode==='bad-job'?'foreign':'job',session_id:'session',capability_verified:false});
 update({commissioning:false,can_collect:true});
};
const report=(request,status='tool_completed')=>{
 const resource={repository:'PRIVATE-RESOURCE',nested:[{id:1}]};
 const native={profile_id:'required-read',application_id:'app',instance_id:'instance',login_scope_id:'login',viewer_id:'viewer',origin:'https://host.invalid',realm:'realm',session_id:'session',job_id:'job',server:'selected-server',agent:'agent',operation:'repository_read',resource,status,input_id:'input',run_id:'run',tool_call_id:'call',result_sha256:'a'.repeat(64),installed_connection_sha256:'b'.repeat(64),dispatch_outcome:'committed',native_terminal_committed:true,native_close_joined:true};
 const call={source_id:request.params.source_id,required_operation_id:'required-read',resource,ordinal:1,raw_operation:'repository_read',arguments_sha256:'c'.repeat(64),native};
 const row={required_operation_id:'required-read',resource,raw_operation_ceiling:['repository_read'],assessment:'verified',calls:[call]};
 const value={source_id:request.params.source_id,attempt_id:'00000000-0000-4000-8000-00000000000'+(request.params.source_id==='read'?'1':'2'),application_id:'app',instance_id:'instance',login_scope_id:'login',viewer_id:'viewer',origin:'https://host.invalid',realm:'realm',session_id:'session',job_id:'job',family:'selected',profile:'default',agent:'agent',status:'historical_checks_completed',requirements:[row],observed_selection:'rechecked_after_last_close',native_own_config_generation:1,reusable_source_coverage:'unqualified_credential_continuity',limits:{calls:3,argument_bytes:1024,result_bytes:1024},total_calls:1,total_argument_bytes:2,total_result_bytes:64};
 if(status==='cancelled'){value.status='cancelled';row.assessment=null;}
 if(['unknown','failed','cleanup'].includes(mode)){
  value.status={unknown:'outcome_unknown',failed:'tool_failed',cleanup:'cleanup_unconfirmed'}[mode];row.assessment=null;
  native.status=value.status;native.dispatch_outcome={unknown:'unknown',failed:'failed',cleanup:'committed'}[mode];
  if(mode==='cleanup')native.native_close_joined=false;
 }
 if(mode==='budget'){value.status='budget_exceeded';value.total_result_bytes=2048;row.assessment=null;}
 if(mode==='reserved'){value.status='program_refused';value.total_calls=2;}
 if(mode==='not-observed'){value.status='not_observed';value.total_calls=0;value.total_argument_bytes=0;value.total_result_bytes=0;row.assessment=null;row.calls=[];}
 if(mode.startsWith('foreign-'))value[mode.slice(8)]='foreign';
 if(mode==='extra-report')value.raw_result='PRIVATE-RAW';
 if(mode==='zero-success'){row.calls=[];value.total_calls=0;}
 if(mode==='unknown-success')native.dispatch_outcome='unknown';
 if(mode==='missing-close')native.native_close_joined=false;
 if(mode==='missing-terminal')native.native_terminal_committed=false;
 if(mode==='missing-result')native.result_sha256=null;
 if(mode==='wrong-resource')call.resource={repository:'other'};
 if(mode==='wrong-nested-source')call.source_id='foreign';
 if(mode==='wrong-native-profile')native.profile_id='foreign';
 if(mode==='wrong-ordinal')call.ordinal=2;
 if(mode==='wrong-operation')call.raw_operation='repository_write';
 if(mode==='missing-row')value.requirements=[];
 if(mode==='false-coverage')value.reusable_source_coverage='qualified';
 if(mode==='unsafe-generation')value.native_own_config_generation=Number.MAX_SAFE_INTEGER+1;
 if(mode.startsWith('reobserve')){
  const second=structuredClone(call);second.ordinal=2;
  Object.assign(second.native,{input_id:'input-2',run_id:'run-2',tool_call_id:'call-2'});
  if(['reobserve-unentered','reobserve-partial','reobserve-partial-replay','reobserve-unentered-replay'].includes(mode)){
   const unentered=['reobserve-unentered','reobserve-unentered-replay'].includes(mode);
   value.status=unentered?'selection_changed':'cancelled';row.assessment=null;
   Object.assign(second.native,{status:value.status,input_id:unentered?null:'input-2',run_id:null,tool_call_id:null,result_sha256:null,dispatch_outcome:null,native_terminal_committed:false,native_close_joined:!unentered});
   if(unentered)second.native.installed_connection_sha256=null;
  }
  if(mode==='reobserve-unknown'){
   value.status='outcome_unknown';row.assessment=null;
   Object.assign(second.native,{status:'outcome_unknown',dispatch_outcome:'unknown',result_sha256:null,native_terminal_committed:false,native_close_joined:true});
  }
  if(mode==='reobserve-cleanup'){
   value.status='cleanup_unconfirmed';row.assessment=null;
   Object.assign(second.native,{status:'cleanup_unconfirmed',result_sha256:null,native_terminal_committed:false,native_close_joined:false});
  }
  if(mode==='reobserve-replay')second.native=structuredClone(native);
  if(mode==='reobserve-other-args-replay'){second.native=structuredClone(native);second.arguments_sha256='d'.repeat(64);}
  if(mode==='reobserve-foreign-resource')second.native.resource={repository:'other'};
  if(mode==='reobserve-foreign-context')second.native.realm='foreign';
  if(mode==='reobserve-false-success')second.native.native_close_joined=false;
  if(mode==='reobserve-missing-success')Object.assign(second.native,{input_id:null,run_id:null,tool_call_id:null});
  if(mode==='reobserve-other-requirement-replay'){second.native=structuredClone(native);second.required_operation_id='required-members';second.native.profile_id='required-members';}
  row.calls.push(second);Object.assign(value,{total_calls:2,total_argument_bytes:4,total_result_bytes:128});
  if(mode==='reobserve-other-requirement-replay'){
   row.calls.pop();const other=structuredClone(row);Object.assign(other,{required_operation_id:'required-members',calls:[second]});value.requirements.push(other);
  }
  if(['reobserve-partial-replay','reobserve-unentered-replay'].includes(mode)){
   const third=structuredClone(second);third.ordinal=3;row.calls.push(third);
   Object.assign(value,{total_calls:3,total_argument_bytes:6});
  }
 }
 if(['multi','duplicate-call'].includes(mode)){
  const second=structuredClone(call);second.ordinal=2;
  if(mode==='multi'){second.arguments_sha256='d'.repeat(64);Object.assign(second.native,{input_id:'input-2',run_id:'run-2',tool_call_id:'call-2'});}
  row.calls.push(second);
  const lastrow=structuredClone(row);lastrow.required_operation_id='required-members';lastrow.calls=lastrow.calls.slice(0,1);
  Object.assign(lastrow.calls[0],{required_operation_id:'required-members',ordinal:3});
  Object.assign(lastrow.calls[0].native,{profile_id:'required-members',input_id:'input-3',run_id:'run-3',tool_call_id:'call-3'});
  value.requirements.push(lastrow);Object.assign(value,{total_calls:3,total_argument_bytes:6,total_result_bytes:192});
 }
 return value;
};
const finish=(request,status)=>{
 const value=report(request,status);
 Object.assign(state,{active_collection:null,can_collect:true});
 reply(request.id,{scope:'scope',collection_request_id:mode==='wrong-request'?'foreign':request.id,report:value,state});
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
  event('unavailable',{reason:'finalizing',first_fence:{origin:'other_native_transition',scope:'scope',send_request_id:null,cancellation_requested_before:null}});
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
 }else if(request.method==='connect/collect'){
  pending=request;update({active_collection:request.id,can_collect:false});
  if(['refused','known-refused'].includes(mode)) {emit({jsonrpc:'2.0',id:request.id,error:{code:-32000,message:'PRIVATE-ERROR',data:{code:mode==='known-refused'?'collection_already_attempted':'PRIVATE-UNKNOWN'}}});pending=null;update({active_collection:null,can_collect:true});}
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

function fixture(t, mode = 'normal', peer = PEER) {
  const root = mkdtempSync(join(tmpdir(), 'collection-pipe-')), script = join(root, 'peer.mjs');
  writeFileSync(script, peer); writeFileSync(join(root, 'mode'), mode);
  const binding = { application_id: 'app', instance_id: 'instance', origin: 'https://host.invalid',
    principal_id: 'viewer', login_scope_id: 'login', expires_at_ms: Date.now() + 120000,
    realm_id: 'realm', context_root: root, state_root: join(root, 'state'), user_config_root: join(root, 'config'),
    resource_manifest_sha256: 'a'.repeat(64), credential_scope: { kind: 'file', root: join(root, 'vault') } };
  const launcher = { protocol_version: 7, executable: process.execPath, args: [script, '--host-selected'],
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
    launch() { try { const owner = new ServiceCollectionClient(launcher, binding); clients.push(owner); return owner; }
      catch (error) { if (error instanceof ServiceSetupLaunchError) clients.push(error.owner); throw error; } },
    requests() { const file = join(root, 'requests.jsonl'); return existsSync(file) ? readFileSync(file, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse) : []; } };
}
async function until(client, predicate) {
  for (let i = 0; i < 100; i++) { const e = await client.nextEvent(5000); if (predicate(e)) return e; }
  assert.fail('missing expected native observation');
}
async function commission(client, onFence = () => {}) {
  await until(client, e => e.payload.method === 'connect/initialize');
  client.submit('Prepare the host-selected service');
  const prompt = await until(client, e => { if (e.kind === 'unavailable') onFence(e); return e.kind === 'prompt'; });
  assert.equal(prompt.payload.kind, 'approval');
  client.answer(prompt.request_id, { kind: 'approval', decision: 'approve' });
  const observed = await until(client, e => e.kind === 'service_commissioning');
  await until(client, e => e.kind === 'state' && client.state?.can_collect);
  return observed;
}
const selection = (profile = 'read') => ({ job_id: 'job', source_id: profile });

test('v7 launcher freezes command, environment and binding without source authority', async t => {
  const f=fixture(t),client=f.launch();f.launcher.args.push('--changed');f.launcher.environment.LANG='changed';f.binding.principal_id='changed';
  await until(client,e=>e.payload.method==='connect/initialize');
  const actual=JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8'));
  assert.deepEqual(actual.argv,['--host-selected']);assert.equal(actual.env.LANG,'C');
  assert.equal(actual.initial.params.protocol_version,7);assert.equal(actual.initial.params.viewer.principal_id,'viewer');
  for(const key of ['collections','programs','operations','resources','caller','credential_scope'])assert.ok(!(key in actual.initial.params));
  assert.equal((await client.close(5000)).native_resources_closed,true);
});

test('closed launcher refuses browser authority, getters and downgrade before spawning', t => {
  const f=fixture(t);
  for(const changes of [{protocol_version:6},{executable:'relative'},{programs:[]},{collections:[]},{environment:{API_KEY:'PRIVATE'}},{args:['x'.repeat(65537)]}])
    assert.throws(()=>new ServiceCollectionClient({...f.launcher,...changes},f.binding),/trusted protocol7/);
  let accessed=false;
  assert.throws(()=>new ServiceCollectionClient({...f.launcher,get environment(){accessed=true;return {}; }},f.binding));
  assert.equal(accessed,false);assert.equal(existsSync(join(f.root,'launch.json')),false);
});

for(const mode of ['downgrade','bad-capabilities'])test(`protocol7 does not fall back: ${mode}`,async t=>{
  const f=fixture(t,mode),client=f.launch();
  await assert.rejects(until(client,e=>e.payload.method==='connect/initialize'),/transport failed/);
  assert.equal((await client.close(5000)).native_resources_closed,false);assert.equal(f.clients.length,1);
});

test('early state/provider prompt and closed setup owner retain commissioning',async t=>{
  const f=fixture(t,'preinit'),client=f.launch();
  const early=await client.nextEvent(5000);assert.equal(early.kind,'state');assert.equal(early.payload.target.initialize_request_id,'h:initialize');
  const prompt=await until(client,e=>e.kind==='prompt');client.answer(prompt.request_id,{kind:'provider_action',action:'use'});
  const observed=await commission(client);assert.equal(client.commissioning(observed).capability_verified,false);
  assert.equal(client.collectionReport(observed),undefined);assert.equal(client.state.can_collect,true);
  assert.throws(()=>client.submit('do not restart setup'));assert.throws(()=>client.connectorCancel());
});

test('only observed opaque source/job selectors cross the collection boundary',async t=>{
  const f=fixture(t),client=f.launch();await commission(client);
  for(const selected of [{job_id:'foreign',source_id:'read'},{job_id:'job',source_id:'foreign'},
    {...selection(),resource:{}},{...selection(),operations:['write']},{...selection(),program:'anything'}])assert.throws(()=>client.collect(selected));
  let accessed=false;assert.throws(()=>client.collect({job_id:'job',get source_id(){accessed=true;return 'read';}}));assert.equal(accessed,false);
  const id=client.collect(selection());const event=await until(client,e=>e.request_id===id);
  const report=client.collectionReport(event);assert.equal(report.status,'historical_checks_completed');
  assert.equal(report.reusable_source_coverage,'unqualified_credential_continuity');
  assert.equal(report.requirements[0].calls[0].native.profile_id,'required-read');
  assert.throws(()=>{report.requirements[0].resource.repository='changed';});
  assert.equal(client.collectionReport(new ServiceSetupEvent(event.kind,event.request_id,event.payload)),undefined);
  assert.throws(()=>client.collect(selection()));
  assert.deepEqual(f.requests().filter(r=>r.method==='connect/collect').map(r=>r.params),[{scope:'scope',job_id:'job',source_id:'read'}]);
});

for(const [mode,status] of [['unknown','outcome_unknown'],['failed','tool_failed'],['cleanup','cleanup_unconfirmed'],
  ['budget','budget_exceeded'],['reserved','program_refused'],['not-observed','not_observed']])test(`partial historical outcome preserved: ${mode}`,async t=>{
  const f=fixture(t,mode),client=f.launch();await commission(client);const id=client.collect(selection());
  const event=await until(client,e=>e.request_id===id),report=client.collectionReport(event);
  assert.equal(report.status,status);assert.equal(report.reusable_source_coverage,'unqualified_credential_continuity');
  if(mode==='budget')assert.ok(report.total_result_bytes>report.limits.result_bytes);
  if(mode==='reserved')assert.ok(report.total_calls>report.requirements[0].calls.length);
  assert.throws(()=>client.collect(selection()));
});

test('foreign, empty, uncommitted and authority-bearing reports fail closed',async t=>{
  for(const mode of ['foreign-source_id','foreign-session_id','foreign-viewer_id','foreign-job_id','wrong-request','extra-report',
    'zero-success','unknown-success','missing-close','missing-terminal','missing-result','wrong-resource','wrong-nested-source',
    'wrong-native-profile','wrong-ordinal','wrong-operation','missing-row','false-coverage','unsafe-generation','duplicate-call'])await t.test(mode,async st=>{
    const f=fixture(st,mode),client=f.launch();await commission(client);const id=client.collect(selection());
    await assert.rejects(until(client,e=>e.request_id===id),/transport failed/);
    assert.equal((await client.close(5000)).native_resources_closed,false);
  });
});

test('native read reobservation keeps distinct and partial observations',async t=>{
  for(const [mode,status] of [['reobserve','historical_checks_completed'],['reobserve-unentered','selection_changed'],
    ['reobserve-partial','cancelled'],['reobserve-unknown','outcome_unknown'],['reobserve-cleanup','cleanup_unconfirmed']])await t.test(mode,async st=>{
    const f=fixture(st,mode),client=f.launch();await commission(client);const id=client.collect(selection());
    const report=client.collectionReport(await until(client,e=>e.request_id===id));
    assert.equal(report.status,status);
    const [first,second]=report.requirements[0].calls;
    assert.deepEqual([first.ordinal,second.ordinal],[1,2]);
    assert.equal(first.arguments_sha256,second.arguments_sha256);
    assert.notEqual(first.native.input_id,second.native.input_id);
    assert.equal(second.native.run_id,['reobserve-unentered','reobserve-partial'].includes(mode)?null:'run-2');
    assert.equal(second.native.tool_call_id,['reobserve-unentered','reobserve-partial'].includes(mode)?null:'call-2');
    assert.equal(report.requirements[0].assessment,mode==='reobserve'?'verified':null);
    assert.equal(report.reusable_source_coverage,'unqualified_credential_continuity');
    assert.equal(f.requests().filter(r=>r.method==='connect/collect').length,1);
    assert.throws(()=>client.collect(selection()));
  });
});

test('read reobservation does not allow replayed or foreign observations',async t=>{
  for(const mode of ['reobserve-replay','reobserve-other-args-replay','reobserve-other-requirement-replay',
    'reobserve-partial-replay','reobserve-unentered-replay','reobserve-foreign-resource',
    'reobserve-foreign-context','reobserve-false-success','reobserve-missing-success'])await t.test(mode,async st=>{
    const f=fixture(st,mode),client=f.launch();await commission(client);const id=client.collect(selection());
    await assert.rejects(until(client,e=>e.request_id===id),/transport failed/);
    assert.equal((await client.close(5000)).native_resources_closed,false);
  });
});

test('active collection cancellation permanently retires the same owner',async t=>{
  const f=fixture(t,'pending'),client=f.launch();await commission(client);const id=client.collect(selection());
  const cancelled=client.cancel(id);assert.throws(()=>client.collect(selection('second')));
  const event=await until(client,e=>e.request_id===id);assert.equal(client.collectionReport(event).status,'cancelled');
  assert.equal(client.viewerCurrentAtObservation(event),false);
  await until(client,e=>e.request_id===cancelled);assert.equal((await client.close(5000)).native_resources_closed,true);
});

test('stale completed IDs cannot retire a newer collection',async t=>{
  const f=fixture(t),client=f.launch();await commission(client);
  const first=client.collect(selection());await until(client,e=>e.request_id===first);
  const second=client.collect(selection('second'));await until(client,e=>e.request_id===second);
  assert.throws(()=>client.cancel(first));const cancel=client.cancel(second);await until(client,e=>e.request_id===cancel);
  assert.equal(client.viewerCurrent,false);
});

test('native refusal burns attempt and exposes no arbitrary error or report',async t=>{
  const f=fixture(t,'refused'),client=f.launch();await commission(client);const id=client.collect(selection());
  const event=await until(client,e=>e.request_id===id);assert.equal(event.kind,'refused');assert.equal(client.collectionReport(event),undefined);
  assert.ok(!JSON.stringify(event.payload).includes('PRIVATE'));assert.throws(()=>client.collect(selection()));
});

test('observation timeout retains owner and never repeats a collection',async t=>{
  const f=fixture(t,'held');writeFileSync(join(f.root,'hold'),'hold');const client=f.launch();await commission(client);
  const id=client.collect(selection());await until(client,e=>e.kind==='state'&&client.state.active_collection===id);
  await assert.rejects(client.nextEvent(10),ServiceSetupTimeout);await assert.rejects(client.close(10),ServiceSetupTimeout);
  unlinkSync(join(f.root,'hold'));const result=await client.close(5000);assert.equal(result.process_joined,true);
  assert.equal(f.requests().filter(r=>r.method==='connect/collect').length,1);assert.equal(f.clients.length,1);
});

test('external retirement is never reversed by later commissioning or report state',async t=>{
  const f=fixture(t,'external-retired'),client=f.launch();await until(client,e=>e.payload.method==='connect/initialize');client.submit('prepare');
  await until(client,e=>e.kind==='unavailable'&&e.payload.reason==='expired');
  assert.throws(()=>client.collect(selection()));assert.equal(client.viewerCurrent,false);
});

test('closed collection refusal codes retain meaning without error text',async t=>{
  const f=fixture(t,'known-refused'),client=f.launch();await commission(client);const id=client.collect(selection());
  const event=await until(client,e=>e.request_id===id);assert.equal(event.payload.code,'collection_already_attempted');
  assert.ok(!JSON.stringify(event.payload).includes('PRIVATE-ERROR'));assert.throws(()=>client.collect(selection()));
});

test('multi-call and multi-requirement reports preserve every native correlation',async t=>{
  const f=fixture(t,'multi'),client=f.launch();await commission(client);const id=client.collect(selection());
  const report=client.collectionReport(await until(client,e=>e.request_id===id));
  assert.equal(report.total_calls,3);assert.deepEqual(report.requirements.map(r=>r.calls.length),[2,1]);
  assert.deepEqual(report.requirements.flatMap(r=>r.calls.map(c=>c.ordinal)),[1,2,3]);
  assert.equal(report.requirements[1].calls[0].native.profile_id,'required-members');
});

test('closed early v7 state rejects hidden authority and premature active owner',async t=>{
  for(const mode of ['bad-state-extra','bad-state-coverage','bad-state-active'])await t.test(mode,async st=>{
    const f=fixture(st,mode),client=f.launch();await assert.rejects(until(client,e=>e.payload.method==='connect/initialize'),/transport failed/);
    assert.equal((await client.close(5000)).native_resources_closed,false);
  });
});

test('commissioning and finalization alone do not enable collection admission',async t=>{
  const f=fixture(t,'no-ready'),client=f.launch();await until(client,e=>e.payload.method==='connect/initialize');client.submit('prepare');
  const prompt=await until(client,e=>e.kind==='prompt');client.answer(prompt.request_id,{kind:'approval',decision:'approve'});
  await until(client,e=>e.kind==='service_finalization'&&e.payload.state==='observed');
  assert.throws(()=>client.collect(selection()));assert.equal(f.requests().filter(r=>r.method==='connect/collect').length,0);
});


// Two synthetic setup owners in the same process, still not native acceptance.
const MULTI_PEER = PEER
 .replace("else if(mode!=='pending')finish(request);", "else if(mode!=='pending' && !(mode==='second-pending' && phase===1))finish(request);")
 .replace("const target=", "let phase=0,session='session',job='job';\nconst target=")
 .replaceAll("session_id:'session'", "session_id:session")
 .replaceAll("job_id:'job'", "job_id:job")
 .replace("job_id:mode==='bad-job'?'foreign':'job'", "job_id:mode==='reused-service-job' && phase===1?'job':job")
 .replace("if(mode==='bad-state-extra')", "Object.assign(state,{service_ids:['official','python'],service_id:'official',can_open_next_service:false});\nif(mode==='bad-state-extra')")
 .replace("Object.assign(state,{active_collection:null,can_collect:true});", "Object.assign(state,{active_collection:null,can_collect:true,can_open_next_service:phase===0});")
 .replace("if(mode==='no-ready')return;", "if(mode==='no-ready' || mode==='future-fence-no-ready')return;")
 .replace("reply(request.id,{target:request.params.target,resources_closed:true});", "if(mode!=='future-fence-no-ready')reply(request.id,{target:request.params.target,resources_closed:true});")
 .replace("event('unavailable',{reason:'finalizing',first_fence:{origin:'other_native_transition',scope:'scope',send_request_id:null,cancellation_requested_before:null}});", `const fence={origin:mode==='boundary-ascii'?'x'.repeat(256):mode==='boundary-multibyte'?'é'.repeat(128):mode.startsWith('future-fence')?'future_native_transition_v8':mode==='changed-first-fence' && phase===1?'other_native_failure':'other_native_transition',scope:'scope',send_request_id:null,cancellation_requested_before:null};
  const invalid={'bad-first-fence':[], 'null-first-fence':null, 'empty-first-fence':'', 'long-first-fence':'x'.repeat(257), 'control-first-fence':String.fromCharCode(10), 'multibyte-long-first-fence':'é'.repeat(128)+'x'};
  if(Object.hasOwn(invalid,mode))fence.origin=invalid[mode];
  if(mode==='foreign-first-fence')fence.scope='foreign';
  if(mode==='bad-fence-send')fence.send_request_id=3;
  if(mode==='bad-fence-cancel')fence.cancellation_requested_before='false';
  if(mode==='missing-fence-origin')delete fence.origin;
  if(mode==='extra-fence-field')fence.authority=true;
  event('unavailable',{reason:'finalizing',...(mode==='legacy-fence'?{}:{first_fence:fence})});`)
 .replace("if(request.method==='connect/send'){", `if(request.method==='connect/collection/open_next'){
  event('state',{state}); // Already-buffered old phase is still legal here.
  if(mode==='refused-next'){
   emit({jsonrpc:'2.0',id:request.id,error:{code:-32010,message:'refused',data:{code:'collection_unavailable'}}});continue;
  }
  phase=1;session='session-2';job='job-2';
  update({service_id:'python',can_open_next_service:false,available:false,can_send:false,can_collect:false,last_completion:null});
  Object.assign(state,{available:true,can_send:true});
  reply(request.id,{scope:mode==='wrong-next-scope'?'foreign':'scope',service_id:mode==='wrong-next-service'?'foreign':'python',session_id:mode==='reused-service-session'?'session':session,
   state:mode==='wrong-next-state'?{...state,service_id:'official'}:state});
  if(mode==='backward-service-state')update({service_id:'official'});
 }else if(request.method==='connect/send'){`);

// A protected setup review uses the existing prompt correlation while open_next is pending.
const STARTUP_REVIEW_PEER = MULTI_PEER.replace(
 "  Object.assign(state,{available:true,can_send:true});",
 `  prompt('p:startup',{kind:'approval',summary:'Review next service setup',body:{change:'review_next_service_setup'}});
  const answer=JSON.parse((await lines.next()).value);
  appendFileSync(join(root,'requests.jsonl'),JSON.stringify(answer)+'\\n');
  event('prompt_closed',{prompt_id:'p:startup',disposition:'answered'});
  if(answer.result.answer.decision==='deny'){
   emit({jsonrpc:'2.0',id:request.id,error:{code:-32010,message:'refused',data:{code:'connector_unavailable'}}});continue;
  }
  Object.assign(state,{available:true,can_send:true});`);

// Native pre-admission refusal sends only the error. The synthetic admitted
// control also resets the active state it emitted before refusing.
const REJECTED_SOURCE_PEER = MULTI_PEER.replace("}else if(request.method==='connect/collect'){", `}else if(request.method==='connect/collect' && phase===0 && request.params.source_id==='second'){
  if(mode==='rejected-after-admission')update({active_collection:request.id,can_collect:false});
  const code=({'rejected-unavailable':'collection_unavailable','rejected-already':'collection_already_attempted','rejected-unknown':'PRIVATE_ERROR'})[mode]??'invalid_binding';
  emit({jsonrpc:'2.0',id:request.id,error:{code:-32010,message:'PRIVATE_ERROR',data:{code}}});
   if(mode==='rejected-after-admission')update({active_collection:null,can_collect:true});
 }else if(request.method==='connect/collect'){`);

const THREE_SERVICE_PEER = MULTI_PEER
 .replaceAll("['read','second']", "['read','second','third']")
 .replaceAll("['official','python']", "['official','python','third']")
 .replace("can_open_next_service:phase===0", "can_open_next_service:phase<2")
 .replace("phase=1;session='session-2';job='job-2';", "phase+=1;session='session-'+(phase+1);job='job-'+(phase+1);")
 .replace("service_id:'python'", "service_id:['official','python','third'][phase]")
 .replace("mode==='wrong-next-service'?'foreign':'python'", "mode==='wrong-next-service'?'foreign':['official','python','third'][phase]")
 .replace("(request.params.source_id==='read'?'1':'2')", "String(['read','second','third'].indexOf(request.params.source_id)+1)")
 .replace("}else if(request.method==='connect/collect'){", `}else if(request.method==='connect/collect' && request.params.source_id!==['read','second','third'][phase]){
   emit({jsonrpc:'2.0',id:request.id,error:{code:-32010,message:'refused',data:{code:'invalid_binding'}}});
  }else if(request.method==='connect/collect'){`);

async function nextCommission(client, onFence = () => {}, serviceId = 'python') {
 const request=client.openNextService({service_id:serviceId});
 // No new collection can use the buffered preceding service state.
 assert.throws(()=>client.collect({job_id:'job',source_id:'second'}));
 assert.throws(()=>client.openNextService({service_id:'python'}));
 await until(client,e=>e.request_id===request);
 client.submit('Prepare next preselected service');
 const prompt=await until(client,e=>{if(e.kind==='unavailable')onFence(e);return e.kind==='prompt';});
 client.answer(prompt.request_id,{kind:'approval',decision:'approve'});
 await until(client,e=>e.kind==='state' && client.state?.can_collect);
}

test('two service setups retain one process, immutable prior reports and first fence',async t=>{
 const f=fixture(t,'normal',MULTI_PEER),client=f.launch();await commission(client);
 const pid=JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid;
 assert.throws(()=>client.openNextService({service_id:'python'}));
 const firstId=client.collect(selection());const firstEvent=await until(client,e=>e.request_id===firstId);
 const first=client.collectionReport(firstEvent);
 assert.equal(first.session_id,'session');assert.equal(first.job_id,'job');
 for(const value of [{service_id:'official'},{service_id:'foreign'},{service_id:'python',receipt:{}}])assert.throws(()=>client.openNextService(value));
 await nextCommission(client);
 const secondId=client.collect({job_id:'job-2',source_id:'second'});
 const second=client.collectionReport(await until(client,e=>e.request_id===secondId));
 assert.equal(second.session_id,'session-2');assert.equal(second.job_id,'job-2');
 assert.equal(first.instance_id,second.instance_id);assert.equal(first.login_scope_id,second.login_scope_id);
 assert.equal(client.collectionReport(firstEvent),first);assert.ok(Object.isFrozen(first));
 assert.equal(JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid,pid);
 assert.throws(()=>client.collect({job_id:'job-2',source_id:'read'}));
 assert.throws(()=>client.openNextService({service_id:'official'}));
 assert.equal((await client.close(5000)).native_resources_closed,true);
 assert.equal(f.clients.length,1);
 assert.equal(f.requests().filter(r=>r.method==='connect/collection/open_next').length,1);
 assert.equal(f.requests().filter(r=>r.method==='connect/initialize').length,0); // The peer records the original init separately.
 assert.deepEqual(f.requests().find(r=>r.method==='connect/collection/open_next').params,{service_id:'python',scope:'scope'});
});

for (const decision of ['approve','deny']) test(`governed next setup ${decision} precedes result on the same pipe`,async t=>{
 const f=fixture(t,'normal',STARTUP_REVIEW_PEER),client=f.launch();await commission(client);
 const firstId=client.collect(selection()),firstEvent=await until(client,e=>e.request_id===firstId);
 const first=client.collectionReport(firstEvent);
 const pid=JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid;
 const request=client.openNextService({service_id:'python'});
 const prompt=await until(client,e=>e.kind==='prompt');
 assert.equal(prompt.payload.kind,'approval');
 assert.equal(client.state.service_id,'python');assert.equal(client.state.can_collect,false);
 assert.throws(()=>client.submit('before setup approval'));
 client.answer(prompt.request_id,{kind:'approval',decision});
 const result=await until(client,e=>e.request_id===request);
 assert.equal(result.kind,decision==='deny'?'refused':'result');
 assert.equal(client.collectionReport(firstEvent),first);
 assert.equal(first.session_id,'session');assert.equal(first.job_id,'job');
 assert.equal(JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid,pid);
 if(decision==='deny'){assert.equal(client.viewerCurrent,false);assert.throws(()=>client.openNextService({service_id:'python'}));}
 assert.equal((await client.close(5000)).native_resources_closed,true);
 assert.equal(f.requests().filter(r=>r.method==='connect/collection/open_next').length,1);
});

test('refused next service is terminal and never requeued',async t=>{
 const f=fixture(t,'refused-next',MULTI_PEER),client=f.launch();await commission(client);
 const id=client.collect(selection());await until(client,e=>e.request_id===id);
 const request=client.openNextService({service_id:'python'});
 assert.equal((await until(client,e=>e.request_id===request)).kind,'refused');assert.equal(client.viewerCurrent,false);
 assert.throws(()=>client.openNextService({service_id:'python'}));assert.throws(()=>client.submit('retry'));
 assert.throws(()=>client.collect({job_id:'job',source_id:'second'}));
 assert.throws(()=>client.cancel(id),/setup send is not current/);
 await client.close(5000);
 assert.equal(f.requests().filter(r=>r.method==='connect/cancel').length,0);
 assert.equal(f.requests().filter(r=>r.method==='connect/collection/open_next').length,1);
});

test('stale cancel in queued service transition keeps the original client usable',async t=>{
 const f=fixture(t,'normal',MULTI_PEER),client=f.launch();await commission(client);
 const firstId=client.collect(selection()),firstEvent=await until(client,e=>e.request_id===firstId);
 const first=client.collectionReport(firstEvent);
 const pid=JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid;
 const request=client.openNextService({service_id:'python'});
 // No await: native cannot deliver a next-phase frame before this call.
 assert.throws(()=>client.cancel(firstId),/setup send is not current/);
 assert.equal(client.viewerCurrent,true);
 await until(client,e=>e.request_id===request);
 client.submit('Prepare next preselected service');
 const prompt=await until(client,e=>e.kind==='prompt');
 client.answer(prompt.request_id,{kind:'approval',decision:'approve'});
 await until(client,e=>e.kind==='state' && client.state?.can_collect);
 const secondId=client.collect({job_id:'job-2',source_id:'second'});
 const second=client.collectionReport(await until(client,e=>e.request_id===secondId));
 assert.equal(second.job_id,'job-2');assert.equal(first.job_id,'job');
 assert.equal(client.collectionReport(firstEvent),first);
 assert.equal(JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid,pid);
 assert.equal((await client.close(5000)).native_resources_closed,true);
 assert.equal(f.requests().filter(r=>r.method==='connect/cancel').length,0);
 assert.equal(f.requests().filter(r=>r.method==='connect/collection/open_next').length,1);
});

for(const mode of ['wrong-next-scope','wrong-next-service','wrong-next-state','reused-service-session','reused-service-job','backward-service-state','changed-first-fence'])
 test(`multiple service observation refuses ${mode}`,async t=>{
  const f=fixture(t,mode,MULTI_PEER),client=f.launch();await commission(client);
  const id=client.collect(selection());await until(client,e=>e.request_id===id);
  await assert.rejects(async()=>{
   await nextCommission(client);
   await until(client,()=>false);
  },/service transport failed; retain native owner|service binding unavailable/);
  // The parser may retire before the next submit, or while its observer waits.
  // Both paths must reject the flow and leave cleanup unconfirmed.
  assert.equal((await client.close(5000)).native_resources_closed,false);
  assert.equal(f.requests().filter(r=>r.method==='connect/collection/open_next').length,1);
 });

for(const mode of ['bad-first-fence','null-first-fence','empty-first-fence','long-first-fence','control-first-fence','multibyte-long-first-fence',
 'foreign-first-fence','bad-fence-send','bad-fence-cancel','missing-fence-origin','extra-fence-field'])
 test('malformed first fence diagnostic is rejected: '+mode,async t=>{
  const f=fixture(t,mode,MULTI_PEER),client=f.launch();
  await assert.rejects(commission(client),/transport failed/);
  assert.equal((await client.close(5000)).native_resources_closed,false);
 });

for(const mode of ['future-fence','legacy-fence','boundary-ascii','boundary-multibyte'])
 test('diagnostic origin compatibility across both service phases: '+mode,async t=>{
  const f=fixture(t,mode,MULTI_PEER),client=f.launch(),fences=[];
  const observe=e=>fences.push(e.payload.first_fence);
  await commission(client,observe);
  const pid=JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid;
  const firstId=client.collect(selection()),firstEvent=await until(client,e=>e.request_id===firstId);
  const first=client.collectionReport(firstEvent);
  await nextCommission(client,observe);
  const origin=mode==='boundary-ascii'?'x'.repeat(256):mode==='boundary-multibyte'?'é'.repeat(128):'future_native_transition_v8';
  assert.equal(fences.length,2);
  if(mode==='legacy-fence')assert.deepEqual(fences,[undefined,undefined]);
  else {assert.equal(fences[0].origin,origin);assert.deepEqual(fences[1],fences[0]);}
  if(mode.startsWith('boundary-'))assert.equal(Buffer.byteLength(fences[0].origin,'utf8'),256);
  const id=client.collect({job_id:'job-2',source_id:'second'});
  const second=client.collectionReport(await until(client,e=>e.request_id===id));
  assert.equal(first.job_id,'job');assert.equal(second.job_id,'job-2');
  assert.equal(client.collectionReport(firstEvent),first);
  assert.equal(JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid,pid);
  assert.equal((await client.close(5000)).native_resources_closed,true);
 });

test('unknown diagnostic does not authorize collection or confirm close',async t=>{
 const f=fixture(t,'future-fence-no-ready',MULTI_PEER),client=f.launch();
 await until(client,e=>e.payload.method==='connect/initialize');client.submit('prepare');
 const fence=await until(client,e=>e.kind==='unavailable');
 assert.equal(fence.payload.first_fence.origin,'future_native_transition_v8');
 const prompt=await until(client,e=>e.kind==='prompt');client.answer(prompt.request_id,{kind:'approval',decision:'approve'});
 await until(client,e=>e.kind==='service_finalization' && e.payload.state==='observed');
 assert.equal(client.viewerCurrent,true);
 assert.throws(()=>client.collect(selection()));assert.throws(()=>client.openNextService({service_id:'python'}));
 assert.equal((await client.close(5000)).native_resources_closed,false);
 assert.equal(f.requests().filter(r=>r.method==='connect/collect').length,0);
});


test('second-service cancellation keeps earlier reports and refuses preceding cancel ids',async t=>{
 const f=fixture(t,'second-pending',MULTI_PEER),client=f.launch();await commission(client);
 const firstId=client.collect(selection());const firstEvent=await until(client,e=>e.request_id===firstId);
 const first=client.collectionReport(firstEvent);
 await nextCommission(client);
 assert.throws(()=>client.cancel(firstId));assert.equal(client.viewerCurrent,true);
 const secondId=client.collect({job_id:'job-2',source_id:'second'});
 await until(client,e=>e.kind==='state' && client.state?.active_collection===secondId);
 client.cancel(secondId);
 const second=client.collectionReport(await until(client,e=>e.request_id===secondId));
 assert.equal(second.status,'cancelled');assert.equal(second.job_id,'job-2');
 assert.equal(client.collectionReport(firstEvent),first);assert.equal(first.status,'historical_checks_completed');
 assert.equal(client.viewerCurrent,false);assert.throws(()=>client.openNextService({service_id:'official'}));
 assert.equal((await client.close(5000)).native_resources_closed,true);
 assert.equal(f.requests().filter(r=>r.method==='connect/cancel').length,1);
});

async function rejectedSource(t, mode) {
 const f=fixture(t,mode,REJECTED_SOURCE_PEER),client=f.launch();await commission(client);
 const id=client.collect({job_id:'job',source_id:'second'});
 const refused=await until(client,e=>e.request_id===id);
 assert.equal(refused.kind,'refused');assert.equal(client.collectionReport(refused),undefined);
 if(mode==='rejected-after-admission')await until(client,e=>e.kind==='state' && client.state?.can_collect);
 assert.equal(client.state.can_collect,true);
 assert.equal(client.viewerCurrent,true);
 assert.throws(()=>client.collect({job_id:'job',source_id:'second'}));
 assert.throws(()=>client.openNextService({service_id:'python'}));
 const firstId=client.collect(selection()),firstEvent=await until(client,e=>e.request_id===firstId);
 await nextCommission(client);
 return {f,client,refused,firstEvent};
}

test('pre-admission refusal does not burn later service source',async t=>{
 const {f,client,refused,firstEvent}=await rejectedSource(t,'rejected-before-admission');
 assert.equal(refused.payload.code,'invalid_binding');
 const pid=JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid;
 const id=client.collect({job_id:'job-2',source_id:'second'});
 const second=client.collectionReport(await until(client,e=>e.request_id===id));
 const first=client.collectionReport(firstEvent);
 assert.equal(first.source_id,'read');assert.equal(first.job_id,'job');
 assert.equal(second.source_id,'second');assert.equal(second.job_id,'job-2');
 for(const source_id of ['read','second'])assert.throws(()=>client.collect({job_id:'job-2',source_id}));
 assert.equal(JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid,pid);
 assert.equal((await client.close(5000)).native_resources_closed,true);
 assert.equal(f.clients.length,1);
 assert.deepEqual(f.requests().filter(r=>r.method==='connect/collect').map(r=>[r.params.job_id,r.params.source_id]),
  [['job','second'],['job','read'],['job-2','second']]);
});

for(const mode of ['rejected-after-admission','rejected-unavailable','rejected-already','rejected-unknown'])
 test('admitted or other refusal remains burned across services: '+mode,async t=>{
  const {f,client,refused,firstEvent}=await rejectedSource(t,mode);
  assert.throws(()=>client.collect({job_id:'job-2',source_id:'second'}));
  assert.equal(client.collectionReport(firstEvent).source_id,'read');
  assert.equal(client.collectionReport(refused),undefined);
  assert.equal((await client.close(5000)).native_resources_closed,true);
  assert.equal(f.requests().filter(r=>r.method==='connect/collect').length,2);
 });

test('future source refused in two jobs is collectable in its own third service',async t=>{
 const f=fixture(t,'normal',THREE_SERVICE_PEER),client=f.launch();await commission(client);
 const pid=JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid;
 const reports=[];
 for(const [index,[service_id,job_id,source_id]] of [
   ['official','job','read'],['python','job-2','second'],['third','job-3','third']].entries()){
  assert.equal(client.state.service_id,service_id);
  assert.deepEqual(client.state.collection_sources,['read','second','third']);
  if(index<2){
   const id=client.collect({job_id,source_id:'third'}),refused=await until(client,e=>e.request_id===id);
   assert.equal(refused.kind,'refused');assert.equal(refused.payload.code,'invalid_binding');
   assert.equal(client.collectionReport(refused),undefined);
   // No follow-up state frame is sent. Recovery uses the refusal.
   assert.equal(client.viewerCurrent,true);assert.equal(client.state.can_collect,true);
   assert.throws(()=>client.collect({job_id,source_id:'third'}));
  }
  const id=client.collect({job_id,source_id}),event=await until(client,e=>e.request_id===id);
  const report=client.collectionReport(event);reports.push({event,report});
  assert.deepEqual([report.job_id,report.source_id],[job_id,source_id]);
  for(const earlier of reports)assert.throws(()=>client.collect({job_id,source_id:earlier.report.source_id}));
  if(index<2)await nextCommission(client,()=>{},['python','third'][index]);
 }
 assert.deepEqual(reports.map(({report})=>report.session_id),['session','session-2','session-3']);
 assert.equal(new Set(reports.map(({report})=>report.attempt_id)).size,3);
 for(const {event,report} of reports){
  assert.equal(client.collectionReport(event),report);assert.ok(Object.isFrozen(report));
  assert.equal(report.instance_id,reports[0].report.instance_id);assert.equal(report.login_scope_id,reports[0].report.login_scope_id);
 }
 assert.equal(client.state.can_open_next_service,false);
 assert.equal(JSON.parse(readFileSync(join(f.root,'launch.json'),'utf8')).pid,pid);
 assert.equal((await client.close(5000)).native_resources_closed,true);assert.equal(f.clients.length,1);
 assert.deepEqual(f.requests().filter(r=>r.method==='connect/collect').map(r=>[r.params.job_id,r.params.source_id]),
  [['job','third'],['job','read'],['job-2','third'],['job-2','second'],['job-3','third']]);
 assert.deepEqual(f.requests().filter(r=>r.method==='connect/collection/open_next').map(r=>r.params.service_id),['python','third']);
});

const RETIREMENT_PEER=MULTI_PEER.replace("event('state',{state});pending=null;", `event('state',{state});pending=null;
 const fence={origin:'other_native_transition',scope:'scope',send_request_id:null,cancellation_requested_before:null};
 if(mode==='retirement-changed')fence.origin='different_origin';
 if(mode==='retirement-foreign')fence.scope='foreign';
 if(mode==='retirement-bad-type')fence.origin=[];
 if(mode==='retirement-extra')fence.authority=true;
 if(mode==='retirement-missing')delete fence.origin;
 const fields={reason:'expired',...(mode==='retirement-legacy'?{}:{first_fence:fence})};
 if(mode==='retirement-bad-reason')fields.reason=[];
 if(mode==='retirement-no-reason')delete fields.reason;
 if(mode==='retirement-extra-event')fields.authority=true;
 event('unavailable',fields);`);
for(const mode of ['retirement-same','retirement-legacy'])
 test('later retirement keeps valid or omitted diagnostic: '+mode,async t=>{
  const f=fixture(t,mode,RETIREMENT_PEER),client=f.launch();await commission(client);
  const id=client.collect(selection()),completed=await until(client,e=>e.request_id===id);
  const retired=await until(client,e=>e.kind==='unavailable' && e.payload.reason==='expired');
  if(mode==='retirement-same')assert.equal(retired.payload.first_fence.origin,'other_native_transition');
  else assert.equal(retired.payload.first_fence,undefined);
  assert.equal(client.collectionReport(completed).job_id,'job');assert.equal(client.viewerCurrent,false);
  assert.throws(()=>client.openNextService({service_id:'python'}));
  assert.equal((await client.close(5000)).native_resources_closed,true);
 });
for(const mode of ['retirement-changed','retirement-foreign','retirement-bad-type','retirement-extra','retirement-missing','retirement-bad-reason','retirement-no-reason','retirement-extra-event'])
 test('later retirement rejects malformed or changed diagnostic: '+mode,async t=>{
  const f=fixture(t,mode,RETIREMENT_PEER),client=f.launch();await commission(client);
  const id=client.collect(selection()),completed=await until(client,e=>e.request_id===id);
  await assert.rejects(until(client,e=>e.kind==='unavailable'),/transport failed/);
  assert.equal(client.collectionReport(completed).job_id,'job');
  assert.equal((await client.close(5000)).native_resources_closed,false);
 });
