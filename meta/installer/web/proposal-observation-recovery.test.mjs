import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath,pathToFileURL } from 'node:url';
import { createRequire } from 'node:module';
import { setTimeout as observationDelay } from 'node:timers/promises';
import React,{act} from 'react';
import { fingerprint } from './state.mjs';
import { animationObservationUnavailable } from './animation-publication.mjs';
import { animationFailureDetail } from './freeform-animation.mjs';
const { JSDOM }=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
// All scene execution is replaced with inert fixture classes and all transport is public fake I/O.
const ownedPlan=(id='earlier',count=1)=>({project:{name:'Owned animation fixture',slug:'animation-fixture'},
  agent_classes:[{id,name:`${id} owner`,description:`${id} domain context`,instructions:`${id} responsibilities`,
    instances:count,tools:{memory:true}}],topology:{mode:'manual',entrypoint:`${id}:${count===1?'main':1}`,edges:[]},
  connections:[],knowledge:{mode:'none'},flows:[]});

async function mountedAnimationRecovery(verify,{visualReview=false}={}) {
  const require=createRequire(import.meta.url),instances=[],posts=[],timers=new Map(),receipts=new Map(),reads=[];
  const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/',pretendToBeVisual:true});
  let timerId=0,receiptId=0,latestPlan=ownedPlan(),latestSetup='owned-recovery',latestBrief,resolveReview;
  let nextStatus='completed',nextObservation='',holdRecovery=false; const admittedBodies=new Map();
  const recoveryResponses=[];
  const recoveryReads=()=>reads.filter(target=>/^\/api\/visualizations\/[a-f0-9]{64}$/.test(target));
  const reviewPosts=[];
  const rendererHash=(await import('./animation-review-renderer-hash.mjs')).ANIMATION_RENDERER_HASH;
  const replacements={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,sessionStorage:dom.window.sessionStorage,
    HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true,__ownedAnimationInstances:instances,
    matchMedia:()=>({matches:false,addEventListener(){},removeEventListener(){}}),
    ResizeObserver:class {observe(){}disconnect(){}},
    setTimeout:(callback,delay)=>{const id=++timerId;timers.set(id,{callback,delay});return id;},
    clearTimeout:id=>timers.delete(id),
    fetch:async(target,options={})=>{
      if(target.startsWith('/api/visualization-intents/')){
        const body=JSON.parse(options.body);
        return {ok:true,json:async()=>({...body,status:target.endsWith('/release')?'released':'live',
          intent_token:`00000000-0000-4000-8000-${String(body.intent_revision).padStart(12,'0')}`,lease_seconds:90})};
      }
      if(visualReview && target==='/api/visualization-review-capability')return {ok:true,json:async()=>({available:true,renderer_hash:rendererHash})};
      if(visualReview && target.endsWith('/review') && options.method==='POST'){
        const body=JSON.parse(options.body);reviewPosts.push(body);
        return {ok:true,json:()=>new Promise(resolve=>{resolveReview=()=>resolve({id:'9'.repeat(64),status:'reviewed',current:true,
          binding:body.binding,renderer_hash:rendererHash,report:{verdict:'needs_revision',summary:'A synthetic fixture needs revision'},
          correction_advisory:{review_id:'9'.repeat(64),feedback:'Synthetic visual correction',eligible_now:true,
            requires_atomic_expected_parent_admission:true,expected_parent:{operation_id:body.binding.id,setup_id:body.binding.setup_id,
              stream_id:body.binding.stream_id,code_hash:body.binding.code_hash,blueprint_hash:body.binding.blueprint_hash,model_hash:body.binding.model_hash}}});})};
      }
      if(options.method!=='POST'){
        reads.push(target);
        const saved=receipts.get(target.split('/').at(-1));
        if(!saved)return {ok:false,status:404,json:async()=>({})};
        return {ok:true,json:()=>holdRecovery ? new Promise(resolve=>recoveryResponses.push(()=>resolve(structuredClone(saved))))
          : Promise.resolve(structuredClone(saved))};
      }
      assert.equal(target,'/api/visualizations');
      const body=JSON.parse(options.body);posts.push(body);
      const {consumer_intent: ignoredIntent,...factualBody}=body;
      const admissionKey=JSON.stringify(factualBody); if(admittedBodies.has(admissionKey))return {ok:true,json:async()=>structuredClone(admittedBodies.get(admissionKey))};
      const [blueprint_hash,model_hash,request_fingerprint]=await Promise.all([
        fingerprint(body.blueprint),fingerprint(body.model),fingerprint({blueprint:body.blueprint,model:body.model,
          ...(body.feedback?{feedback:body.feedback}:{})})]);
      const id=String(++receiptId).padStart(64,'0');
      const code=`return program${receiptId}; // inert renderer fixture`;
      const code_hash=[...new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(code)))].map(value=>value.toString(16).padStart(2,'0')).join('');
      const native_admission={interaction_id:`offline-${receiptId}`,cursor:'console:1'};
      const revision_binding={version:'prior-program-v1',setup_id:body.setup_id,stream_id:body.stream_id,context_hash:'a'.repeat(64)};
      const publication_receipt={authority:'publish_visualization',request_key:'e'.repeat(64),blueprint_hash,model_hash,
        reference_hash:'d'.repeat(64),code_hash,revision_binding};
      const saved={id,status:nextStatus,native_run_status:nextStatus==='completed'?'completed':'unknown',source_generation:receiptId,native_admission,revision_binding,
        ...(nextObservation?{observation_status:nextObservation}:{}),
        setup_id:body.setup_id,stream_id:body.stream_id,request_fingerprint,blueprint_hash,model_hash,
        native_request_key:'e'.repeat(64),artifact:{key:'e'.repeat(64),contract:'freeform-animation-v3',
          blueprint_hash,model_hash,reference_hash:'d'.repeat(64),code_hash,code,native_admission,revision_binding,publication_receipt}};
      if(!['published','completed'].includes(nextStatus))delete saved.artifact;
      receipts.set(id,saved);admittedBodies.set(admissionKey,saved);
      return {ok:true,json:async()=>structuredClone(saved)};
    }};
  const previous=new Map(Object.keys(replacements).map(key=>[key,Object.getOwnPropertyDescriptor(globalThis,key)]));
  for(const [key,value] of Object.entries(replacements))Object.defineProperty(globalThis,key,{configurable:true,value});
  const result=await build({entryPoints:[fileURLToPath(new URL('proposal-simulation.jsx',import.meta.url))],
    bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent',loader:{'.css':'empty'},
    nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
    alias:{'@console-core':fileURLToPath(new URL('../../packages/console-core/src',import.meta.url))},
    plugins:[{name:'mounted-animation-boundaries',setup(builder){
      builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({path:pathToFileURL(require.resolve(args.path)).href,external:true}));
      builder.onResolve({filter:/freeform-animation\.mjs$/},()=>({path:'authored-canvas',namespace:'recovery-fixture'}));
      builder.onResolve({filter:/mob-sim-renderer\.mjs$/},()=>({path:'automatic-canvas',namespace:'recovery-fixture'}));
      builder.onLoad({filter:/.*/,namespace:'recovery-fixture'},args=>({contents:args.path==='automatic-canvas'
        ? 'export class MobSim {setScene(){}seek(){}destroy(){}}'
        : `export const animationFailureDetail=${animationFailureDetail.toString()};
        export class FreeformAnimation {
          constructor(layer,options){this.layer=layer;this.options=options;this.alive=true;this.time=9;this.duration=30;this.paused=false;this.captureId=0;
            this.frame=document.createElement('iframe');this.frame.dataset.testProgram=options.code;layer.append(this.frame);
            globalThis.__ownedAnimationInstances.push(this);}
          seek(time){this.time=time;this.options.onFrame?.(time,30);}
          async capture(){return {frameId:++this.captureId,time:this.time,progress:this.time/this.duration,playing:!this.paused,
            reducedMotion:this.reducedMotion,selectedId:this.selectedId,showTools:this.showPeers,width:600,height:340,ratio:1,
            mime:'image/png',bytes:new ArrayBuffer(40)};}
          cancelCapture(){this.cancelledCapture=true;}
          destroy(){this.alive=false;this.destroyed=true;}
          first(){this.options.onReady({duration:30,chapters:[{at:0,title:this.options.code,description:'Source-bound chapter'}]});
            this.options.onCaption({title:this.options.code,description:'Source-bound caption'});this.options.onFirstFrame();}
          fail(){this.destroy();this.options.onError();}
        }`,loader:'js'}));
    }}]});
  const {ProposalSimulation:MountedSimulation}=await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
  const {createRoot}=await import('react-dom/client');
  let root=createRoot(dom.window.document.getElementById('root'));
  const settle=callback=>act(async()=>{await callback?.();await new Promise(resolve=>setImmediate(resolve));});
  const mount=(plan=latestPlan,setup=latestSetup,activity=null,designBrief=latestBrief)=>settle(()=>{
    latestPlan=plan;latestSetup=setup;latestBrief=designBrief;root.render(React.createElement(MountedSimulation,
      {plan,setupId:setup,rehearsalActivity:activity,designBrief,interfaceFact:designBrief?.facts?.interfaces}));
  });
  const generate=()=>settle(async()=>{
    const entry=[...timers].find(([,value])=>value.delay===650);assert.ok(entry,'One pending scene generation');
    timers.delete(entry[0]);await entry[1].callback();
  });
  const waitForTimer=async delay=>{
    let entry;
    await settle(async()=>{
      // Node's crypto completion can take more than one event-loop turn. Wait
      // for the actual scheduled sample, without firing or inventing a timer.
      for(const deadline=performance.now()+5000;performance.now()<deadline;){
        entry=[...timers].find(([,value])=>value.delay===delay);
        if(entry)return;
        await observationDelay(1);
      }
    });
    return entry;
  };
  const waitForReviewPost=()=>settle(async()=>{
    // The final sampled frame still awaits its real SHA-256 before submission.
    // Observe that already-started POST without advancing playback or fake time.
    const deadline=Date.now()+5000;
    while(reviewPosts.length===0){
      assert.ok(Date.now()<deadline,'The already-started review POST must be observed within 5 seconds');
      await observationDelay(5);
    }
  });
  const visible=()=>instances.filter(instance=>instance.alive && instance.layer.style.visibility==='visible');
  try {
    await mount();
    await verify({dom,instances,posts,reviewPosts,timers,receipts,reads,recoveryReads,mount,generate,settle,waitForTimer,waitForReviewPost,visible,resolveReview:()=>resolveReview?.(),
      nextStatus:value=>{nextStatus=value;},nextObservation:value=>{nextObservation=value;},holdRecovery:value=>{holdRecovery=value;},releaseRecovery:()=>recoveryResponses.splice(0).forEach(resolve=>resolve()),
      remount:async()=>{await settle(()=>root.unmount());root=createRoot(dom.window.document.getElementById('root'));await mount();},
      text:()=>dom.window.document.body.textContent,
      count:()=>dom.window.document.querySelector('.proposal-simulation').dataset.nodeCount});
  }finally{
    await settle(()=>root.unmount());dom.window.close();
    assert.ok(instances.every(instance=>!instance.alive),'Unmount releases active, retained and failed scene owners');
    for(const [key,descriptor] of previous){if(descriptor)Object.defineProperty(globalThis,key,descriptor);else delete globalThis[key];}
  }
}


