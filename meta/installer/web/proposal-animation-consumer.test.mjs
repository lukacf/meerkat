import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {createRequire} from 'node:module';
import {setTimeout as wait} from 'node:timers/promises';
import React,{act} from 'react';
import {fingerprint} from './state.mjs';
import {animationFailureDetail} from './freeform-animation.mjs';

const {JSDOM}=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
const plan=name=>({project:{name,slug:'consumer-fixture'},agent_classes:[{id:'lead',name,description:'Own a continuing context',
  instructions:'Maintain this context.',instances:1,tools:{memory:true}}],topology:{mode:'manual',entrypoint:'lead:main',edges:[]},
  connections:[],knowledge:{mode:'none'},flows:[{id:'summary',name:'Context summary',steps:[{id:'prepare',agent:'lead:main',prompt:'Summarize context.',depends_on:[]}]}]});

async function mounted(verify,{strict=false}={}) {
  const require=createRequire(import.meta.url),dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/',pretendToBeVisual:true});
  const instances=[],calls=[],posts=[],timers=new Map(),declares=[],responses=[],hashes=[],trace=[];
  let timerId=0,operationId=0,holdDeclare=false,holdPost=false,holdHash=false,postFailure=null,renewFailure=null,currentPlan=plan('Initial context');
  const hostReceipts=new Map(),realCrypto=globalThis.crypto;
  const ack=body=>({...body,status:'live',intent_token:`aaaaaaaa-aaaa-4aaa-8aaa-${String(body.intent_revision).padStart(12,'0')}`,
    lease_seconds:90,expires_at:new Date(Date.now()+90000).toISOString()});
  const receipt=async body=>{
    const [blueprint_hash,model_hash,request_fingerprint]=await Promise.all([fingerprint(body.blueprint),fingerprint(body.model),
      fingerprint({blueprint:body.blueprint,model:body.model,...(body.feedback?{feedback:body.feedback}:{})})]);
    const identity=JSON.stringify([body.setup_id,body.stream_id,request_fingerprint,body.expected_parent,body.review_id]);
    if(hostReceipts.has(identity))return hostReceipts.get(identity);
    const id=String(++operationId).padStart(64,'0'),code=`return inertProgram${operationId}; // never executed`;
    const code_hash=[...new Uint8Array(await realCrypto.subtle.digest('SHA-256',new TextEncoder().encode(code)))].map(value=>value.toString(16).padStart(2,'0')).join('');
    const native_admission={interaction_id:`offline-${operationId}`,cursor:'console:1'},revision_binding={version:'prior-program-v1',
      setup_id:body.setup_id,stream_id:body.stream_id,context_hash:'a'.repeat(64)};
    const publication_receipt={authority:'publish_visualization',request_key:'e'.repeat(64),blueprint_hash,model_hash,reference_hash:'d'.repeat(64),code_hash,revision_binding};
    const value={id,status:'completed',source_generation:operationId,native_admission,revision_binding,setup_id:body.setup_id,stream_id:body.stream_id,
      request_fingerprint,blueprint_hash,model_hash,native_request_key:'e'.repeat(64),artifact:{key:'e'.repeat(64),contract:'freeform-animation-v3',
        blueprint_hash,model_hash,reference_hash:'d'.repeat(64),code_hash,code,native_admission,revision_binding,publication_receipt}};
    hostReceipts.set(identity,value);return value;
  };
  const replacements={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,HTMLElement:dom.window.HTMLElement,
    sessionStorage:dom.window.sessionStorage,IS_REACT_ACT_ENVIRONMENT:true,__consumerInstances:instances,
    crypto:{randomUUID:()=>realCrypto.randomUUID(),subtle:{digest:(algorithm,bytes)=>{
      trace.push('hash');const work=()=>realCrypto.subtle.digest(algorithm,bytes);
      return holdHash?new Promise(resolve=>hashes.push(()=>resolve(work()))):work();
    }}},matchMedia:()=>({matches:false,addEventListener(){},removeEventListener(){}}),ResizeObserver:class{observe(){}disconnect(){}},
    setTimeout:(callback,delay)=>{const id=++timerId;timers.set(id,{callback,delay});return id;},clearTimeout:id=>timers.delete(id),
    fetch:async(url,options={})=>{
      const body=options.body?JSON.parse(options.body):null;calls.push({url,options,body});trace.push(url);
      const response=data=>({ok:true,json:async()=>data});
      if(url==='/api/visualization-intents/release')return response({...body,status:'released'});
      if(url==='/api/visualization-intents/declare'){
        if(holdDeclare)return new Promise(resolve=>declares.push({body,resolve:()=>resolve(response(ack(body)))}));
        return response(ack(body));
      }
      if(url==='/api/visualization-intents/renew'){
        if(renewFailure==='unknown')throw new Error('Lost renewal response');
        if(renewFailure==='stale')return {ok:false,status:409,json:async()=>({code:'stale_visualization_intent'})};
        if(renewFailure==='held')return new Promise(resolve=>responses.push(()=>resolve(response(ack(body)))));
        return response(ack(body));
      }
      if(url==='/api/visualizations' && options.method==='POST'){
        posts.push(body);
        if(postFailure==='unknown')throw new Error('Lost admission response');
        if(postFailure==='stale')return {ok:false,status:409,json:async()=>({code:'stale_visualization_intent'})};
        if(postFailure==='conflict')return {ok:false,status:409,json:async()=>({detail:'Checked correction conflict'})};
        if(holdPost)return new Promise(resolve=>responses.push(async()=>resolve(response(await receipt(body)))));
        return response(await receipt(body));
      }
      return {ok:false,status:404,json:async()=>({})};
    }};
  const previous=new Map(Object.keys(replacements).map(key=>[key,Object.getOwnPropertyDescriptor(globalThis,key)]));
  for(const [key,value] of Object.entries(replacements))Object.defineProperty(globalThis,key,{configurable:true,value});
  const bundled=await build({entryPoints:[fileURLToPath(new URL('proposal-simulation.jsx',import.meta.url))],bundle:true,write:false,format:'esm',platform:'node',
    logLevel:'silent',loader:{'.css':'empty'},nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
    alias:{'@console-core':fileURLToPath(new URL('../../packages/console-core/src',import.meta.url))},plugins:[{name:'inert-consumer-preview',setup(builder){
      builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({path:pathToFileURL(require.resolve(args.path)).href,external:true}));
      builder.onResolve({filter:/(?:freeform-animation|mob-sim-renderer)\.mjs$/},args=>({path:args.path,namespace:'inert-consumer'}));
      builder.onLoad({filter:/.*/,namespace:'inert-consumer'},args=>({loader:'js',contents:args.path.includes('mob-sim-renderer')
        ?'export class MobSim {setScene(){}seek(){}destroy(){}}':`export const animationFailureDetail=${animationFailureDetail.toString()};
        export class FreeformAnimation {
          constructor(layer,options){this.layer=layer;this.options=options;this.alive=true;this.time=0;this.duration=30;
            this.frame=document.createElement('iframe');layer.append(this.frame);globalThis.__consumerInstances.push(this);}
          seek(time){this.time=time;}destroy(){this.alive=false;}
          first(){this.options.onReady({duration:30,chapters:[]});this.options.onCaption({title:'Inert scene',description:'Offline fixture'});this.options.onFirstFrame();}
        }`}));
    }}]});
  const {ProposalSimulation}=await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
  const {createRoot}=await import('react-dom/client');let root=createRoot(dom.window.document.getElementById('root'));
  const settle=callback=>act(async()=>{await callback?.();await new Promise(resolve=>setImmediate(resolve));});
  const mount=(next=currentPlan)=>settle(()=>{currentPlan=next;const element=React.createElement(ProposalSimulation,{plan:next,setupId:'owned-guide'});
    root.render(strict?React.createElement(React.StrictMode,null,element):element);});
  const until=async check=>{const deadline=performance.now()+5000;
    while(!check()&&performance.now()<deadline)await settle(()=>wait(5));
    assert.ok(check(),'Reached the actual owned boundary');};
  const fire=(delay,wait=true)=>settle(async()=>{const entry=[...timers].find(([,item])=>item.delay===delay);assert.ok(entry,`Existing ${delay}ms timer`);
    timers.delete(entry[0]);const result=entry[1].callback();if(wait)await result;});
  const button=text=>[...dom.window.document.querySelectorAll('button')].find(item=>item.textContent===text);
  try{await mount();await verify({dom,calls,posts,instances,timers,trace,mount,settle,until,fire,button,
    text:()=>dom.window.document.body.textContent,visible:()=>instances.filter(item=>item.alive&&item.layer.style.visibility==='visible'),
    holdDeclare:value=>{holdDeclare=value;},holdPost:value=>{holdPost=value;},holdHash:value=>{holdHash=value;},postFailure:value=>{postFailure=value;},renewFailure:value=>{renewFailure=value;},
    releaseDeclares:()=>settle(()=>declares.splice(0).forEach(item=>item.resolve())),releaseResponses:()=>settle(async()=>{for(const resolve of responses.splice(0))await resolve();}),
    releaseHashes:()=>settle(()=>hashes.splice(0).forEach(resolve=>resolve())),
    remount:async()=>{await settle(()=>root.unmount());root=createRoot(dom.window.document.getElementById('root'));await mount();},
    view:()=>settle(()=>{const select=dom.window.document.querySelector('select[aria-label="View to simulate"]');select.value='summary';select.dispatchEvent(new dom.window.Event('change',{bubbles:true}));})});
  }finally{holdHash=false;hashes.splice(0).forEach(resolve=>resolve());declares.splice(0).forEach(item=>item.resolve());
    await settle(()=>root.unmount());for(const resolve of responses.splice(0))await resolve();dom.window.close();
    assert.ok(instances.every(item=>!item.alive));
    for(const [key,descriptor]of previous){if(descriptor)Object.defineProperty(globalThis,key,descriptor);else delete globalThis[key];}
  }
}

