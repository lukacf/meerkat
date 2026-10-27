import test from 'node:test';
import assert from 'node:assert/strict';
import {PassThrough} from 'node:stream';
import {MockAgent} from 'undici';
import {StdioServerTransport} from '@modelcontextprotocol/server/stdio';
import {PROTOCOL_VERSION_META_KEY,CLIENT_INFO_META_KEY,CLIENT_CAPABILITIES_META_KEY} from '@modelcontextprotocol/server';
import {mkdtemp,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createOwnedHttpTransport,serveMcpStdio} from '../dist/mcp.js';
import {sdkProfiles as profiles,KEY} from './fixtures/sdk-expansion-profiles.mjs';
const metadata={[PROTOCOL_VERSION_META_KEY]:'2026-07-28',[CLIENT_INFO_META_KEY]:{name:'sdk-expansion-fixture',version:'1'},[CLIENT_CAPABILITIES_META_KEY]:{}};
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
async function eventually(predicate){const end=Date.now()+5000;while(!predicate()){if(Date.now()>end)throw Error('MCP fixture timed out');await new Promise(r=>setTimeout(r,2));}}
function harness(d,era,key=KEY,httpFactory){
 const stdin=new PassThrough(),stdout=new PassThrough(),replies=[],diagnostics=[];let pending='',id=0;
 stdout.on('data',chunk=>{pending+=chunk;let i;while((i=pending.indexOf('\n'))>=0){replies.push(JSON.parse(pending.slice(0,i)));pending=pending.slice(i+1)}});
 const dispatcher=new MockAgent();dispatcher.disableNetConnect();
 const env={SELECTED_TOKEN:d.token(key),...d.environment,AMBIENT_SECRET:'unselected-secret'};
 const owner=serveMcpStdio({profile:d.profile,token_env:'SELECTED_TOKEN',...d.config},{environment:env,httpFactory:httpFactory??(()=>createOwnedHttpTransport(dispatcher)),transport:new StdioServerTransport(stdin,stdout),onerror:(...v)=>diagnostics.push(v)});
 for(const name of Object.keys(env))env[name]='unselected-secret';
 const send=(method,params={})=>{const n=++id;stdin.write(JSON.stringify({jsonrpc:'2.0',id:n,method,params:{...params,...(era==='modern'?{_meta:metadata}:{})}})+'\n');return n};
 const request=async(method,params={})=>{const n=send(method,params);await eventually(()=>replies.some(r=>r.id===n));return replies.find(r=>r.id===n)};
 return {owner,dispatcher,request,send,replies,diagnostics,async start(){if(era==='legacy'){const value=await request('initialize',{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'sdk-fixture',version:'1'}});assert.equal(value.result.protocolVersion,'2024-11-05');stdin.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})+'\n')}},async close(){try{await owner.close()}finally{if(httpFactory)await dispatcher.close()}}};
}
function intercept(h,d,key,{status=200,body=d.body}={}){
 const url=new URL(d.url);h.dispatcher.get(url.origin).intercept({method:d.method,path:url.pathname+url.search,headers:headers=>{
  const v=new Headers(headers);if(d.profile.startsWith('ghost'))assert.match(v.get('authorization'),/^Ghost /);else if(d.profile.startsWith('amazon-s3')){assert.match(v.get('authorization'),/^AWS4-HMAC-SHA256 Credential=AKIAABCDEFGHIJKLMNOP\//);assert.equal(v.get('x-amz-security-token'),'selected-session-token');}else assert.equal(v.get('authorization'),(d.profile.startsWith('mailchimp')?'Bearer ':'')+key);return true;
 },...(d.requestBody?{body:JSON.stringify(d.requestBody)}:{})}).reply(status,body,{headers:{location:'https://unselected.invalid','retry-after':'30'}});
}
for(const d of profiles){
 for(const era of ['legacy','modern'])test(`${era} MCP ${d.profile} calls real donor on independent selected accounts and advertises read-only schema`,async()=>{
  const a=harness(d,era,'account-a'),b=harness(d,era,'account-b');intercept(a,d,'account-a');intercept(b,d,'account-b');
  try{await Promise.all([a.start(),b.start()]);for(const h of [a,b]){
   const listed=await h.request('tools/list'),descriptor=listed.result.tools[0];assert.equal(descriptor.name,d.profile.replaceAll('.','_'));assert.equal(descriptor.annotations.readOnlyHint,true);assert.equal(descriptor.inputSchema.additionalProperties,false);assert.ok(!JSON.stringify(descriptor).includes('SELECTED_TOKEN'));
   const value=await h.request('tools/call',{name:descriptor.name,arguments:d.args});assert.equal(value.result.isError,undefined);if(d.result)assert.deepEqual(value.result.structuredContent.value,d.result);else {assert.equal(value.result.structuredContent.value.success,true);assert.equal(value.result.structuredContent.value.summary.bounce_rate,3);assert.equal(value.result.structuredContent.value.performance_metrics.clicks.total,null)}
   h.dispatcher.assertNoPendingInterceptors();
  }}finally{await Promise.all([a.close(),b.close()]);}
 });
 test(`MCP ${d.profile} provider errors remain typed/redacted and same-session recovery works`,async()=>{
  const h=harness(d,'modern');for(const status of [302,401,403,404,429,503])intercept(h,d,KEY,{status,body:KEY+' unselected-secret'});intercept(h,d,KEY);
  try{for(const status of [302,401,403,404,429,503]){const value=await h.request('tools/call',{name:d.profile.replaceAll('.','_'),arguments:d.args});assert.equal(value.result.isError,true);assert.equal(value.result.structuredContent.error.kind,status===302?'redirect_refused':'http_error');assert.ok(!JSON.stringify(value).includes(KEY));assert.ok(!JSON.stringify(value).includes('unselected-secret'));}
   const value=await h.request('tools/call',{name:d.profile.replaceAll('.','_'),arguments:d.args});assert.equal(value.result.isError,undefined);h.dispatcher.assertNoPendingInterceptors();
  }finally{await h.close()}
 });
 test(`MCP ${d.profile} close joins in-flight body cleanup before closing its HTTP owner`,async()=>{
  const entered=deferred(),cancelling=deferred(),release=deferred();let closed=0,settled=false,calls=0;
  const body=new ReadableStream({cancel(){cancelling.resolve();return release.promise}});
  const h=harness(d,'modern',KEY,()=>({send:async()=>{calls++;entered.resolve();return new Response(body)},close:async()=>{closed++;assert.equal(body.locked,false)}}));
  try{h.send('tools/call',{name:d.profile.replaceAll('.','_'),arguments:d.args});await entered.promise;const pending=h.owner.close().finally(()=>settled=true);await cancelling.promise;await new Promise(r=>setImmediate(r));assert.equal(settled,false);assert.equal(closed,0);release.resolve();await pending;assert.equal(closed,1);assert.equal(calls,1);}finally{release.resolve();await h.close()}
 });
 test(`CLI ${d.profile} describe needs no credentials and serving process joins on SIGTERM`,async()=>{
  const dir=await mkdtemp(join(tmpdir(),'toolkit-sdk4-cli-')),file=join(dir,'config.json');await writeFile(file,JSON.stringify({profile:d.profile,token_env:'SELECTED_TOKEN',...d.config}));
  const run=async(command,environment,signal=false)=>{
   const child=spawn(process.execPath,['dist/mcp-cli.js',command,'--config',file],{cwd:new URL('..',import.meta.url),env:{PATH:process.env.PATH,...environment},stdio:['pipe','pipe','pipe']});let output='',error='';child.stdout.on('data',v=>output+=v);child.stderr.on('data',v=>error+=v);const exited=once(child,'exit');
   if(signal){child.stdin.write(JSON.stringify({jsonrpc:'2.0',id:1,method:'tools/list',params:{_meta:metadata}})+'\n');await eventually(()=>output.includes('"tools"'));child.kill('SIGTERM');}else child.stdin.end();
   const [code,termination]=await exited;assert.equal(code,0);assert.equal(termination,null);assert.ok(!error.includes(KEY));return JSON.parse(output.trim());
  };
  const described=await run('describe',{});assert.equal(described.tools[0].name,d.profile.replaceAll('.','_'));await run('serve',{SELECTED_TOKEN:d.token(KEY),...d.environment},true);
 });
}