function button(ui,label){const value=[...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent===label);assert.ok(value,`Missing ${label}`);return value;}
const generationPolls=ui=>[...ui.timers.values()].filter(value=>value.delay===1800);
function assertAdvancedIntent(actual,previous){
 const {consumer_intent: currentIntent,...currentBody}=actual;
 const {consumer_intent: previousIntent,...previousBody}=previous;
 assert.deepEqual(currentBody,previousBody,'Every factual source and review authorization field stays exact');
 assert.deepEqual(Object.keys(currentIntent).sort(),['consumer_id','intent_revision','intent_token','setup_id','stream_id']);
 assert.equal(currentIntent.consumer_id,previousIntent.consumer_id);
 assert.equal(currentIntent.setup_id,previousIntent.setup_id);assert.equal(currentIntent.stream_id,previousIntent.stream_id);
 assert.equal(currentIntent.intent_revision,previousIntent.intent_revision+1);assert.notEqual(currentIntent.intent_token,previousIntent.intent_token);
}

test('only nonterminal unobservable receipts without a publication pause automatic scene polling',()=>{
 for(const status of ['queued','running','observing'])assert.equal(animationObservationUnavailable({status,observation_status:'unavailable'}),true);
 for(const status of ['failed','interrupted','published','completed'])assert.equal(animationObservationUnavailable({status,observation_status:'unavailable',artifact:{code:'accepted'}}),false);
 assert.equal(animationObservationUnavailable({status:'observing',observation_status:'reconnecting'}),false);
});