test('declaration precedes hashing and debounce; view changes release pre-ack scope and ignore its late ack',async()=>{
  await mounted(async ui=>{
    assert.equal(ui.trace[0],'/api/visualization-intents/declare');assert.equal(ui.posts.length,0);
    ui.holdDeclare(true);await ui.mount(plan('Waiting context'));
    const waiting=ui.calls.filter(call=>call.url.endsWith('/declare')).at(-1).body;
    await ui.view();
    const release=ui.calls.find(call=>call.url.endsWith('/release')&&call.body.intent_revision===waiting.intent_revision);
    assert.deepEqual(release.body,waiting);assert.equal(release.options.keepalive,true);assert.equal(Object.hasOwn(release.options,'signal'),false);
    await ui.fire(650,false);assert.equal(ui.posts.length,0);
    ui.holdDeclare(false);await ui.releaseDeclares();await ui.until(()=>ui.instances.length===1);
    assert.match(ui.posts[0].stream_id,/:summary$/);
    assert.ok(ui.posts[0].consumer_intent.intent_revision>waiting.intent_revision);
    assert.equal(ui.instances.length,1);
  });
});

test('spec changes while the factual hash is pending cannot submit the abandoned body',async()=>{
  await mounted(async ui=>{
    ui.holdHash(true);await ui.mount(plan('Abandoned context'));await ui.fire(650,false);
    ui.holdHash(false);await ui.mount(plan('Current context'));await ui.releaseHashes();
    assert.equal(ui.posts.length,0);
    await ui.fire(650);assert.equal(ui.posts.length,1);assert.equal(ui.posts[0].blueprint.project.name,'Current context');
  });
});

