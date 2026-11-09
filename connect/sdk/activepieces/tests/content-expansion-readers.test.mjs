import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import https from 'node:https';
import childProcess from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { parseConfiguration, readerConfiguration } from '../dist/mcp-config.js';
import { select, inputs } from '../dist/profiles.js';
import { Invocation } from '../dist/transport.js';
import { contentExpansionProfiles as profiles, KEY } from './fixtures/content-expansion-profiles.mjs';
const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return {promise,resolve}; };
const reader = (d, transport, key=KEY, options={}) => createReader({profile:d.profile,auth:d.auth(key),transport,...options});
const failure = (kind, entered=true, dispatched=true) => error => {
 assert.ok(error instanceof ActivepiecesError); assert.equal(error.kind,kind); assert.equal(error.entered,entered); assert.equal(error.dispatched,dispatched);
 assert.equal(error.cause,undefined); assert.equal(error.writeOutcome,'not_applicable');
 for(const secret of [KEY,encodeURIComponent(KEY),'unselected-secret',Buffer.from(`${KEY}:X`).toString('base64')]) assert.ok(!`${error.stack} ${JSON.stringify(error)}`.includes(secret));
 return true;
};
for (const d of profiles) {
 test(`${d.profile} executes the real action and SDK with selected request and useful output`, async () => {
  let calls=0, response;
  const run=reader(d,async(url,init)=>{
   calls++; assert.equal(url,d.url(KEY)); assert.equal(init.method,'GET'); assert.equal(init.redirect,'error'); assert.ok(init.signal instanceof AbortSignal);
   assert.deepEqual(Object.fromEntries(init.headers),d.headers(KEY)); assert.equal(init.body,undefined); return response=Response.json(d.body);
  });
  assert.equal(run.describe().action,d.action); assert.equal(run.describe().effect,'read');
  assert.deepEqual(await run.invoke(d.args),d.result); assert.equal(calls,1); assert.equal(response.body.locked,false); assert.equal(response.bodyUsed,true);
 });
 test(`${d.profile} snapshots connection/input and keeps overlapping accounts independent`, async()=>{
  const entered=deferred(),release=deferred(); const observed=[];
  const config={profile:d.profile,auth:d.auth('account-a'),transport:async(url,init)=>{observed.push([url,Object.fromEntries(init.headers)]);entered.resolve();await release.promise;return Response.json(d.body)}};
  const first=createReader(config); for(const key of Object.keys(config.auth)) config.auth[key]='unselected-secret';
  const args=structuredClone(d.args), pending=first.invoke(args); await entered.promise;
  for(const key of Object.keys(args)) args[key]='changed';
  try { assert.deepEqual(await reader(d,async(url,init)=>{observed.push([url,Object.fromEntries(init.headers)]);return Response.json(d.body)},'account-b').invoke(d.args),d.result); }
  finally { release.resolve(); }
  assert.deepEqual(await pending,d.result); assert.deepEqual(observed,['account-a','account-b'].map(key=>[d.url(key),d.headers(key)]));
 });
 test(`${d.profile} joins asynchronous body cancellation and then recovers`,async()=>{
  const entered=deferred(),cancelling=deferred(),release=deferred(),controller=new AbortController();let calls=0,settled=false,cancellations=0,body;
  const run=reader(d,async()=>{if(++calls>1)return Response.json(d.body);body=new ReadableStream({cancel(){cancellations++;cancelling.resolve();return release.promise}});entered.resolve();return new Response(body)});
  const pending=run.invoke(d.args,{signal:controller.signal}).finally(()=>{settled=true}), rejected=assert.rejects(pending,failure('cancelled'));
  await entered.promise;controller.abort();await cancelling.promise;await tick();assert.equal(settled,false);
  release.resolve();await rejected;assert.equal(cancellations,1);assert.equal(body.locked,false);assert.deepEqual(await run.invoke(d.args),d.result);
 });
 test(`${d.profile} joins a host request that ignores abort and its late response body`,async()=>{
  const entered=deferred(),headers=deferred(),cancelling=deferred(),release=deferred(),controller=new AbortController();let settled=false;
  const body=new ReadableStream({cancel(){cancelling.resolve();return release.promise}});
  const run=reader(d,async(_url,init)=>{entered.resolve(init.signal);await headers.promise;return new Response(body)});
  const pending=run.invoke(d.args,{signal:controller.signal}).finally(()=>{settled=true}),rejected=assert.rejects(pending,failure('cancelled'));
  const signal=await entered.promise;controller.abort();await tick();assert.equal(signal.aborted,true);assert.equal(settled,false);
  headers.resolve();await cancelling.promise;await tick();assert.equal(settled,false);release.resolve();await rejected;assert.equal(body.locked,false);
 });
 test(`${d.profile} refuses redirects and provider errors without replay or response reflection`,async()=>{
  for(const status of [301,302,307,401,403,404,429,503]){let calls=0,cancelled=0;
   const body=new ReadableStream({start(c){c.enqueue(new TextEncoder().encode(KEY+' unselected-secret'))},cancel(){cancelled++}});
   await assert.rejects(reader(d,async()=>{calls++;return new Response(body,{status,headers:{location:'https://unselected.invalid'}})}).invoke(d.args),failure(status<400?'redirect_refused':'http_error'));
   assert.equal(calls,1);assert.equal(cancelled,1);assert.equal(body.locked,false);
  }
 });
 test(`${d.profile} distinguishes malformed/oversized/invalid-UTF8 responses from a usable record`,async()=>{
  for(const body of [null,[],{},42])await assert.rejects(reader(d,async()=>Response.json(body)).invoke(d.args),failure('invalid_response'));
  await assert.rejects(reader(d,async()=>new Response(new Uint8Array([0xc3,0x28]))).invoke(d.args),failure('invalid_response'));
  await assert.rejects(reader(d,async()=>new Response('x'.repeat(1025)),KEY,{maxResponseBytes:1024}).invoke(d.args),failure('response_too_large'));
  await assert.rejects(reader(d,async()=>{throw Error(KEY+' unselected-secret')}).invoke(d.args),failure('transport_error'));
 });
 test(`${d.profile} rejects pre-abort, absent, null, empty and credential-bearing arguments before entry`,async()=>{
  let calls=0;const run=reader(d,async()=>{calls++;return Response.json(d.body)}),controller=new AbortController();controller.abort();
  await assert.rejects(run.invoke(d.args,{signal:controller.signal}),failure('cancelled',false,false));
  const key=Object.keys(d.args)[0];
  for(const args of [{},{...d.args,token:KEY},{...d.args,[key]:null},{...d.args,[key]:''},{...d.args,[key]:'\ud800'},{...d.args,[key]:'x\n'}])await assert.rejects(run.invoke(args),failure('invalid_arguments',false,false));
  assert.equal(calls,0);
 });
}