test('mounted first-scene orphan pauses polling and explicitly rechecks exactly the saved request',async()=>{
 await mountedAnimationRecovery(async ui=>{
  ui.nextStatus('running');ui.nextObservation('unavailable');await ui.generate();
  assert.equal(ui.posts.length,1);assert.equal(ui.instances.length,0);assert.equal(generationPolls(ui).length,0);
  assert.match(ui.text(),/could not be checked/);assert.match(ui.text(),/native status is unknown/);
  assert.doesNotMatch(ui.text(),/could not finish|Composing your first|Retry agent/);
  await ui.settle(()=>{const check=button(ui,'Check again');check.click();check.click();});await ui.generate();
  assert.equal(ui.posts.length,2);assertAdvancedIntent(ui.posts[1],ui.posts[0]);assert.equal(ui.receipts.size,1);
  assert.equal(generationPolls(ui).length,0);assert.match(ui.text(),/native status is unknown/);
 });
});

test('mounted orphan update keeps the earlier working scene with its exact old facts while rechecking',async()=>{
 await mountedAnimationRecovery(async ui=>{
  await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
  ui.nextStatus('observing');ui.nextObservation('unavailable');await ui.mount(ownedPlan('latest',2));await ui.generate();
  assert.deepEqual(ui.visible(),[previous]);assert.equal(ui.count(),'1');assert.equal(previous.alive,true);
  assert.match(ui.text(),/does not include the latest proposal changes/);assert.match(ui.text(),/native status is unknown/);
  assert.equal(generationPolls(ui).length,0);
  await ui.settle(()=>button(ui,'Check again').click());await ui.generate();
  assertAdvancedIntent(ui.posts[2],ui.posts[1]);assert.equal(ui.receipts.size,2);assert.deepEqual(ui.visible(),[previous]);
  assert.equal(ui.instances.length,1);assert.equal(generationPolls(ui).length,0);
 });
});