test('a delayed old POST and first frame cannot take over the current scene',async()=>{
  await mounted(async ui=>{
    await ui.fire(650);const initial=ui.instances[0];await ui.settle(()=>initial.first());
    ui.holdPost(true);await ui.mount(plan('Abandoned replacement'));await ui.fire(650,false);await ui.until(()=>ui.posts.length===2);
    ui.holdPost(false);await ui.mount(plan('Current replacement'));await ui.fire(650);const current=ui.instances.at(-1);
    await ui.settle(()=>current.first());await ui.releaseResponses();await ui.settle(()=>initial.first());
    assert.deepEqual(ui.visible(),[current]);assert.equal(ui.instances.length,2);
  });
});

test('lost POST responses use one exact intent/body retry; explicit Check again captures source and changes only scheduling',async()=>{
  await mounted(async ui=>{
    ui.postFailure('unknown');await ui.fire(650);await ui.fire(2000);
    assert.equal(ui.posts.length,2);assert.deepEqual(ui.posts[1],ui.posts[0]);
    assert.equal(ui.calls.filter(call=>call.url.endsWith('/declare')).length,1);
    assert.ok(ui.button('Check again'));assert.equal([...ui.timers.values()].some(item=>item.delay===2000),false);
    ui.postFailure(null);await ui.settle(()=>ui.button('Check again').click());await ui.fire(650);
    const {consumer_intent:old,...saved}=ui.posts[0],{consumer_intent:next,...recovered}=ui.posts[2];
    assert.deepEqual(recovered,saved);assert.equal(next.consumer_id,old.consumer_id);assert.ok(next.intent_revision>old.intent_revision);
    assert.notEqual(next.intent_token,old.intent_token);assert.equal(ui.instances.length,1);
  });
});

