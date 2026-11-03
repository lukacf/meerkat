import test from 'node:test';
import assert from 'node:assert/strict';
import {createReader,ActivepiecesError} from '../dist/index.js';
import {select,inputs} from '../dist/profiles.js';
import {oneDrivePathBoundaries,trimCharacters} from './fixtures/onedrive-boundaries.mjs';
const configuration={profile:'onedrive.get-item',auth:{accessToken:'onedrive-boundary-fixture',cloud:'commercial'},transport:async()=>{throw Error('Unexpected host call')}};
const selected=select(configuration);

test('OneDrive path reads keep folder names items as literal data at every depth',async()=>{
 const observed=[],body={id:'provider-item-123',name:'a.txt'};
 const reader=createReader({...configuration,transport:async url=>{observed.push(url);return Response.json(body)}});
 for(const path of ['items/a.txt','reports/items/a.txt','items/items/a.txt','reports/items/items','items']){
  assert.equal((await reader.invoke({path})).id,body.id);
  assert.equal(observed.at(-1),'https://graph.microsoft.com/v1.0/me/drive/root:/'+path+':');
 }
});

test('OneDrive exact ID route still binds identity including IDs with encoded path-like text',async()=>{
 for(const itemId of ['selected-id','items/a.txt','root:/items/a.txt:']){
  const observed=[];
  const read=id=>createReader({...configuration,transport:async url=>{observed.push(url);return Response.json({id,name:'File'})}}).invoke({itemId});
  assert.equal((await read(itemId)).id,itemId);
  await assert.rejects(read('different-id'),error=>error instanceof ActivepiecesError&&error.kind==='invalid_response'&&error.dispatched===true);
  assert.ok(observed.every(url=>url==='https://graph.microsoft.com/v1.0/me/drive/items/'+encodeURIComponent(itemId)));
 }
 const root=createReader({...configuration,transport:async url=>{assert.equal(url,'https://graph.microsoft.com/v1.0/me/drive/root');return Response.json({id:'actual-root-id',name:'Root'})}});
 assert.equal((await root.invoke({itemId:'root'})).id,'actual-root-id');
});

test('OneDrive schema preserves all outer-slash and trim boundaries without narrowing runtime normalization',()=>{
 const schema=createReader(configuration).describe().inputSchema.properties.path;
 const patterns=['','u'].map(flags=>({text:new RegExp(schema.pattern,flags),invalid:new RegExp(schema.not.pattern,flags)}));
 for(const path of oneDrivePathBoundaries()){
  let runtime=true;try{inputs(selected,{path})}catch{runtime=false}
  const length=[...path].length;
  for(const {text,invalid}of patterns){const accepted=length>=schema.minLength&&length<=schema.maxLength&&text.test(path)&&!invalid.test(path);
   assert.equal(accepted,runtime,JSON.stringify({path,runtime,accepted}));
  }
 }
});

test('OneDrive original outer-whitespace regressions and every printable ECMAScript trim character reach the real donor',async()=>{
 const observed=[],reader=createReader({...configuration,transport:async url=>{observed.push(url);return Response.json({id:'resolved-id',name:'a'})}});
 const paths=[' //a','a// ',' //a// ','a///  ',' //a/',...trimCharacters.filter(c=>!/[\x00-\x1f]/.test(c)).map(c=>c+'///a//'+c)];
 for(const path of paths)assert.equal((await reader.invoke({path})).id,'resolved-id');
 assert.ok(observed.every(url=>url==='https://graph.microsoft.com/v1.0/me/drive/root:/a:'));
 // Spaces between outer slashes are filename data, not another trim pass.
 assert.equal((await reader.invoke({path:'/ /'})).id,'resolved-id');
 assert.equal(observed.at(-1),'https://graph.microsoft.com/v1.0/me/drive/root:/%20:');
});
