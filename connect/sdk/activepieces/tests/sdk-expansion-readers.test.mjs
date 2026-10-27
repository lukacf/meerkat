import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import http from 'node:http';
import fs from 'node:fs';
import https from 'node:https';
import childProcess from 'node:child_process';
import {createHmac} from 'node:crypto';
import {createReader,ActivepiecesError} from '../dist/index.js';
import {select,inputs} from '../dist/profiles.js';
import {Invocation} from '../dist/transport.js';
import {createDonorBundle} from '../dist/donor-factory.js';
import {parseConfiguration,readerConfiguration,selectedEnvironment,describeConfiguration} from '../dist/mcp-config.js';
import {MONDAY_QUERY} from '../dist/sdk-contracts.js';
import {sdkProfiles as profiles,KEY,response,xml,ghostKey} from './fixtures/sdk-expansion-profiles.mjs';
const tick=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
const reader=(d,transport,key=KEY,options={})=>createReader({profile:d.profile,auth:d.auth(key),transport,...options});
const failure=(kind,entered=true,dispatched=true)=>error=>{assert.ok(error instanceof ActivepiecesError);assert.equal(error.kind,kind);assert.equal(error.entered,entered);assert.equal(error.dispatched,dispatched);assert.equal(error.cause,undefined);for(const secret of [KEY,ghostKey(KEY),'unselected-secret','selected-session-token'])assert.ok(!`${error.stack} ${JSON.stringify(error)}`.includes(secret));return true;};
const result=(d,value)=>{if(d.result)assert.deepEqual(value,d.result);else {assert.equal(value.success,true);assert.deepEqual(value.summary,{emails_sent:100,unique_opens:20,unique_clicks:0,open_rate:0.2,click_rate:0,bounce_rate:3,unsub_rate:0,abuse_rate:0});assert.equal(value.performance_metrics.clicks.total,null);}};
function checkRequest(d,key,url,init){
 assert.equal(url,d.url);assert.equal(init.method,d.method);assert.equal(init.redirect,'error');assert.ok(init.signal instanceof AbortSignal);assert.equal(init.body,d.requestBody?JSON.stringify(d.requestBody):undefined);
 const authorization=init.headers.get('authorization');
 if(d.profile==='ghost.get-post'){
  const jwt=authorization.slice(6).split('.'),header=JSON.parse(Buffer.from(jwt[0],'base64url')),claims=JSON.parse(Buffer.from(jwt[1],'base64url'));
  assert.deepEqual(header,{alg:'HS256',typ:'JWT',kid:ghostKey(key).split(':')[0]});assert.equal(claims.aud,'/admin/');assert.equal(claims.exp-claims.iat,300);
  assert.equal(jwt[2],createHmac('sha256',Buffer.from(ghostKey(key).split(':')[1],'hex')).update(jwt[0]+'.'+jwt[1]).digest('base64url'));
 }else if(d.profile==='amazon-s3.list-files'){
  assert.match(authorization,/Credential=AKIAABCDEFGHIJKLMNOP\/[0-9]{8}\/eu-north-1\/s3\/aws4_request/);assert.equal(init.headers.get('x-amz-security-token'),'selected-session-token');assert.equal(init.headers.get('amz-sdk-request'),'attempt=1; max=1');
 }else assert.equal(authorization,(d.profile.startsWith('mailchimp')?'Bearer ':'')+key);
}
for(const d of profiles){
 test(`${d.profile} deadline owns an abort-ignoring SDK request until late rejection and then recovers`,async t=>{
  t.mock.timers.enable({apis:['setTimeout']});const entered=deferred(),release=deferred(),aborted=deferred();let calls=0,settled=false;const unhandled=[];const onUnhandled=e=>unhandled.push(e);process.on('unhandledRejection',onUnhandled);
  const run=reader(d,async(_url,init)=>{if(++calls>1)return response(d);init.signal.addEventListener('abort',()=>aborted.resolve(),{once:true});entered.resolve();await release.promise;throw Error(KEY+' late SDK rejection')},KEY,{timeoutMs:60000});
  try{const pending=run.invoke(d.args).finally(()=>settled=true),rejected=assert.rejects(pending,failure('deadline_exceeded'));await entered.promise;t.mock.timers.tick(60000);await aborted.promise;await tick();assert.equal(settled,false);release.resolve();await rejected;result(d,await run.invoke(d.args));await tick();assert.deepEqual(unhandled,[]);}finally{release.resolve();t.mock.timers.reset();process.removeListener('unhandledRejection',onUnhandled);}
 });
 test(`${d.profile} executes unchanged donor behavior through actual SDK/helper with one owned request`,async()=>{
  let calls=0,body;const run=reader(d,async(url,init)=>{calls++;checkRequest(d,KEY,url,init);return body=response(d)});
  assert.equal(run.describe().action,d.action);assert.equal(run.describe().effect,'read');result(d,await run.invoke(d.args));assert.equal(calls,1);assert.equal(body.bodyUsed,true);assert.equal(body.body.locked,false);
 });
 test(`${d.profile} snapshots inputs/configuration and isolates overlapping account instances`,async()=>{
  const entered=deferred(),release=deferred();let calls=0;
  const config={profile:d.profile,auth:d.auth('account-a'),transport:async(url,init)=>{calls++;checkRequest(d,'account-a',url,init);entered.resolve();await release.promise;return response(d)}};
  const first=createReader(config),args=structuredClone(d.args),pending=first.invoke(args);await entered.promise;Object.keys(config.auth).forEach(k=>config.auth[k]='unselected-secret');Object.keys(args).forEach(k=>args[k]='changed');
  try{result(d,await reader(d,async(url,init)=>{calls++;checkRequest(d,'account-b',url,init);return response(d)},'account-b').invoke(d.args));}finally{release.resolve();}
  result(d,await pending);assert.equal(calls,2);
 });
 test(`${d.profile} cancellation joins response body cleanup and recovers`,async()=>{
  const entered=deferred(),cancelling=deferred(),release=deferred(),controller=new AbortController();let calls=0,settled=false,cancellations=0,body;
  const run=reader(d,async()=>{if(++calls>1)return response(d);body=new ReadableStream({cancel(){cancellations++;cancelling.resolve();return release.promise}});entered.resolve();return new Response(body)});
  const pending=run.invoke(d.args,{signal:controller.signal}).finally(()=>settled=true),rejected=assert.rejects(pending,failure('cancelled'));
  await entered.promise;controller.abort();await cancelling.promise;await tick();assert.equal(settled,false);release.resolve();await rejected;assert.equal(cancellations,1);assert.equal(body.locked,false);result(d,await run.invoke(d.args));
 });
 test(`${d.profile} late transport completion and body retirement stay owned after cancellation`,async()=>{
  const entered=deferred(),headers=deferred(),cancelling=deferred(),release=deferred(),controller=new AbortController();let settled=false;
  const body=new ReadableStream({cancel(){cancelling.resolve();return release.promise}});
  const run=reader(d,async(_url,init)=>{entered.resolve(init.signal);await headers.promise;return new Response(body)});
  const pending=run.invoke(d.args,{signal:controller.signal}).finally(()=>settled=true),rejected=assert.rejects(pending,failure('cancelled'));
  const signal=await entered.promise;controller.abort();await tick();assert.equal(signal.aborted,true);assert.equal(settled,false);headers.resolve();await cancelling.promise;await tick();assert.equal(settled,false);release.resolve();await rejected;assert.equal(body.locked,false);
 });
 test(`${d.profile} refuses redirect, authentication, rate-limit and provider failures without retry or reflection`,async()=>{
  for(const status of [301,302,307,401,403,404,429,503]){let calls=0,cancelled=0;const body=new ReadableStream({start(c){c.enqueue(new TextEncoder().encode(KEY+' unselected-secret'))},cancel(){cancelled++}});
   await assert.rejects(reader(d,async()=>{calls++;return new Response(body,{status,headers:{location:'https://unselected.invalid','retry-after':'30'}})}).invoke(d.args),failure(status<400?'redirect_refused':'http_error'));assert.equal(calls,1);assert.equal(cancelled,1);assert.equal(body.locked,false);
  }
 });
 test(`${d.profile} refuses malformed/oversized/invalid UTF8 bodies and sanitizes transport failures`,async()=>{
  for(const body of [null,[],{},42])await assert.rejects(reader(d,async()=>Response.json(body)).invoke(d.args),failure('invalid_response'));
  await assert.rejects(reader(d,async()=>new Response(new Uint8Array([0xc3,0x28]))).invoke(d.args),failure('invalid_response'));
  await assert.rejects(reader(d,async()=>new Response('x'.repeat(1025)),KEY,{maxResponseBytes:1024}).invoke(d.args),failure('response_too_large'));
  await assert.rejects(reader(d,async()=>{throw Error(KEY+' unselected-secret')}).invoke(d.args),failure('transport_error'));
 });
 test(`${d.profile} rejects pre-abort and credential-bearing inputs before donor or transport entry`,async()=>{
  let calls=0;const run=reader(d,async()=>{calls++;return response(d)}),controller=new AbortController();controller.abort();
  await assert.rejects(run.invoke(d.args,{signal:controller.signal}),failure('cancelled',false,false));
  for(const args of [{...d.args,token:KEY},{...d.args,method:'DELETE'},{...d.args,body:{}}])await assert.rejects(run.invoke(args),failure('invalid_arguments',false,false));assert.equal(calls,0);
 });
}
const dFor=p=>profiles.find(d=>d.profile.startsWith(p+'.'));
const call=(d,args=d.args,body=d.body,check=()=>{})=>reader(d,async(url,init)=>{check(url,init);return typeof body==='string'?new Response(body):Response.json(body)}).invoke(args);
const invalidArguments=async(d,args)=>{let calls=0;await assert.rejects(reader(d,async()=>{calls++;return response(d)}).invoke(args),failure('invalid_arguments',false,false));assert.equal(calls,0);};
test('Ghost strict key/origin/ID, exact post identity and five-minute real helper JWT',async()=>{
 const d=dFor('ghost');for(const key of ['x','a'.repeat(24)+':','a'.repeat(24)+':'+'g'.repeat(64),ghostKey(KEY)+'\n'])assert.throws(()=>createReader({profile:d.profile,auth:{...d.auth(KEY),adminApiKey:key},transport:async()=>{}}));
 for(const origin of ['http://selected.invalid','https://selected.invalid/path','https://user@selected.invalid'])assert.throws(()=>createReader({profile:d.profile,auth:{...d.auth(KEY),origin},transport:async()=>{}}));
 for(const post_id of ['',null,'abc','a'.repeat(24)+'\n','../'+'a'.repeat(24)])await invalidArguments(d,{post_id});
 for(const body of [{posts:[]},{posts:[{id:'another'}]},{posts:[...d.body.posts,...d.body.posts]}])await assert.rejects(call(d,d.args,body),failure('invalid_response'));
});
test('Monday preserves pinned GraphQL, literal unique IDs and response identity while refusing mutation/query inputs',async()=>{
 const source=await readFile(new URL('../vendor/activepieces/packages/pieces/community/monday/src/lib/actions/ai/items/get-items.ts',import.meta.url),'utf8');assert.equal(MONDAY_QUERY,/query: `([\s\S]*?)`,/.exec(source)[1]);
 const d=dFor('monday'),ids=Array.from({length:100},(_,i)=>String(i+1));assert.deepEqual(await call(d,{item_ids:ids},{data:{items:[]}},(_url,init)=>assert.deepEqual(JSON.parse(init.body),{query:MONDAY_QUERY,variables:{ids}})),{items:[],count:0});
 for(const args of [{item_ids:[]},{item_ids:['42','42']},{item_ids:['0']},{item_ids:['42\n']},{item_ids:[42]},{item_ids:[...ids,'101']},{item_ids:['42'],query:'mutation{}'}])await invalidArguments(d,args);
 for(const item of [{...d.body.data.items[0],id:'43'},{...d.body.data.items[0],column_values:null}])await assert.rejects(call(d,d.args,{data:{items:[item]}}),failure('invalid_response'));
 await assert.rejects(call(d,d.args,{data:{items:[...d.body.data.items,...d.body.data.items]}}),failure('invalid_response'));
 const messages=[];const warn=console.warn;console.warn=(...v)=>messages.push(v);
 try{await call(d,d.args,{...d.body,extensions:{warnings:[{message:KEY}]}});await assert.rejects(call(d,d.args,{errors:[{message:KEY}],data:{items:[]}}),failure('upstream_error'));}finally{console.warn=warn;}assert.deepEqual(messages,[]);
});
test('Mailchimp selected data center avoids discovery and fields string mismatch; missing metrics stay unknown',async()=>{
 const d=dFor('mailchimp');for(const args of [{...d.args,fields:'opens.unique_opens'},{...d.args,exclude_fields:'timeseries'},{campaign_id:'../42'},{campaign_id:'id\n'}])await invalidArguments(d,args);
 const missing=await call(d,d.args,{id:'campaign_42',emails_sent:0});assert.deepEqual(missing.summary,{emails_sent:0,unique_opens:null,unique_clicks:null,open_rate:null,click_rate:null,bounce_rate:null,unsub_rate:null,abuse_rate:null});assert.equal(missing.performance_metrics.bounces.total,null);
 const zeros=await call(d,d.args,{...d.body,emails_sent:0,bounces:{hard_bounces:0,soft_bounces:0,syntax_errors:0}});assert.equal(zeros.summary.bounce_rate,null);assert.equal(zeros.summary.unsub_rate,null);assert.equal(zeros.performance_metrics.bounces.total,0);assert.equal(zeros.summary.unique_clicks,0);
 const partial=await call(d,d.args,{...d.body,bounces:{hard_bounces:2},ab_split:{a:{opens:0}}});assert.equal(partial.summary.bounce_rate,null);assert.equal(partial.performance_metrics.bounces.total,null);assert.equal(partial.ab_split_data.group_a.opens,0);assert.equal(partial.ab_split_data.group_a.bounces,null);
 for(const body of [{...d.body,id:'other'},{id:'campaign_42'},{...d.body,emails_sent:-1},{...d.body,opens:{unique_opens:'20'}},{...d.body,clicks:[]},{...d.body,bounces:{hard_bounces:-1}}])await assert.rejects(call(d,d.args,body),failure('invalid_response'));
});
test('S3 real XML parser, page limits, zero-byte objects and continuation metadata retain honest page scope',async()=>{
 const d=dFor('amazon-s3');result(d,await call(d));const page=await call(d,d.args,xml({truncated:true,token:'opaque+cursor/=='}));assert.equal(page.isTruncated,true);assert.equal(page.nextContinuationToken,'opaque+cursor/==');
 assert.deepEqual(await call(d,d.args,xml({count:0,contents:''})),{files:[],isTruncated:false,nextContinuationToken:null});
 for(const args of [{},{prefix:null,maxKeys:null},{prefix:'',maxKeys:100}])await call(d,args,xml({count:0,contents:''}),url=>{const q=new URL(url).searchParams;assert.equal(q.get('max-keys'),'100');assert.equal(q.get('prefix'),null)});
 const prefix='a å +?%/#💡';await call(d,{prefix,maxKeys:1},xml({count:0,contents:''}),url=>assert.equal(new URL(url).searchParams.get('prefix'),prefix));
 for(const args of [{maxKeys:0},{maxKeys:1001},{maxKeys:1.1},{maxKeys:'10'},{prefix:'x\n'},{prefix:'\ud800'},{prefix:'💡'.repeat(257)},{continuation_token:'opaque'}])await invalidArguments(d,args);
 for(const body of [xml({count:2}),xml({bucket:'another-bucket'}),xml({truncated:true}),xml({token:'unexpected'}),xml().replace('<Size>0</Size>',''),xml().replace('<Size>0</Size>','<Size>-1</Size>'),xml().replace('reports/a.txt','other/a.txt'),xml().replace('2026-10-01T10:00:00.000Z','bad-date'),xml().replace('<IsTruncated>false</IsTruncated>',''),xml().replace('<KeyCount>1</KeyCount>',''),'<ListBucketResult>', '<!DOCTYPE root [<!ENTITY x "value">]>'+xml()])await assert.rejects(call(d,d.args,body),failure('invalid_response'));
 const noSession={...d,auth:key=>{const auth=d.auth(key);delete auth.sessionToken;return auth}};await call(noSession,d.args,d.body,(_url,init)=>assert.equal(init.headers.get('x-amz-security-token'),null));
});
test('SDK credentials stay host configuration; describe is inert and S3 resolves only explicit environment names',()=>{
 for(const d of profiles){const config={profile:d.profile,token_env:'SELECTED_TOKEN',...d.config};assert.deepEqual(parseConfiguration(config),config);assert.equal(describeConfiguration(config).effect,'read');
  const env={SELECTED_TOKEN:d.token(KEY),...d.environment,AMBIENT_TOKEN:'unselected-secret'},selected=selectedEnvironment(config,env);assert.deepEqual(readerConfiguration(config,selected.SELECTED_TOKEN,async()=>{},selected).auth,d.auth(KEY));assert.equal(selected.AMBIENT_TOKEN,undefined);
  for(const field of ['accessToken','apiKey','secretAccessKey','sessionToken','endpoint'])assert.throws(()=>parseConfiguration({...config,[field]:KEY}));
  for(const name of Object.keys(selected)){const incomplete={...env};delete incomplete[name];assert.throws(()=>selectedEnvironment(config,incomplete));}
 }
 const d=dFor('amazon-s3'),config={profile:d.profile,token_env:'SECRET',...d.config};for(const value of [null,'','BAD\n','lower'])for(const name of ['access_key_id_env','session_token_env'])assert.throws(()=>parseConfiguration({...config,[name]:value}));
});
test('each selected SDK request is constrained by exact route, read method, auth, body and query',async()=>{
 for(const d of profiles){
  const selection=select({profile:d.profile,auth:d.auth(KEY),transport:async()=>response(d)}),p=inputs(selection,d.args);let request;
  await createDonorBundle(async value=>{request=structuredClone(value);return {status:200,headers:{},body:d.body}},{mailchimpDataCenter:'us1',invalidResponse(){throw Error('Invalid fixture')},s3Page(){}})[d.export].run({auth:selection.auth,propsValue:p.props});
  const mutations=[{method:'DELETE'},{method:d.method==='GET'?'POST':'GET'},{url:'https://unselected.invalid/read'},{url:request.url+'/other'},{body:{mutation:true}},{queryParams:{unselected:'true'}},{headers:{authorization:'Bearer wrong'}}];
  if(d.profile==='amazon-s3.list-files')mutations.push({headers:{...request.headers,authorization:request.headers.authorization.replace(/Signature=([a-f0-9])/,(_m,c)=>'Signature='+(c==='0'?'1':'0'))}},{headers:{...request.headers,'x-amz-security-token':'another-token'}});
  if(d.profile==='ghost.get-post')mutations.push({headers:{Authorization:request.headers.Authorization.slice(0,-8)+'aaaaaaaa'}});
  for(const mutation of mutations){let calls=0;const own=new Invocation({...selection,transport:async()=>{calls++;return response(d)}},p.pathname,undefined,p.query,p.inlineQuery,p.read);own.entered=true;
   try{await assert.rejects(own.request({...request,...mutation}),failure('upstream_error',true,false));}finally{await own.finish();}assert.equal(calls,0);
  }
 }
});
test('SDK bundles use only supplied HTTP and never ambient network, subprocess or provider credential chains',async()=>{
 const original={readFile:fs.readFile,readFileSync:fs.readFileSync,promisesReadFile:fs.promises.readFile,fetch:globalThis.fetch,httpRequest:http.request,httpGet:http.get,httpsRequest:https.request,httpsGet:https.get,spawn:childProcess.spawn,exec:childProcess.exec};
 const attempted=[];const forbidden=()=>{attempted.push('ambient I/O');throw Error('Ambient I/O was attempted')};const envNames=['AWS_ACCESS_KEY_ID','AWS_SECRET_ACCESS_KEY','AWS_SESSION_TOKEN','AWS_PROFILE','AWS_CONFIG_FILE','AWS_SHARED_CREDENTIALS_FILE','AWS_CONTAINER_CREDENTIALS_FULL_URI','AWS_SDK_UA_APP_ID','AWS_AUTH_SCHEME_PREFERENCE','AWS_SIGV4A_SIGNING_REGION_SET','AWS_EXECUTION_ENV','HTTP_PROXY','HTTPS_PROXY'];const old=Object.fromEntries(envNames.map(k=>[k,process.env[k]]));
 try{fs.readFile=fs.readFileSync=fs.promises.readFile=forbidden;globalThis.fetch=forbidden;http.request=http.get=https.request=https.get=childProcess.spawn=childProcess.exec=forbidden;for(const k of envNames)process.env[k]='unselected-secret';for(const d of profiles)result(d,await reader(d,async(_url,init)=>{assert.ok(!JSON.stringify(Object.fromEntries(init.headers)).includes('unselected-secret'));return response(d)}).invoke(d.args));assert.deepEqual(attempted,[]);}
 finally{fs.readFile=original.readFile;fs.readFileSync=original.readFileSync;fs.promises.readFile=original.promisesReadFile;globalThis.fetch=original.fetch;http.request=original.httpRequest;http.get=original.httpGet;https.request=original.httpsRequest;https.get=original.httpsGet;childProcess.spawn=original.spawn;childProcess.exec=original.exec;for(const [k,v]of Object.entries(old))if(v===undefined)delete process.env[k];else process.env[k]=v;}
});