test('stale intent rejection is separate from correction conflicts and preserves displayed scene plus unsent refinement',async()=>{
  await mounted(async ui=>{
    await ui.fire(650);const initial=ui.instances[0];await ui.settle(()=>initial.first());
    await ui.settle(()=>ui.button('Refine animation').click());const textarea=ui.dom.window.document.querySelector('textarea');
    await ui.settle(()=>{Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(textarea,'Keep my unsent refinement');
      textarea.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));});
    ui.renewFailure('stale');await ui.fire(30000);
    assert.match(ui.text(),/This view.s animation request could not be confirmed/);assert.deepEqual(ui.visible(),[initial]);
    assert.equal(textarea.value,'Keep my unsent refinement');assert.equal(ui.calls.filter(call=>call.url.endsWith('/declare')).length,1);
    assert.equal(ui.posts.length,1);assert.ok(ui.button('Check again'));
  });
  await mounted(async ui=>{ui.postFailure('conflict');await ui.fire(650);assert.doesNotMatch(ui.text(),/This view.s animation request could not be confirmed/);
    assert.match(ui.text(),/native status is unknown/);assert.equal(ui.posts.length,1);assert.equal([...ui.timers.values()].some(item=>item.delay===2000),false);});
});

test('lease expiry before first frame cannot publish or trigger fresh generation through a late callback',async()=>{
  await mounted(async ui=>{
    await ui.fire(650);const candidate=ui.instances[0];ui.renewFailure('held');await ui.fire(30000,false);
    const expiry=[...ui.timers].find(([,item])=>item.delay>80000);assert.ok(expiry);
    await ui.settle(()=>{ui.timers.delete(expiry[0]);expiry[1].callback();});
    await ui.releaseResponses();await ui.settle(()=>candidate.first());
    assert.deepEqual(ui.visible(),[]);assert.equal(candidate.alive,false);assert.equal(ui.posts.length,1);
    assert.equal(ui.calls.filter(call=>call.url.endsWith('/declare')).length,1);assert.match(ui.text(),/This view.s animation request could not be confirmed/);
  });
});

test('strict effect cleanup and remount use monotonic scoped revisions and fresh component UUIDs',async()=>{
  await mounted(async ui=>{
    const declarations=ui.calls.filter(call=>call.url.endsWith('/declare'));
    assert.equal(declarations.length,2);assert.equal(declarations[0].body.consumer_id,declarations[1].body.consumer_id);
    assert.ok(declarations[1].body.intent_revision>declarations[0].body.intent_revision);
    const old=declarations.at(-1).body.consumer_id;await ui.remount();
    const current=ui.calls.filter(call=>call.url.endsWith('/declare')).at(-1).body;
    assert.notEqual(current.consumer_id,old);assert.equal(ui.posts.length,0);
    assert.ok(ui.calls.some(call=>call.url.endsWith('/release')&&call.body.consumer_id===old));
  },{strict:true});
});
