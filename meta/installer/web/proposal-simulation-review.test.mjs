import { setTimeout as realDelay } from 'node:timers/promises';
import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath,pathToFileURL } from 'node:url';
import { dirname,resolve } from 'node:path';
import { existsSync,readFileSync } from 'node:fs';
const candidateDir=dirname(fileURLToPath(import.meta.url)),webDir=existsSync(resolve(candidateDir,'package.json'))?candidateDir:resolve(candidateDir,'../../../web');
const {ANIMATION_RENDERER_HASH}=await import(pathToFileURL(resolve(webDir,'animation-review-renderer-hash.mjs')));
const {animationFailureDetail}=await import(pathToFileURL(resolve(candidateDir,'freeform-animation.mjs')));
const require=createRequire(resolve(webDir,'package.json'));
const {build}=await import(pathToFileURL(require.resolve('esbuild')));
const React=await import(pathToFileURL(require.resolve('react'))),{act}=React;
const {JSDOM}=createRequire(resolve(webDir,'../../console/package.json'))('jsdom');
const {fingerprint}=await import(pathToFileURL(resolve(webDir,'state.mjs')));
const digest=value=>String(value).padStart(64,'0');
const ownedPlan=(id='lead')=>({project:{name:'Owned visual-review fixture',slug:'review-fixture'},
  agent_classes:[{id,name:`${id} owner`,description:`${id} domain context`,instructions:`${id} responsibilities`,instances:1,tools:{memory:true}}],
  topology:{mode:'manual',entrypoint:`${id}:main`,edges:[]},connections:[],knowledge:{mode:'none'},flows:[]});
const largePlan=()=>({...ownedPlan(),agent_classes:[...ownedPlan().agent_classes,
  {id:'specialist',name:'Specialist',description:'Independent context owners',instructions:'Use the assigned context.',instances:609,tools:{memory:true,workspace:true}}]});