test('content configuration requires explicit selected service/account and excludes mutation fields',()=>{
 for(const d of profiles){const raw={profile:d.profile,token_env:'SELECTED_TOKEN',...d.config};
  assert.deepEqual(parseConfiguration(raw),raw);assert.equal(createReader(readerConfiguration(raw,KEY,async()=>{throw 0})).describe().profile,d.profile);
  for(const key of ['accessToken','apiKey','serviceRoleKey','refreshToken','clientSecret'])assert.throws(()=>parseConfiguration({...raw,[key]:KEY}));
  assert.throws(()=>reader(d,async()=>{throw 0},'invalid\n'));
 }
 for(const cloud of [undefined,null,'',{},'login.microsoftonline.com','unknown'])assert.throws(()=>createReader({profile:'outlook.get-message',auth:{accessToken:KEY,cloud},transport:async()=>{throw 0}}));
 for(const region of [undefined,null,'',{},'US','unknown'])assert.throws(()=>createReader({profile:'sendgrid.find-list-by-name',auth:{apiKey:KEY,region},transport:async()=>{throw 0}}));
});

test('response identity binding catches a real record from another selection',async()=>{
 for(const d of profiles.filter(d=>d.profile!=='sendgrid.find-list-by-name')){
  const body=structuredClone(d.body);
  if(d.profile==='contentful.get-entry')body.sys.id='another';
  else if(d.profile==='google-forms.get-form')body.formId='another';
  else if(d.profile==='google-slides.get-presentation')body.presentationId='another';
  else if(d.profile==='google-docs.get-document')body.documentId='another';else body.id='another';
  await assert.rejects(reader(d,async()=>Response.json(body)).invoke(d.args),failure('invalid_response'));
 }
});

