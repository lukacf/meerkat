import test from 'node:test';
import assert from 'node:assert/strict';
import {PassThrough} from 'node:stream';
import {mkdtemp,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {MockAgent} from 'undici';
import {StdioServerTransport} from '@modelcontextprotocol/server/stdio';
import {PROTOCOL_VERSION_META_KEY,CLIENT_INFO_META_KEY,CLIENT_CAPABILITIES_META_KEY} from '@modelcontextprotocol/server';
import {parseConfiguration,describeMcpConfiguration,createMcpServer,serveMcpStdio,createOwnedHttpTransport} from '../dist/mcp.js';
import {cases} from './effects-fixtures.mjs';
const metadata={[PROTOCOL_VERSION_META_KEY]:'2026-07-28',[CLIENT_INFO_META_KEY]:{name:'effect-fixture',version:'1'},[CLIENT_CAPABILITIES_META_KEY]:{}};
const json=(value,status=200)=>new Response(JSON.stringify(value),{status});
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve};};
async function eventually(predicate){const end=Date.now()+10000;while(!predicate()){if(Date.now()>end)throw Error('Fixture deadline');await new Promise(r=>setTimeout(r,2));}}
function configuration(item){const value={profile:item.profile,token_env:'SELECTED_TOKEN'};for(const key of ['origin','domain','owner','repository','site','bucket'])if(key in item.auth)value[key]=item.auth[key];return value;}
function harness(item,era='modern',httpFactory){
 const stdin=new PassThrough(),stdout=new PassThrough(),replies=[];let buffer='',id=0;
 stdout.on('data',bytes=>{buffer+=bytes;let index;while((index=buffer.indexOf('\n'))>=0){replies.push(JSON.parse(buffer.slice(0,index)));buffer=buffer.slice(index+1);}});
 const owner=serveMcpStdio(configuration(item),{environment:{SELECTED_TOKEN:'synthetic-a',UNSELECTED_TOKEN:'unselected-secret'},httpFactory:httpFactory??(()=>({send:async()=>json(item.response,item.status),close:async()=>{}})),transport:new StdioServerTransport(stdin,stdout)});
 const send=(method,params={})=>{const current=++id;stdin.write(JSON.stringify({jsonrpc:'2.0',id:current,method,params:{...params,...(era==='modern'?{_meta:metadata}:{})}})+'\n');return current;};
 const reply=async requestId=>{await eventually(()=>replies.some(x=>x.id===requestId));return replies.find(x=>x.id===requestId);};
 return {stdin,owner,replies,send,reply,async request(method,params){return reply(send(method,params));},async start(){if(era==='legacy'){const result=await reply(send('initialize',{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'fixture',version:'1'}}));assert.equal(result.error,undefined);stdin.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})+'\n');}}};
}
for(const item of cases){
 for(const era of ['legacy','modern'])test(`${era} MCP ${item.profile}: real SDK wire and explicit effect`,async()=>{
  let calls=0;const dispatcher=new MockAgent();dispatcher.disableNetConnect();
  const origin=item.auth.origin??(item.profile.startsWith('freshservice')?`https://${item.auth.domain}.freshservice.com`:item.profile.startsWith('datadog')?`https://http-intake.logs.${item.auth.site}`:item.profile.startsWith('gmail')?'https://gmail.googleapis.com':item.profile.startsWith('slack')?'https://slack.com':item.profile.startsWith('linear')?'https://api.linear.app':'https://www.googleapis.com');
  dispatcher.get(origin).intercept({method:item.body||item.profile.startsWith('slack')||item.profile.startsWith('linear')?'POST':'GET',path:path=>new URL(path,origin).pathname===item.path}).reply(item.status??200,()=>{calls++;return JSON.stringify(item.response);});
  const h=harness(item,era,()=>createOwnedHttpTransport(dispatcher));try{
   await h.start();const list=await h.request('tools/list');const descriptor=list.result.tools[0];assert.equal(descriptor.annotations.readOnlyHint,!item.body);assert.equal(descriptor.annotations.idempotentHint,!item.body);assert.equal(descriptor.annotations.destructiveHint,!!item.body);assert.ok(!JSON.stringify(descriptor).includes('synthetic-a'));
   const result=await h.request('tools/call',{name:descriptor.name,arguments:item.args});assert.equal(result.result.isError,undefined);if(item.body)assert.equal(result.result.structuredContent.value.writeOutcome,'acknowledged');assert.equal(calls,1);dispatcher.assertNoPendingInterceptors();
  }finally{await h.owner.close();}
 });
 test(`MCP ${item.profile}: errors carry effect metadata without provider text`,async()=>{
  let calls=0;const h=harness(item,'modern',()=>({send:async()=>{calls++;throw Error('synthetic-a unselected-secret');},close:async()=>{}}));try{
   const result=await h.request('tools/call',{name:item.profile.replaceAll('.','_'),arguments:item.args});assert.equal(result.result.isError,true);assert.equal(result.result.structuredContent.error.effect,item.body?'write':'read');assert.equal(result.result.structuredContent.error.writeOutcome,item.body?'uncertain':'not_applicable');assert.ok(!JSON.stringify(result).includes('synthetic-a'));assert.ok(!JSON.stringify(result).includes('unselected-secret'));assert.equal(calls,1);
   const invalid=await h.request('tools/call',{name:'unknown',arguments:{}});assert.equal(invalid.result.structuredContent.error.writeOutcome,item.body?'not_attempted':'not_applicable');
  }finally{await h.owner.close();}
 });
 test(`MCP ${item.profile}: shutdown joins body cancellation before HTTP close`,async()=>{
  const entered=deferred(),cancelling=deferred(),release=deferred();let closed=false,settled=false;
  const body=new ReadableStream({cancel(){cancelling.resolve();return release.promise;}});
  const h=harness(item,'modern',()=>({send:async()=>{entered.resolve();return new Response(body);},close:async()=>{assert.equal(body.locked,false);closed=true;}}));
  const call=h.send('tools/call',{name:item.profile.replaceAll('.','_'),arguments:item.args});await entered.promise;const pending=h.owner.close().finally(()=>settled=true);await cancelling.promise;await new Promise(r=>setImmediate(r));assert.equal(settled,false);assert.equal(closed,false);release.resolve();await pending;assert.equal(closed,true);assert.equal(h.replies.some(x=>x.id===call&&!x.result?.isError),false);
 });
 test(`CLI ${item.profile}: describe without credentials and clean serve shutdown`,async()=>{
  const dir=await mkdtemp(join(tmpdir(),'toolkit-effects8-cli-')),file=join(dir,'config.json');await writeFile(file,JSON.stringify(configuration(item)));
  async function run(command){
   const child=spawn(process.execPath,['dist/mcp-cli.js',command,'--config',file],{cwd:new URL('..',import.meta.url),env:command==='serve'?{SELECTED_TOKEN:'synthetic-a'}:{},stdio:['pipe','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',x=>stdout+=x);child.stderr.on('data',x=>stderr+=x);const exit=once(child,'exit');
   if(command==='serve'){child.stdin.write(JSON.stringify({jsonrpc:'2.0',id:1,method:'tools/list',params:{_meta:metadata}})+'\n');await eventually(()=>stdout.includes('"tools"'));child.kill('SIGTERM');}else child.stdin.end();
   const [code,signal]=await exit;assert.equal(code,0,stderr);assert.equal(signal,null);assert.ok(!stderr.includes('synthetic-a'));return JSON.parse(stdout.trim());
  }
  const value=await run('describe');assert.equal(value.tools[0].annotations.readOnlyHint,!item.body);await run('serve');
 });
}
test('MCP writes require exact host-selected nonsecret configuration and named credential',async()=>{
 const item=cases[0];for(const change of [{apiKey:'forbidden'},{origin:'http://not-https.invalid'},{origin:'https://x.invalid/path'}])assert.throws(()=>parseConfiguration({...configuration(item),...change}));
 let owners=0;await assert.rejects(createMcpServer(configuration(item),{environment:{UNSELECTED_TOKEN:'synthetic-a'},httpFactory(){owners++;throw 0;}}));assert.equal(owners,0);
 assert.equal(describeMcpConfiguration(configuration(item)).tools[0].annotations.readOnlyHint,false);
});
test('MCP selected free text accepts Unicode scalar pairs and preserves the real request',async()=>{
 const item={...cases[0],args:{...cases[0].args,firstName:'\u{1F4A1}'}};let calls=0;
 const h=harness(item,'modern',()=>({send:async(_url,init)=>{calls++;assert.equal(JSON.parse(init.body).contact.firstName,'\u{1F4A1}');return json(item.response,item.status);},close:async()=>{}}));
 try{const result=await h.request('tools/call',{name:item.profile.replaceAll('.','_'),arguments:item.args});assert.equal(result.result.isError,undefined);assert.equal(calls,1);}finally{await h.owner.close();}
});