async function mounted(verify,{reduced=true,verdict='acceptable',deferReview=false,advisoryFeedback='Use fine lines. Keep labels readable.',capability=true,initialPlan=ownedPlan(),durable=false}={}) {
  const instances=[],posts=[],reviewPosts=[],reviewResults=new Map(),timers=new Map(),requests=[],guideRequests=[];
  const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/',pretendToBeVisual:true});
  let timerId=0,operationId=0,reviewId=100,plan=initialPlan,setup='review-setup',correctionFailure=null;
  const savedReceipts=new Map(),receiptIds=new Map(),consumers=new Map(),reservations=new Map();
  let parentReadFault=null;const parentReads=[];
  const lateReviews=[],resizeObservers=[],motionListeners=new Set();
  const media={matches:reduced,addEventListener:(_name,listener)=>motionListeners.add(listener),removeEventListener:(_name,listener)=>motionListeners.delete(listener)};
  const makeReview=body=>{
    const sampledVerdict=typeof verdict==='function'?verdict(body,reviewPosts.length):verdict;
    const id=digest(++reviewId),binding=structuredClone(body.binding),result={id,status:'reviewed',binding,renderer_hash:ANIMATION_RENDERER_HASH,current:true,
      report:{verdict:sampledVerdict,summary:'Sampled scene checked'}};
    if(sampledVerdict==='needs_revision')result.correction_advisory={review_id:id,eligible_now:true,requires_atomic_expected_parent_admission:true,
      feedback:advisoryFeedback,expected_parent:{operation_id:binding.id,setup_id:binding.setup_id,stream_id:binding.stream_id,
        code_hash:binding.code_hash,blueprint_hash:binding.blueprint_hash,model_hash:binding.model_hash}};
    reviewResults.set(id,result);return result;
  };
  const capturePacket=instance=>{
    instance.captureCount=(instance.captureCount || 0)+1;
    const {width,height}=instance.frame.getBoundingClientRect(),ratio=Math.min(globalThis.devicePixelRatio || 1,2);
    const pixelWidth=Math.round(width*ratio),pixelHeight=Math.round(height*ratio);
    const bytes=new ArrayBuffer(40),prefix=new Uint8Array(bytes),view=new DataView(bytes);
    prefix.set([137,80,78,71,13,10,26,10]);view.setUint32(8,13);prefix.set([73,72,68,82],12);view.setUint32(16,pixelWidth);view.setUint32(20,pixelHeight);
    return {captureId:`capture-${instance.captureCount}`,frameId:instance.captureCount,time:instance.time,progress:instance.time/instance.duration,
      playing:!instance.paused,selectedId:instance.selectedId ?? null,showTools:!!instance.showPeers,reducedMotion:!!instance.reducedMotion,
      runtimeActivity:instance.runtimeActivity || null,width,height,ratio,pixelWidth,pixelHeight,mime:'image/png',bytes};
  };
  const replacements={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,HTMLElement:dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT:true,__reviewInstances:instances,__capturePacket:capturePacket,
    devicePixelRatio:2,matchMedia:()=>media,ResizeObserver:class{
      constructor(callback){this.callback=callback;resizeObservers.push(this);}observe(target){this.target=target;}disconnect(){this.target=null;}
    },
    setTimeout:(callback,delay)=>{const id=++timerId;timers.set(id,{callback,delay});return id;},clearTimeout:id=>timers.delete(id),
    fetch:async(url,options={})=>{
      requests.push({url,options});const response=value=>({ok:true,json:async()=>value});
      if(url.startsWith('/api/visualizations/published/latest?'))return {ok:false,status:404,json:async()=>({})};
      if(url.startsWith('/api/visualization-intents/')){
        const body=JSON.parse(options.body);
        if(durable && url.endsWith('/declare')){
          const previous=consumers.get(body.consumer_id),history=previous?.history || new Map();
          consumers.set(body.consumer_id,{...body,history,receipt:null,previous:history.get(JSON.stringify([body.setup_id,body.stream_id]))});
        }
        return response({...body,status:url.endsWith('/release')?'released':'live',
          intent_token:`00000000-0000-4000-8000-${String(body.intent_revision).padStart(12,'0')}`,lease_seconds:90});
      }
      if(url==='/api/visualization-review-capability')return response({available:capability,renderer_hash:ANIMATION_RENDERER_HASH});
      if(durable && options.method!== 'POST' && /^\/api\/visualizations\/[a-f0-9]{64}$/.test(url)){
        const value=structuredClone(receiptIds.get(url.split('/').at(-1)));
        if(parentReadFault==='transport')throw new Error('Lost parent observation');
        if(parentReadFault==='held')return new Promise(resolve=>parentReads.push(()=>resolve(response(value))));
        if(parentReadFault==='missing')return {ok:false,status:404,json:async()=>({})};
        if(value && parentReadFault==='source')value.artifact.code_hash=digest(999);
        return value?response(value):{ok:false,status:404,json:async()=>({})};
      }
      if(url==='/api/visualizations' && options.method==='POST'){
        const body=JSON.parse(options.body);posts.push(body);
        if(body.expected_parent && correctionFailure){
          if(correctionFailure==='transport')throw new Error('Lost correction response');
          return {ok:false,status:409,json:async()=>correctionFailure==='stale-intent'
            ? {code:'stale_visualization_intent',detail:'This view no longer requests that animation.'}
            : {detail:correctionFailure==='spent' ? 'A newer scene request superseded this visual correction.' : 'Observation source unavailable.'}};
        }
        const [blueprint_hash,model_hash,request_fingerprint]=await Promise.all([
          fingerprint(body.blueprint),fingerprint(body.model),fingerprint({blueprint:body.blueprint,model:body.model,...(body.feedback?{feedback:body.feedback}:{})})]);
        const identity=JSON.stringify([body.setup_id,body.stream_id,request_fingerprint,body.expected_parent,body.review_id]);
        const consumer=consumers.get(body.consumer_intent?.consumer_id);
        if(durable && body.replay_parent){
          const existing=receiptIds.get(body.replay_parent.operation_id);
          assert.ok(existing,'Parent replay names an already admitted receipt');
          assert.equal(body.expected_parent,undefined);assert.equal(body.review_id,undefined);
          assert.equal(existing.request_fingerprint,request_fingerprint);
          assert.equal(existing.artifact.code_hash,body.replay_parent.code_hash);
          for(const name of ['setup_id','stream_id','blueprint_hash','model_hash'])assert.equal(existing[name],body.replay_parent[name]);
          assert.ok(savedReceipts.has(identity),'Parent replay cannot create another ordinary scene');
        }
        if(durable && body.expected_parent){
          assert.equal(consumer.intent_revision,body.consumer_intent.intent_revision);
          assert.ok([consumer.receipt,consumer.previous].includes(body.expected_parent.operation_id)
            || [consumer.receipt,consumer.previous].includes(reservations.get(body.expected_parent.operation_id)?.id),'Host-owned parent or reserved child required');
          const reserved=reservations.get(body.expected_parent.operation_id);
          if(reserved)assert.equal(reserved.identity,identity,'Spent parent only permits the same accepted correction request');
        }
        const id=digest(++operationId);
        const receipt=durable && savedReceipts.has(identity)?savedReceipts.get(identity):{id,status:'completed',source_generation:operationId,setup_id:body.setup_id,stream_id:body.stream_id,request_fingerprint,
          blueprint_hash,model_hash,native_request_key:'e'.repeat(64),revision_binding:null,
          ...(body.expected_parent?{correction_parent:structuredClone(body.expected_parent)}:{}),
          artifact:{key:'e'.repeat(64),contract:'freeform-animation-v3',blueprint_hash,model_hash,reference_hash:'d'.repeat(64),code_hash:id,code:`return program${operationId};`}};
        if(durable){
          if(savedReceipts.has(identity))operationId-=1;
          savedReceipts.set(identity,receipt);receiptIds.set(receipt.id,receipt);
          consumer.receipt=receipt.id;consumer.history.set(JSON.stringify([body.setup_id,body.stream_id]),receipt.id);
          if(body.expected_parent)reservations.set(body.expected_parent.operation_id,{identity,id:receipt.id});
        }
        return response(receipt);
      }
      if(options.method==='POST' && /\/api\/visualizations\/[a-f0-9]{64}\/review$/.test(url)){
        const body=JSON.parse(options.body);reviewPosts.push(body);const result=makeReview(body);
        if(deferReview)return new Promise(resolve=>lateReviews.push(()=>resolve(response(result))));
        return response(result);
      }
      return {ok:false,status:503,json:async()=>({})};
    }};
  const old=new Map(Object.keys(replacements).map(key=>[key,Object.getOwnPropertyDescriptor(globalThis,key)]));
  for(const [key,value] of Object.entries(replacements))Object.defineProperty(globalThis,key,{configurable:true,value});
  const bundled=await build({entryPoints:[resolve(candidateDir,'proposal-simulation.jsx')],bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent',
    loader:{'.css':'empty'},nodePaths:[resolve(webDir,'node_modules')],alias:{'@console-core':resolve(webDir,'../../packages/console-core/src')},
    plugins:[{name:'isolated-mounted-review',setup(builder){
      builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({path:pathToFileURL(require.resolve(args.path)).href,external:true}));
      builder.onResolve({filter:/freeform-animation\.mjs$/},()=>({path:'authored',namespace:'review-fixture'}));
      builder.onResolve({filter:/mob-sim-renderer\.mjs$/},()=>({path:'automatic',namespace:'review-fixture'}));
      // Redirect missing dependencies only from the isolated candidate directory.
      // In the integrated web slot, native resolution handles extensionless imports.
      builder.onResolve({filter:/^\./},async args=>{if(candidateDir!==webDir && args.resolveDir===candidateDir && !existsSync(resolve(args.resolveDir,args.path)))return builder.resolve(args.path,{resolveDir:webDir,kind:args.kind});});
      builder.onLoad({filter:/.*/,namespace:'review-fixture'},args=>({loader:'js',contents:args.path==='automatic'
        ? 'export class MobSim {setScene(){}seek(){}destroy(){}}'
        : `export const animationFailureDetail=${animationFailureDetail.toString()};
        export class FreeformAnimation {
          constructor(layer,options){this.layer=layer;this.options=options;this.alive=true;this.time=9;this.duration=40;this.paused=false;this.seekCalls=[];
            this.frame=document.createElement('iframe');this.frame.getBoundingClientRect=()=>({width:600,height:340});
            this.frame.dataset.testProgram=options.code;layer.append(this.frame);globalThis.__reviewInstances.push(this);}
          seek(time){this.seekCalls.push(time);this.time=time;this.options.onFrame?.(time,this.duration);}
          capture(){return Promise.resolve(globalThis.__capturePacket(this));}
          cancelCapture(){this.captureCancelled=true;}
          destroy(){this.alive=false;}
          first(){this.options.onReady({duration:40,chapters:[{at:0,title:this.options.code,description:'Owned source chapter'}]});
            this.options.onCaption({title:this.options.code,description:'Unpaired live caption'});this.options.onFirstFrame();}
          fail(failure){this.destroy();this.options.onError(failure);}
        }`}));
    }}]});
  const {ProposalSimulation}=await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
  const {createRoot}=await import(pathToFileURL(require.resolve('react-dom/client')));let root=createRoot(dom.window.document.getElementById('root'));
  const settle=callback=>act(async()=>{await callback?.();await new Promise(resolve=>setImmediate(resolve));});
  const mount=(nextPlan=plan,nextSetup=setup)=>settle(()=>{
    plan=nextPlan;setup=nextSetup;root.render(React.createElement('div',null,
      React.createElement('input',{'aria-label':'Guide draft',defaultValue:'I am still typing'}),
      React.createElement(ProposalSimulation,{plan,setupId:setup,onRequestChange:message=>guideRequests.push(message)})));
  });
  const until=async(check)=>{for(const deadline=performance.now()+5000;performance.now()<deadline&&!check();){await settle();await realDelay(1);}assert.ok(check(),'Mounted work reached expected boundary');};
  const tick=delay=>settle(async()=>{
    const entry=[...timers].find(([,value])=>value.delay===delay);assert.ok(entry,`One ${delay}ms timer`);timers.delete(entry[0]);
    if(delay===12000)for(const instance of instances)if(instance.alive && !instance.paused)instance.time=(instance.time+12)%instance.duration;
    await entry[1].callback();
  });
  const visible=()=>instances.filter(instance=>instance.alive && instance.layer.style.visibility==='visible');
  try{await mount();await verify({dom,posts,reviewPosts,instances,timers,requests,guideRequests,reviewResults,mount,settle,until,tick,visible,
    correctionFailure:value=>{correctionFailure=value;},
    parentReadFault:value=>{parentReadFault=value;},receiptIds,reservations,
    releaseParentReads:()=>settle(()=>parentReads.splice(0).forEach(resolve=>resolve())),
    remount:async()=>{await settle(()=>root.unmount());root=createRoot(dom.window.document.getElementById('root'));await mount();},
    generate:()=>tick(650),review:()=>tick(300),first:async scene=>{await settle(()=>scene.first());await tick(300);},
    inspect:(scene,id)=>settle(()=>scene.options.onInspect(id)),
    releaseReview:()=>settle(()=>lateReviews.shift()?.()),text:()=>dom.window.document.body.textContent,
    motion:value=>settle(()=>{media.matches=value;for(const listener of motionListeners)listener();}),
    resize:(scene,width,height)=>settle(()=>{scene.frame.getBoundingClientRect=()=>({width,height});
      for(const observer of resizeObservers)if(observer.target===scene.frame)observer.callback([{contentRect:{width,height}}]);}),
    click:label=>settle(()=>{const button=[...dom.window.document.querySelectorAll('button')].find(node=>node.getAttribute('aria-label')===label || node.textContent===label);
      assert.ok(button,`Button ${label} is visible`);button.click();}),
    changeInput:(label,value)=>settle(()=>{const input=dom.window.document.querySelector(`[aria-label="${label}"]`);assert.ok(input);
      Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype,'value').set.call(input,value);
      input.dispatchEvent(new dom.window.Event('input',{bubbles:true}));})});}
  finally{await settle(()=>{root.unmount();for(const release of lateReviews.splice(0))release();});dom.window.close();assert.ok(instances.every(instance=>!instance.alive));
    assert.ok(![...timers.values()].some(timer=>timer.delay===300),'Unmount withdraws every pending settled review');
    for(const [key,descriptor] of old){if(descriptor)Object.defineProperty(globalThis,key,descriptor);else delete globalThis[key];}}
}