test('Slides masks preserve absent/null/empty semantics and do not invent a returned identity',async()=>{
 const d=profiles[2];for(const fields of [undefined,null,'','   ']){const args={...d.args,...(fields===undefined?{}:{fields})};
  assert.deepEqual(await reader(d,async(url)=>{assert.equal(url,d.url(KEY));return Response.json(d.body)}).invoke(args),d.result);
 }
 const masked={title:'Title only'};assert.deepEqual(await reader(d,async url=>{assert.equal(url,d.url(KEY)+'?fields=title');return Response.json(masked)}).invoke({presentation_id:'https://docs.google.com/presentation/d/presentation_123/edit',fields:' title '}),masked);
 await assert.rejects(reader(d,async()=>Response.json({...masked,presentationId:'another'})).invoke({...d.args,fields:'title'}),failure('invalid_response'));
});

test('OneDrive path encoding and Outlook explicit mailbox/cloud stay literal selected paths',async()=>{
 const drive=profiles[3];assert.deepEqual(await reader(drive,async url=>{assert.equal(url,'https://graph.microsoft.com/v1.0/me/drive/root:/Folder/%C3%A5%20%23%3F%25.txt:');return Response.json(drive.body)}).invoke({path:' /Folder/å #?%.txt/ '}),drive.result);
 for(const args of [{itemId:'id',path:'x'},{itemId:null},{path:'/'},{path:'a//b'},{path:'a/../b'},{path:'./x'},{path:'a/.',itemId:''}])await assert.rejects(reader(drive,async()=>{throw 0}).invoke(args),failure('invalid_arguments',false,false));
 const outlook=profiles[7];assert.deepEqual(await createReader({profile:outlook.profile,auth:{accessToken:KEY,cloud:'government',mailbox:'user+fixture@example.invalid'},transport:async url=>{assert.equal(url,'https://graph.microsoft.us/v1.0/users/user%2Bfixture%40example.invalid/messages/message%2F%23%3F%25');return Response.json({...outlook.body,id:'message/#?%'})}}).invoke({messageId:' message/#?% '}),{...outlook.body,id:'message/#?%'});
});

test('HubSpot executes the generated parser, normalizes its Date model, and accepts optional null/empty extras',async()=>{
 const d=profiles[8];for(const extras of [undefined,null,[]])assert.deepEqual(await reader(d,async()=>Response.json(d.body)).invoke({...d.args,...(extras===undefined?{}:{additionalPropertiesToRetrieve:extras})}),d.result);
 const body={...d.body,createdAt:'2026-01-02T04:04:05+01:00',unknown:'SDK drops unknown model fields'};
 assert.deepEqual(await reader(d,async url=>{assert.equal(new URL(url).searchParams.get('properties').split(',').at(-1),'custom_field');return Response.json(body)}).invoke({...d.args,additionalPropertiesToRetrieve:['custom_field']}),d.result);
 for(const extra of ['',false,{},['x','x'],['x\n'],['x-y']])await assert.rejects(reader(d,async()=>{throw 0}).invoke({...d.args,additionalPropertiesToRetrieve:extra}),failure('invalid_arguments',false,false));
 for(const bad of [{...d.body,createdAt:'invalid'},{...d.body,properties:[]},{...d.body,properties:{x:42}}])await assert.rejects(reader(d,async()=>Response.json(bad)).invoke(d.args),failure('invalid_response'));
});

test('Supabase uses the actual SDK bare-user and user-envelope transforms with donor null defaults',async()=>{
 const d=profiles[9];assert.deepEqual(await reader(d,async()=>Response.json({user:d.body})).invoke(d.args),d.result);
 assert.deepEqual(await reader(d,async()=>Response.json({...d.body,phone:'',user_metadata:{},app_metadata:null,last_sign_in_at:null})).invoke(d.args),{...d.result,phone:''});
 await assert.rejects(reader(d,async()=>Response.json({...d.body,created_at:null})).invoke(d.args),failure('invalid_response'));
});

