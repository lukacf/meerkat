import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createReader,createWriter} from '../dist/index.js';
import {select,inputs} from '../dist/profiles.js';
import {ReadRequestState} from '../dist/read-profiles.js';
import {Invocation} from '../dist/transport.js';
import {createDonorBundle} from '../dist/donor-factory.js';
import {GET_ISSUE_QUERY,ISSUE_LABELS_PAGE_QUERY,ISSUE_SELECTION} from '../dist/effect-queries.js';
import {cases} from './effects-fixtures.mjs';
import http from 'node:http';import https from 'node:https';
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const read=path=>readFile(new URL('../'+path,import.meta.url));
test('Every SDK port and embedded Linear notice remains retained and hash bound',async()=>{
 const linear=JSON.parse(await read('dist/LINEAR-SDK-PROJECTION.json'));
 for(const key of ['retained_original','retained_projected','retained_source_map'])assert.equal(sha(await read(linear[key].path)),linear[key].sha256);
 assert.equal((await read(linear.retained_original.path)).toString().replace(linear.original,linear.replacement),(await read(linear.retained_projected.path)).toString());
 const ports=JSON.parse(await read('dist/EFFECT-SDK-PORTS.json'));
 for(const item of ports.records)for(const key of ['retained_original','retained_replacement'])assert.equal(sha(await read(item[key].path)),item[key].sha256);
 const old=JSON.parse(await read('dist/SDK-PORTS.json'));for(const item of old.records){assert.equal(sha(await read(item.retained_original)),item.original_sha256);assert.equal(sha(await read(item.retained_replacement)),item.replacement_sha256);}
 const embedded=JSON.parse(await read('licenses/linear-embedded/MANIFEST.json'));
 assert.equal(embedded.packages.length,9);for(const item of embedded.packages)for(const notice of item.notices)assert.equal(sha(await read(notice.path)),notice.sha256);
});
test('Selected Linear queries are exact donor constants',async()=>{
 const manifest=JSON.parse(await read('vendor/SOURCE-MANIFEST.json'));
 const query=manifest.files.find(x=>x.path.includes('/linear/')&&x.path.endsWith('/queries.ts'));assert.ok(query);
 const text=(await read('vendor/activepieces/'+query.path)).toString();for(const value of [ISSUE_SELECTION,ISSUE_LABELS_PAGE_QUERY])assert.ok(text.includes(value));assert.ok(GET_ISSUE_QUERY.includes(ISSUE_SELECTION));
});
for(const item of cases.filter(x=>x.body))test(`${item.profile}: selected contract refuses a second actual donor request`,async()=>{
 let calls=0;const selection=select({profile:item.profile,auth:item.auth,transport:async()=>{calls++;return new Response(JSON.stringify(item.response),{status:item.status});}}),plan=inputs(selection,item.args);
 const invocation=new Invocation(selection,plan.pathname,undefined,plan.query,plan.inlineQuery,plan.effect);invocation.entered=true;
 const actions={'activecampaign.create-contact':'activeCampaignCreateContact','freshservice.create-ticket':'freshserviceCreateTicket','gitea.create-issue':'giteaCreateIssue','datadog.submit-log':'datadogSubmitLog'};
 const bundle=createDonorBundle(async value=>{const response=await invocation.request(value);await assert.rejects(invocation.request(value),e=>e.dispatched&&e.writeOutcome==='uncertain');return response;});
 try{await bundle[actions[item.profile]].run({auth:structuredClone(selection.auth),propsValue:plan.props});assert.equal(calls,1);assert.equal(invocation.fault.writeOutcome,'uncertain');}finally{await invocation.finish();}
});
test('New SDKs and donors never call ambient fetch, HTTP or HTTPS',async t=>{
 let calls=0;const deny=()=>{calls++;throw Error('ambient transport');};t.mock.method(globalThis,'fetch',deny);t.mock.method(http,'request',deny);t.mock.method(https,'request',deny);
 for(const item of cases)await (item.body?createWriter:createReader)({profile:item.profile,auth:item.auth,transport:async()=>new Response(JSON.stringify(item.response),{status:item.status??200})}).invoke(item.args);
 assert.equal(calls,0);
});

test('Earlier selected read plans retain their 32 KiB request ceiling',async()=>{
 let calls=0;const selection=select({profile:'dropbox.get-file-metadata',auth:{accessToken:'synthetic-a'},transport:async()=>{calls++;throw Error('unexpected dispatch');}});
 const prepared=inputs(selection,{path:'/example'});
 // Stress the internal plan ceiling independently of narrower public fields.
 prepared.read.body={path:'/'.repeat(32769)};
 const request={method:'POST',url:'https://api.dropboxapi.com/2/files/get_metadata',authentication:{type:'BEARER_TOKEN',token:'synthetic-a'},headers:{'Content-Type':'application/json'},body:prepared.read.body};
 assert.ok(Buffer.byteLength(new ReadRequestState(selection,prepared.pathname,prepared.query,prepared.inlineQuery,prepared.read).prepare(request).body)>32768);
 const invocation=new Invocation(selection,prepared.pathname,undefined,prepared.query,prepared.inlineQuery,prepared.read);
 try{await assert.rejects(invocation.request(request),e=>e.dispatched===false&&e.effect==='read');assert.equal(calls,0);}finally{await invocation.finish();}
});