test('mounted review checks the exact displayed source while preserving guide typing and playback controls',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
    const body=ui.reviewPosts[0];assert.equal(body.binding.id,digest(1));assert.equal(body.binding.code_hash,digest(1));assert.equal(body.frames.length,1);
    assert.equal(body.frames[0].id,'1');assert.equal(body.frames[0].time,9);assert.equal(body.frames[0].playing,false);assert.equal(body.frames[0].reduced_motion,true);
    assert.equal(Object.hasOwn(body.frames[0],'caption'),false);assert.deepEqual(ui.visible(),[scene]);assert.deepEqual(scene.seekCalls,[]);
    assert.equal(ui.dom.window.document.querySelector('[aria-label="Guide draft"]').value,'I am still typing');assert.deepEqual(ui.guideRequests,[]);
    await ui.until(()=>ui.text().includes('Sampled animation looks clear'));assert.match(ui.text(),/Only the sampled visible scene was checked/);
  });
});

test('mounted playing review observes four later natural frames without auto seek, pause or expansion',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);
    for(let index=0;index<3;index++){
      await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===12000));await ui.tick(12000);
    }
    await ui.until(()=>ui.reviewPosts.length===1);
    assert.deepEqual(ui.reviewPosts[0].frames.map(frame=>frame.time),[9,21,33,5]);assert.equal(scene.paused,false);assert.deepEqual(scene.seekCalls,[]);
    assert.equal(ui.dom.window.document.querySelector('.proposal-simulation').classList.contains('ps-expanded'),false);
    assert.deepEqual(ui.guideRequests,[]);
  },{reduced:false});
});