test('mounted unknown correction recheck retains exact review authorization and an unsent refinement draft',async()=>{
 await mountedAnimationRecovery(async ui=>{
  await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
  const settled=await ui.waitForTimer(300);assert.ok(settled,'The visible presentation must settle before review sampling');
  assert.equal(ui.reviewPosts.length,0);ui.timers.delete(settled[0]);await ui.settle(()=>settled[1].callback());
  for(let index=0;index<3;index++){
   const timer=await ui.waitForTimer(9000);assert.ok(timer);ui.timers.delete(timer[0]);
   await ui.settle(()=>{previous.time=(previous.time+9)%30;timer[1].callback();});
  }
  await ui.waitForReviewPost();
  assert.equal(ui.reviewPosts.length,1);
  ui.nextStatus('running');ui.nextObservation('unavailable');await ui.settle(()=>ui.resolveReview());await ui.generate();
  const correction=structuredClone(ui.posts[1]);assert.ok(correction.expected_parent);assert.equal(correction.review_id,'9'.repeat(64));
  assert.equal(generationPolls(ui).length,0);assert.deepEqual(ui.visible(),[previous]);
  await ui.settle(()=>button(ui,'Refine animation').click());
  const input=ui.dom.window.document.querySelector('textarea');assert.ok(input);
  await ui.settle(()=>{
   Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(input,'My unsent next refinement.');
   input.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
   input.dispatchEvent(new ui.dom.window.Event('change',{bubbles:true}));
  });
  assert.equal(input.value,'My unsent next refinement.');
  await ui.settle(()=>{const check=button(ui,'Check again');check.click();check.click();});await ui.generate();
  assertAdvancedIntent(ui.posts[2],correction);assert.equal(ui.receipts.size,2);
  assert.equal(ui.dom.window.document.querySelector('textarea').value,'My unsent next refinement.');
  assert.deepEqual(ui.visible(),[previous]);assert.equal(generationPolls(ui).length,0);
 },{visualReview:true});
});

