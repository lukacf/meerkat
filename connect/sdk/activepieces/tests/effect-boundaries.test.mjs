import test from 'node:test';
import assert from 'node:assert/strict';
import {createReader,createWriter,ActivepiecesError} from '../dist/index.js';
import {cases,issue} from './effects-fixtures.mjs';
const json=(value,status=200)=>new Response(JSON.stringify(value),{status,headers:{'content-type':'application/json'}});
const make=(item,transport,extra={})=>(item.body?createWriter:createReader)({profile:item.profile,auth:item.auth,transport,...extra});
const turn=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
const failure=(kind,outcome)=>e=>e instanceof ActivepiecesError&&e.kind===kind&&e.writeOutcome===outcome&&!String(e).includes('synthetic-secret');
for(const item of cases){
 const outcome=item.body?'uncertain':'not_applicable';
 test(`${item.profile}: pre-entry cancellation has no request`,async()=>{let calls=0;const abort=new AbortController();abort.abort();await assert.rejects(make(item,async()=>{calls++;return json(item.response,item.status);}).invoke(item.args,{signal:abort.signal}),failure('cancelled',item.body?'not_attempted':'not_applicable'));assert.equal(calls,0);});
 test(`${item.profile}: cancellation after transport entry joins late response`,async()=>{
  const entered=deferred(),release=deferred(),abort=new AbortController();let done=false,calls=0,cancelled=false;
  const pending=make(item,async()=>{calls++;entered.resolve();await release.promise;return new Response(new ReadableStream({cancel(){cancelled=true;}}));}).invoke(item.args,{signal:abort.signal}).finally(()=>{done=true;});
  const rejected=assert.rejects(pending,failure('cancelled',outcome));await entered.promise;abort.abort();await turn();assert.equal(done,false);release.resolve();await rejected;assert.equal(calls,1);assert.equal(cancelled,true);
 });
 test(`${item.profile}: body cancellation cleanup must finish`,async()=>{
  const entered=deferred(),release=deferred(),abort=new AbortController();let done=false,joined=false;
  const pending=make(item,async()=>new Response(new ReadableStream({start(){entered.resolve();},async cancel(){await release.promise;joined=true;}}))).invoke(item.args,{signal:abort.signal}).finally(()=>{done=true;});
  const rejected=assert.rejects(pending,failure('cancelled',outcome));await entered.promise;await turn();abort.abort();await turn();assert.equal(done,false);release.resolve();await rejected;assert.equal(joined,true);
 });
 test(`${item.profile}: cleanup rejection is owned`,async()=>{
  const pending=make(item,async()=>new Response(new ReadableStream({start(c){c.enqueue(new TextEncoder().encode('{'));},cancel(){throw Error('synthetic-secret');}})),{maxResponseBytes:1,timeoutMs:500}).invoke(item.args);
  await assert.rejects(pending,e=>e instanceof ActivepiecesError&&e.writeOutcome===outcome);
 });
 test(`${item.profile}: malformed or oversized result remains safe`,async()=>{
  for(const response of [()=>new Response('synthetic-secret'),()=>new Response('x'.repeat(5000))])await assert.rejects(make(item,async()=>response(),{maxResponseBytes:4096}).invoke(item.args),e=>e instanceof ActivepiecesError&&e.writeOutcome===outcome&&!String(e).includes('synthetic-secret'));
 });
 test(`${item.profile}: two explicit accounts can overlap without credential mixing`,async()=>{
  const entered=deferred(),release=deferred();let count=0;const seen=[];
  const calls=['synthetic-a','synthetic-b'].map(key=>{
   const auth={...item.auth,...('apiKey'in item.auth?{apiKey:key}:{accessToken:key})};
   const current={...item,auth};return make(current,async(url,init)=>{seen.push({url,headers:Object.fromEntries(init.headers)});if(++count===2)entered.resolve();await release.promise;return json(item.response,item.status);}).invoke(item.args);
  });
  await entered.promise;assert.equal(count,2);release.resolve();await Promise.all(calls);
  for(let i=0;i<2;i++){const encoded=JSON.stringify(seen[i].headers);const key=i?'synthetic-b':'synthetic-a';assert.ok(encoded.includes(key)||encoded.includes(Buffer.from(`${key}:X`).toString('base64')));assert.equal(encoded.includes(i?'synthetic-a':'synthetic-b'),false);}
 });
 test(`${item.profile}: 401, 429 and redirect refuse replay`,async()=>{
  for(const status of [401,429,302]){let calls=0;await assert.rejects(make(item,async()=>{calls++;return new Response('synthetic-secret',{status,headers:{'retry-after':'3600',location:'https://other.invalid'}});}).invoke(item.args),e=>e instanceof ActivepiecesError&&e.writeOutcome===outcome&&e.kind===(status===302?'redirect_refused':'http_error'));assert.equal(calls,1);}
 });
 if(item.body)test(`${item.profile}: later JSON size conversion keeps uncertain effect`,async()=>{let calls=0;await assert.rejects(make(item,async()=>{calls++;return json(item.response,item.status);},{maxResponseBytes:Buffer.byteLength(JSON.stringify(item.response))+5}).invoke(item.args),e=>e instanceof ActivepiecesError&&e.dispatched&&e.writeOutcome==='uncertain');assert.equal(calls,1);});
}
test('Gmail schema forbids raw and validates recursive message identity',async()=>{
 const item=cases.find(x=>x.profile==='gmail.get-thread');let calls=0;
 await assert.rejects(make(item,async()=>{calls++;return json(item.response);}).invoke({...item.args,format:'raw'}),{kind:'invalid_arguments'});assert.equal(calls,0);
 for(const body of [{id:'other',messages:[]},{id:'abc123',messages:[{id:'x',threadId:'other'}]},{id:'abc123',messages:[{id:'x',threadId:'abc123',payload:{parts:[{body:{size:-1}}]}}]},{id:'abc123',messages:[{id:'x',threadId:'abc123'},{id:'x',threadId:'abc123'}]},{id:'abc123'}])await assert.rejects(make(item,async()=>json(body)).invoke(item.args),{kind:'invalid_response'});
 assert.deepEqual(await make(item,async()=>json({id:'abc123',messages:[]})).invoke(item.args),{id:'abc123',messages:[]});
});
test('Slack real helper deduplicates pages and refuses cursor cycles',async()=>{
 const item=cases.find(x=>x.profile==='slack.get-thread-replies');let calls=0;
 const result=await make(item,async(_url,init)=>{const form=new URLSearchParams(init.body);calls++;assert.equal(form.get('cursor'),calls===1?null:'cursor-two');return json(calls===1?{ok:true,messages:[{ts:item.args.threadTs}],response_metadata:{next_cursor:'cursor-two'}}:{ok:true,messages:[{ts:item.args.threadTs},{ts:'1710304379.000000',thread_ts:item.args.threadTs}],response_metadata:{next_cursor:''}});}).invoke(item.args);
 assert.equal(calls,2);assert.equal(result.messages.length,2);assert.equal(result.has_more,false);
 calls=0;await assert.rejects(make(item,async()=>{calls++;return json({ok:true,messages:[{ts:item.args.threadTs}],response_metadata:{next_cursor:'repeated'}});}).invoke(item.args),{kind:'invalid_response'});assert.equal(calls,2);
 calls=0;await assert.rejects(make(item,async()=>{calls++;return json({ok:true,messages:[],response_metadata:{next_cursor:'start'}});}).invoke({...item.args,cursor:'start'}),{kind:'invalid_response'});assert.equal(calls,1);
});
test('Slack maximum 50 pages preserves a usable continuation',async()=>{
 const item=cases.find(x=>x.profile==='slack.get-thread-replies');let calls=0;
 const result=await make(item,async()=>json({ok:true,messages:[{ts:calls===0?item.args.threadTs:`1710304379.${String(calls).padStart(6,'0')}`}],response_metadata:{next_cursor:`cursor-${++calls}`}})).invoke(item.args);
 assert.equal(calls,50);assert.equal(result.has_more,true);assert.equal(result.response_metadata.next_cursor,'cursor-50');
});
test('Linear real mapper and label page filter preserve completeness and reject swallowed faults',async()=>{
 const item=cases.find(x=>x.profile==='linear.get-issue');let calls=0;
 const labelPage=(nodes,hasNextPage)=>({data:{issue:{labels:{nodes,pageInfo:{hasNextPage,endCursor:null}}}}});
 const first=()=>({data:{issue:{...issue,labels:{nodes:[{id:'a',name:'A'}],pageInfo:{hasNextPage:true,endCursor:'one'}}}}});
 const result=await make(item,async(_url,init)=>{
  calls++;const body=JSON.parse(init.body);
  if(calls===1)return json(first());
  assert.equal(body.variables.id,issue.id);assert.equal(body.variables.first,250);
  if(calls===2){assert.equal(body.variables.filter,undefined);return json(labelPage([{id:'a',name:'A'}],true));}
  assert.deepEqual(body.variables.filter,{id:{nin:['a']}});return json(labelPage([{id:'b',name:'B'}],false));
 }).invoke(item.args);
 assert.equal(calls,3);assert.deepEqual(result.label_ids,['a','b']);assert.equal(result.labels_complete,true);
 for(const variant of ['graphql','malformed','too-large']){
  calls=0;await assert.rejects(make(item,async()=>{
   calls++;if(calls===1)return json(first());
   if(variant==='graphql')return json({...labelPage([],false),errors:[{message:'synthetic-secret'}]});
   if(variant==='too-large')return json(labelPage(Array.from({length:251},(_,i)=>({id:String(i),name:'a'})),false));
   return json({data:{issue:{labels:{nodes:[]}}}});
  }).invoke(item.args),e=>e instanceof ActivepiecesError&&!String(e).includes('synthetic-secret'));assert.equal(calls,2);
 }
});
test('GCS one page has honest counts, fixed bucket and supported option constraints',async()=>{
 const item=cases.find(x=>x.profile==='google-cloud-storage.search-objects');
 const result=await make(item,async(url)=>{assert.equal(new URL(url).searchParams.get('delimiter'),'/');return json({prefixes:['folder/']});}).invoke({delimiter:'/',includeFoldersAsPrefixes:true,versions:false});assert.equal(result.totalItems,0);assert.deepEqual(result.items,[]);assert.equal(result.pageOnly,true);assert.equal('nextPageToken'in result,false);
 for(const args of [{bucket:'other'},{includeFoldersAsPrefixes:true},{delimiter:'x'},{maxResults:1001},{matchGlob:''}])await assert.rejects(make(item,async()=>{throw Error('must not dispatch');}).invoke(args),{kind:'invalid_arguments'});
 for(const body of [{items:null},{prefixes:null},{items:[{bucket:'other',name:'x'}]},{nextPageToken:42}])await assert.rejects(make(item,async()=>json(body)).invoke({}),{kind:'invalid_response'});
});
test('Writes reject ambiguous overrides and bad enum values before dispatch',async()=>{
 for(const [profile,args]of [['datadog.submit-log',{message:'x',additionalProperties:{message:'overwrite'}}],['freshservice.create-ticket',{subject:'x',description:'y',requester_id:1,priority:0}],['gitea.create-issue',{title:'x',repository:{owner:'other',repo:'other'}}],['activecampaign.create-contact',{email:'a@example.test',contactCustomFields:{__bad:'x'}}]]){
  const item=cases.find(x=>x.profile===profile);let calls=0;await assert.rejects(make(item,async()=>{calls++;return json(item.response,item.status);}).invoke(args),failure('invalid_arguments','not_attempted'));assert.equal(calls,0);
 }
});
for(const item of cases.filter(x=>x.body))test(`${item.profile}: deadline after entry waits and keeps uncertainty`,async()=>{
 const entered=deferred(),expired=deferred(),release=deferred();let done=false,calls=0;
 const pending=make(item,async(_url,init)=>{calls++;init.signal.addEventListener('abort',()=>expired.resolve(),{once:true});entered.resolve();await release.promise;return json(item.response,item.status);},{timeoutMs:1000}).invoke(item.args).finally(()=>done=true);
 const rejected=assert.rejects(pending,failure('deadline_exceeded','uncertain'));await entered.promise;await expired.promise;assert.equal(done,false);release.resolve();await rejected;assert.equal(calls,1);
});
test('Writer configuration failures carry pre-dispatch effect metadata',()=>{
 for(const item of cases.filter(x=>x.body))assert.throws(()=>createWriter({profile:item.profile,auth:{...item.auth,apiKey:'',accessToken:''},transport:async()=>{throw 0}}),failure('invalid_configuration','not_attempted'));
});
test('Host-selected origins, Datadog sites and Gitea repositories remain isolated during overlap',async()=>{
 const changes={
  'activecampaign.create-contact':[{origin:'https://one.example'},{origin:'https://two.example'}],
  'freshservice.create-ticket':[{domain:'one'},{domain:'two'}],
  'gitea.create-issue':[{origin:'https://one.example',owner:'one',repository:'repo-one'},{origin:'https://two.example',owner:'two',repository:'repo-two'}],
  'datadog.submit-log':[{site:'datadoghq.eu'},{site:'us3.datadoghq.com'}],
 };
 for(const item of cases.filter(x=>x.body)){
  const release=deferred(),entered=deferred();let count=0;const urls=[];
  const pending=changes[item.profile].map(change=>make({...item,auth:{...item.auth,...change}},async url=>{urls.push(url);if(++count===2)entered.resolve();await release.promise;return json(item.response,item.status);}).invoke(item.args));
  await entered.promise;assert.notEqual(urls[0],urls[1]);
  for(let i=0;i<2;i++){const c=changes[item.profile][i];const url=new URL(urls[i]);assert.equal(url.origin,c.origin??(c.domain?`https://${c.domain}.freshservice.com`:`https://http-intake.logs.${c.site}`));if(c.repository)assert.equal(url.pathname,`/api/v1/repos/${c.owner}/${c.repository}/issues`);}
  release.resolve();await Promise.all(pending);
 }
});
test('Linear label helper cannot swallow cancellation after a successful issue request',async()=>{
 const item=cases.find(x=>x.profile==='linear.get-issue'),entered=deferred(),release=deferred(),abort=new AbortController();let calls=0,done=false;
 const pending=make(item,async()=>{if(++calls===1)return json({data:{issue:{...issue,labels:{nodes:[],pageInfo:{hasNextPage:true,endCursor:'a'}}}}});entered.resolve();await release.promise;return json({data:{issue:{labels:{nodes:[],pageInfo:{hasNextPage:false,endCursor:null}}}}});}).invoke(item.args,{signal:abort.signal}).finally(()=>done=true);
 const rejected=assert.rejects(pending,failure('cancelled','not_applicable'));await entered.promise;abort.abort();await turn();assert.equal(done,false);release.resolve();await rejected;assert.equal(calls,2);
});
test('Slack aggregate bytes include all pages and do not return partial success',async()=>{
 const item=cases.find(x=>x.profile==='slack.get-thread-replies');let calls=0;
 await assert.rejects(make(item,async()=>json({ok:true,messages:[{ts:item.args.threadTs,text:'x'.repeat(900)}],response_metadata:{next_cursor:++calls===1?'next':''}}),{maxResponseBytes:1500}).invoke(item.args),{kind:'response_too_large'});assert.equal(calls,2);
});
test('GCS 200 error envelope is not an empty successful page',async()=>{
 const item=cases.find(x=>x.profile==='google-cloud-storage.search-objects');await assert.rejects(make(item,async()=>json({error:{message:'synthetic-secret'}})).invoke({}),{kind:'upstream_error'});
});
test('Linear successful label-page limit stays explicitly partial',async()=>{
 const item=cases.find(x=>x.profile==='linear.get-issue');let calls=0;
 const result=await make(item,async()=>{
  if(++calls===1)return json({data:{issue:{...issue,labels:{nodes:[{id:'known',name:'Known'}],pageInfo:{hasNextPage:true,endCursor:'a'}}}}});
  return json({data:{issue:{labels:{nodes:[{id:`page-${calls-1}`,name:`Label ${calls-1}`}],pageInfo:{hasNextPage:true,endCursor:`page-${calls-1}`}}}}});
 }).invoke(item.args);
 assert.equal(calls,21);assert.equal(result.labels_complete,false);assert.equal(result.label_ids.length,21);assert.ok(result.label_ids.includes('known'));
});