test('long authored step captions show a clamped verbatim preview and native disclosure while preserving caption ownership',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);
    const {document,getComputedStyle}=ui.dom.window,style=document.createElement('style');
    style.textContent=readFileSync(resolve(candidateDir,'proposal-simulation.css'),'utf8');document.head.append(style);
    const full='Weather evidence arrives first, including uncertainty and the age of every observation.\n'
      +'The astronomer combines that evidence with seasonal targets and user preferences; the equipment executor retains its own device context and waits for a confirmed handoff before acting. '
      +'No capture is running, and changing the preview never authorizes hardware operation.';
    await ui.settle(()=>scene.options.onCaption({title:'Evidence before a device handoff',description:full}));
    const caption=document.querySelector('.ps-caption');
    assert.equal(caption.querySelector('strong').textContent,'Evidence before a device handoff');
    const preview=caption.querySelector('.ps-caption-preview');assert.ok(preview);assert.equal(preview.textContent,full);
    assert.equal(preview.nextElementSibling === caption.querySelector('details'), true,'Read the preview before its disclosure');
    const previewStyle=getComputedStyle(preview);
    assert.equal(previewStyle.display,'-webkit-box');assert.equal(previewStyle.getPropertyValue('-webkit-line-clamp'),'3');
    assert.equal(previewStyle.maxHeight,'4.65em');assert.equal(previewStyle.overflow,'hidden');assert.equal(previewStyle.whiteSpace,'pre-wrap');
    const details=caption.querySelector('details');assert.ok(details);assert.equal(details.open,false);
    assert.equal(details.querySelector('summary').textContent,'More about this step');
    assert.equal(details.querySelector('p').textContent,full);
    assert.equal(caption.getAttribute('aria-live'),'polite');
    assert.equal(scene.reducedMotion,true);assert.equal(scene.paused,true);
    details.querySelector('summary').focus();assert.equal(document.activeElement === details.querySelector('summary'), true);
    await ui.settle(()=>{details.open=true;});assert.equal(details.open,true);assert.equal(details.querySelector('p').textContent,full);
    assert.equal(getComputedStyle(preview).display,'none','Expanded native disclosure hides the duplicate preview');
    await ui.click('Expand');assert.ok(document.querySelector('.ps-expanded'));
    assert.equal(getComputedStyle(caption).overflow,'auto','Expanded captions retain their bounded scrolling region');
    assert.equal(getComputedStyle(caption).maxHeight,'min(18dvh,144px)');
    assert.equal(getComputedStyle(preview).display,'none');assert.equal(details.querySelector('p').textContent,full);
    await ui.settle(()=>{details.open=false;});assert.equal(getComputedStyle(preview).display,'-webkit-box');
    await ui.click('Close expanded view');
    await ui.click('Play simulation');assert.equal(caption.getAttribute('aria-live'),'off');
    await ui.click('Pause simulation');assert.equal(caption.getAttribute('aria-live'),'polite');
    const short='The coordinator asks the weather specialist for fresh evidence.';
    await ui.settle(()=>scene.options.onCaption({title:'Ask for current conditions',description:short}));
    assert.equal(caption.querySelector('details') === null,true);assert.equal(caption.querySelector('p').textContent,short);
    assert.equal(caption.querySelector('strong').textContent,'Ask for current conditions');
    const selected='The selected owner keeps its own responsibilities and source context.\n'+full;
    await ui.inspect(scene,'lead:main');
    assert.equal(caption.querySelector('.ps-caption-context').textContent,'Playback chapter');
    await ui.settle(()=>scene.options.onCaption({title:'Selected owner context',description:selected}));
    assert.equal(caption.querySelector('strong').textContent,'Selected owner context');assert.equal(caption.querySelector('.ps-caption-context') === null,true);
    assert.equal(caption.querySelector('.ps-caption-preview').textContent,selected);assert.equal(caption.querySelector('details p').textContent,selected);
    await ui.click('Close inspector');
    assert.equal(caption.querySelector('strong').textContent,'Ask for current conditions');assert.equal(caption.querySelector('p').textContent,short);
    assert.equal(caption.querySelector('details') === null,true);assert.equal(caption.querySelector('.ps-caption-context') === null,true);
    assert.deepEqual(ui.visible(),[scene]);assert.equal(ui.posts.length,1);assert.deepEqual(scene.seekCalls,[]);
  });
});

test('mounted saved multiline advisory normalizes atomically and keeps the previous scene during correction',async()=>{
  await mounted(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.first(previous);
    await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===650));assert.deepEqual(ui.visible(),[previous]);await ui.generate();
    assert.equal(ui.posts.length,2);assert.equal(ui.posts[1].feedback,'Use fine lines. Keep labels readable.');
    assert.deepEqual(ui.posts[1].expected_parent,ui.reviewResults.values().next().value.correction_advisory.expected_parent);
    assert.equal(ui.posts[1].review_id,digest(101));assert.deepEqual(ui.posts[1].blueprint,ui.posts[0].blueprint);assert.deepEqual(ui.posts[1].model,ui.posts[0].model);
    assert.equal(ui.posts[1].consumer_intent.consumer_id,ui.posts[0].consumer_intent.consumer_id);
    assert.ok(ui.posts[1].consumer_intent.intent_revision>ui.posts[0].consumer_intent.intent_revision);
    assert.notEqual(ui.posts[1].consumer_intent.intent_token,ui.posts[0].consumer_intent.intent_token);
    assert.equal(ui.dom.window.document.querySelector('[aria-label="Guide draft"]').value,'I am still typing');assert.deepEqual(ui.guideRequests,[]);
  },{verdict:'needs_revision',advisoryFeedback:'Use fine lines.\n  Keep labels readable.'});
});

async function acceptedCorrection(ui){
  await ui.generate();await ui.first(ui.instances[0]);
  await ui.until(()=>ui.reviewPosts.length===1);await ui.generate();
  const corrected=ui.instances.at(-1);await ui.first(corrected);
  await ui.until(()=>ui.text().includes('Sampled animation looks clear'));
  return {corrected,parent:structuredClone(ui.posts[0]),correction:structuredClone(ui.posts[1])};
}
const oneCorrection={durable:true,verdict:(_body,count)=>count===1?'needs_revision':'acceptable'};

test('switching setups restores the accepted correction through exact parent ownership without another correction reservation',async()=>{
  await mounted(async ui=>{
    const {parent,correction}=await acceptedCorrection(ui);
    const key=`mobkit-animation-feedback-v1:${correction.stream_id}`,saved=JSON.parse(ui.dom.window.localStorage.getItem(key));
    assert.deepEqual(saved.correction,{feedback:correction.feedback,expected_parent:correction.expected_parent,review_id:correction.review_id});
    assert.equal(saved.feedback,parent.feedback);assert.equal(ui.reservations.size,1);
    await ui.mount(ownedPlan('company'),'company-setup');await ui.generate();await ui.first(ui.instances.at(-1));
    await ui.mount(ownedPlan(),'review-setup');await ui.generate();
    const restored=ui.instances.at(-1);
    assert.equal(restored.options.code,'return program2;','The host returns the already accepted child code');
    assert.equal(ui.instances.length,4,'The restored parent is not rendered before its correction');
    const factual=body=>{const {consumer_intent,...value}=body;return value;};
    const {replay_parent,...restoredParent}=factual(ui.posts[3]);
    assert.deepEqual(replay_parent,correction.expected_parent);assert.deepEqual(restoredParent,factual(parent));
    assert.deepEqual(factual(ui.posts[4]),factual(correction));assert.equal(Object.hasOwn(ui.posts[4],'replay_parent'),false);
    assert.ok(ui.posts[4].consumer_intent.intent_revision>ui.posts[3].consumer_intent.intent_revision);
    assert.equal(ui.receiptIds.size,3);assert.equal(ui.reservations.size,1);
    await ui.first(restored);assert.deepEqual(ui.visible(),[restored]);
    assert.match(ui.text(),/Scene composed by your visualization agent/);
    assert.equal(ui.dom.window.document.querySelector('[aria-label="Guide draft"]').value,'I am still typing');
  },oneCorrection);
});