test('mounted accepted publication stays visible without polling its unavailable optional native tail',async()=>{
 await mountedAnimationRecovery(async ui=>{
  ui.nextStatus('published');ui.nextObservation('unavailable');await ui.generate();
  assert.equal(ui.instances.length,1);const accepted=ui.instances[0];await ui.settle(()=>accepted.first());
  assert.deepEqual(ui.visible(),[accepted]);assert.equal(generationPolls(ui).length,0);
  assert.match(ui.text(),/Scene composed by your visualization agent/);
  const sceneStatus=ui.dom.window.document.querySelector('.proposal-simulation > .ps-animation-status');assert.ok(sceneStatus);
  assert.doesNotMatch(sceneStatus.textContent,/could not finish|could not be checked|Check again|Retry agent/);
  const receipt=[...ui.receipts.values()][0];assert.equal(receipt.native_run_status,'unknown');
  assert.equal(receipt.status,'published');assert.equal(ui.posts.length,1);
 });
});

test('mounted correction recheck keeps exact authorization through one missing-receipt reattachment',async()=>{
 await mountedAnimationRecovery(async ui=>{
  await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
  const settled=await ui.waitForTimer(300);assert.ok(settled,'The visible presentation must settle before review sampling');
  assert.equal(ui.reviewPosts.length,0);ui.timers.delete(settled[0]);await ui.settle(()=>settled[1].callback());
  for(let index=0;index<3;index++){
   const timer=await ui.waitForTimer(9000);assert.ok(timer);ui.timers.delete(timer[0]);
   await ui.settle(()=>{previous.time=(previous.time+9)%30;timer[1].callback();});
  }
  await ui.waitForReviewPost();
  assert.equal(ui.reviewPosts.length,1);
  ui.nextStatus('running');ui.nextObservation('unavailable');await ui.settle(()=>ui.resolveReview());await ui.generate();
  const correction=structuredClone(ui.posts[1]);assert.ok(correction.expected_parent);assert.ok(correction.review_id);
  const [id,receipt]=[...ui.receipts].at(-1);delete receipt.observation_status;
  await ui.settle(()=>button(ui,'Check again').click());await ui.generate();
  assertAdvancedIntent(ui.posts[2],correction);assert.equal(generationPolls(ui).length,1);
  ui.receipts.delete(id);
  const poll=await ui.waitForTimer(1800);assert.ok(poll);ui.timers.delete(poll[0]);await ui.settle(()=>poll[1].callback());
  const reattach=await ui.waitForTimer(2000);assert.ok(reattach);ui.timers.delete(reattach[0]);await ui.settle(()=>reattach[1].callback());
  assert.equal(ui.posts.length,4);assert.deepEqual(ui.posts[3],ui.posts[2]);
  assert.deepEqual(ui.visible(),[previous]);assert.equal(ui.instances.length,1);
 },{visualReview:true});
});
