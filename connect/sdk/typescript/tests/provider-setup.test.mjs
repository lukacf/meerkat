import test from 'node:test';
import childProcess from 'node:child_process';
import {syncBuiltinESMExports} from 'node:module';
import assert from 'node:assert/strict';
import {mkdtempSync,writeFileSync,chmodSync,readFileSync,existsSync,unlinkSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {ProviderSetupClient,decodeProviderSelection} from '../dist/provider-setup.js';

// A real isolated process/pipe, with no native auth, credentials or providers.
const peer = String.raw`
import {createInterface} from 'node:readline';
import {writeFileSync,existsSync,appendFileSync} from 'node:fs';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
setTimeout(()=>process.exit(88),10000).unref();
const lines=createInterface({input:process.stdin})[Symbol.asyncIterator]();
const first=await lines.next();if(first.done)process.exit(0);
const request=JSON.parse(first.value),b=request.binding,root=b.context_root;
writeFileSync(join(root,'observed.json'),JSON.stringify({pid:process.pid,env:process.env,argv:process.argv.slice(2),binding:b}));
process.on('exit',code=>writeFileSync(join(root,'exited.json'),JSON.stringify({pid:process.pid,code})));
const out=value=>process.stdout.write(JSON.stringify(value)+'\n');
out({kind:'initialized',id:'initialize',protocol:'connect-provider-setup/1',interaction_id:'actual'});
out({kind:'prompt',interaction_id:'actual',prompt_id:'p',prompt:{kind:'choices',choices:[]}});
const next=await lines.next();if(next.done)process.exit(0);
const answer=JSON.parse(next.value);appendFileSync(join(root,'requests.jsonl'),JSON.stringify(answer)+'\n');
if(answer.method==='cancel'){
 out({kind:'cancelling',id:answer.id,interaction_id:'actual'});out({kind:'prompt_closed',prompt_id:'p'});
 while(existsSync(join(root,'hold')))await delay(10);
 out({kind:'cancelled',interaction_id:'actual',joined:true,join_scope:'provider_interaction',native_effects:'reobserve'});
}else{
 out({kind:'answered',id:answer.id,prompt_id:'p'});
 out({kind:'selected',selection:{interaction_id:'actual',model:'native-model',provider:'openai',auth_binding:{realm:'owned',binding:'selected'},credential:'ready',live_entitlement:'unobserved'}});
}
// The peer deliberately owns the stdin reader until its terminal write drains.
await new Promise(resolve=>process.stdout.write('',resolve));process.exit(0);
`;
function fixture(t){
 const root=mkdtempSync(join(tmpdir(),'provider-pipe-')),exe=join(root,'peer.mjs');
 writeFileSync(exe,'#!'+process.execPath+'\n'+peer);chmodSync(exe,0o700);
 const binding={application_id:'application',instance_id:'instance',principal_id:'user',login_scope_id:'login',expires_at_ms:Date.now()+120000,
  realm_id:'owned',context_root:root,state_root:join(root,'state'),user_config_root:join(root,'home'),credential_scope:{kind:'file',root:join(root,'credentials')},browser:'manual_return'};
 const clients=[];
 t.after(async()=>{if(existsSync(join(root,'hold')))unlinkSync(join(root,'hold'));
  let failure;
  for(const client of clients){for(;;){try{await client.close(1000);break;}catch(error){
   failure??=error;
   // Observation expiry retains this exact child. Its own bounded fixture
   // exit guarantees eventual terminal observation even if EOF regresses.
   if(String(error.message).includes('same child retained'))continue;
   break; // Other close errors are surfaced only after its retained join.
  }}}
  rmSync(root,{recursive:true,force:true});if(failure)throw failure;
 });
 return{root,exe,binding,clients,async open(environment){const client=new ProviderSetupClient(exe,binding,environment);clients.push(client);
  assert.equal((await client.nextEvent(10000)).kind,'initialized');assert.equal((await client.nextEvent(10000)).kind,'prompt');return client;}};
}
test('real provider pipe preserves explicit binding and never claims a session',async t=>{
 const f=fixture(t),client=await f.open();client.answer('p',{kind:'choice',choice_id:'offered'});
 assert.equal((await client.nextEvent(10000)).kind,'answered');const selected=await client.nextEvent(10000);
 assert.deepEqual(decodeProviderSelection(selected.selection).auth_binding,{realm:'owned',binding:'selected'});
 assert.ok(!('session_id' in selected));assert.equal((await client.close(10000)).kind,'selected');
 assert.equal(JSON.parse(readFileSync(join(f.root,'exited.json'))).code,0);
});
test('launch environment is host selected and does not inherit ambient auth',async t=>{
 const f=fixture(t),launched=[],realSpawn=childProcess.spawn;
 // Observe the real built-in invocation without substituting a child or pipe.
 childProcess.spawn=(...args)=>{launched.push({...args[2].env});return realSpawn(...args);};
 syncBuiltinESMExports();
 let client;
 try{client=await f.open();}finally{childProcess.spawn=realSpawn;syncBuiltinESMExports();}
 await client.close(10000);
 const expected={PATH:'/usr/bin:/bin',LANG:'C'};
 assert.deepEqual(launched,[expected],'exact host spawn environment before runtime initialization');
 const observed=JSON.parse(readFileSync(join(f.root,'observed.json'))),runtime=observed.env;
 assert.deepEqual(Object.fromEntries(Object.keys(expected).map(key=>[key,runtime[key]])),expected);
 const additions=Object.keys(runtime).filter(key=>!(key in expected));
 assert.deepEqual(additions,process.platform==='darwin'&&('__CF_USER_TEXT_ENCODING' in runtime)?['__CF_USER_TEXT_ENCODING']:[]);
 if('__CF_USER_TEXT_ENCODING' in runtime){
  const fields=runtime.__CF_USER_TEXT_ENCODING.split(':');assert.equal(fields.length,3);
  assert.ok(fields.every(field=>/^0x[0-9a-fA-F]{1,8}$/u.test(field)));
  assert.equal(Number.parseInt(fields[0].slice(2),16),process.geteuid());
 }
 for(const key of ['HOME','OPENAI_API_KEY','ANTHROPIC_API_KEY','RKAT_CONFIG','MEERKAT_CONFIG'])assert.ok(!(key in runtime));
 assert.deepEqual(observed.argv,['--provider-stdio','--provider-store-root',f.binding.credential_scope.root]);
});
test('native default requires explicit fixed host HOME',async t=>{
 const f=fixture(t);f.binding.credential_scope={kind:'native_default'};
 assert.throws(()=>new ProviderSetupClient(f.exe,f.binding),/fixed HOME/);
 const client=await f.open({PATH:'/usr/bin:/bin',LANG:'C',HOME:join(f.root,'fixed-home')});await client.close(10000);
 const observed=JSON.parse(readFileSync(join(f.root,'observed.json')));
 assert.equal(observed.env.HOME,join(f.root,'fixed-home'));assert.deepEqual(observed.argv,['--provider-stdio','--provider-native-default']);
});
test('invalid finite timeout arguments do not cancel an active interaction',async t=>{
 const f=fixture(t),client=await f.open();
 for(const timeout of [0,-1,NaN,Infinity,2_147_483_648]){await assert.rejects(client.close(timeout),/positive and finite/);await assert.rejects(client.nextEvent(timeout),/positive and finite/);}
 assert.ok(!existsSync(join(f.root,'requests.jsonl')));
 client.answer('p',{kind:'confirm'});assert.equal((await client.nextEvent(10000)).kind,'answered');
 assert.equal((await client.nextEvent(10000)).kind,'selected');await client.close(10000);
 const requests=readFileSync(join(f.root,'requests.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
 assert.deepEqual(requests.map(r=>r.method),['answer']);
});
test('observation timeout retains the actual child and a later close joins it',async t=>{
 const f=fixture(t);writeFileSync(join(f.root,'hold'),'');const client=await f.open();
 const pid=JSON.parse(readFileSync(join(f.root,'observed.json'))).pid;
 await assert.rejects(client.close(25),/same child retained/);process.kill(pid,0);
 assert.ok(!existsSync(join(f.root,'exited.json')));unlinkSync(join(f.root,'hold'));
 assert.equal((await client.close(10000)).kind,'cancelled');assert.equal(JSON.parse(readFileSync(join(f.root,'exited.json'))).pid,pid);
});
test('oversized initialization and forbidden environment refuse before spawn',async t=>{
 const f=fixture(t);assert.throws(()=>new ProviderSetupClient(f.exe,{...f.binding,context_root:'/'+'x'.repeat(1024*1024)}),/frame bound/);
 for(const key of ['OPENAI_API_KEY','__CF_USER_TEXT_ENCODING','LC_CTYPE'])assert.throws(()=>new ProviderSetupClient(f.exe,f.binding,{[key]:'not-host-admitted'}),/isolated/);
 assert.throws(()=>new ProviderSetupClient(f.exe,{...f.binding,credential_scope:{kind:'file',root:'relative'}}),/credential scope/);
 await delay(30);assert.ok(!existsSync(join(f.root,'observed.json')));
});
test('stale answers never reach the protected pipe',async t=>{
 const f=fixture(t),client=await f.open();assert.throws(()=>client.answer('old',{kind:'callback',url:'private'}),/current/);
 await client.close(10000);assert.throws(()=>client.answer('p',{kind:'confirm'}),/current/);
 assert.deepEqual(readFileSync(join(f.root,'requests.jsonl'),'utf8').trim().split('\n').map(line=>JSON.parse(line).method),['cancel']);
});
test('selection decoder requires configured origin and nonempty optional profile',()=>{
 const base={interaction_id:'actual',model:'model',provider:'openai',auth_binding:{realm:'owned',binding:'selected'},credential:'ready',live_entitlement:'unobserved'};
 for(const extra of [{},{origin:'configured'},{origin:'configured',profile:'named'},{profile:null}])decodeProviderSelection({...base,auth_binding:{...base.auth_binding,...extra}});
 for(const extra of [{origin:'synthetic_env_default'},{origin:'unknown'},{origin:null},{origin:7},{profile:''},{profile:7}])assert.throws(()=>decodeProviderSelection({...base,auth_binding:{...base.auth_binding,...extra}}));
 for(const changes of [{auth_binding:null},{auth_binding:{realm:'owned'}},{session_id:'false'},{live_entitlement:'proven'}])assert.throws(()=>decodeProviderSelection({...base,...changes}));
});
