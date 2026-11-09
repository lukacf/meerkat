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
import { createDonorBundle } from '../dist/donor-factory.js';
import { readExpansionProfiles as profiles, KEY } from './fixtures/read-expansion-profiles.mjs';
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
   calls++; assert.equal(url,d.url(KEY)); assert.equal(init.method,d.method); assert.equal(init.redirect,'error'); assert.ok(init.signal instanceof AbortSignal);
   assert.deepEqual(Object.fromEntries(init.headers),d.headers(KEY)); assert.equal(init.body,d.requestBody?JSON.stringify(d.requestBody):undefined); return response=Response.json(d.body);
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
 test(`${d.profile} rejects pre-abort and invalid or credential-bearing arguments before entry`,async()=>{
  let calls=0;const run=reader(d,async()=>{calls++;return Response.json(d.body)}),controller=new AbortController();controller.abort();
  await assert.rejects(run.invoke(d.args,{signal:controller.signal}),failure('cancelled',false,false));
  const key=Object.keys(d.args)[0];
  for(const args of [ ...(d.profile==='algolia.browse-records'?[]:[{}, {...d.args,[key]:null}]), {...d.args,token:KEY},{...d.args,[key]:''},{...d.args,[key]:'\ud800'},{...d.args,[key]:'x\n'}])await assert.rejects(run.invoke(args),failure('invalid_arguments',false,false));
  assert.equal(calls,0);
 });
}