test('a fresh viewer remount restores parent lineage before replaying only its exact accepted correction',async()=>{
  await mounted(async ui=>{
    const {parent,correction}=await acceptedCorrection(ui),oldConsumer=correction.consumer_intent.consumer_id;
    await ui.remount();await ui.generate();
    const restored=ui.instances.at(-1),parentReplay=ui.posts[2],childReplay=ui.posts[3];
    assert.notEqual(parentReplay.consumer_intent.consumer_id,oldConsumer);
    assert.equal(childReplay.consumer_intent.consumer_id,parentReplay.consumer_intent.consumer_id);
    assert.ok(childReplay.consumer_intent.intent_revision>parentReplay.consumer_intent.intent_revision);
    assert.equal(parentReplay.feedback,parent.feedback);
    assert.equal(Object.hasOwn(parentReplay,'expected_parent'),false);
    assert.deepEqual(parentReplay.replay_parent,correction.expected_parent);
    assert.deepEqual(childReplay.expected_parent,correction.expected_parent);
    assert.equal(childReplay.review_id,correction.review_id);assert.equal(childReplay.feedback,correction.feedback);
    assert.equal(restored.options.code,'return program2;');assert.equal(ui.instances.length,3);
    assert.equal(ui.receiptIds.size,2);assert.equal(ui.reservations.size,1);
    await ui.first(restored);assert.deepEqual(ui.visible(),[restored]);
  },oneCorrection);
});

test('missing, foreign or unavailable parent observation cannot admit a saved correction or silently replace its source',async t=>{
  for(const fault of ['missing','source','transport'])await t.test(fault,async()=>{
    await mounted(async ui=>{
      const {correction}=await acceptedCorrection(ui),saved=ui.dom.window.localStorage.getItem(`mobkit-animation-feedback-v1:${correction.stream_id}`);
      ui.parentReadFault(fault);await ui.remount();await ui.generate();
      assert.equal(ui.posts.length,2);assert.equal(ui.instances.length,2);assert.equal(ui.reservations.size,1);
      assert.match(ui.text(),/native status is unknown/);
      assert.equal([...ui.timers.values()].some(timer=>timer.delay===2000),false,'Unknown parent is not retried automatically');
      assert.equal(ui.dom.window.localStorage.getItem(`mobkit-animation-feedback-v1:${correction.stream_id}`),saved);
      ui.parentReadFault(null);await ui.click('Check again');await ui.generate();
      assert.equal(ui.posts.length,4);assert.deepEqual(ui.posts[3].expected_parent,correction.expected_parent);
      assert.equal(ui.posts[3].review_id,correction.review_id);assert.equal(ui.instances.at(-1).options.code,'return program2;');
      assert.equal(ui.receiptIds.size,2);assert.equal(ui.reservations.size,1);
    },oneCorrection);
  });
});

test('lost correction restore responses recheck only the accepted exact body and preserve the spent reservation',async()=>{
  await mounted(async ui=>{
    const {correction}=await acceptedCorrection(ui);
    ui.correctionFailure('transport');await ui.remount();await ui.generate();await ui.tick(2000);
    assert.equal(ui.posts.length,5);assert.deepEqual(ui.posts[4],ui.posts[3]);
    assert.equal(ui.reservations.size,1);assert.equal(ui.receiptIds.size,2);assert.match(ui.text(),/native status is unknown/);
    ui.correctionFailure(null);await ui.click('Check again');await ui.generate();
    const {consumer_intent:ignored,...original}=correction,{consumer_intent:renewed,...replayed}=ui.posts[5];
    assert.deepEqual(replayed,original);assert.ok(renewed.intent_revision>ui.posts[4].consumer_intent.intent_revision);
    assert.equal(ui.instances.at(-1).options.code,'return program2;');assert.equal(ui.reservations.size,1);assert.equal(ui.receiptIds.size,2);
  },oneCorrection);
});

test('a delayed parent restoration from an abandoned setup cannot attach or replay its correction in the new view',async()=>{
  await mounted(async ui=>{
    await acceptedCorrection(ui);ui.parentReadFault('held');await ui.remount();
    await ui.settle(()=>{const entry=[...ui.timers].find(([,item])=>item.delay===650);assert.ok(entry);
      ui.timers.delete(entry[0]);void entry[1].callback();});
    await ui.until(()=>ui.requests.some(item=>item.url===`/api/visualizations/${digest(1)}`));
    await ui.mount(ownedPlan('company'),'company-setup');ui.parentReadFault(null);await ui.generate();
    await ui.releaseParentReads();
    assert.equal(ui.posts.length,3);assert.equal(ui.posts.at(-1).setup_id,'company-setup');
    assert.equal(Object.hasOwn(ui.posts.at(-1),'expected_parent'),false);
    assert.equal(ui.reservations.size,1);assert.equal(ui.receiptIds.size,3);
    await ui.first(ui.instances.at(-1));assert.equal(ui.visible()[0].options.code,'return program3;');
  },oneCorrection);
});