test('SendGrid refuses paginated catalogs before reporting complete matches or a false miss',async()=>{
 const d=profiles[10];for(const next of ['https://api.sendgrid.com/v3/marketing/lists?page_token=2','https://unselected.invalid', ' ']){let calls=0;
  await assert.rejects(reader(d,async()=>{calls++;return Response.json({...d.body,_metadata:{next}})}).invoke(d.args),failure('response_too_large'));assert.equal(calls,1);
 }
 for(const body of [{result:[]},{result:[],_metadata:null},{result:[],_metadata:{next:null}},{result:[],_metadata:{next:''}}])assert.deepEqual(await reader(d,async()=>Response.json(body)).invoke(d.args),{found:false,lists:[]});
 for(const body of [{},{result:null},{result:[{}]},{result:[],_metadata:[]},{result:[],_metadata:{next:3}}])await assert.rejects(reader(d,async()=>Response.json(body)).invoke(d.args),failure('invalid_response'));
 const matches=[{id:'a',name:'NEWSLETTER'},{id:'b',name:' newsletter '}];assert.deepEqual(await reader(d,async()=>Response.json({result:matches})).invoke(d.args),{found:true,lists:matches});
 await assert.rejects(reader(d,async()=>Response.json({result:Array.from({length:1001},(_,i)=>({id:String(i),name:'x'}))})).invoke(d.args),failure('response_too_large'));
});

test('all twelve seams refuse retargeting, credential replacement, writes and queries before host dispatch',async()=>{
 // Obtain real donor requests from the real factory, then independently perturb them.
 const {createDonorBundle}=await import('../dist/donor-factory.js');
 for(const d of profiles){let request;
  const selection=select({profile:d.profile,auth:d.auth(KEY),transport:async()=>{throw Error('Host must not be reached')}}),prepared=inputs(selection,d.args);
  const donor=createDonorBundle(async value=>{request=value;return{status:200,headers:{},body:structuredClone(d.body)}});
  await donor[d.export].run({auth:structuredClone(selection.auth),propsValue:prepared.props});assert.ok(request);
  const changes=[{url:'https://unselected.invalid/x'},{method:'POST',body:{}},{url:request.url+'/other'},{queryParams:{unselected:'x'}},request.authentication?{authentication:{...request.authentication,token:'unselected-secret'}}:{headers:{...request.headers,authorization:'Bearer unselected-secret'}}];
  for(const change of changes){const inv=new Invocation(selection,prepared.pathname,undefined,prepared.query,prepared.inlineQuery);
   try{await assert.rejects(inv.request({...request,...change}),failure('upstream_error',false,false));}finally{await inv.finish()}
  }
 }
});

test('SDK reads and metadata start no ambient network, subprocess, refresh or session machinery',async t=>{
 let ambient=0;const refuse=()=>{ambient++;throw Error('Ambient operation refused')};
 t.mock.method(http,'request',refuse);t.mock.method(https,'request',refuse);t.mock.method(globalThis,'fetch',refuse);t.mock.method(childProcess,'spawn',refuse);t.mock.method(childProcess,'exec',refuse);
 const vars={GOOGLE_APPLICATION_CREDENTIALS:'/unselected/private.json',GOOGLE_CLOUD_UNIVERSE_DOMAIN:'unselected.invalid',GOOGLE_CLOUD_QUOTA_PROJECT:'unselected-project',HTTP_PROXY:'http://unselected.invalid:123',HTTPS_PROXY:'http://unselected.invalid:123',ALL_PROXY:'http://unselected.invalid:123',SUPABASE_URL:'https://unselected.invalid',SUPABASE_KEY:'unselected-secret'};
 const previous=Object.fromEntries(Object.keys(vars).map(key=>[key,process.env[key]]));Object.assign(process.env,vars);
 try{for(const d of profiles)assert.deepEqual(await reader(d,async()=>Response.json(d.body)).invoke(d.args),d.result);await tick();assert.equal(ambient,0)}
 finally{for(const[key,value]of Object.entries(previous))if(value===undefined)delete process.env[key];else process.env[key]=value}
});

test('new SDK request deadlines retain the real pending work and late body until cleanup finishes',async t=>{
 for(const d of profiles.filter(d=>['google-docs.get-document','google-drive.get-file','contentful.get-entry','outlook.get-message','hubspot.get-contact','supabase.get-user'].includes(d.profile))){
  const entered=deferred(),headers=deferred(),cancelling=deferred(),release=deferred();let settled=false;
  const body=new ReadableStream({cancel(){cancelling.resolve();return release.promise}});
  const run=reader(d,async(_url,init)=>{entered.resolve(init.signal);await headers.promise;return new Response(body)},KEY,{timeoutMs:1000});
  t.mock.timers.enable({apis:['setTimeout']});
  try{
   const pending=run.invoke(d.args).finally(()=>{settled=true}),rejected=assert.rejects(pending,failure('deadline_exceeded'));
   const signal=await entered.promise;t.mock.timers.tick(1000);await tick();assert.equal(signal.aborted,true);assert.equal(settled,false);
   headers.resolve();await cancelling.promise;await tick();assert.equal(settled,false);release.resolve();await rejected;assert.equal(body.locked,false);
  }finally{headers.resolve();release.resolve();t.mock.timers.reset()}
 }
});

