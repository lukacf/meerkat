import test from 'node:test';
import assert from 'node:assert/strict';
import {gzipSync,gunzipSync} from 'node:zlib';
import {createReader,createWriter,ActivepiecesError} from '../dist/index.js';
import {cases} from './effects-fixtures.mjs';
import {sdkProfiles,KEY} from './fixtures/sdk-expansion-profiles.mjs';
const writes=cases.filter(item=>item.body);
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
const tick=()=>new Promise(r=>setImmediate(r));
const jsonBytes=body=>Buffer.from(JSON.stringify(body));
const chunks=bytes=>new ReadableStream({start(controller){for(let i=0;i<bytes.length;i+=3)controller.enqueue(bytes.subarray(i,i+3));controller.close();}});
const writer=(item,transport,options={})=>createWriter({profile:item.profile,auth:item.auth,transport,...options});
const failure=(kind='invalid_response',effect='write')=>error=>error instanceof ActivepiecesError&&error.kind===kind&&error.dispatched===true&&error.effect===effect&&error.writeOutcome===(effect==='write'?'uncertain':'not_applicable');
for(const item of writes){
 for(const offset of [-1,1])test(`${item.profile}: identity framing ${offset} refuses acknowledgement with one request`,async()=>{
  const bytes=jsonBytes(item.response);let calls=0;
  await assert.rejects(writer(item,async()=>{calls++;return new Response(chunks(bytes),{status:item.status,headers:{'content-length':String(bytes.length+offset),'content-encoding':'identity'}});}).invoke(item.args),failure());assert.equal(calls,1);
 });
 test(`${item.profile}: absent, exact and zero-prefixed length survive UTF8 chunk boundaries`,async()=>{
  const bytes=jsonBytes(item.response);
  for(const headers of [{},{'content-length':String(bytes.length)},{'content-length':'000'+bytes.length,'content-encoding':'Identity'},{'transfer-encoding':'chunked'}]){
   let calls=0;const result=await writer(item,async()=>{calls++;return new Response(chunks(bytes),{status:item.status,headers});}).invoke(item.args);assert.equal(calls,1);assert.equal(result.writeOutcome,'acknowledged');
  }
 });
 test(`${item.profile}: decoded gzip preserves wire length without comparing it to decoded bytes`,async()=>{
  const bytes=jsonBytes(item.response),wire=gzipSync(bytes);assert.notEqual(wire.length,bytes.length);let calls=0;
  const result=await writer(item,async()=>{calls++;return new Response(chunks(gunzipSync(wire)),{status:item.status,headers:{'content-encoding':'gzip','content-length':String(wire.length)}});}).invoke(item.args);
  assert.equal(result.writeOutcome,'acknowledged');assert.equal(calls,1);
 });
 test(`${item.profile}: malformed, duplicate and conflicting lengths retain uncertain effect`,async()=>{
  const bytes=jsonBytes(item.response);
  for(const length of ['', '-1', '+2','2.0','0x2','2 2',`${bytes.length}, ${bytes.length}`,`${bytes.length}, ${bytes.length+1}`]){
   let calls=0;await assert.rejects(writer(item,async()=>{calls++;return new Response(chunks(bytes),{status:item.status,headers:{'content-length':length}});}).invoke(item.args),failure());assert.equal(calls,1);
  }
  for(const headers of [new Headers([['content-length',String(bytes.length)],['content-length',String(bytes.length)]]),new Headers({'content-length':String(bytes.length),'transfer-encoding':'chunked'})]){
   let calls=0;await assert.rejects(writer(item,async()=>{calls++;return new Response(chunks(bytes),{status:item.status,headers});}).invoke(item.args),failure());assert.equal(calls,1);
  }
 });
 test(`${item.profile}: identity overshoot waits for actual stream cancellation`,async()=>{
  const bytes=jsonBytes(item.response),cancelling=deferred(),release=deferred();let done=false,calls=0;
  const body=new ReadableStream({start(c){c.enqueue(bytes);},cancel(){cancelling.resolve();return release.promise;}});
  const pending=writer(item,async()=>{calls++;return new Response(body,{status:item.status,headers:{'content-length':String(bytes.length-1)}});}).invoke(item.args).finally(()=>done=true);
  const rejected=assert.rejects(pending,failure());await cancelling.promise;await tick();assert.equal(done,false);assert.equal(body.locked,true);release.resolve();await rejected;assert.equal(body.locked,false);assert.equal(calls,1);
 });
}
test('Identity byte accounting counts UTF8 bytes, not string length or chunk count',async()=>{
 const item=writes[0],response={...item.response,note:'\u{1F4A1} \u00e5'},bytes=jsonBytes(response);assert.notEqual(bytes.length,JSON.stringify(response).length);
 const result=await writer(item,async()=>new Response(chunks(bytes),{status:item.status,headers:{'content-length':String(bytes.length)}})).invoke(item.args);assert.equal(result.data.note,response.note);
});
test('Malformed header rejection still joins a pending body cancellation',async()=>{
 const item=writes[0],cancelling=deferred(),release=deferred();let done=false;
 const body=new ReadableStream({cancel(){cancelling.resolve();return release.promise;}});
 const pending=writer(item,async()=>new Response(body,{status:item.status,headers:{'content-length':'2, 2'}})).invoke(item.args).finally(()=>done=true);
 const rejected=assert.rejects(pending,failure());await cancelling.promise;await tick();assert.equal(done,false);release.resolve();await rejected;assert.equal(body.locked,false);
});
test('Identity EOF shortfall releases the completed body without replay or artificial cancellation',async()=>{
 const item=writes[0],bytes=jsonBytes(item.response);let calls=0,cancelled=0;
 const body=new ReadableStream({start(c){c.enqueue(bytes);c.close();},cancel(){cancelled++;}});
 await assert.rejects(writer(item,async()=>{calls++;return new Response(body,{status:item.status,headers:{'content-length':String(bytes.length+1)}});}).invoke(item.args),failure());assert.equal(body.locked,false);assert.equal(cancelled,0);assert.equal(calls,1);
});
test('Large identity decimal lengths are bounded without overflow or precision loss',async()=>{
 const item=writes[0];let calls=0;
 await assert.rejects(writer(item,async()=>{calls++;return new Response(chunks(jsonBytes(item.response)),{status:item.status,headers:{'content-length':'999999999999999999999999999999999999999999'}});}).invoke(item.args),failure('response_too_large'));assert.equal(calls,1);
});
test('Malformed content codings cannot bypass identity validation',async()=>{
 const item=writes[0],bytes=jsonBytes(item.response);
 for(const encoding of ['',',','identity,','gzip;foo=bar'])await assert.rejects(writer(item,async()=>new Response(chunks(bytes),{status:item.status,headers:{'content-length':String(bytes.length-1),'content-encoding':encoding}})).invoke(item.args),failure());
});
const readControls=[
 {profile:'tally.get-form',auth:{apiKey:'synthetic-key'},args:{form_id:'form_1'},bytes:jsonBytes({id:'form_1'})},
 {profile:'dropbox.get-file-metadata',auth:{accessToken:'synthetic-key'},args:{path:'id:selected'},bytes:jsonBytes({'.tag':'file',id:'id:selected',name:'example'})},
 ...sdkProfiles.filter(item=>['monday.get-items','amazon-s3.list-files'].includes(item.profile)).map(item=>({...item,auth:item.auth(KEY),bytes:typeof item.body==='string'?Buffer.from(item.body):jsonBytes(item.body)})),
 {...cases.find(item=>item.profile==='gmail.get-thread'),bytes:jsonBytes(cases.find(item=>item.profile==='gmail.get-thread').response)},
];
for(const item of readControls)test(`${item.profile}: shared reader and XML framing controls`,async()=>{
 for(const offset of [0,1]){
  let calls=0;const operation=createReader({profile:item.profile,auth:item.auth,transport:async()=>{calls++;return new Response(chunks(item.bytes),{headers:{'content-length':String(item.bytes.length+offset)}});}});
  if(offset)await assert.rejects(operation.invoke(item.args),failure('invalid_response','read'));else await operation.invoke(item.args);assert.equal(calls,1);
 }
});
test('Decoded gzip wire bytes may exceed the reader decoded-body budget',async()=>{
 const item=readControls[0],wire=gzipSync(item.bytes);assert.ok(wire.length>item.bytes.length);
 const result=await createReader({profile:item.profile,auth:item.auth,maxResponseBytes:item.bytes.length,transport:async()=>new Response(gunzipSync(wire),{headers:{'content-encoding':'gzip','content-length':String(wire.length)}})}).invoke(item.args);assert.deepEqual(result,{id:'form_1'});
});
test('Decoded gzip still has the actual stream byte limit',async()=>{
 const item=readControls[0],wire=gzipSync(item.bytes);
 await assert.rejects(createReader({profile:item.profile,auth:item.auth,maxResponseBytes:item.bytes.length-1,transport:async()=>new Response(gunzipSync(wire),{headers:{'content-encoding':'gzip','content-length':String(wire.length)}})}).invoke(item.args),failure('response_too_large','read'));
});
test('Freshservice every tag position must be an own string before transport',async()=>{
 const item=writes.find(item=>item.profile==='freshservice.create-ticket');
 const leading=Array(2);leading[1]='tag';const trailing=['tag'];trailing.length=2;const middle=['first',,'last'];
 for(const tags of [Array(1),leading,trailing,middle,[undefined],['tag',null]]){
  let calls=0;await assert.rejects(writer(item,async()=>{calls++;return Response.json(item.response,{status:item.status});}).invoke({...item.args,tags}),e=>e instanceof ActivepiecesError&&e.kind==='invalid_arguments'&&e.writeOutcome==='not_attempted'&&!e.dispatched);assert.equal(calls,0);
 }
 for(const tags of [[],['first','second']]){
  let calls=0;await writer(item,async(_url,init)=>{calls++;assert.deepEqual(JSON.parse(init.body).tags,tags.length?tags:undefined);return Response.json(item.response,{status:item.status});}).invoke({...item.args,tags});assert.equal(calls,1);
 }
});
test('Zero length with nonempty content and identity-only coding lists cannot acknowledge',async()=>{
 const item=writes[0],bytes=jsonBytes(item.response);
 for(const headers of [{'content-length':'0'},{'content-length':String(bytes.length+1),'content-encoding':'IDENTITY, identity'}])await assert.rejects(writer(item,async()=>new Response(chunks(bytes),{status:item.status,headers})).invoke(item.args),failure());
});
test('A raw compressed body is not silently treated as the required JSON representation',async()=>{
 const item=writes[0],wire=gzipSync(jsonBytes(item.response));
 await assert.rejects(writer(item,async()=>new Response(chunks(wire),{status:item.status,headers:{'content-encoding':'gzip','content-length':String(wire.length)}})).invoke(item.args),failure());
});