test('mounted correction failure uses ordinary bounded execution repair without leaking prior review authorization',async()=>{
  await mounted(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.first(previous);await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===650));
    await ui.generate();const correction=ui.instances[1];await ui.first(correction);
    await ui.settle(()=>correction.fail({kind:'program',phase:'initialize',exception:'SyntaxError',name:'PrivateFailure',message:'PRIVATE_MESSAGE',stack:'PRIVATE_STACK'}));
    assert.deepEqual(ui.visible(),[previous]);await ui.generate();assert.equal(ui.posts.length,3);
    assert.match(ui.posts[2].feedback,/failed isolated canvas execution\. Failure: program\/initialize \(SyntaxError\)\./);
    assert.doesNotMatch(ui.posts[2].feedback,/PrivateFailure|PRIVATE_MESSAGE|PRIVATE_STACK/);
    assert.deepEqual(ui.posts[2].blueprint,ui.posts[0].blueprint);assert.deepEqual(ui.posts[2].model,ui.posts[0].model);
    assert.equal(Object.hasOwn(ui.posts[2],'expected_parent'),false);assert.equal(Object.hasOwn(ui.posts[2],'review_id'),false);
    await ui.settle(()=>ui.instances[2].fail());assert.equal(ui.posts.length,3);assert.deepEqual(ui.visible(),[previous]);assert.match(ui.text(),/Retry agent/);
  },{verdict:'needs_revision'});
});

test('execution repair discards custom exception text and stale failure callbacks cannot repair a new source or setup',async t=>{
  await t.test('unknown exception',async()=>mounted(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].fail({kind:'program',phase:'render',exception:'PrivateFailure',message:'PRIVATE_MESSAGE',stack:'PRIVATE_STACK'}));
    await ui.generate();assert.equal(ui.posts.length,2);
    assert.match(ui.posts[1].feedback,/Failure: program\/render\./);assert.doesNotMatch(ui.posts[1].feedback,/PrivateFailure|PRIVATE_MESSAGE|PRIVATE_STACK/);
  }));
  for(const change of ['source','setup'])await t.test(change,async()=>mounted(async ui=>{
    await ui.generate();const old=ui.instances[0];await ui.first(old);
    await ui.mount(ownedPlan('company'),change==='setup' ? 'company-setup':'review-setup');
    await ui.settle(()=>old.fail({kind:'program',phase:'initialize',exception:'SyntaxError'}));
    await ui.generate();assert.equal(ui.posts.length,2);assert.equal(ui.posts[1].feedback,'');
    assert.equal(ui.posts[1].setup_id,change==='setup' ? 'company-setup':'review-setup');
    assert.equal(ui.posts[1].blueprint.agent_classes[0].id,'company');
    await ui.first(ui.instances[1]);assert.deepEqual(ui.visible(),[ui.instances[1]]);
  }));
});

test('explicitly rejected automatic correction preserves the accepted frame and permits fresh manual feedback without replay',async()=>{
  await mounted(async ui=>{
    await ui.generate();const accepted=ui.instances[0];await ui.first(accepted);
    await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===650));
    ui.correctionFailure('spent');await ui.generate();
    assert.equal(ui.posts.length,2);assert.equal(ui.instances.length,1);assert.deepEqual(ui.visible(),[accepted]);
    assert.match(ui.text(),/Automatic refinement could not be applied/);assert.doesNotMatch(ui.text(),/native status is unknown|Check again/);
    assert.ok(![...ui.timers.values()].some(timer=>[650,2000].includes(timer.delay)),'A known refusal must not retry or resend');
    const originalSource=structuredClone(ui.posts[0]);
    await ui.click('Refine animation');
    const input=ui.dom.window.document.querySelector('textarea');assert.ok(input);
    await ui.settle(()=>{
      Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(input,'Show the weather evidence handoff clearly.');
      input.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
      input.dispatchEvent(new ui.dom.window.Event('change',{bubbles:true}));
    });
    await ui.until(()=>[...ui.dom.window.document.querySelectorAll('button')].some(button=>button.textContent==='Send to animation agent' && !button.disabled));
    await ui.click('Send to animation agent');await ui.generate();
    assert.equal(ui.posts.length,3);assert.equal(ui.posts[2].feedback,'Show the weather evidence handoff clearly.');
    assert.equal(Object.hasOwn(ui.posts[2],'expected_parent'),false);assert.equal(Object.hasOwn(ui.posts[2],'review_id'),false);
    assert.deepEqual(ui.posts[2].blueprint,originalSource.blueprint);assert.deepEqual(ui.posts[2].model,originalSource.model);
    assert.deepEqual(ui.visible(),[accepted]);
    await ui.first(ui.instances[1]);await ui.until(()=>ui.reviewPosts.length===2);
    assert.ok(![...ui.timers.values()].some(timer=>timer.delay===650),'Manual replacement does not reset the automatic correction budget');
    assert.equal(ui.posts.length,3);assert.deepEqual(ui.guideRequests,[]);
  },{verdict:'needs_revision'});
});

test('unknown transport, unrelated conflict and lost intent never unlock manual feedback as a declined correction',async()=>{
  for(const failure of ['transport','other-conflict','stale-intent'])await mounted(async ui=>{
    await ui.generate();const accepted=ui.instances[0];await ui.first(accepted);
    await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===650));
    ui.correctionFailure(failure);await ui.generate();
    if(failure==='transport')await ui.tick(2000);
    assert.deepEqual(ui.visible(),[accepted]);assert.doesNotMatch(ui.text(),/Automatic refinement could not be applied/);
    assert.match(ui.text(),failure==='stale-intent' ? /animation request could not be confirmed/ : /native status is unknown/);
    await ui.click('Refine animation');
    const button=[...ui.dom.window.document.querySelectorAll('button')].find(node=>node.textContent==='Waiting for current preview');
    assert.ok(button?.disabled);assert.equal(ui.instances.length,1);
  },{verdict:'needs_revision'});
});

test('mounted stale review cannot correct a different proposal, setup or view',async()=>{
  await mounted(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.first(previous);await ui.until(()=>ui.reviewPosts.length===1);
    await ui.mount(ownedPlan('new-owner'),'other-setup');await ui.releaseReview();assert.equal(ui.posts.length,1);assert.deepEqual(ui.visible(),[]);
    assert.equal(ui.dom.window.document.querySelector('[aria-label="Guide draft"]').value,'I am still typing');assert.deepEqual(ui.guideRequests,[]);
  },{verdict:'needs_revision',deferReview:true});
});