const selected = name => profiles.find(d => d.profile.startsWith(name+'.'));
const call = (d,args,body=d.body,check=()=>{}) => reader(d,async(url,init)=>{check(url,init);return Response.json(body)}).invoke(args);
const invalidArguments = async(d,args) => {
 let calls=0;await assert.rejects(reader(d,async()=>{calls++;return Response.json(d.body)}).invoke(args),failure('invalid_arguments',false,false));assert.equal(calls,0);
};
test('new reader configurations select identity explicitly and never accept secrets as public configuration',()=>{
 for(const d of profiles){
  const value={profile:d.profile,token_env:'SELECTED_TOKEN',...d.config};
  assert.deepEqual(parseConfiguration(value),value);
  assert.deepEqual(readerConfiguration(value,KEY,async()=>{}).auth,d.auth(KEY));
  for(const key of ['token_env',...Object.keys(d.config??{})]){const copy={...value};delete copy[key];assert.throws(()=>parseConfiguration(copy));}
  for(const key of ['accessToken','apiKey','refreshToken','method','body'])assert.throws(()=>parseConfiguration({...value,[key]:KEY}));
  assert.throws(()=>parseConfiguration({...value,token_env:'SELECTED_TOKEN\n'}));
  if(d.config?.cloud)assert.throws(()=>parseConfiguration({...value,cloud:'unknown'}));
 }
});
test('read POST permission admits only the selected real donor route, body, query and authentication',async()=>{
 for(const d of profiles){
  const config={profile:d.profile,auth:d.auth(KEY),transport:async()=>{throw Error('Unexpected I/O')}};
  const selection=select(config),prepared=inputs(selection,d.args);let request;
  await createDonorBundle(async value=>{request=structuredClone(value);return {status:200,headers:{},body:d.body}})[d.export].run({auth:selection.auth,propsValue:prepared.props});
  const mutations=[{method:'DELETE'},{method:'PUT'},{method:d.method==='GET'?'POST':'GET'},{url:'https://unselected.invalid/read'},
   {url:request.url+'/batch'},{body:{mutation:true}},{queryParams:{unselected:'true'}},{authentication:{type:'BEARER_TOKEN',token:'wrong'}},{headers:{authorization:'Bearer wrong'}}];
  for(const mutation of mutations){
   let calls=0;const own=new Invocation({...selection,transport:async()=>{calls++;return Response.json(d.body)}},prepared.pathname,undefined,prepared.query,prepared.inlineQuery,prepared.read);own.entered=true;
   try{await assert.rejects(own.request({...request,...mutation}),failure('upstream_error',true,false));}finally{await own.finish();}assert.equal(calls,0);
  }
 }
});
test('Jira expansion is a bounded literal selection and returned issue identity is bound',async()=>{
 const d=selected('jira-cloud');
 for(const expand of [undefined,null,[],['names','changelog']])await call(d,{...d.args,...(expand===undefined?{}:{expand})},d.body,url=>assert.equal(new URL(url).searchParams.get('expand'),Array.isArray(expand)?expand.join(','):null));
 await call(d,{issueId:'42'},d.body);
 for(const args of [{issueId:'../42'},{issueId:'PROJECT-42\n'},{...d.args,expand:['names','names']},{...d.args,mapNames:true},{...d.args,mapTransitions:true}])await invalidArguments(d,args);
 for(const body of [{...d.body,key:'OTHER-42'},{...d.body,fields:null}])await assert.rejects(call(d,d.args,body),failure('invalid_response'));
 await assert.rejects(call(d,{issueId:'42'},{...d.body,id:'43'}),failure('invalid_response'));
});
test('Dropbox literal path and flags retain donor defaults and distinguish deleted entries and identity mismatches',async()=>{
 const d=selected('dropbox');
 for(const flag of [undefined,null,false,true]){
  const args={path:'/å #?% 💡',...(flag===undefined?{}:{include_deleted:flag,include_media_info:flag,include_has_explicit_shared_members:flag})};
  await call(d,args,d.body,(_url,init)=>assert.deepEqual(JSON.parse(init.body),{path:args.path,include_deleted:flag??false,include_media_info:flag??false,include_has_explicit_shared_members:flag??false}));
 }
 await call(d,{path:'id:folder_42'},{'.tag':'folder',id:'id:folder_42',name:'Folder'});
 await call(d,{path:'rev:abc123'},d.body);
 const deleted={'.tag':'deleted',name:'Removed'};
 await call(d,{path:'/Removed',include_deleted:true},deleted);
 await assert.rejects(call(d,{path:'/Removed'},deleted),failure('invalid_response'));
 await assert.rejects(call(d,{path:'id:other'},d.body),failure('invalid_response'));
 await assert.rejects(call(d,{path:'rev:abc124'},d.body),failure('invalid_response'));
 for(const path of ['relative','',null,'id:','rev:xyz'])await invalidArguments(d,{path});
 await invalidArguments(d,{...d.args,include_deleted:'true'});
});
test('Sheets quotes literal tab titles, retains falsy/ragged values and binds returned range geometry',async()=>{
 const d=selected('google-sheets');
 for(const title of ["'Quoted'",'Sales!A1','A1',' spaced ','å 💡']){
  const range="'"+title.replace(/'/g,"''")+"'!B2:D4",body={...d.body,range};
  assert.deepEqual(await call(d,{...d.args,worksheet_title:title},body,url=>assert.equal(decodeURIComponent(new URL(url).pathname.split('/').at(-1)),range)),body);
 }
 const absent={range:d.body.range,majorDimension:'ROWS'};assert.deepEqual(await call(d,d.args,absent),{...absent,values:[]});
 const columns={...d.body,majorDimension:'COLUMNS',values:[[0,false,''],['row']]};
 assert.deepEqual(await call(d,{...d.args,major_dimension:'COLUMNS',value_render_option:'UNFORMATTED_VALUE'},columns,url=>assert.equal(new URL(url).searchParams.get('valueRenderOption'),'UNFORMATTED_VALUE')),columns);
 for(const body of [{...d.body,range:'Other!B2:D4'},{...d.body,range:"'Q1''s ! Summary'!A2:C4"},{...d.body,values:null},{...d.body,values:[[null]]},{...d.body,values:[{}]}])await assert.rejects(call(d,d.args,body),failure('invalid_response'));
 for(const values of [[[],[],[],[]],[[1,2,3,4]]])await assert.rejects(call(d,d.args,{...d.body,values}),failure('response_too_large'));
});
test('Sheets requires finite ordered A1 geometry and omits only genuinely optional enums',async()=>{
 const d=selected('google-sheets');
 for(const range of [undefined,null,'',' ','A:A','1:5','A1:A1001','A1:CW1','A1:CV101','B2:A3','A3:B2','Other!A1','A0','A1000001','AAAA1','A1\n'])await invalidArguments(d,{...d.args,range});
 for(const name of ['major_dimension','value_render_option'])for(const value of [null,'',false,'unexpected'])await invalidArguments(d,{...d.args,[name]:value});
 await call(d,{...d.args,range:'ZZZ1000000'},{range:"'Q1''s ! Summary'!ZZZ1000000",majorDimension:'ROWS',values:[]});
});
test('Excel encodes selected SharePoint identifiers and refuses continuation instead of claiming complete columns',async()=>{
 const d=selected('microsoft-excel-365'),args={...d.args,storageSource:'sharepoint',siteId:'host,site-guid,web-guid',documentId:'library/#?%',worksheetId:'Sheet ! A'};
 const gov={...d,auth:key=>({...d.auth(key),cloud:'government'})};
 await call(gov,args,d.body,url=>assert.equal(url,'https://graph.microsoft.us/v1.0/sites/host%2Csite-guid%2Cweb-guid/drives/library%2F%23%3F%25/items/workbook_42/workbook/worksheets/Sheet%20!%20A/tables/Budget%20table/columns?%24top=100'));
 for(const next of [undefined,null,''])assert.deepEqual(await call(d,d.args,{value:[],...(next===undefined?{}:{'@odata.nextLink':next})}),[]);
 for(const next of ['https://graph.microsoft.com/next',false,{},[]])await assert.rejects(call(d,d.args,{...d.body,'@odata.nextLink':next}),failure('invalid_response'));
 await assert.rejects(call(d,{...d.args,limit:1},d.body),failure('response_too_large'));
 for(const args of [{...d.args,storageSource:'other'},{...d.args,storageSource:'sharepoint'},{...d.args,siteId:'not-used'},...[null,0,101].map(limit=>({...d.args,limit}))])await invalidArguments(d,args);
});
test('Teams optional reply semantics preserve exact returned identity and 429 cannot replay',async()=>{
 const d=selected('microsoft-teams');
 for(const replyId of [undefined,null,''])await call(d,{...d.args,...(replyId===undefined?{}:{replyId})},d.body,url=>assert.equal(url,d.url()));
 const replyId='reply/#?% å';await call(d,{...d.args,replyId},{id:replyId},url=>assert.equal(url,d.url()+'/replies/'+encodeURIComponent(replyId)));
 await assert.rejects(call(d,d.args,{id:'another-message'}),failure('invalid_response'));
 let calls=0;const start=performance.now();
 await assert.rejects(reader(d,async()=>{calls++;return Response.json({error:{message:KEY}},{status:429,headers:{'Retry-After':'30'}})}).invoke(d.args),failure('http_error'));
 assert.equal(calls,1);assert.ok(performance.now()-start<900,'zero retry should avoid the donor minimum one-second backoff');
});
test('QuickBooks literal invoice lookup preserves its escape and makes only an explicit empty result a miss',async()=>{
 const d=selected('quickbooks');await call(d,d.args,d.body,url=>assert.equal(new URL(url).searchParams.get('query'),"SELECT * FROM Invoice WHERE DocNumber = 'INV\\' 42' MAXRESULTS 1"));
 for(const QueryResponse of [{},{Invoice:[]},{maxResults:0,totalCount:0}])assert.deepEqual(await call(d,d.args,{QueryResponse}),{found:false,result:{}});
 for(const body of [{Fault:{}},{Fault:null,...d.body},{QueryResponse:null},{QueryResponse:{Invoice:null}},{QueryResponse:{Invoice:{}}},{QueryResponse:{unknown:[]}},{QueryResponse:{maxResults:1}},{QueryResponse:{Invoice:[{Id:'42',DocNumber:'wrong'}]}},{QueryResponse:{Invoice:[...d.body.QueryResponse.Invoice,...d.body.QueryResponse.Invoice]}}])await assert.rejects(call(d,d.args,body),failure('invalid_response'));
 for(const invoice_number of ['INV\\42',"INV\\' OR Id > '0"])await invalidArguments(d,{invoice_number});
});
test('QuickBooks optional result counts agree with empty and populated first-match results',async()=>{
 const d=selected('quickbooks'),Invoice=d.body.QueryResponse.Invoice;
 const empty=[{}, {Invoice:[]}, {maxResults:0}, {totalCount:0}, {Invoice:[],maxResults:0,totalCount:0}];
 for(const QueryResponse of empty)assert.deepEqual(await call(d,d.args,{QueryResponse}),{found:false,result:{}});
 for(const counts of [{},{maxResults:1},{totalCount:1},{maxResults:1,totalCount:1},{maxResults:1,totalCount:2}])assert.deepEqual(await call(d,d.args,{QueryResponse:{Invoice,...counts}}),d.result);
 const contradictions=[{maxResults:1},{totalCount:1},{Invoice:[],maxResults:1},{Invoice:[],totalCount:1},{Invoice:[],maxResults:1,totalCount:1},
  {Invoice,maxResults:0},{Invoice,totalCount:0},{Invoice,maxResults:1,totalCount:0},{Invoice,maxResults:0,totalCount:1},{Invoice,maxResults:2,totalCount:2}];
 for(const QueryResponse of contradictions)await assert.rejects(call(d,d.args,{QueryResponse}),failure('invalid_response'));
});
test('Xero exposes only tenant-scoped SearchTerm with literal characters and selected page/type/defaults',async()=>{
 const d=selected('xero'),value='INV \\" &where=Type=="ACCPAY"',args={value,type_filter:'ACCREC',summary_only:null,page:2};
 await call(d,args,{Invoices:[{InvoiceID:d.body.Invoices[0].InvoiceID,Type:'ACCREC'}]},(url,init)=>{
  const q=new URL(url).searchParams;assert.equal(q.get('SearchTerm'),value);assert.equal(q.get('where'),'Type=="ACCREC"');assert.equal(q.get('summaryOnly'),null);assert.equal(q.get('page'),'2');assert.equal(init.headers.get('xero-tenant-id'),d.config.tenant_id);
 });
 await call(d,{value,type_filter:'',summary_only:false,page:null},{Invoices:[]},url=>{assert.equal(new URL(url).searchParams.get('page'),'1');assert.equal(new URL(url).searchParams.get('where'),null)});
 for(const body of [{ErrorNumber:0,...d.body},{Status:'ERROR',...d.body},{Invoices:null},{Invoices:[{}]},{Invoices:[{InvoiceID:d.body.Invoices[0].InvoiceID,Type:'ACCPAY'}]}])await assert.rejects(call(d,args,body),failure('invalid_response'));
 for(const args of [{...d.args,search_by:'ID'},{...d.args,tenant_id:d.config.tenant_id},{...d.args,page:0},{...d.args,page:10001},{...d.args,type_filter:'OTHER'},{value:'   '}])await invalidArguments(d,args);
});
test('Algolia executes the real cursor loop with opaque cursors and slices to selected limit',async()=>{
 const d=selected('algolia'),bodies=[],cursor='opaque #?% å \\"';let calls=0;
 const result=await reader(d,async(_url,init)=>{bodies.push(JSON.parse(init.body));return Response.json(++calls===1?{hits:[{objectID:'1'}],cursor}:{hits:[{objectID:'2'},{objectID:'3'}],cursor:'unused-after-limit'})}).invoke({limit:2});
 assert.deepEqual(bodies,[{},{cursor}]);assert.deepEqual(result,[{objectID:'1'},{objectID:'2'}]);
 for(const args of [{},{limit:null}])assert.deepEqual(await call(d,args,{hits:[]}),[]);
 for(const limit of [0,-1,0.5,'2',1001])await invalidArguments(d,{limit});
 for(const body of [{hits:null},{hits:[{}]},{hits:[{objectID:''}]},{hits:[],cursor:{}},{hits:[],cursor:'bad\n'},{hits:[],cursor:'\ud800'}])await assert.rejects(call(d,{limit:2},body),failure('invalid_response'));
});
test('Algolia refuses cursor cycles and request/page caps without a successful partial result',async()=>{
 const d=selected('algolia');let calls=0;
 await assert.rejects(reader(d,async()=>{calls++;return Response.json({hits:[],cursor:'same'})}).invoke({limit:1}),failure('invalid_response'));assert.equal(calls,2);
 calls=0;await assert.rejects(reader(d,async()=>Response.json({hits:[],cursor:'cursor-'+(++calls)})).invoke({limit:1}),failure('response_too_large'));assert.equal(calls,10);
 await assert.rejects(call(d,{limit:1},{hits:Array.from({length:1001},(_,i)=>({objectID:String(i)}))}),failure('response_too_large'));
});
test('Algolia bounds bytes cumulatively across pages even when each response fits',async()=>{
 const d=selected('algolia'),first={hits:[{objectID:'1',text:'x'.repeat(800)}],cursor:'second'},second={hits:[{objectID:'2',text:'x'.repeat(800)}]};let calls=0;
 await assert.rejects(reader(d,async()=>Response.json(++calls===1?first:second),KEY,{maxResponseBytes:1024}).invoke({limit:2}),failure('response_too_large'));assert.equal(calls,2);
});
test('Algolia cancellation joins a held later page and fresh invocation starts without the old cursor',async()=>{
 const d=selected('algolia'),entered=deferred(),cancelling=deferred(),release=deferred(),controller=new AbortController();let calls=0,settled=false;
 const run=reader(d,async(_url,init)=>{
  calls++;if(calls===1)return Response.json({hits:[],cursor:'second'});
  if(calls===2){assert.deepEqual(JSON.parse(init.body),{cursor:'second'});entered.resolve();return new Response(new ReadableStream({cancel(){cancelling.resolve();return release.promise}}));}
  assert.deepEqual(JSON.parse(init.body),{});return Response.json(d.body);
 });
 const pending=run.invoke(d.args,{signal:controller.signal}).finally(()=>{settled=true}),rejected=assert.rejects(pending,failure('cancelled'));
 await entered.promise;controller.abort();await cancelling.promise;await tick();assert.equal(settled,false);assert.equal(calls,2);release.resolve();await rejected;assert.deepEqual(await run.invoke(d.args),d.result);assert.equal(calls,3);
});
test('the eight new reads use only the selected transport, including real SDK construction',async t=>{
 let ambient=0;const refuse=()=>{ambient++;throw Error('Ambient operation refused')};
 t.mock.method(http,'request',refuse);t.mock.method(https,'request',refuse);t.mock.method(globalThis,'fetch',refuse);
 t.mock.method(childProcess,'spawn',refuse);t.mock.method(childProcess,'exec',refuse);
 for(const d of profiles)assert.deepEqual(await call(d,d.args),d.result);
 assert.equal(ambient,0);
});
test('build proof retains actual selected actions and Sheets SDK while Xero index registrations stay excluded',async()=>{
 const build=JSON.parse(await readFile(new URL('../dist/BUILD-MANIFEST.json',import.meta.url),'utf8'));
 const output=Object.values(build.metafile.outputs).flatMap(item=>Object.entries(item.inputs));
 const actions=['jira-cloud/src/lib/actions/get-issue.ts','dropbox/src/lib/actions/get-file-metadata.ts','google-sheets/src/lib/actions/sheets-get-values.ts',
  'microsoft-excel-365/src/lib/actions/get-table-columns.ts','microsoft-teams/src/lib/actions/get-channel-message.ts','quickbooks/src/actions/find-invoice.ts',
  'xero/src/lib/actions/find-invoice.ts','algolia/src/lib/actions/browse-records.ts'];
 for(const action of actions){
  const path='vendor/activepieces/packages/pieces/community/'+action;
  assert.ok(output.some(([name,item])=>name===path&&item.bytesInOutput>0),path);
 }
 assert.ok(output.some(([name,item])=>name==='node_modules/@googleapis/sheets/build/v4.js'&&item.bytesInOutput>0));
 const projection=JSON.parse(await readFile(new URL('../dist/SOURCE-PROJECTIONS.json',import.meta.url),'utf8'));
 assert.ok(projection.records.some(item=>item.source==='packages/pieces/community/xero/src/index.ts'));
 for(const name of Object.keys(build.metafile.inputs))if(name.includes('community/xero/src/lib/'))assert.ok(!name.includes('/triggers/')&&(!name.includes('/actions/')||name.endsWith('/find-invoice.ts')),name);
});
