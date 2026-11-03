import test from 'node:test';
import assert from 'node:assert/strict';
import {createReader,createWriter,ActivepiecesError} from '../dist/index.js';
const json=(value,status=200)=>new Response(JSON.stringify(value),{status,headers:{'content-type':'application/json'}});
import {cases} from './effects-fixtures.mjs';
const make=(item,transport,extra={})=>(item.body?createWriter:createReader)({profile:item.profile,auth:item.auth,transport,...extra});
for(const item of cases){
 test(`${item.profile}: real donor happy path`,async()=>{
  let calls=0;
  const op=make(item,async(url,init)=>{calls++;assert.equal(new URL(url).pathname,item.path);assert.equal(init.redirect,'error');assert.ok(init.signal instanceof AbortSignal);if(item.body)assert.deepEqual(JSON.parse(init.body),item.body);return json(item.response,item.status);});
  assert.equal(op.describe().effect,item.body?'write':'read');
  const value=await op.invoke(item.args);assert.equal(calls,1);assert.ok(value);
  if(item.body)assert.equal(value.writeOutcome,'acknowledged');
  if(item.profile==='datadog.submit-log')assert.equal(value.acknowledgement,'accepted_for_processing');
  if(item.profile==='google-cloud-storage.search-objects'){assert.equal(value.pageOnly,true);assert.equal(value.totalItems,1);assert.equal(value.nextPageToken,'next-token');}
 });
 test(`${item.profile}: lost response is single request and safe`,async()=>{
  let calls=0;const op=make(item,async()=>{calls++;throw Error('synthetic-a reflected');});
  await assert.rejects(op.invoke(item.args),e=>e instanceof ActivepiecesError&&e.dispatched&&e.writeOutcome===(item.body?'uncertain':'not_applicable')&&!JSON.stringify(e).includes('synthetic-a'));
  assert.equal(calls,1);
 });
}
test('reader refuses all write selections',()=>{for(const item of cases.filter(c=>c.body))assert.throws(()=>createReader({...item,transport:async()=>json({})}),ActivepiecesError);});