test('late pixel-ratio and geometry changes are rejected even without a resize callback',async()=>{
  const prior=Object.getOwnPropertyDescriptor(globalThis,'devicePixelRatio');
  try{
    for(const changed of ['pixel-ratio','geometry'])await mounted(async ui=>{
      await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
      assert.match(ui.text(),/Checking the visible animation/);
      if(changed==='pixel-ratio')Object.defineProperty(globalThis,'devicePixelRatio',{configurable:true,value:1});
      else scene.frame.getBoundingClientRect=()=>({width:700,height:400});
      await ui.releaseReview();
      assert.equal(ui.posts.length,1);assert.equal(ui.reviewPosts.length,1);
      assert.doesNotMatch(ui.text(),/Checking the visible animation|Sampled animation looks clear|needs a visual revision/);
      assert.ok(![...ui.timers.values()].some(value=>value.delay===650));
      await ui.review();await ui.until(()=>ui.reviewPosts.length===2);
      const frames=ui.reviewPosts[1].frames;
      assert.equal(frames[0].pixel_ratio,changed==='pixel-ratio'?1:2);
      assert.equal(frames[0].width,changed==='geometry'?700:600);
      assert.deepEqual(ui.reviewPosts[1].binding,ui.reviewPosts[0].binding);
      if(prior)Object.defineProperty(globalThis,'devicePixelRatio',prior);else delete globalThis.devicePixelRatio;
    },{verdict:'needs_revision',deferReview:true});
  }finally{if(prior)Object.defineProperty(globalThis,'devicePixelRatio',prior);else delete globalThis.devicePixelRatio;}
});

test('late identity, exact tool visibility and expanded states are reviewed without replacing the scene',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
    assert.equal(scene.options.model.nodes.filter(node=>node.kind==='agent').length,610);
    assert.equal(scene.options.model.nodes.find(node=>node.id==='specialist:609').name,'Specialist 609');
    await ui.inspect(scene,'specialist:609');assert.equal(ui.reviewPosts.length,1);
    await ui.review();await ui.until(()=>ui.reviewPosts.length===2);
    assert.equal(ui.reviewPosts[1].frames[0].selected_id,'specialist:609');
    assert.equal(ui.reviewPosts[1].frames[0].playing,false);
    await ui.click('Tool access');await ui.review();await ui.until(()=>ui.reviewPosts.length===3);
    assert.equal(ui.reviewPosts[2].frames[0].show_tools,true);
    assert.equal(ui.reviewPosts[2].frames[0].selected_id,'specialist:609');
    await ui.click('Expand');await ui.resize(scene,1040,680);await ui.review();await ui.until(()=>ui.reviewPosts.length===4);
    assert.equal(ui.reviewPosts[3].frames[0].width,1040);assert.equal(ui.reviewPosts[3].frames[0].height,680);
    assert.equal(ui.reviewPosts[3].frames[0].selected_id,'specialist:609');
    assert.ok(ui.dom.window.document.querySelector('.ps-expanded'));
    for(const review of ui.reviewPosts)assert.deepEqual(review.binding,ui.reviewPosts[0].binding);
    assert.deepEqual(ui.visible(),[scene]);assert.equal(ui.instances.length,1);assert.equal(ui.posts.length,1);
    assert.deepEqual(scene.seekCalls,[]);assert.equal(scene.selectedId,'specialist:609');assert.equal(scene.showPeers,true);
    assert.deepEqual(ui.guideRequests,[]);assert.equal(ui.dom.window.document.querySelector('[aria-label="Guide draft"]').value,'I am still typing');
  },{initialPlan:largePlan()});
});

test('pause, play and reduced motion recheck only their naturally visible presentation',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);
    for(let index=0;index<3;index++){
      await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===12000));await ui.tick(12000);
    }
    await ui.until(()=>ui.reviewPosts.length===1);
    await ui.click('Pause simulation');await ui.review();await ui.until(()=>ui.reviewPosts.length===2);
    assert.equal(ui.reviewPosts[1].frames.length,1);assert.equal(ui.reviewPosts[1].frames[0].playing,false);
    await ui.click('Play simulation');await ui.review();
    for(let index=0;index<3;index++){
      await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===12000));await ui.tick(12000);
    }
    await ui.until(()=>ui.reviewPosts.length===3);
    assert.equal(ui.reviewPosts[2].frames.length,4);assert.ok(ui.reviewPosts[2].frames.every(frame=>frame.playing));
    await ui.motion(true);await ui.review();await ui.until(()=>ui.reviewPosts.length===4);
    assert.equal(ui.reviewPosts[3].frames.length,1);assert.equal(ui.reviewPosts[3].frames[0].reduced_motion,true);
    assert.equal(ui.reviewPosts[3].frames[0].playing,false);
    await ui.motion(false);await ui.review();await ui.until(()=>ui.reviewPosts.length===5);
    assert.equal(ui.reviewPosts[4].frames[0].reduced_motion,false);assert.equal(scene.paused,true);
    assert.deepEqual(scene.seekCalls,[]);assert.deepEqual(ui.visible(),[scene]);assert.equal(ui.posts.length,1);assert.deepEqual(ui.guideRequests,[]);
  },{reduced:false});
});

test('resize and control bursts coalesce to one current review after settling',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
    const captures=scene.captureCount;
    await ui.resize(scene,390,300);await ui.resize(scene,420,310);await ui.click('Tool access');
    await ui.click('Expand');await ui.resize(scene,740,444);await ui.mount(structuredClone(ownedPlan()));
    assert.equal(scene.captureCount,captures);assert.equal(ui.reviewPosts.length,1);
    assert.equal([...ui.timers.values()].filter(timer=>timer.delay===300).length,1);
    await ui.review();await ui.until(()=>ui.reviewPosts.length===2);
    const frame=ui.reviewPosts[1].frames[0];assert.equal(frame.width,740);assert.equal(frame.height,444);assert.equal(frame.show_tools,true);
    assert.equal([...ui.timers.values()].filter(timer=>timer.delay===300).length,0);
    await ui.mount(structuredClone(ownedPlan()));assert.equal([...ui.timers.values()].filter(timer=>timer.delay===300).length,0);
    assert.equal(ui.instances.length,1);assert.equal(ui.posts.length,1);assert.deepEqual(ui.visible(),[scene]);assert.deepEqual(scene.seekCalls,[]);
  });
});