test('selected action bodies contribute to the bundle while unrelated index registrations and Supabase session client do not',async()=>{
 const root=new URL('../',import.meta.url),build=JSON.parse(await readFile(new URL('dist/BUILD-MANIFEST.json',root),'utf8'));
 const output=build.metafile.outputs['dist/donor-factory.js'].inputs;
 const actions=['freshdesk/src/lib/actions/get-contact-from-id.ts','google-forms/src/lib/actions/forms-get-form.ts','google-slides/src/lib/actions/get-presentation.ts',
  'microsoft-onedrive/src/lib/actions/onedrive-get-item.ts','google-docs/src/lib/actions/get-document.action.ts','google-drive/src/lib/action/drive-get-file.ts',
  'contentful/src/lib/actions/records/get-record.ts','microsoft-outlook/src/lib/actions/outlook-get-message.ts','hubspot/src/lib/actions/get-contact.ts',
  'supabase/src/lib/actions/get-user.ts','sendgrid/src/lib/actions/find-list-by-name.ts','webflow/src/lib/actions/get-collection-item.ts'];
 for(const action of actions){const item=Object.entries(output).find(([name])=>name.endsWith('/community/'+action));assert.ok(item,action);assert.ok(item[1].bytesInOutput>0,action)}
 for(const name of Object.keys(build.metafile.inputs)){
  assert.ok(!name.includes('/@supabase/auth-js/dist/module/GoTrueClient.js'),name);
  assert.ok(!/community\/(freshdesk|webflow)\/src\/lib\/(trigger|triggers)\//.test(name),name);
  if(/community\/(freshdesk|webflow)\/src\/lib\/actions\//.test(name))assert.match(name,/\/(get-contact-from-id|get-collection-item)\.ts$/);
 }
 const projection=JSON.parse(await readFile(new URL('dist/SOURCE-PROJECTIONS.json',root),'utf8'));
 for(const piece of ['freshdesk','webflow']){
  const match=projection.records.find(row=>row.source.endsWith('/'+piece+'/src/index.ts'));assert.ok(match);
  const source=await readFile(new URL('dist/SOURCE-PROJECTIONS/'+match.source,root),'utf8');assert.ok(!source.includes('createPiece('));assert.ok(!source.includes('actions:'));assert.ok(!source.includes('triggers:'));
 }
});

test('Supabase SDK stderr never receives the original credential-bearing transport exception',async t=>{
 const observed=[];t.mock.method(console,'error',(...items)=>observed.push(items));const d=profiles[9];
 await assert.rejects(reader(d,async()=>{throw new Error(KEY+' '+d.url(KEY),{cause:{credential:KEY}})}).invoke(d.args),failure('transport_error'));
 assert.equal(observed.length,1);assert.equal(observed[0].length,1);assert.equal(observed[0][0].message,'Supabase owned transport failed');assert.equal(observed[0][0].cause,undefined);
 assert.ok(!observed[0][0].stack.includes(KEY));assert.ok(!observed[0][0].stack.includes(d.url(KEY)));
});

test('real elapsed deadline before donor entry refuses action and transport even before the timer callback runs',async t=>{
 const original=globalThis.setTimeout, pause=new Int32Array(new SharedArrayBuffer(4));let calls=0;
 // Hold real timer registration briefly, making expiry precede factory entry.
 // No clock or timer callback is advanced artificially in this test.
 t.mock.method(globalThis,'setTimeout',(callback,delay,...args)=>{
  const timer=original(callback,delay,...args);Atomics.wait(pause,0,0,10);return timer;
 });
 const started=performance.now();
 await assert.rejects(createReader({profile:'tally.get-form',auth:{apiKey:KEY},transport:async()=>{calls++;return Response.json({})},timeoutMs:1}).invoke({form_id:'form123'}),failure('deadline_exceeded',false,false));
 assert.ok(performance.now()-started>=1);assert.equal(calls,0);
});