test('a delayed old presentation verdict cannot control its newly reviewed selection',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
    await ui.inspect(scene,'specialist:609');await ui.click('Tool access');await ui.review();await ui.until(()=>ui.reviewPosts.length===2);
    const old=ui.requests.find(request=>request.options.method==='POST' && request.url.endsWith('/review'));
    assert.equal(old.options.signal.aborted,true);
    await ui.releaseReview();assert.equal(ui.posts.length,1);assert.ok(![...ui.timers.values()].some(timer=>timer.delay===650));
    assert.doesNotMatch(ui.text(),/needs a visual revision/);
    await ui.releaseReview();await ui.until(()=>ui.text().includes('Sampled animation looks clear'));
    assert.equal(ui.reviewPosts[1].frames[0].selected_id,'specialist:609');assert.equal(ui.reviewPosts[1].frames[0].show_tools,true);
    assert.equal(ui.posts.length,1);assert.equal(ui.instances.length,1);assert.deepEqual(ui.visible(),[scene]);assert.deepEqual(ui.guideRequests,[]);
  },{initialPlan:largePlan(),deferReview:true,verdict:(_body,index)=>index===1?'needs_revision':'acceptable'});
});

test('explicit replay and chapter navigation schedule reviews even when control values stay unchanged',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
    await ui.click('Replay simulation');await ui.review();await ui.until(()=>ui.reviewPosts.length===2);
    assert.equal(ui.reviewPosts[1].frames[0].time,.05);assert.deepEqual(scene.seekCalls,[.05]);
    await ui.click('Next simulation step');await ui.review();await ui.until(()=>ui.reviewPosts.length===3);
    assert.equal(ui.reviewPosts[2].frames[0].time,0);assert.deepEqual(scene.seekCalls,[.05,0]);
    assert.equal(ui.posts.length,1);assert.equal(ui.instances.length,1);assert.deepEqual(ui.guideRequests,[]);
  });
});

test('new presentation checks retain the single automatic correction budget for the source',async()=>{
  await mounted(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.first(previous);await ui.until(()=>[...ui.timers.values()].some(timer=>timer.delay===650));
    await ui.generate();const corrected=ui.instances[1];await ui.first(corrected);await ui.until(()=>ui.reviewPosts.length===2);
    assert.equal(ui.posts.length,2);assert.ok(![...ui.timers.values()].some(timer=>timer.delay===650));
    await ui.click('Tool access');await ui.review();await ui.until(()=>ui.reviewPosts.length===3);
    assert.equal(ui.posts.length,2);assert.ok(![...ui.timers.values()].some(timer=>timer.delay===650));
    assert.deepEqual(ui.visible(),[corrected]);assert.deepEqual(corrected.seekCalls,[]);assert.deepEqual(ui.guideRequests,[]);
  },{verdict:'needs_revision'});
});

test('geometry discovered before capture schedules a replacement without a resize notification',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.settle(()=>scene.first());
    scene.frame.getBoundingClientRect=()=>({width:390,height:300});
    await ui.review();assert.equal(ui.reviewPosts.length,0);assert.equal(scene.captureCount,undefined);
    assert.equal([...ui.timers.values()].filter(timer=>timer.delay===300).length,1);
    await ui.review();await ui.until(()=>ui.reviewPosts.length===1);
    assert.equal(ui.reviewPosts[0].frames[0].width,390);assert.equal(ui.reviewPosts[0].frames[0].height,300);
    assert.equal([...ui.timers.values()].filter(timer=>timer.delay===300).length,0);
    assert.equal(ui.posts.length,1);assert.equal(ui.instances.length,1);assert.deepEqual(ui.visible(),[scene]);
  });
});

test('source and simulation-view changes retire delayed review evidence and pending presentation checks',async()=>{
  const initialPlan={...ownedPlan(),flows:[{id:'job',name:'Owned job',description:'One bounded request',steps:[{id:'answer',agent:'lead:main',prompt:'Return owned evidence.',depends_on:[]}]}]};
  for(const change of ['source','view'])await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
    await ui.click('Tool access');
    assert.equal([...ui.timers.values()].filter(timer=>timer.delay===300).length,1);
    if(change==='source')await ui.mount({...initialPlan,project:{...initialPlan.project,description:'A changed factual proposal.'}});
    else await ui.settle(()=>{const select=ui.dom.window.document.querySelector('[aria-label="View to simulate"]');select.value='job';select.dispatchEvent(new ui.dom.window.Event('change',{bubbles:true}));});
    await ui.releaseReview();assert.equal(ui.reviewPosts.length,1);assert.equal(ui.posts.length,1);
    assert.equal([...ui.timers.values()].filter(timer=>timer.delay===300).length,0);
    assert.doesNotMatch(ui.text(),/Sampled animation looks clear|needs a visual revision/);
    if(change==='source'){assert.deepEqual(ui.visible(),[scene]);assert.equal(ui.dom.window.document.querySelector('.proposal-simulation').dataset.updating,'true');}
    else assert.deepEqual(ui.visible(),[]);
    assert.deepEqual(ui.guideRequests,[]);
  },{initialPlan,deferReview:true,verdict:'needs_revision'});
});

test('an expired consumer intent cannot start or resurrect a pending presentation review',async()=>{
  await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.reviewPosts.length===1);
    await ui.resize(scene,740,444);const captures=scene.captureCount;
    await ui.tick(90000);await ui.review();await ui.releaseReview();
    assert.equal(scene.captureCount,captures);assert.equal(ui.reviewPosts.length,1);assert.equal(ui.posts.length,1);
    assert.doesNotMatch(ui.text(),/Sampled animation looks clear|needs a visual revision/);
    assert.deepEqual(ui.visible(),[scene]);assert.deepEqual(scene.seekCalls,[]);assert.deepEqual(ui.guideRequests,[]);
    const releases=ui.requests.filter(request=>request.url==='/api/visualization-intents/release');assert.equal(releases.length,1);
    assert.equal(JSON.parse(releases[0].options.body).intent_revision,JSON.parse(ui.requests.find(request=>request.url==='/api/visualization-intents/declare').options.body).intent_revision);
  },{deferReview:true,verdict:'needs_revision'});
});

test('mounted inconclusive capability or result preserves working scene without generation loops',async()=>{
  for(const config of [{capability:false},{verdict:'inconclusive'}])await mounted(async ui=>{
    await ui.generate();const scene=ui.instances[0];await ui.first(scene);await ui.until(()=>ui.text().includes(config.capability===false?'Automatic animation check unavailable':'Animation quality not confirmed'));
    assert.deepEqual(ui.visible(),[scene]);assert.equal(ui.posts.length,1);assert.equal([...ui.timers.values()].some(timer=>timer.delay===650),false);
    assert.deepEqual(ui.guideRequests,[]);
  },config);
});
