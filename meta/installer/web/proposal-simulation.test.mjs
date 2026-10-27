import { setTimeout as realDelay } from 'node:timers/promises';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createRequire } from 'node:module';
import React, { act } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { fingerprint } from './state.mjs';
import { readAnimationRecovery, readAnimationSelection } from './animation-recovery.mjs';
import { IDENTITY_PAGE_SIZE } from './population-inspection.mjs';
import { animationFailureDetail } from './freeform-animation.mjs';
import { animationFailure, animationRequestMatches, animationGenerationStatus,
  pendingAnimationNotice, hasPublishedAnimation, shouldObserveAnimationTail } from './animation-publication.mjs';

const bundled=await build({entryPoints:[fileURLToPath(new URL('proposal-simulation.jsx',import.meta.url))],
  bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent',loader:{'.css':'empty'},
  nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
  alias:{'@console-core':fileURLToPath(new URL('../../packages/console-core/src',import.meta.url))}});
const { JSDOM }=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
const {AnimationGenerationStatus,AnimationActivityView,ProposalChangeNotice,expandedAnimationFocusables,trapExpandedAnimationFocus}=await import(
  `data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
const evolvingBundle=await build({entryPoints:[fileURLToPath(new URL('evolving-plan.jsx',import.meta.url))],
  bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent',loader:{'.css':'empty'},
  nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
  alias:{'@console-core':fileURLToPath(new URL('../../packages/console-core/src',import.meta.url))}});
const {EvolvingPlan}=await import(
  `data:text/javascript;base64,${Buffer.from(evolvingBundle.outputFiles[0].text).toString('base64')}`);
const request={streamId:'guide:workflow',proposalKey:'exact proposal and selected workflow',attempt:0};
const render=props=>renderToStaticMarkup(React.createElement(AnimationGenerationStatus,{request,...props}));

test('concrete proposal changes keep the previous scene explicit and collapse additional changes',()=>{
  const changes=['Weather specialist added - 1 starting agent proposed.',
    'Observing planners: 1 to 4 starting agents proposed.','Telescope executor: instructions updated.'];
  const html=renderToStaticMarkup(React.createElement(ProposalChangeNotice,{changes,working:true}));
  const dom=new JSDOM(html);
  try{
    const {document}=dom.window,notice=document.querySelector('section');
    assert.equal(notice.getAttribute('aria-label'),'Changes to this proposal');
    assert.equal(notice.querySelector('h4').textContent,'Changes being illustrated');
    assert.match(notice.querySelector('p').textContent,/previous proposal.*not yet shown/);
    assert.deepEqual([...notice.querySelectorAll('div>ul>li')].map(li=>li.textContent),changes.slice(0,2));
    assert.equal(notice.querySelector('details').open,false);
    assert.equal(notice.querySelector('summary').textContent,'1 more change');
    assert.equal(notice.querySelector('details li').textContent,changes[2]);
    assert.equal(notice.querySelectorAll('button,[role="status"],[aria-live]').length,0);
    assert.equal(renderToStaticMarkup(React.createElement(ProposalChangeNotice,{changes:[],working:true})), '');
    const waiting=renderToStaticMarkup(React.createElement(ProposalChangeNotice,{changes:changes.slice(0,2)}));
    assert.match(waiting,/Changes awaiting illustration/);
    assert.doesNotMatch(waiting,/being illustrated|<details/);
  }finally{dom.window.close();}
});

function focusDocument(html){
  const dom=new JSDOM(html);
  // JSDOM has no layout. Only geometry is supplied; native DOM semantics and
  // computed visibility still determine the production helper's candidates.
  dom.window.HTMLElement.prototype.getClientRects=function(){return this.style.display==='none'?[]:[{}];};
  return dom;
}

test('expanded regeneration keeps actual activity/source summaries after Explore identities and wraps only at the real boundary',()=>{
  const activity=renderToStaticMarkup(React.createElement(AnimationActivityView,{scope:{id:'owned-operation'},expanded:false,
    page:{status:'running',native:{admission_confirmed:true,status:'unknown'},publication:{available:false}}}));
  const dom=focusDocument(`<section><button id="close">Close expanded view</button><select id="view"><option>Current view</option></select>
    <button disabled>Refine animation</button><button id="explore">Explore identities</button>${activity}
    <details class="ps-code"><summary>Animation source</summary><a href="#hidden">Hidden source link</a></details></section>`);
  try{
    const {document}=dom.window,root=document.querySelector('section');
    const activitySummary=root.querySelector('.ps-agent-activity > summary'),sourceSummary=root.querySelector('.ps-code > summary');
    const stops=expandedAnimationFocusables(root);
    assert.deepEqual(stops,[document.getElementById('close'),document.getElementById('view'),document.getElementById('explore'),activitySummary,sourceSummary]);
    document.getElementById('explore').focus();
    const middle=new dom.window.KeyboardEvent('keydown',{key:'Tab',cancelable:true});
    assert.equal(trapExpandedAnimationFocus(middle,root),false);
    assert.equal(middle.defaultPrevented,false);
    sourceSummary.focus();
    const forward=new dom.window.KeyboardEvent('keydown',{key:'Tab',cancelable:true});
    assert.equal(trapExpandedAnimationFocus(forward,root),true);
    assert.equal(forward.defaultPrevented,true);
    assert.equal(document.activeElement === document.getElementById('close'), true);
    assert.equal(trapExpandedAnimationFocus(new dom.window.KeyboardEvent('keydown',{key:'Tab',shiftKey:true,cancelable:true}),root),true);
    assert.equal(document.activeElement === sourceSummary, true);
    assert.equal(trapExpandedAnimationFocus(new dom.window.KeyboardEvent('keydown',{key:'Escape',cancelable:true}),root),false);
  }finally{dom.window.close();}
});

test('expanded focus includes visible editable, link and frame controls in DOM order but excludes unavailable keyboard stops',()=>{
  const dom=focusDocument(`<section><button id="close">Close</button><a id="link" href="#details">Details</a>
    <textarea id="draft">Unsent refinement</textarea><iframe id="scene" title="Scene"></iframe><div id="editable" contenteditable>Notes</div>
    <input type="hidden"><button hidden>Hidden</button><button style="visibility:hidden">Invisible</button><button tabindex="-1">Programmatic</button>
    <div inert><button>Inactive</button></div><fieldset disabled><input><textarea></textarea></fieldset>
    <details><summary id="summary">More</summary><summary>Non-primary summary</summary><textarea>Closed</textarea></details>
    <input id="radio-unchecked" name="mode" type="radio"><input id="radio-checked" name="mode" type="radio" checked>
    <button id="send">Send refinement</button></section>`);
  try{
    const {document}=dom.window,root=document.querySelector('section');
    assert.deepEqual(expandedAnimationFocusables(root).map(node=>node.id),['close','link','draft','scene','editable','summary','radio-checked','send']);
    document.getElementById('send').focus();
    assert.equal(trapExpandedAnimationFocus(new dom.window.KeyboardEvent('keydown',{key:'Tab',cancelable:true}),root),true);
    assert.equal(document.activeElement.id,'close');
    document.getElementById('draft').focus();
    assert.equal(trapExpandedAnimationFocus(new dom.window.KeyboardEvent('keydown',{key:'Tab',cancelable:true}),root),false);
    assert.equal(document.getElementById('draft').value,'Unsent refinement');
  }finally{dom.window.close();}
});

test('the production plan component carries its current saved brief and interface fact without changing guide state',()=>{
  const fact={status:'confirmed',summary:'Personal browser accounts use the everyday application; native Console is separate administration.',
    source_message_ids:['actual-guide-answer']};
  const state={id:'current-guide',version:9,brief:{facts:{interfaces:fact,outcome:{status:'confirmed',summary:'Keep domain state private.',source_message_ids:['other-answer']}}}};
  const before=structuredClone(state);
  const findSimulation=node=>{
    if(!node || typeof node!=='object')return null;
    if(node.type?.name==='ProposalSimulation')return node;
    return React.Children.toArray(node.props?.children).map(findSimulation).find(Boolean) || null;
  };
  const simulation=findSimulation(EvolvingPlan({state}));
  assert.ok(simulation);
  assert.equal(simulation.props.setupId,state.id);
  assert.equal(simulation.props.interfaceFact,fact);
  assert.equal(simulation.props.designBrief,state.brief);
  assert.deepEqual(state,before);
  const nextFact={...fact,summary:'Use native Console directly for ordinary conversation.',source_message_ids:['new-guide-answer']};
  assert.equal(findSimulation(EvolvingPlan({state:{...state,brief:{facts:{interfaces:nextFact}}}})).props.interfaceFact,nextFact);
  const changedBrief={...state.brief,facts:{...state.brief.facts,outcome:{...state.brief.facts.outcome,summary:'A newly clarified outcome.'}}};
  assert.equal(findSimulation(EvolvingPlan({state:{...state,brief:changedBrief}})).props.designBrief,changedBrief);
  assert.equal(findSimulation(EvolvingPlan({state:{id:'another-guide',version:1}})).props.interfaceFact,undefined);
  assert.equal(findSimulation(EvolvingPlan({state:{id:'another-guide',version:1}})).props.designBrief,undefined);
});

test('failed same-proposal refinement visibly identifies the retained authored scene and retries only on a click',()=>{
  let retries=0;
  const failure=animationFailure(request,'failed'),before=structuredClone(failure);
  const props={request,generation:'fallback',failure,authored:true,updating:false,onRetry:()=>retries++};
  const html=render(props);
  assert.match(html,/role="status" aria-live="polite" aria-atomic="true"/);
  assert.match(html,/could not finish this scene/);
  assert.match(html,/last working animation is still shown/);
  assert.match(html,/No replacement was applied/);
  assert.doesNotMatch(html,/Scene composed|automatic network preview|title=/);
  assert.equal(retries,0);
  const element=AnimationGenerationStatus(props);
  const retry=element.props.children.find(child=>child?.type==='button');
  assert.equal(retry.props.type,'button');
  retry.props.onClick();
  assert.equal(retries,1);
  assert.deepEqual(failure,before);
});

test('a failed updated proposal labels the prior scene as missing the latest changes',()=>{
  const html=render({generation:'fallback',failure:animationFailure(request,'interrupted'),authored:true,updating:true});
  assert.match(html,/update was interrupted/);
  assert.match(html,/previous animation is still shown/);
  assert.match(html,/does not include the latest proposal changes/);
  assert.match(html,/Retry agent/);
  assert.doesNotMatch(html,/Scene composed|automatic network preview/);
});

test('a runtime rollback does not claim its displayed replacement was never applied',()=>{
  for(const updating of [false,true]){
    const html=render({generation:'fallback',failure:animationFailure(request,'execution'),authored:true,
      updating,restored:true});
    assert.match(html,/replacement stopped while running/);
    assert.match(html,/previous.*animation is shown/);
    assert.doesNotMatch(html,/No replacement was applied|current animation is still shown/);
    if(updating)assert.match(html,/does not include the latest proposal changes/);
  }
});

test('initial failure and exhausted canvas repair truthfully identify automatic fallback',()=>{
  for(const kind of ['failed','execution']){
    const html=render({generation:'fallback',failure:animationFailure(request,kind),authored:false});
    assert.match(html,/automatic network preview is shown/);
    assert.match(html,/Retry agent/);
    assert.doesNotMatch(html,/last working animation|previous animation|Scene composed/);
  }
});

test('observation unavailability shows fixed bounded copy rather than claiming native failure or exposing exceptions',()=>{
  const raw='private path and provider exception';
  const failure=animationFailure(request,raw);
  assert.equal(failure.kind,'unavailable');
  const html=render({generation:'fallback',failure,authored:true});
  assert.match(html,/could not be checked/);
  assert.match(html,/native status is unknown/);
  assert.doesNotMatch(html,/could not finish|private path|provider exception/);
});

test('ongoing same-handle observation and canvas self-repair remain visible pending states without Retry',()=>{
  const receipt={status:'observing',observation_status:'reconnecting',native_run_status:'unknown'};
  const html=render({generation:'working',notice:pendingAnimationNotice(receipt),authored:true,
    failure:animationFailure(request,'failed')});
  assert.match(html,/Still composing.*Reconnecting to the same animation request/);
  assert.match(html,/previous scene remains visible/);
  assert.doesNotMatch(html,/Retry agent|could not finish|has-error/);
  assert.equal(receipt.native_run_status,'unknown');
  const repairing=render({generation:'working',authored:true,
    notice:'The custom animation could not run. Asking the animation agent to repair this scene.'});
  assert.match(repairing,/Asking the animation agent to repair/);
  assert.match(repairing,/previous scene remains visible/);
  assert.doesNotMatch(repairing,/Retry agent|has-error/);
});

test('scope and retry ownership reject stale errors before they reach the current visible status',()=>{
  const failure=animationFailure(request,'failed');
  for(const current of [{...request,streamId:'other-guide:workflow'},
    {...request,streamId:'guide:other-workflow'},{...request,proposalKey:'new proposal'},
    {...request,attempt:1}]){
    assert.equal(animationRequestMatches(failure.request,current),false);
    const html=render({request:current,generation:'fallback',failure,authored:true,updating:true});
    assert.match(html,/latest changes still need a scene/);
    assert.doesNotMatch(html,/could not finish|Retry agent|has-error/);
  }
  const retry={...request,attempt:1},retryFailure=animationFailure(retry,'failed');
  assert.match(render({request:retry,generation:'fallback',failure:retryFailure,authored:true}),/could not finish/);
  assert.doesNotMatch(render({request:retry,generation:'ready',failure:retryFailure,authored:true}),/could not finish|Retry agent/);
  assert.equal(animationGenerationStatus({request:retry,generation:'ready',failure:retryFailure,authored:true}).message,
    'Scene composed by your visualization agent');
});

test('a published program remains usable when its optional native-tail observation times out',()=>{
  const receipt={status:'published',observation_status:'timed_out',native_run_status:'unknown',artifact:{code:'return program;'}};
  assert.equal(hasPublishedAnimation(receipt),true);
  assert.equal(shouldObserveAnimationTail(receipt),false);
  const html=render({generation:'ready',authored:true});
  assert.match(html,/Scene composed by your visualization agent/);
  assert.doesNotMatch(html,/Retry agent|failure|could not|native.*completed/);
  assert.equal(receipt.native_run_status,'unknown');
});

test('a lost native-tail observation does not claim its already applied current animation was rejected',()=>{
  const html=render({generation:'fallback',failure:animationFailure(request,'unavailable'),authored:true,currentScene:true});
  assert.match(html,/native status is unknown/);
  assert.match(html,/current animation is still shown/);
  assert.doesNotMatch(html,/No replacement was applied|previous animation|automatic network preview|could not finish/);
});

test('failure records copy only their exact request and malformed attempt ownership never matches',()=>{
  const mutable={...request},failure=animationFailure(mutable,'failed');
  mutable.proposalKey='changed later';
  assert.equal(failure.request.proposalKey,request.proposalKey);
  assert.equal(animationRequestMatches(failure.request,request),true);
  for(const attempt of [-1,NaN,Infinity,'0',null,undefined])
    assert.equal(animationRequestMatches({...request,attempt},request),false);
  assert.equal(animationRequestMatches(null,request),false);
});

const ownedPlan=(id='earlier',count=1)=>({project:{name:'Owned animation fixture',slug:'animation-fixture'},
  agent_classes:[{id,name:`${id} owner`,description:`${id} domain context`,instructions:`${id} responsibilities`,
    instances:count,tools:{memory:true}}],topology:{mode:'manual',entrypoint:`${id}:${count===1?'main':1}`,edges:[]},
  connections:[],knowledge:{mode:'none'},flows:[]});

const inspectorPlan=(count=633)=>({...ownedPlan('desk'),
  agent_classes:[{...ownedPlan('desk').agent_classes[0],tools:{memory:true,workspace:true,notes:'write',workgraph:true,
    mcp:[{connection_id:'records',tool_names:['get_file']}]}},
    {...ownedPlan('specialist',count).agent_classes[0],tools:{memory:true,notes:'read',
      mcp:[{connection_id:'records',tool_names:['read_summary']}]}}],
  topology:{mode:'hub',entrypoint:'desk:main',edges:[]},
  connections:[{id:'records',name:'Source records',description:'Read selected records.',transport:'stdio'}]});

async function writeRelationshipSearch(ui,input,text){
  await ui.settle(()=>{
    Object.getOwnPropertyDescriptor(ui.dom.window.HTMLInputElement.prototype,'value').set.call(input,text);
    input.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
  });
}

async function mountedAnimationRecovery(verify,{visualReview=false}={}) {
  const require=createRequire(import.meta.url),instances=[],posts=[],timers=new Map(),receipts=new Map(),reads=[];
  const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/',pretendToBeVisual:true});
  const guideRequests=[],resizeObservers=[];
  let timerId=0,receiptId=0,latestPlan=ownedPlan(),latestSetup='owned-recovery',latestBrief,latestFeedback,resolveReview;
  let nextStatus='completed',holdRecovery=false;
  const recoveryResponses=[];
  const nativeCrypto=globalThis.crypto,recoveryDigests=[];
  let holdRecoveryValidation=false;
  const releaseRecoveryValidation=()=>{holdRecoveryValidation=false;recoveryDigests.splice(0).forEach(resolve=>resolve());};
  const observedSubtle=new Proxy(nativeCrypto.subtle,{get(target,name){
    if(name==='digest')return async(...args)=>{
      const bytes=await nativeCrypto.subtle.digest(...args);
      if(holdRecoveryValidation)await new Promise(resolve=>recoveryDigests.push(resolve));
      return bytes;
    };
    const value=Reflect.get(target,name,target);return typeof value==='function'?value.bind(target):value;
  }});
  const observedCrypto=new Proxy(nativeCrypto,{get(target,name){
    if(name==='subtle')return observedSubtle;
    const value=Reflect.get(target,name,target);return typeof value==='function'?value.bind(target):value;
  }});
  const recoveryReads=()=>reads.filter(target=>/^\/api\/visualizations\/[a-f0-9]{64}$/.test(target));
  const reviewPosts=[];
  const adoptionPosts=[];
  let latestSaved=null,latestReadStatus=null,allowForeignLatest=false;
  const rendererHash=(await import('./animation-review-renderer-hash.mjs')).ANIMATION_RENDERER_HASH;
  const replacements={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,sessionStorage:dom.window.sessionStorage,crypto:observedCrypto,
    HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true,__ownedAnimationInstances:instances,
    matchMedia:()=>({matches:false,addEventListener(){},removeEventListener(){}}),
    ResizeObserver:class {
      constructor(callback){this.callback=callback;resizeObservers.push(this);}
      observe(target){this.target=target;}disconnect(){this.target=null;}
    },
    setTimeout:(callback,delay)=>{const id=++timerId;timers.set(id,{callback,delay});return id;},
    clearTimeout:id=>timers.delete(id),
    fetch:async(target,options={})=>{
      if(target.startsWith('/api/visualization-intents/')){
        const body=JSON.parse(options.body);return {ok:true,json:async()=>({...body,status:target.endsWith('/release')?'released':'live',
          intent_token:`00000000-0000-4000-8000-${String(body.intent_revision).padStart(12,'0')}`,lease_seconds:90})};
      }
      if(target.startsWith('/api/visualizations/published/latest?')){
        reads.push(target);
        const query=new URL(target,'http://localhost/').searchParams;
        const matches=latestSaved && (allowForeignLatest || ['setup_id','stream_id','blueprint_hash','model_hash']
          .every(field=>query.get(field)===latestSaved[field]));
        const status=latestReadStatus ?? (matches?200:404);
        return {ok:status===200,status,json:async()=>structuredClone(status===200?latestSaved:{} )};
      }
      if(target==='/api/visualizations/published/adopt'){
        const body=JSON.parse(options.body);adoptionPosts.push(body);
        assert.equal(body.publication.operation_id,latestSaved.id);
        assert.equal(body.publication.code_hash,latestSaved.artifact.code_hash);
        return {ok:true,json:async()=>structuredClone(latestSaved)};
      }
      if(visualReview && target==='/api/visualization-review-capability')return {ok:true,json:async()=>({available:true,renderer_hash:rendererHash})};
      if(visualReview && target.endsWith('/review') && options.method==='POST'){
        const body=JSON.parse(options.body);reviewPosts.push(body);
        return {ok:true,json:()=>new Promise(resolve=>{resolveReview=(verdict='needs_revision')=>resolve({id:'9'.repeat(64),status:'reviewed',current:true,
          binding:body.binding,renderer_hash:rendererHash,report:{verdict,summary:'A synthetic fixture was reviewed'},
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
      const saved={id,status:nextStatus,source_generation:receiptId,native_admission,revision_binding,
        ...(nextStatus==='published'?{native_run_status:'unknown',observation_status:'observing'}:{}),
        setup_id:body.setup_id,stream_id:body.stream_id,request_fingerprint,blueprint_hash,model_hash,
        native_request_key:'e'.repeat(64),artifact:{key:'e'.repeat(64),contract:'freeform-animation-v3',
          blueprint_hash,model_hash,reference_hash:'d'.repeat(64),code_hash,code,native_admission,revision_binding,publication_receipt}};
      receipts.set(id,saved);
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
            this.frame.getBoundingClientRect=()=>({width:600,height:340});
            globalThis.__ownedAnimationInstances.push(this);}
          seek(time){this.time=time;this.options.onFrame?.(time,30);}
          async capture(){const {width,height}=this.frame.getBoundingClientRect(),bytes=new ArrayBuffer(40),prefix=new Uint8Array(bytes),view=new DataView(bytes);
            prefix.set([137,80,78,71,13,10,26,10]);view.setUint32(8,13);prefix.set([73,72,68,82],12);view.setUint32(16,width);view.setUint32(20,height);
            return {frameId:++this.captureId,time:this.time,progress:this.time/this.duration,playing:!this.paused,
              reducedMotion:this.reducedMotion,selectedId:this.selectedId,showTools:this.showPeers,width,height,ratio:1,mime:'image/png',bytes};}
          cancelCapture(){this.cancelledCapture=true;}
          destroy(){this.alive=false;this.destroyed=true;}
          first(){this.options.onReady({duration:30,chapters:[{at:0,title:this.options.code,description:'Source-bound chapter'}]});
            this.options.onCaption({title:this.options.code,description:'Source-bound caption'});this.options.onFirstFrame();}
          fail(failure){this.destroy();this.options.onError(failure);}
        }`,loader:'js'}));
    }}]});
  const {ProposalSimulation:MountedSimulation,OwnerRelationships:MountedRelationships}=await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
  const {createRoot}=await import('react-dom/client');
  let root=createRoot(dom.window.document.getElementById('root'));
  const settle=callback=>act(async()=>{await callback?.();await new Promise(resolve=>setImmediate(resolve));});
  const mount=(plan=latestPlan,setup=latestSetup,activity=null,designBrief=latestBrief,visualizationFeedback=latestFeedback)=>settle(()=>{
    latestPlan=plan;latestSetup=setup;latestBrief=designBrief;latestFeedback=visualizationFeedback;root.render(React.createElement(MountedSimulation,
      {plan,setupId:setup,rehearsalActivity:activity,designBrief,visualizationFeedback,interfaceFact:designBrief?.facts?.interfaces,
        onRequestChange:message=>guideRequests.push(message)}));
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
        await realDelay(1);
      }
    });
    return entry;
  };
  const waitForReview=async count=>settle(async()=>{
    // Wait for the final frame hash to reach the actual POST transport, using
    // the same bounded event-loop wait as sample readiness above.
    for(const deadline=performance.now()+5000;performance.now()<deadline && reviewPosts.length<count;)
      await realDelay(1);
  });
  const waitForAdoption=async count=>settle(async()=>{
    // Hash verification completes on native crypto workers, independently of
    // how many setImmediate turns a busy runner can execute.
    for(const deadline=performance.now()+5000;performance.now()<deadline && adoptionPosts.length<count;)
      await realDelay(1);
    assert.equal(adoptionPosts.length,count,'Checked saved-program adoption reaches the actual transport');
  });
  const visible=()=>instances.filter(instance=>instance.alive && instance.layer.style.visibility==='visible');
  try {
    await mount();
    await verify({dom,instances,posts,reviewPosts,adoptionPosts,timers,receipts,reads,recoveryReads,mount,generate,settle,waitForTimer,waitForReview,waitForAdoption,visible,guideRequests,resolveReview:verdict=>resolveReview?.(verdict),
      mountRelationships:(selected,model,onInspect)=>settle(()=>root.render(React.createElement(MountedRelationships,{selected,model,onInspect}))),
      resize:(scene,width,height)=>settle(()=>{scene.frame.getBoundingClientRect=()=>({width,height});
        for(const observer of resizeObservers)if(observer.target===scene.frame)observer.callback([{contentRect:{width,height}}]);}),
      setLatest:(saved,{allowForeign=false}={})=>{latestSaved=structuredClone(saved);allowForeignLatest=allowForeign;},
      holdRecoveryValidation:value=>{holdRecoveryValidation=value;},heldRecoveryValidations:()=>recoveryDigests.length,releaseRecoveryValidation,
      latestReadStatus:value=>{latestReadStatus=value;},
      nextStatus:value=>{nextStatus=value;},holdRecovery:value=>{holdRecovery=value;},releaseRecovery:()=>recoveryResponses.splice(0).forEach(resolve=>resolve()),
      remount:async()=>{await settle(()=>root.unmount());root=createRoot(dom.window.document.getElementById('root'));await mount();},
      text:()=>dom.window.document.body.textContent,
      count:()=>dom.window.document.querySelector('.proposal-simulation').dataset.nodeCount});
  }finally{
    await settle(()=>{releaseRecoveryValidation();root.unmount();});dom.window.close();
    assert.ok(instances.every(instance=>!instance.alive),'Unmount releases active, retained and failed scene owners');
    for(const [key,descriptor] of previous){if(descriptor)Object.defineProperty(globalThis,key,descriptor);else delete globalThis[key];}
  }
}

async function newerSavedFixture(receipt){
  const saved=structuredClone(receipt);
  saved.id='7'.repeat(64);saved.request_fingerprint='8'.repeat(64);saved.source_generation+=1;
  saved.status='completed';
  saved.native_admission={interaction_id:'saved-native-2',cursor:'console:2'};
  saved.artifact.native_admission=structuredClone(saved.native_admission);
  saved.artifact.code='return approvedSavedProgram; // inert saved native fixture';
  saved.artifact.code_hash=[...new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(saved.artifact.code)))].map(value=>value.toString(16).padStart(2,'0')).join('');
  saved.artifact.publication_receipt.code_hash=saved.artifact.code_hash;
  return saved;
}

async function sampleRecoveryReview(ui,count){
  const timer=await ui.waitForTimer(300);assert.ok(timer,'The visible presentation schedules one settled review');
  ui.timers.delete(timer[0]);await ui.settle(()=>timer[1].callback());
  await ui.waitForReview(count);assert.equal(ui.reviewPosts.length,count);
}

async function clickRecoveryControl(ui,label){
  const button=[...ui.dom.window.document.querySelectorAll('button')]
    .find(item=>item.getAttribute('aria-label')===label || item.textContent===label);
  assert.ok(button,`The ${label} control is available`);await ui.settle(()=>button.click());
}

test('a default resumed publication receives actual-size observation-only reviews without authoring or pinning a selection',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const original=structuredClone([...ui.receipts.values()][0]);
    const saved=await newerSavedFixture(original);ui.setLatest(saved);ui.receipts.set(saved.id,saved);
    const frozen=structuredClone(saved),scope={setup_id:saved.setup_id,stream_id:saved.stream_id};
    ui.dom.window.sessionStorage.clear();await ui.remount();await ui.generate();
    const resumed=ui.instances.at(-1);await ui.settle(()=>resumed.first());
    await clickRecoveryControl(ui,'Pause simulation');await ui.resize(resumed,764,480);
    await sampleRecoveryReview(ui,1);
    const first=ui.reviewPosts[0];assert.equal(first.binding.id,saved.id);assert.equal(first.binding.code_hash,saved.artifact.code_hash);
    assert.equal(first.frames.length,1);assert.equal(first.frames[0].width,764);assert.equal(first.frames[0].height,480);
    await ui.settle(()=>ui.resolveReview('needs_revision'));
    assert.match(ui.text(),/The animation needs a visual revision/);
    assert.ok(![...ui.timers.values()].some(timer=>timer.delay===650),'An observation cannot queue an author correction');
    assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,1);assert.deepEqual(ui.visible(),[resumed]);
    assert.equal(resumed.options.code,saved.artifact.code);assert.equal(resumed.time,9);assert.equal(resumed.paused,true);
    assert.equal(readAnimationSelection(ui.dom.window.sessionStorage,scope),null,'Default resume does not create an explicit choice');
    await ui.resize(resumed,980,600);await sampleRecoveryReview(ui,2);
    assert.equal(ui.reviewPosts[1].frames[0].width,980);assert.equal(ui.reviewPosts[1].frames[0].height,600);
    await ui.settle(()=>ui.resolveReview('acceptable'));assert.match(ui.text(),/Sampled animation looks clear/);
    assert.deepEqual(ui.receipts.get(saved.id),frozen);assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,1);
    assert.deepEqual(ui.visible(),[resumed]);assert.deepEqual(ui.guideRequests,[]);
  },{visualReview:true});
});

test('an explicit saved-source review preserves the choice and leaves the automatic correction allowance for later user refinement',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const saved=await newerSavedFixture([...ui.receipts.values()][0]);ui.setLatest(saved);ui.receipts.set(saved.id,saved);
    const frozen=structuredClone(saved),scope={setup_id:saved.setup_id,stream_id:saved.stream_id};
    await clickRecoveryControl(ui,'Use latest saved animation');
    await ui.waitForAdoption(1);
    const selected=ui.instances.at(-1);await ui.settle(()=>selected.first());
    const choice=readAnimationSelection(ui.dom.window.sessionStorage,scope);assert.equal(choice.operation_id,saved.id);
    await clickRecoveryControl(ui,'Pause simulation');await ui.resize(selected,720,450);await sampleRecoveryReview(ui,1);
    assert.equal(ui.reviewPosts[0].binding.id,saved.id);assert.equal(ui.reviewPosts[0].frames[0].width,720);
    assert.equal(ui.reviewPosts[0].frames[0].height,450);await ui.settle(()=>ui.resolveReview('needs_revision'));
    assert.match(ui.text(),/The animation needs a visual revision/);assert.match(ui.text(),/Showing the saved animation you selected/);
    assert.ok(![...ui.timers.values()].some(timer=>timer.delay===650));assert.equal(ui.posts.length,1);
    assert.deepEqual(readAnimationSelection(ui.dom.window.sessionStorage,scope),choice);
    assert.deepEqual(ui.receipts.get(saved.id),frozen);assert.deepEqual(ui.visible(),[selected]);
    assert.equal(selected.options.code,saved.artifact.code);assert.equal(selected.time,9);assert.equal(selected.paused,true);

    await clickRecoveryControl(ui,'Refine animation');
    const textarea=ui.dom.window.document.querySelector('.ps-animation-feedback textarea');
    await ui.settle(()=>{
      Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(textarea,'Make the population legible');
      textarea.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
    });
    await ui.settle(()=>textarea.form.dispatchEvent(new ui.dom.window.Event('submit',{bubbles:true,cancelable:true})));
    await ui.generate();const refined=ui.instances.at(-1);await ui.settle(()=>refined.first());await sampleRecoveryReview(ui,2);
    const authored=structuredClone([...ui.receipts.values()].at(-1));
    await ui.settle(()=>ui.resolveReview('needs_revision'));
    assert.ok([...ui.timers.values()].some(timer=>timer.delay===650),'The saved-source observation did not spend the correction allowance');
    await ui.generate();assert.equal(ui.posts.length,3);assert.equal(ui.posts[1].feedback,'Make the population legible');
    assert.equal(ui.posts[2].expected_parent.operation_id,authored.id);
    assert.equal(ui.posts[2].expected_parent.code_hash,authored.artifact.code_hash);assert.deepEqual(ui.guideRequests,[]);
  },{visualReview:true});
});

test('a fresh viewer resumes the latest checked current publication without authoring or pinning an explicit choice',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const original=structuredClone([...ui.receipts.values()][0]);
    const saved=await newerSavedFixture(original);ui.setLatest(saved);ui.receipts.set(saved.id,saved);
    ui.dom.window.sessionStorage.clear();
    await ui.remount();await ui.generate();
    assert.equal(ui.posts.length,1,'A new viewer must not replay the old empty-feedback generation request');
    assert.equal(ui.adoptionPosts.length,1);
    assert.equal(ui.adoptionPosts[0].publication.operation_id,saved.id);
    assert.equal(ui.adoptionPosts[0].require_latest,undefined,'The fresh intent requires the latest publication by default');
    const resumed=ui.instances.at(-1);assert.equal(resumed.options.code,saved.artifact.code);
    await ui.settle(()=>resumed.first());
    assert.deepEqual(ui.visible(),[resumed]);assert.match(ui.text(),/Resumed the latest saved animation/);
    assert.doesNotMatch(ui.text(),/saved animation you selected/);
    const scope={setup_id:saved.setup_id,stream_id:saved.stream_id};
    assert.equal(readAnimationSelection(ui.dom.window.sessionStorage,scope),null);
    assert.equal(readAnimationRecovery(ui.dom.window.sessionStorage,scope).operation_id,saved.id);
    assert.deepEqual(ui.receipts.get(original.id),original,'Default resume leaves the older native receipt intact');
    const newer=await newerSavedFixture(saved);newer.id='6'.repeat(64);
    ui.setLatest(newer);ui.receipts.set(newer.id,newer);
    await ui.remount();await ui.generate();
    assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,2);
    assert.equal(ui.adoptionPosts[1].publication.operation_id,newer.id,'A later visit checks current publication rather than pinning an automatic default');
    assert.notEqual(ui.adoptionPosts[1].consumer_intent.consumer_id,ui.adoptionPosts[0].consumer_intent.consumer_id);
    await ui.settle(()=>ui.instances.at(-1).first());
    assert.equal(readAnimationSelection(ui.dom.window.sessionStorage,scope),null);
    await ui.mount(ownedPlan('changed',2));await ui.generate();
    assert.equal(ui.posts.length,2,'Changed proposal inputs require their own author request');
    assert.equal(ui.adoptionPosts.length,2,'The previous publication cannot attach to changed facts');
  });
});

test('an absent latest publication admits ordinary generation but an unavailable read admits no author work',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,0);
    assert.ok(ui.reads.some(target=>target.startsWith('/api/visualizations/published/latest?')));
    await ui.settle(()=>ui.instances[0].first());
    ui.dom.window.sessionStorage.clear();ui.latestReadStatus(503);
    await ui.remount();await ui.generate();
    assert.equal(ui.posts.length,1,'An unavailable current-source read cannot authorize another author request');
    assert.equal(ui.adoptionPosts.length,0);assert.match(ui.text(),/could not be checked.*native status is unknown/);
    assert.match(ui.text(),/Check again/);assert.doesNotMatch(ui.text(),/agent failed/);
    const before=ui.reads.filter(target=>target.startsWith('/api/visualizations/published/latest?')).length;
    await ui.settle(()=>[...ui.dom.window.document.querySelectorAll('button')].find(button=>button.textContent==='Check again').click());
    await ui.generate();assert.equal(ui.posts.length,1);
    assert.equal(ui.reads.filter(target=>target.startsWith('/api/visualizations/published/latest?')).length,before+1,
      'Check again resolves the unknown publication read rather than admitting ordinary generation');
    const saved=await newerSavedFixture([...ui.receipts.values()][0]);ui.setLatest(saved);ui.latestReadStatus(null);
    await ui.settle(()=>[...ui.dom.window.document.querySelectorAll('button')].find(button=>button.textContent==='Check again').click());
    await ui.generate();assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,1);
    await ui.settle(()=>ui.instances.at(-1).first());assert.match(ui.text(),/Resumed the latest saved animation/);
  });
});

test('a successful latest read with a falsy payload never establishes absence or author permission',async()=>{
  await mountedAnimationRecovery(async ui=>{
    ui.latestReadStatus(200);
    for(const payload of [null,false,'']){
      ui.setLatest(payload);ui.dom.window.sessionStorage.clear();await ui.remount();await ui.generate();
      assert.equal(ui.posts.length,0);assert.equal(ui.adoptionPosts.length,0);
      assert.match(ui.text(),/could not be checked/);assert.doesNotMatch(ui.text(),/Resumed the latest saved animation/);
    }
  });
});

test('a malformed current publication cannot attach ownership or silently author a replacement',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const saved=await newerSavedFixture([...ui.receipts.values()][0]);
    saved.artifact.code+=' unacknowledged change';ui.setLatest(saved);
    ui.dom.window.sessionStorage.clear();await ui.remount();await ui.generate();
    assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,0);
    assert.match(ui.text(),/could not be checked.*native status is unknown/);
    assert.doesNotMatch(ui.text(),/Resumed the latest saved animation/);
  });
});

test('a stale browser explicitly selects the checked latest saved program without regenerating or replacing its draft',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const button=text=>[...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent===text);
    ui.nextStatus('published');
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    const oldTail=[...ui.timers.values()].find(timer=>timer.delay===1800);assert.ok(oldTail);
    const saved=await newerSavedFixture([...ui.receipts.values()][0]);ui.setLatest(saved);
    const stale=structuredClone([...ui.receipts.values()][0]);
    await ui.settle(()=>button('Refine animation').click());
    const textarea=ui.dom.window.document.querySelector('.ps-animation-feedback textarea');
    await ui.settle(()=>{
      Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(textarea,'Make the review handoff clearer');
      textarea.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
    });
    assert.equal(button('Use latest saved animation').disabled,true,'Active editing owns its view');
    await ui.settle(()=>button('Refine animation').click());
    ui.holdRecoveryValidation(true);
    await ui.settle(()=>button('Use latest saved animation').click());
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.heldRecoveryValidations()<2;){await ui.settle();await realDelay(1);}
    assert.equal(ui.heldRecoveryValidations(),2,'Both real request hashes finish while their verification is held');
    assert.equal(ui.adoptionPosts.length,0,'An unfinished verification cannot adopt the publication');
    ui.releaseRecoveryValidation();
    await ui.waitForAdoption(1);
    assert.equal(ui.posts.length,1,'Selecting a publication does not call the generation route');
    assert.equal(ui.adoptionPosts.length,1);
    assert.equal(ui.adoptionPosts[0].consumer_intent.intent_revision,ui.posts[0].consumer_intent.intent_revision+1);
    assert.equal(ui.adoptionPosts[0].consumer_intent.consumer_id,ui.posts[0].consumer_intent.consumer_id);
    assert.equal(ui.adoptionPosts[0].publication.operation_id,saved.id);
    assert.equal(ui.adoptionPosts[0].publication.code_hash,saved.artifact.code_hash);
    const query=new URL(ui.reads.find(value=>value.includes('/published/latest?')),'http://localhost/').searchParams;
    assert.equal(query.get('blueprint_hash'),saved.blueprint_hash);assert.equal(query.get('model_hash'),saved.model_hash);
    const replacement=ui.instances[1];assert.equal(replacement.options.code,saved.artifact.code);
    assert.deepEqual(ui.visible(),[previous],'The prior drawing stays visible until first frame');
    await ui.settle(()=>replacement.first());assert.deepEqual(ui.visible(),[replacement]);
    assert.match(ui.text(),/Showing the saved animation you selected/);
    assert.deepEqual([...ui.receipts.values()][0],stale,'The browser does not rewrite the earlier receipt');
    await ui.settle(()=>oldTail.callback());
    assert.deepEqual(ui.visible(),[replacement],'The stopped older native-tail observer cannot overwrite the selected source');
    ui.receipts.set(saved.id,structuredClone(saved));
    await ui.remount();await ui.generate();
    assert.equal(ui.posts.length,1,'Reload restores the explicit choice without generation or synthetic feedback');
    assert.equal(ui.adoptionPosts.length,2);assert.equal(ui.adoptionPosts[1].require_latest,false);
    assert.notEqual(ui.adoptionPosts[1].consumer_intent.consumer_id,ui.adoptionPosts[0].consumer_intent.consumer_id);
    const restored=ui.instances.at(-1);assert.equal(restored.options.code,saved.artifact.code);
    await ui.settle(()=>restored.first());assert.deepEqual(ui.visible(),[restored]);
    await ui.settle(()=>oldTail.callback());assert.deepEqual(ui.visible(),[restored]);
    await ui.settle(()=>button('Refine animation').click());
    const retained=ui.dom.window.document.querySelector('.ps-animation-feedback textarea');
    assert.equal(retained.value,'Make the review handoff clearer');
    await ui.settle(()=>retained.form.dispatchEvent(new ui.dom.window.Event('submit',{bubbles:true,cancelable:true})));
    await ui.generate();assert.equal(ui.posts[1].feedback,'Make the review handoff clearer');
  },{visualReview:true});
});

test('an exact selected scene survives remount while its acknowledged visual refinement fails before native admission',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const button=text=>[...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent===text);
    const failPending=async()=>{
      const receipt=[...ui.receipts.values()].at(-1);
      receipt.status='failed';delete receipt.artifact;delete receipt.native_admission;
      const timer=await ui.waitForTimer(1800);assert.ok(timer);
      await ui.settle(()=>timer[1].callback());
    };
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const saved=await newerSavedFixture([...ui.receipts.values()][0]);ui.setLatest(saved);ui.receipts.set(saved.id,saved);
    await ui.settle(()=>button('Use latest saved animation').click());
    await ui.waitForAdoption(1);
    const selected=ui.instances.at(-1);await ui.settle(()=>selected.first());
    const scope={setup_id:saved.setup_id,stream_id:saved.stream_id};
    assert.equal(readAnimationSelection(ui.dom.window.sessionStorage,scope).operation_id,saved.id);
    await ui.settle(()=>button('Refine animation').click());
    const input=ui.dom.window.document.querySelector('.ps-animation-feedback textarea');
    const feedback='Keep this exact proposal and change only the visual colors.';
    await ui.settle(()=>{
      Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(input,feedback);
      input.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
    });
    ui.nextStatus('queued');
    await ui.settle(()=>input.form.dispatchEvent(new ui.dom.window.Event('submit',{bubbles:true,cancelable:true})));
    await ui.generate();assert.equal(ui.posts[1].feedback,feedback);
    await failPending();assert.deepEqual(ui.visible(),[selected]);
    assert.match(ui.text(),/could not finish this scene/);assert.match(ui.text(),/Retry agent/);
    const failed=structuredClone([...ui.receipts.values()].at(-1));
    await ui.remount();
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<3;){await ui.settle();await realDelay(1);}
    const recovered=ui.instances[2];assert.ok(recovered);assert.equal(recovered.options.code,saved.artifact.code);
    await ui.settle(()=>recovered.first());
    assert.deepEqual(ui.visible(),[recovered]);assert.equal(recovered.runtimeActivity,null);
    assert.equal(ui.posts.length,2,'Historical hydration admits no author work');
    assert.equal(ui.adoptionPosts.length,1,'Historical hydration does not replace the current request owner');
    await ui.generate();assert.equal(ui.posts[2].feedback,feedback);
    assert.deepEqual(ui.posts[2].blueprint,ui.posts[1].blueprint);assert.deepEqual(ui.posts[2].model,ui.posts[1].model);
    await failPending();
    assert.deepEqual(ui.visible(),[recovered]);assert.equal(ui.adoptionPosts.length,1);
    assert.match(ui.text(),/could not finish this scene/);assert.match(ui.text(),/Retry agent/);
    assert.match(ui.text(),/last working animation is still shown/);
    assert.match(ui.text(),/No replacement was applied/);
    assert.doesNotMatch(ui.text(),/latest proposal changes|automatic network preview is shown|saved animation you selected/);
    assert.deepEqual(ui.receipts.get(failed.id),failed,'Recovery cannot rewrite the failed operation');
  });
});

test('an exact saved choice is ignored for a changed proposal and a stale selected scene cannot replace its status',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const saved=await newerSavedFixture([...ui.receipts.values()][0]);ui.setLatest(saved);ui.receipts.set(saved.id,saved);
    await ui.settle(()=>[...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent==='Use latest saved animation').click());
    await ui.waitForAdoption(1);
    const selected=ui.instances.at(-1);await ui.settle(()=>selected.first());
    await ui.mount(ownedPlan('changed',2));
    const message=()=>ui.dom.window.document.querySelector('.ps-animation-status > span > span').textContent;
    await ui.settle(()=>selected.fail());
    assert.match(message(),/latest proposal update continues/);
    assert.equal(ui.dom.window.document.querySelector('.ps-animation-status').classList.contains('has-error'),false,
      'A stale saved scene cannot turn the new generation into a failed request');
    assert.doesNotMatch(ui.text(),/saved animation could not run here/);
    await ui.generate();assert.equal(ui.posts.length,2);assert.equal(ui.adoptionPosts.length,1);
    await ui.settle(()=>ui.instances.at(-1).first());
    await ui.remount();await ui.generate();assert.equal(ui.posts.length,3);
    assert.equal(ui.adoptionPosts.length,1,'Changed factual inputs cannot restore an old selected program');
  });
});

test('an adopted execution failure discloses only bounded host details and preserves the prior source and intent',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const button=text=>[...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent===text);
    const diagnostic=()=>[...ui.dom.window.document.querySelectorAll('details')]
      .find(item=>item.querySelector('summary')?.textContent==='Animation diagnostics');
    const adopt=async()=>{
      const count=ui.adoptionPosts.length+1;
      await ui.settle(()=>button('Use latest saved animation').click());
      await ui.waitForAdoption(count);
    };
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    assert.equal(diagnostic() === undefined, true);
    const original=structuredClone([...ui.receipts.values()][0]);
    const saved=await newerSavedFixture(original);ui.setLatest(saved);
    await adopt();const selected=ui.instances.at(-1);await ui.settle(()=>selected.first());
    const attached=structuredClone(ui.adoptionPosts[0]);
    await ui.settle(()=>selected.fail({kind:'program',phase:'render',exception:'TypeError',
      message:'Private authored error text',stack:'Private authored stack',code:'Private authored source'}));
    assert.match(diagnostic().textContent,/Failure: program\/render \(TypeError\)\./);
    assert.doesNotMatch(ui.text(),/Private authored/);
    assert.match(ui.text(),/saved animation could not run here/);
    assert.match(ui.text(),/previous working animation is shown/);
    assert.deepEqual(ui.visible(),[previous]);assert.equal(previous.options.code,original.artifact.code);
    assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,1);
    assert.deepEqual(ui.adoptionPosts[0],attached,'Diagnostics do not redeclare or alter the attached intent');
    assert.deepEqual([...ui.receipts.values()][0],original,'The native source receipt stays immutable');
    await adopt();assert.equal(diagnostic() === undefined, true,'A fresh saved-source retry clears the old diagnostic');
    const retry=ui.instances.at(-1);assert.equal(retry.options.code,saved.artifact.code);
    await ui.settle(()=>retry.first());assert.equal(diagnostic() === undefined, true,'Successful first paint has no failure disclosure');
    assert.equal(ui.posts.length,1);assert.equal(ui.adoptionPosts.length,2);
    assert.equal(ui.adoptionPosts[1].publication.code_hash,attached.publication.code_hash);
    await ui.settle(()=>retry.fail({kind:'timeout',phase:'draw',exception:'TypeError',message:'Private authored text',
      diagnostics:{frameId:12,time:1.25,width:668,height:501,ratio:1,
        lastStage:'worker-rendered',stageSource:'untrusted-worker-report',parentVisibility:'visible',
        parentRafAgeMs:2100,deadlineOvershootMs:125,renderMs:3.4,workerTimingSource:'untrusted-worker-report',
        privateText:'Private diagnostic text',code:'Private diagnostic source'}}));
    assert.match(diagnostic().textContent,/Failure: timeout\/draw\./);
    assert.doesNotMatch(diagnostic().textContent,/TypeError|Private/,'Timeouts cannot borrow authored exception text');
    const observations=JSON.parse(diagnostic().querySelector('pre').textContent);
    assert.equal(observations.frameId,12);assert.equal(observations.time,1.25);
    assert.equal(observations.lastStage,'worker-rendered');assert.equal(observations.parentVisibility,'visible');
    assert.equal(observations.workerTimingSource,'untrusted-worker-report');
    assert.equal(observations.privateText,undefined);assert.equal(observations.code,undefined);
    assert.match(diagnostic().textContent,/does not verify a displayed frame/);
    assert.equal(ui.posts.length,1,'Failure observations do not start another generation');
    assert.deepEqual(ui.visible(),[previous],'Observations preserve the previously painted scene');
    await ui.settle(()=>button('Retry agent').click());
    assert.equal(diagnostic() === undefined, true,'Ordinary generation retry clears the old execution detail');
  });
});

test('a pending saved candidate failure is sanitized and its diagnostic cannot follow a changed proposal',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const diagnostic=()=>[...ui.dom.window.document.querySelectorAll('details')]
      .find(item=>item.querySelector('summary')?.textContent==='Animation diagnostics');
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    const saved=await newerSavedFixture([...ui.receipts.values()][0]);ui.setLatest(saved);
    await ui.settle(()=>[...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent==='Use latest saved animation').click());
    await ui.waitForAdoption(1);
    const pending=ui.instances.at(-1),attached=structuredClone(ui.adoptionPosts[0]);
    await ui.settle(()=>pending.fail({kind:'program',phase:'private secret phase',exception:'PrivateError',message:'Private error text'}));
    assert.match(diagnostic().textContent,/Failure: runtime\/worker\./);
    assert.doesNotMatch(ui.text(),/private secret phase|PrivateError|Private error text/);
    assert.deepEqual(ui.visible(),[previous],'Startup failure does not replace the displayed owner');
    assert.equal(ui.posts.length,1);assert.deepEqual(ui.adoptionPosts[0],attached);
    await ui.mount(ownedPlan('changed',2));
    assert.equal(diagnostic() === undefined, true,'An earlier proposal cannot disclose a failure under new facts');
    await ui.settle(()=>pending.options.onError({kind:'timeout',phase:'startup'}));
    assert.equal(diagnostic() === undefined, true,'A late failure from the old candidate cannot reattach diagnostics');
    assert.equal(ui.adoptionPosts.length,1);assert.equal(ui.posts.length,1);
  });
});

test('saved animation selection refuses another setup or changed code before ownership attachment',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    const valid=await newerSavedFixture([...ui.receipts.values()][0]);
    for(const saved of [{...valid,setup_id:'another-setup'},
      {...valid,model_hash:'b'.repeat(64)}, {...valid,artifact:{...valid.artifact,code:valid.artifact.code+' changed'}}]){
      ui.setLatest(saved,{allowForeign:true});
      await ui.settle(async()=>{
        [...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent==='Use latest saved animation').click();
        for(const deadline=performance.now()+5000;performance.now()<deadline && !ui.text().includes('does not match this proposal');)
          await realDelay(1);
      });
      assert.equal(ui.adoptionPosts.length,0);assert.equal(ui.instances.length,1);assert.equal(ui.posts.length,1);
      assert.deepEqual(ui.visible(),[previous]);assert.match(ui.text(),/does not match this proposal/);
    }
  });
});

test('closing an authored identity restores its overview caption and later chapters without reviving a retained selection',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const caption=()=>ui.dom.window.document.querySelector('.ps-caption strong').textContent;
    const context=()=>ui.dom.window.document.querySelector('.ps-caption-context')?.textContent || null;
    const click=selector=>ui.settle(()=>ui.dom.window.document.querySelector(selector).click());
    const select=async(scene,id,title)=>{
      await ui.settle(()=>scene.options.onInspect(id));
      await ui.settle(()=>scene.options.onCaption({title,description:'Selected identity context'}));
      assert.equal(caption(),title);
      assert.equal(context(),null,'A caption authored for the selected identity retains its own narration');
    };
    const plan=ownedPlan();
    await ui.mount({...plan,agent_classes:[{...plan.agent_classes[0],tools:{memory:true,notes:'write'}}]});
    await ui.generate();const previous=ui.instances[0];
    await ui.settle(()=>{
      previous.options.onReady({duration:30,chapters:[
        {at:0,title:'Frame the question'}, {at:12,title:'Judgments arrive'}, {at:22,title:'Independent challenge'}]});
      previous.options.onFirstFrame();
    });
    assert.equal(caption(),'Frame the question');
    assert.equal(context(),null);
    await ui.settle(()=>previous.options.onInspect('earlier:main'));
    assert.equal(context(),'Playback chapter','An inspected identity does not own the unchanged story caption');
    assert.equal(caption(),'Frame the question');
    await select(previous,'earlier:main','Earlier owner details');
    await click('[aria-label="Close inspector"]');
    assert.equal(ui.dom.window.document.querySelector('.ps-inspector') === null,true);
    assert.equal(caption(),'Frame the question');
    assert.equal(context(),null);
    await ui.settle(()=>previous.options.onInspect('resource:local:notes'));
    assert.equal(context(),'Playback chapter','Resource inspection also distinguishes playback from selected context');
    assert.equal(caption(),'Frame the question');
    await click('[aria-label="Close inspector"]');
    await click('.ps-footer button:last-child');
    assert.equal(context(),null,'Population browsing does not relabel ordinary narration');
    await click('[aria-label="Close inspector"]');
    await click('[aria-label="Play simulation"]');
    await ui.settle(()=>{previous.time=14;previous.options.onFrame(14,30);});
    assert.equal(previous.paused,false);
    assert.equal(caption(),'Judgments arrive','Ordinary playback resumes chapter captions');
    assert.equal(context(),null);

    await ui.settle(()=>previous.options.onCaption({title:'Current evidence annotation',description:'Program-authored overview'}));
    await select(previous,'earlier:main','Earlier selected context');
    await click('[aria-label="Close inspector"]');
    assert.equal(caption(),'Current evidence annotation','Closing preserves the last legitimate overview program caption');
    await select(previous,'earlier:main','Retained selected context');
    await ui.mount(ownedPlan('latest',2));await ui.generate();
    const replacement=ui.instances[1];await ui.settle(()=>replacement.first());
    await select(replacement,'latest:1','Replacement selected context');
    await click('[aria-label="Close inspector"]');
    assert.equal(caption(),replacement.options.code);
    await ui.settle(()=>replacement.fail());
    assert.deepEqual(ui.visible(),[previous]);
    assert.equal(caption(),'Current evidence annotation','Restoration cannot resurrect a retained selection caption');
    assert.equal(context(),null,'Rollback restores unselected narration without a playback selection label');
    assert.equal(ui.posts.length,2,'Caption changes do not admit new animation work');
  });
});

test('large owner inspection puts exact source access first and reaches every allowed peer once through bounded pages',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const plan=inspectorPlan(),before=structuredClone(plan);
    await ui.mount(plan);await ui.generate();const scene=ui.instances[0];await ui.settle(()=>scene.first());
    await ui.settle(()=>scene.options.onInspect('desk:main'));
    const {document}=ui.dom.window,root=document.querySelector('.ps-owner-relationships');
    const tools=root.firstElementChild,peers=root.querySelector('details');
    assert.match(tools.textContent,/Tools and source access/);
    assert.equal(tools.querySelectorAll('li').length,4);
    assert.match(tools.textContent,/Read workspace text/);assert.match(tools.textContent,/Read and write notes/);
    assert.match(tools.textContent,/WorkGraph access/);assert.match(tools.textContent,/get_file/);
    assert.match(peers.querySelector('summary').textContent,/Allowed communication \(633 relationships\)/);
    assert.equal(peers.open,false);
    assert.equal(peers.querySelectorAll('li').length,IDENTITY_PAGE_SIZE);
    await ui.settle(()=>{peers.open=true;});
    const seen=[],expected=scene.options.model.edges.filter(edge=>edge.kind==='peer').map(edge=>edge.id);
    while(true){
      const rows=[...peers.querySelectorAll('li')];
      assert.ok(rows.length<=IDENTITY_PAGE_SIZE);seen.push(...rows.map(row=>row.dataset.relationshipId));
      const next=[...peers.querySelectorAll('button')].find(button=>button.textContent==='Next relationships');
      if(next.disabled)break;
      peers.querySelector('ul').scrollTop=100;
      await ui.settle(()=>next.click());
      assert.equal(peers.querySelector('ul').scrollTop,0);
      assert.equal(document.activeElement === peers.querySelector('ul'), true);
    }
    assert.deepEqual(seen,expected);assert.equal(new Set(seen).size,633);
    assert.match(peers.textContent,/Showing 601-633 of 633 matching relationships/);
    assert.match(peers.textContent,/specialist:633/);
    assert.equal(ui.posts.length,1,'Inspection does not submit another animation request');
    assert.deepEqual(plan,before);
  });
});

test('relationship search resets paging and finds exact identity or access text without losing source labels',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.mount(inspectorPlan());await ui.generate();const scene=ui.instances[0];await ui.settle(()=>scene.first());
    await ui.settle(()=>scene.options.onInspect('desk:main'));
    const {document}=ui.dom.window,peers=document.querySelector('.ps-owner-relationships details'),input=peers.querySelector('input');
    await ui.settle(()=>{peers.open=true;peers.querySelector('.ps-relationship-pages button:last-child').click();});
    await writeRelationshipSearch(ui,input,'SPECIALIST:633');
    assert.equal(peers.querySelectorAll('li').length,1);assert.match(peers.textContent,/specialist:633/);
    assert.match(peers.textContent,/Showing 1-1 of 1 matching relationship\./);
    await writeRelationshipSearch(ui,input,'unmatched identity');
    assert.equal(peers.querySelectorAll('li').length,0);assert.match(peers.textContent,/No matching relationships/);
    await writeRelationshipSearch(ui,input,'');
    assert.match(peers.textContent,/Showing 1-60 of 633 matching relationships/);
    await ui.settle(()=>scene.options.onInspect('resource:connector:records'));
    const access=document.querySelector('.ps-owner-relationships details');
    assert.equal(access.open,false);assert.match(access.querySelector('summary').textContent,/Configured access \(634 relationships\)/);
    await writeRelationshipSearch(ui,access.querySelector('input'),'GET_FILE');
    assert.equal(access.querySelectorAll('li').length,1);assert.match(access.textContent,/desk:main/);assert.match(access.textContent,/get_file/);
    await writeRelationshipSearch(ui,access.querySelector('input'),'SPECIALIST:633');
    assert.equal(access.querySelectorAll('li').length,1);assert.match(access.textContent,/read_summary/);
    assert.doesNotMatch(access.textContent,/get_file/);
  });
});

test('relationship navigation inspects an exact late peer and returns to the desk through its peer',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const plan=inspectorPlan(),before=structuredClone(plan);
    await ui.mount(plan);await ui.generate();const scene=ui.instances[0];await ui.settle(()=>scene.first());
    await ui.settle(()=>scene.options.onInspect('desk:main'));
    const {document}=ui.dom.window,peers=document.querySelector('.ps-owner-relationships details');
    await ui.settle(()=>{peers.open=true;[...peers.querySelectorAll('button')].find(button=>button.textContent==='Next relationships').click();});
    await writeRelationshipSearch(ui,peers.querySelector('input'),'SPECIALIST:633');
    const late=scene.options.model.nodes.find(node=>node.id==='specialist:633'),button=peers.querySelector('.ps-relationship-owner');
    assert.equal(peers.querySelectorAll('li').length,1);
    assert.equal(button.getAttribute('aria-label'),`Inspect ${late.name} (${late.id})`);
    assert.equal(button.querySelector('code').textContent,late.id);
    await ui.settle(()=>{button.focus();button.click();});
    const inspector=document.querySelector('.ps-inspector');
    assert.equal(inspector.querySelector('h4').textContent,late.name);
    assert.equal(inspector.querySelector('details code').textContent,late.id);
    assert.equal(document.activeElement === inspector, true,'The existing selection callback focuses the new exact identity');
    assert.equal(scene.selectedId,late.id);assert.equal(scene.paused,true);
    const returning=document.querySelector('.ps-owner-relationships details');
    assert.match(returning.querySelector('summary').textContent,/Allowed communication \(1 relationship\)/);
    await ui.settle(()=>{returning.open=true;returning.querySelector('.ps-relationship-owner').click();});
    assert.equal(scene.selectedId,'desk:main');
    assert.equal(document.querySelector('.ps-inspector details code').textContent,'desk:main');
    assert.equal(document.querySelector('.ps-owner-relationships details input').value,'');
    assert.equal(ui.posts.length,1,'Relationship navigation does not request another animation');
    assert.deepEqual(plan,before,'Navigation preserves every original proposal fact');
  });
});

test('relationship navigation follows exact tool and shared-resource access back to the selected agent',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.mount(inspectorPlan());await ui.generate();const scene=ui.instances[0];await ui.settle(()=>scene.first());
    await ui.settle(()=>scene.options.onInspect('desk:main'));
    const {document}=ui.dom.window;
    const ownerButton=id=>[...document.querySelectorAll('.ps-relationship-owner')].find(button=>button.querySelector('code').textContent===id);
    const records=ownerButton('resource:connector:records');assert.ok(records);
    assert.match(records.closest('li').textContent,/get_file/);
    await ui.settle(()=>records.click());
    assert.equal(scene.selectedId,'resource:connector:records');
    const access=document.querySelector('.ps-owner-relationships details');
    assert.match(access.querySelector('summary').textContent,/Configured access \(634 relationships\)/);
    await ui.settle(()=>{access.open=true;});
    await writeRelationshipSearch(ui,access.querySelector('input'),'specialist:633');
    assert.match(access.querySelector('li').textContent,/read_summary/);
    assert.doesNotMatch(access.querySelector('li').textContent,/get_file/);
    await ui.settle(()=>ownerButton('specialist:633').click());
    assert.equal(scene.selectedId,'specialist:633');
    const notes=ownerButton('resource:local:notes');assert.ok(notes);
    assert.match(notes.closest('li').textContent,/Read notes/);
    await ui.settle(()=>notes.click());
    assert.equal(scene.selectedId,'resource:local:notes');
    const shared=document.querySelector('.ps-owner-relationships details');
    await ui.settle(()=>{shared.open=true;});
    await writeRelationshipSearch(ui,shared.querySelector('input'),'desk:main');
    assert.match(shared.querySelector('li').textContent,/Read and write notes/);
    await ui.settle(()=>ownerButton('desk:main').click());
    assert.equal(scene.selectedId,'desk:main');
    assert.equal(document.querySelector('.ps-owner-relationships').firstElementChild.querySelectorAll('li').length,4);
    assert.equal(ui.posts.length,1);
  });
});

test('relationship navigation keeps missing counterparts nonselectable and supports exact workflow owners',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.mount(inspectorPlan(2));await ui.generate();const scene=ui.instances[0];await ui.settle(()=>scene.first());
    const model=structuredClone(scene.options.model),selected=model.nodes.find(node=>node.id==='desk:main');
    model.edges.push({id:'missing-peer',kind:'peer',source:selected.id,target:'missing:owner',label:'Original unresolved counterpart'},
      {id:'known-workflow',kind:'workflow',source:selected.id,target:'specialist:2',label:'Returns selected evidence'});
    const selections=[];
    await ui.mountRelationships(selected,model,(id,event)=>selections.push({id,target:event.currentTarget}));
    const {document}=ui.dom.window,missing=document.querySelector('[data-relationship-id="missing-peer"]');
    assert.match(missing.textContent,/missing:owner/);assert.match(missing.textContent,/Original unresolved counterpart/);
    assert.equal(missing.querySelector('button') === null,true,'An absent model node cannot become an inspection target');
    const workflow=document.querySelector('[data-relationship-id="known-workflow"]'),button=workflow.querySelector('button');
    assert.equal(button.type,'button');assert.match(button.getAttribute('aria-label'),/specialist:2/);
    await ui.settle(()=>{button.focus();button.click();});
    assert.deepEqual(selections,[{id:'specialist:2',target:button}]);
    assert.match(workflow.textContent,/Returns selected evidence/);
    await ui.mountRelationships(selected,model);
    assert.equal(document.querySelectorAll('.ps-relationship-owner').length,0,'Read-only standalone inspection remains usable without a selection callback');
    assert.match(document.querySelector('[data-relationship-id="known-workflow"]').textContent,/specialist:2/);
  });
});

test('large shared resource inspection preserves every exact owner grant through the final partial page',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.mount(inspectorPlan());await ui.generate();const scene=ui.instances[0];await ui.settle(()=>scene.first());
    await ui.settle(()=>scene.options.onInspect('resource:local:notes'));
    const access=ui.dom.window.document.querySelector('.ps-owner-relationships details'),seen=[];
    assert.equal(access.open,false);await ui.settle(()=>{access.open=true;});
    while(true){
      const rows=[...access.querySelectorAll('li')];assert.ok(rows.length<=IDENTITY_PAGE_SIZE);
      seen.push(...rows.map(row=>row.dataset.relationshipId));
      const next=[...access.querySelectorAll('button')].find(button=>button.textContent==='Next relationships');
      if(next.disabled)break;
      await ui.settle(()=>next.click());
    }
    const expected=scene.options.model.edges.filter(edge=>edge.target==='resource:local:notes').map(edge=>edge.id);
    assert.deepEqual(seen,expected);assert.equal(seen.length,634);assert.equal(new Set(seen).size,634);
    assert.match(access.textContent,/Showing 601-634 of 634 matching relationships/);
    await writeRelationshipSearch(ui,access.querySelector('input'),'READ AND WRITE NOTES');
    assert.equal(access.querySelectorAll('li').length,1);assert.match(access.textContent,/desk:main/);
    await writeRelationshipSearch(ui,access.querySelector('input'),'specialist:633');
    assert.equal(access.querySelectorAll('li').length,1);assert.match(access.textContent,/Read notes/);
    assert.doesNotMatch(access.textContent,/Read and write notes/);
  });
});

test('inspection search and disclosure belong to the displayed source and reset for a new owner, source or setup',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const plan=inspectorPlan();await ui.mount(plan);await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    const peers=()=>ui.dom.window.document.querySelector('.ps-owner-relationships details');
    await ui.settle(()=>previous.options.onInspect('desk:main'));
    await ui.settle(()=>{peers().open=true;});
    await writeRelationshipSearch(ui,peers().querySelector('input'),'specialist:633');
    await ui.mount(plan);
    assert.equal(peers().open,true);assert.equal(peers().querySelector('input').value,'specialist:633');
    await ui.settle(()=>previous.options.onInspect('specialist:1'));
    assert.equal(peers().open,false);assert.match(peers().textContent,/Allowed communication \(1 relationship\)/);
    await ui.settle(()=>previous.options.onInspect('desk:main'));
    assert.equal(peers().open,false);assert.equal(peers().querySelector('input').value,'');
    const changed=structuredClone(plan);changed.agent_classes[0].instructions='A newer source with distinct responsibilities.';
    await ui.mount(changed);assert.equal(peers() === null,true,'Changing proposal source clears selected inspection');
    await ui.settle(()=>previous.options.onInspect('desk:main'));
    await writeRelationshipSearch(ui,peers().querySelector('input'),'specialist:633');
    assert.equal(ui.dom.window.document.querySelector('.ps-refine').disabled,true);
    await ui.generate();const current=ui.instances[1];await ui.settle(()=>current.first());
    assert.equal(peers().open,false);assert.equal(peers().querySelector('input').value,'');
    assert.equal(ui.dom.window.document.querySelector('.ps-refine').disabled,false);
    await ui.settle(()=>{peers().open=true;});
    await writeRelationshipSearch(ui,peers().querySelector('input'),'specialist:633');
    await ui.mount(changed,'another-inspection-setup');assert.equal(peers() === null,true);
    await ui.generate();const other=ui.instances.at(-1);await ui.settle(()=>other.first());
    await ui.settle(()=>other.options.onInspect('desk:main'));
    assert.equal(peers().open,false);assert.equal(peers().querySelector('input').value,'');
  });
});

test('context-only updates keep the last good scene and reject obsolete first frames through A-B-C revisions',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const value=ownedPlan(),fact={status:'confirmed',summary:'Use a personal chat; Console is administration.',source_message_ids:['interface-answer']};
    const a={facts:{interfaces:fact,ownership:{status:'confirmed',summary:'One person owns one equipment context.',source_message_ids:['ownership-a']}},requirements:[]};
    const b={...a,facts:{...a.facts,ownership:{...a.facts.ownership,summary:'Two people own independent equipment contexts.',source_message_ids:['ownership-b']}}};
    const c={...b,facts:{...b.facts,outcome:{status:'confirmed',summary:'Compare their observing opportunities with shared domain experts.',source_message_ids:['outcome-c']}}};
    await ui.mount(value,undefined,null,a);await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await ui.mount(value,undefined,null,b);await ui.generate();const stale=ui.instances[1];
    assert.deepEqual(ui.visible(),[previous]);assert.match(ui.text(),/Updating the animation as your spec evolves/);
    await ui.mount(value,undefined,null,c);
    await ui.settle(()=>stale.first());assert.deepEqual(ui.visible(),[previous]);
    assert.equal(stale.alive,false,'The obsolete candidate releases its frame owner');
    await ui.generate();const current=ui.instances[2];await ui.settle(()=>current.first());
    assert.deepEqual(ui.visible(),[current]);assert.equal(ui.posts.length,3);
    assert.deepEqual(ui.posts.map(body=>body.blueprint),[value,value,value]);
    assert.deepEqual(ui.posts.map(body=>body.model.interface_context),Array(3).fill(ui.posts[0].model.interface_context));
    assert.deepEqual(ui.posts.map(body=>body.model.design_context),[a,b,c]);
    assert.notEqual(await fingerprint(ui.posts[0].model),await fingerprint(ui.posts[1].model));
    assert.notEqual(await fingerprint(ui.posts[1].model),await fingerprint(ui.posts[2].model));
  });
});

test('visualization feedback from the guide reaches the production plan component only with its selected checked pair',()=>{
  const findSimulation=node=>{
    if(!node || typeof node!=='object')return null;
    if(node.type?.name==='ProposalSimulation')return node;
    return React.Children.toArray(node.props?.children).map(findSimulation).find(Boolean) || null;
  };
  const saved={text:'Earlier saved composition.',source_message_id:'earlier-human'};
  const current={text:'Use spacious circular clusters.',source_message_id:'current-human'};
  const state={id:'visual-guide',version:4,proposal:ownedPlan(),brief:{facts:{},requirements:[]},visualization_feedback:saved,
    operation:{status:'running',version:4,request_id:'a'.repeat(64)},live_design:{guide_id:'visual-guide',guide_version:4,
      request_id:'a'.repeat(64),blueprint:ownedPlan('current'),brief:{facts:{},requirements:[]},visualization_feedback:current}};
  const original=structuredClone(state);
  const props=value=>findSimulation(EvolvingPlan({state:value})).props;
  assert.equal(props(state).visualizationFeedback,current);
  assert.equal(props(state).plan,state.live_design.blueprint);
  const early={...state,live_design:{...state.live_design,brief:null,context_pending:true}};
  assert.equal(props(early).visualizationFeedback,null);
  assert.equal(props(early).designBrief,null);
  const missing={...state,live_design:{...state.live_design,visualization_feedback:undefined}};
  assert.equal(props(missing).visualizationFeedback,undefined,'The checked draft cannot inherit an earlier saved preference');
  const complete={...state,version:5,proposal:state.live_design.blueprint,brief:state.live_design.brief,
    visualization_feedback:current,operation:{...state.operation,status:'completed'}};
  assert.equal(props(complete).visualizationFeedback,current);
  const clear={...complete,visualization_feedback:null};assert.equal(props(clear).visualizationFeedback,null);
  assert.deepEqual(state,original);
});

test('visualization feedback A-B-C updates keep the last good scene and ignore an obsolete candidate first frame',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const value=ownedPlan(),context={facts:{},requirements:[]};
    const a={text:'Use open circular groups.',source_message_id:'human-a'};
    const b={text:'Keep the groups but animate one collaboration at a time.',source_message_id:'human-b'};
    const c={text:b.text,source_message_id:'human-c'};
    await ui.mount(value,undefined,null,context,a);await ui.generate();
    const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await ui.mount(value,undefined,null,context,b);await ui.generate();const stale=ui.instances[1];
    assert.deepEqual(ui.visible(),[previous]);assert.match(ui.text(),/Updating the animation as your spec evolves/);
    await ui.mount(value,undefined,null,context,c);await ui.settle(()=>stale.first());
    assert.deepEqual(ui.visible(),[previous]);assert.equal(stale.alive,false);
    await ui.generate();const current=ui.instances[2];
    assert.deepEqual(ui.visible(),[previous],'The last good scene remains until the latest candidate renders');
    await ui.settle(()=>current.first());assert.deepEqual(ui.visible(),[current]);
    assert.equal(ui.posts.length,3);assert.deepEqual(ui.posts.map(body=>body.blueprint),[value,value,value]);
    assert.deepEqual(ui.posts.map(body=>body.model.visualization_feedback),[a,b,c]);
    const withoutFeedback=model=>Object.fromEntries(Object.entries(model).filter(([key])=>key!=='visualization_feedback'));
    assert.deepEqual(ui.posts.map(body=>withoutFeedback(body.model)),Array(3).fill(withoutFeedback(ui.posts[0].model)));
    assert.equal(new Set(await Promise.all(ui.posts.map(body=>fingerprint(body.model)))).size,3);
    assert.deepEqual(current.options.model.visualization_feedback,c);assert.deepEqual(ui.guideRequests,[]);
  });
});

test('visualization feedback reload restores exact model input and adopts its saved scene without another generation admission',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const value=ownedPlan(),context={facts:{},requirements:[]};
    const preference={text:'Use circular expertise groups and show the question handoff.',source_message_id:'saved-human-turn'};
    await ui.mount(value,undefined,null,context,preference);await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const saved=structuredClone([...ui.receipts.values()][0]),original=structuredClone(saved);
    assert.deepEqual(ui.posts[0].model.visualization_feedback,preference);
    ui.setLatest(saved);ui.dom.window.sessionStorage.clear();await ui.remount();await ui.generate();
    assert.equal(ui.posts.length,1,'Reload reads and adopts the saved publication instead of submitting the same feedback again');
    assert.equal(ui.adoptionPosts.length,1);assert.equal(ui.adoptionPosts[0].publication.operation_id,saved.id);
    const resumed=ui.instances.at(-1);assert.deepEqual(resumed.options.model,ui.posts[0].model);
    assert.deepEqual(resumed.options.model.visualization_feedback,preference);
    assert.deepEqual(resumed.options.blueprint,value);assert.equal(resumed.options.code,saved.artifact.code);
    await ui.settle(()=>resumed.first());assert.deepEqual(ui.visible(),[resumed]);
    const scope={setup_id:saved.setup_id,stream_id:saved.stream_id};
    assert.equal(readAnimationRecovery(ui.dom.window.sessionStorage,scope).operation_id,saved.id);
    assert.equal(readAnimationSelection(ui.dom.window.sessionStorage,scope),null);
    assert.deepEqual(ui.receipts.get(saved.id),original);assert.deepEqual(ui.guideRequests,[]);
  });
});

test('visualization feedback unchanged across unrelated guide and runtime updates never queues another generation',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const state={id:'visual-guide',version:4,proposal:ownedPlan(),brief:{facts:{},requirements:[]},
      visualization_feedback:{text:'Keep the circular overview.',source_message_id:'original-human'}};
    const findSimulation=node=>{
      if(!node || typeof node!=='object')return null;
      if(node.type?.name==='ProposalSimulation')return node;
      return React.Children.toArray(node.props?.children).map(findSimulation).find(Boolean) || null;
    };
    const selected=value=>findSimulation(EvolvingPlan({state:value})).props;
    const a=selected(state);await ui.mount(a.plan,a.setupId,null,a.designBrief,a.visualizationFeedback);await ui.generate();
    const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await clickRecoveryControl(ui,'Refine animation');
    const draft=ui.dom.window.document.querySelector('.ps-animation-feedback textarea');
    const unsent='  Give the next handoff more space.\nKeep this draft unsent.  ';
    await ui.settle(()=>{
      Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(draft,unsent);
      draft.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
    });
    const unrelated={...structuredClone(state),version:5,messages:[{id:'status-note',text:'A status update'}],
      readiness:{context_ready:true},visualization_feedback:{source_message_id:state.visualization_feedback.source_message_id,
        text:state.visualization_feedback.text}};
    const b=selected(unrelated);await ui.mount(b.plan,b.setupId,{status:'running',activeNodeIds:['earlier:main']},b.designBrief,b.visualizationFeedback);
    await ui.settle();
    assert.equal(ui.posts.length,1);assert.equal(ui.instances.length,1);assert.deepEqual(ui.visible(),[previous]);
    assert.equal([...ui.timers.values()].some(timer=>timer.delay===650),false);
    assert.equal(ui.adoptionPosts.length,0);assert.deepEqual(previous.options.model.visualization_feedback,state.visualization_feedback);
    assert.equal(ui.dom.window.document.querySelector('.ps-animation-feedback textarea').value,unsent);
    assert.equal(ui.posts[0].feedback,'','A retained local refinement draft was not submitted or replaced by the guide preference');
    assert.deepEqual(ui.guideRequests,[]);
  });
});

test('remount restores verified historical inputs without admitting extra work while the latest scene is queued',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const first=ui.instances[0];await ui.settle(()=>first.first());
    const oldModel=structuredClone(first.options.model),oldPlan=structuredClone(first.options.blueprint);
    await ui.mount(ownedPlan('latest',2));ui.nextStatus('queued');await ui.remount();
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<2;){await ui.settle();await realDelay(1);}
    assert.equal(ui.posts.length,1,'Recovery reads an existing operation, without another authoring admission');
    assert.equal(ui.recoveryReads().length,1);
    const recovered=ui.instances[1];assert.ok(recovered);
    assert.deepEqual(recovered.options.model,oldModel);assert.deepEqual(recovered.options.blueprint,oldPlan);
    await ui.settle(()=>recovered.first());
    assert.deepEqual(ui.visible(),[recovered]);assert.equal(ui.count(),'1');
    assert.match(ui.text(),/Previous proposal restored from this browser session/);
    assert.equal(recovered.runtimeActivity,null);
    await ui.generate();assert.equal(ui.posts.length,2);assert.deepEqual(ui.visible(),[recovered]);
    ui.nextStatus('completed');await ui.mount(ownedPlan('newest',3));await ui.generate();
    const current=ui.instances.at(-1);await ui.settle(()=>current.first());
    assert.deepEqual(ui.visible(),[current]);assert.equal(ui.count(),'3');
    assert.doesNotMatch(ui.text(),/Previous proposal restored/);
  });
});

test('a restored equal-input scene remains historical and never receives current runtime activity',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    ui.nextStatus('queued');await ui.remount();
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<2;){await ui.settle();await realDelay(1);}
    const recovered=ui.instances[1];assert.ok(recovered);await ui.settle(()=>recovered.first());
    await ui.mount(undefined,undefined,{status:'running',activeNodeIds:['earlier:main']});
    assert.equal(recovered.runtimeActivity,null);
    assert.match(ui.text(),/Previous proposal restored/);
    const refine=[...ui.dom.window.document.querySelectorAll('button')].find(button=>button.textContent==='Refine animation');
    assert.equal(refine.disabled,false,'A historical scene does not block drafting current-proposal feedback');
    await ui.settle(()=>refine.click());
    assert.equal(ui.dom.window.document.querySelector('.ps-animation-feedback button[type="submit"]').disabled,true);
    await ui.settle(()=>recovered.options.onInspect('earlier:main'));
    assert.match(ui.text(),/earlier responsibilities/);
    assert.equal(ui.dom.window.document.querySelector('.ps-refine').disabled,true);
    assert.match(ui.text(),/This is a previous preview/);
    assert.deepEqual(ui.guideRequests,[]);
  });
});

test('historical identity inspection cannot refine a newer proposal, including a retained earlier click handler',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await ui.settle(()=>previous.options.onInspect('earlier:main'));
    const currentButton=ui.dom.window.document.querySelector('.ps-refine');
    assert.equal(currentButton.disabled,false);
    await ui.settle(()=>currentButton.click());
    assert.equal(ui.guideRequests.length,1);assert.match(ui.guideRequests[0],/earlier owner/);
    ui.guideRequests.length=0;
    // Retain the actual mounted React handler to exercise its latest-state
    // recheck independently of the browser's disabled-button protection.
    const propsKey=Object.keys(currentButton).find(key=>key.startsWith('__reactProps$'));
    const oldClick=currentButton[propsKey].onClick;
    await ui.mount(ownedPlan('latest',2));
    await ui.settle(()=>previous.options.onInspect('earlier:main'));
    assert.deepEqual(ui.visible(),[previous]);assert.match(ui.text(),/earlier responsibilities/);
    assert.equal(ui.dom.window.document.querySelector('.ps-refine').disabled,true);
    assert.match(ui.text(),/This is a previous preview/);
    await ui.settle(()=>oldClick());
    assert.deepEqual(ui.guideRequests,[]);
    await ui.generate();const current=ui.instances[1];await ui.settle(()=>current.first());
    await ui.settle(()=>current.options.onInspect('latest:1'));
    assert.doesNotMatch(ui.text(),/This is a previous preview/);
    assert.equal(ui.dom.window.document.querySelector('.ps-refine').disabled,false);
    await ui.settle(()=>ui.dom.window.document.querySelector('.ps-refine').click());
    assert.equal(ui.guideRequests.length,1);assert.match(ui.guideRequests[0],/latest owner/);
    assert.doesNotMatch(ui.guideRequests[0],/earlier owner/);
  });
});

test('feedback is editable before publication, cannot send early, and stays scoped through proposal changes and remount',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const open=()=>ui.settle(()=>[...ui.dom.window.document.querySelectorAll('button')].find(button=>button.textContent==='Refine animation').click());
    const textarea=()=>ui.dom.window.document.querySelector('.ps-animation-feedback textarea');
    const send=()=>ui.dom.window.document.querySelector('.ps-animation-feedback button[type="submit"]');
    const write=text=>ui.settle(()=>{
      Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(textarea(),text);
      textarea().dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
    });
    const submit=()=>ui.settle(()=>textarea().form.dispatchEvent(new ui.dom.window.Event('submit',{bubbles:true,cancelable:true})));
    await open();await write('Show the original evidence handoff clearly');
    assert.equal(textarea().disabled,false);assert.equal(send().disabled,true);
    assert.equal(send().textContent,'Waiting for current preview');
    await submit();assert.equal(ui.posts.length,0,'An editable draft never admits animation work');
    await ui.generate();assert.equal(ui.posts.length,1);assert.equal(ui.posts[0].feedback || '','');
    assert.equal(textarea().value,'Show the original evidence handoff clearly');
    await ui.settle(()=>ui.instances[0].first());
    assert.equal(send().disabled,false);
    await ui.mount(ownedPlan('latest',2));await open();
    assert.equal(textarea().value,'','A new proposal does not inherit old feedback');
    await write('Emphasize the revised responsibility split');
    assert.equal(send().disabled,true);await submit();assert.equal(ui.posts.length,1);
    await ui.mount(ownedPlan());await open();
    assert.equal(textarea().value,'Show the original evidence handoff clearly');
    await ui.generate();await ui.settle(()=>ui.instances.at(-1).first());
    assert.equal(send().disabled,false);
    await ui.remount();await open();
    assert.equal(textarea().value,'Show the original evidence handoff clearly','Checked proposal draft survives remount');
    assert.equal(send().disabled,true,'Restored history does not authorize feedback submission');
    await ui.generate();await ui.settle(()=>ui.instances.at(-1).first());
    await submit();await ui.generate();
    assert.equal(ui.posts.at(-1).feedback,'Show the original evidence handoff clearly');
    assert.deepEqual(ui.guideRequests,[],'Animation feedback never mutates the design');
  });
});

test('delayed historical hydration cannot replace a newly published current first frame',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    await ui.mount(ownedPlan('latest',2));ui.holdRecovery(true);await ui.remount();
    assert.equal(ui.recoveryReads().length,1);
    await ui.generate();const current=ui.instances[1];await ui.settle(()=>current.first());
    await ui.settle(()=>ui.releaseRecovery());
    for(let turn=0;turn<5;turn++)await ui.settle();
    assert.equal(ui.instances.length,2,'The obsolete recovery never starts another frame owner');
    assert.deepEqual(ui.visible(),[current]);assert.equal(ui.count(),'2');
    assert.doesNotMatch(ui.text(),/Previous proposal restored/);
  });
});

test('historical verification in flight survives a current startup failure before first frame',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const first=ui.instances[0];await ui.settle(()=>first.first());
    const oldInputs=structuredClone({model:first.options.model,plan:first.options.blueprint});
    await ui.mount(ownedPlan('latest',2));ui.holdRecovery(true);await ui.remount();
    await ui.generate();const replacement=ui.instances[1];
    ui.holdRecoveryValidation(true);
    await ui.settle(()=>ui.releaseRecovery());
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.heldRecoveryValidations()<3;){await ui.settle();await realDelay(1);}
    assert.equal(ui.heldRecoveryValidations(),3,'All three real native hashes finished, but their verification remains in flight');
    assert.equal(ui.instances.length,2,'Historical hydration stages without replacing the accepted startup candidate');
    assert.equal(replacement.alive,true);assert.deepEqual(ui.visible(),[]);
    await ui.settle(()=>replacement.fail());
    assert.equal(ui.recoveryReads().length,1,'A repair retry retains the original verification instead of fetching it again');
    await ui.settle(()=>ui.releaseRecoveryValidation());
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<3;){await ui.settle();await realDelay(1);}
    const recovered=ui.instances[2];assert.ok(recovered);await ui.settle(()=>recovered.first());
    assert.deepEqual(recovered.options.model,oldInputs.model);assert.deepEqual(recovered.options.blueprint,oldInputs.plan);
    assert.deepEqual(ui.visible(),[recovered]);assert.equal(ui.count(),'1');
    assert.equal(ui.recoveryReads().length,1,'The exact verified checkpoint survives the bounded retry without another GET');
    assert.equal(ui.posts.length,2,'Recovery did not admit another authoring operation');
    assert.match(ui.text(),/previous scene remains visible/);
    await ui.generate();const repaired=ui.instances.at(-1);await ui.settle(()=>repaired.first());
    assert.equal(ui.posts.length,3);assert.deepEqual(ui.visible(),[repaired]);assert.equal(ui.count(),'2');
  });
});

test('late historical hydration can restore the previous scene after both current startup attempts fail',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const first=ui.instances[0];await ui.settle(()=>first.first());
    await ui.mount(ownedPlan('latest',2));ui.holdRecovery(true);await ui.remount();
    await ui.generate();await ui.settle(()=>ui.instances[1].fail());
    await ui.generate();await ui.settle(()=>ui.instances[2].fail());
    assert.deepEqual(ui.visible(),[]);assert.equal(ui.posts.length,3);
    assert.equal([...ui.timers.values()].some(timer=>timer.delay===650),false,'Automatic authoring repair has stopped');
    await ui.settle(()=>ui.releaseRecovery());
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<4;){await ui.settle();await realDelay(1);}
    const recovered=ui.instances[3];assert.ok(recovered);await ui.settle(()=>recovered.first());
    assert.deepEqual(ui.visible(),[recovered]);assert.equal(ui.count(),'1');
    assert.deepEqual(recovered.options.blueprint,first.options.blueprint);
    assert.match(ui.text(),/previous animation is still shown/);assert.match(ui.text(),/Retry agent/);
    assert.equal(ui.posts.length,3,'Historical recovery cannot create another native repair admission');
    assert.equal([...ui.timers.values()].some(timer=>timer.delay===650),false);
  });
});

test('a known failed current publication is not resurrected on remount while its repair is queued',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const failed=ui.instances[0];await ui.settle(()=>failed.first());
    await ui.settle(()=>failed.fail());assert.deepEqual(ui.visible(),[]);
    const storage=ui.dom.window.sessionStorage;
    assert.equal(Object.keys(storage).some(name=>name.startsWith('mobkit-animation-last-displayed-v1:')),false);
    ui.nextStatus('queued');await ui.remount();
    for(let turn=0;turn<10;turn++)await ui.settle();
    assert.equal(ui.recoveryReads().length,0);assert.equal(ui.instances.length,1);
    await ui.generate();assert.equal(ui.posts.length,2);assert.deepEqual(ui.visible(),[]);
    assert.doesNotMatch(ui.text(),/Previous proposal restored/);
  });
});

test('a known failed displayed scene cannot start another hidden renderer through A-B-A navigation',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const failed=ui.instances[0];await ui.settle(()=>failed.first());
    await ui.mount(ownedPlan('latest',2));await ui.settle(()=>failed.fail());
    assert.deepEqual(ui.visible(),[]);
    assert.equal(Object.keys(ui.dom.window.sessionStorage).some(name=>name.startsWith('mobkit-animation-last-displayed-v1:')),false);
    await ui.mount(ownedPlan());
    assert.equal(ui.instances.length,1,'No frame owner can start from the known failed candidate when its old source returns');
    await ui.generate();const fresh=ui.instances[1];await ui.settle(()=>fresh.first());
    assert.deepEqual(ui.visible(),[fresh]);assert.equal(ui.posts.length,2);
    assert.equal(ui.posts.at(-1).feedback,'','A stale hidden execution cannot request an unsolicited repair');
  });
});

test('an abandoned unpublished candidate cannot block historical recovery when its exact source returns',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const first=ui.instances[0];await ui.settle(()=>first.first());
    const a=ownedPlan('latest',2),b=ownedPlan('different',3);
    await ui.mount(a);ui.holdRecovery(true);await ui.remount();
    await ui.generate();const abandoned=ui.instances[1];
    await ui.mount(b);assert.equal(abandoned.alive,false);
    await ui.mount(a);assert.equal(ui.instances.length,2,'Returning inputs alone cannot restart the old unpublished candidate');
    await ui.settle(()=>ui.releaseRecovery());
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<3;){await ui.settle();await realDelay(1);}
    const recovered=ui.instances[2];assert.ok(recovered);await ui.settle(()=>recovered.first());
    assert.deepEqual(ui.visible(),[recovered]);assert.equal(ui.count(),'1');
    assert.deepEqual(recovered.options.blueprint,first.options.blueprint);
    assert.equal(ui.posts.length,2,'Recovery remains independent of the newly queued current admission');
    assert.match(ui.text(),/Previous proposal restored/);
    await ui.generate();const fresh=ui.instances[3];await ui.settle(()=>fresh.first());
    assert.deepEqual(ui.visible(),[fresh]);assert.equal(ui.count(),'2');
  });
});

test('a failed recovered scene cannot start again when its former desired proposal returns',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const a=ownedPlan('latest',2),b=ownedPlan('different',3);
    await ui.mount(a);await ui.remount();
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<2;){await ui.settle();await realDelay(1);}
    const failed=ui.instances[1];assert.ok(failed);await ui.settle(()=>failed.first());
    await ui.mount(b);await ui.settle(()=>failed.fail());await ui.mount(a);
    assert.equal(ui.instances.length,2,'Only a fresh verified recovery checkpoint may construct a historical frame owner');
    assert.deepEqual(ui.visible(),[]);assert.equal(ui.posts.length,1);
    await ui.generate();const fresh=ui.instances[2];await ui.settle(()=>fresh.first());
    assert.deepEqual(ui.visible(),[fresh]);assert.equal(ui.count(),'2');assert.equal(ui.posts.at(-1).feedback,'');
  });
});

test('a delayed historical response from another guide cannot enter the new guide view',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    await ui.mount(ownedPlan('latest',2));ui.holdRecovery(true);await ui.remount();
    assert.equal(ui.recoveryReads().length,1);
    await ui.mount(ownedPlan('foreign',3),'different-guide');
    await ui.settle(()=>ui.releaseRecovery());
    for(let turn=0;turn<10;turn++)await ui.settle();
    assert.equal(ui.instances.length,1);assert.deepEqual(ui.visible(),[]);assert.equal(ui.count(),'3');
    await ui.generate();const current=ui.instances[1];await ui.settle(()=>current.first());
    assert.deepEqual(ui.visible(),[current]);assert.equal(ui.posts.at(-1).setup_id,'different-guide');
    assert.doesNotMatch(ui.text(),/Previous proposal restored/);
  });
});

test('altered saved source inputs are rejected without blocking current generation',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    const storage=ui.dom.window.sessionStorage,name=Object.keys(storage).find(key=>key.startsWith('mobkit-animation-last-displayed-v1:'));
    const saved=JSON.parse(storage.getItem(name));saved.model.nodes[0].name='Invented saved identity';storage.setItem(name,JSON.stringify(saved));
    await ui.mount(ownedPlan('latest',2));await ui.remount();
    for(let turn=0;turn<10;turn++)await ui.settle();
    assert.equal(ui.instances.length,1);assert.equal(ui.visible().length,0);
    await ui.generate();const current=ui.instances[1];await ui.settle(()=>current.first());
    assert.deepEqual(ui.visible(),[current]);assert.equal(ui.count(),'2');
  });
});

test('historical recovery startup failure does not ask the agent to repair old code for the current proposal',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    await ui.mount(ownedPlan('latest',2));await ui.remount();
    for(const deadline=performance.now()+5000;performance.now()<deadline && ui.instances.length<2;){await ui.settle();await realDelay(1);}
    const recovered=ui.instances[1];assert.ok(recovered);await ui.settle(()=>recovered.fail());
    assert.equal(ui.posts.length,1);assert.equal(ui.visible().length,0);
    await ui.generate();assert.equal(ui.posts.length,2);assert.equal(ui.posts[1].feedback,'');
    const current=ui.instances[2];await ui.settle(()=>current.first());
    assert.deepEqual(ui.visible(),[current]);
  });
});

test('storage quota denial leaves ordinary current generation usable without recovery promises',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const original=globalThis.sessionStorage;
    Object.defineProperty(globalThis,'sessionStorage',{configurable:true,value:{getItem:()=>null,
      setItem:()=>{throw new Error('Synthetic session quota denial');}}});
    try{
      await ui.generate();await ui.settle(()=>ui.instances[0].first());
      assert.equal(ui.visible().length,1);assert.equal(Object.keys(ui.dom.window.sessionStorage).some(name=>name.startsWith('mobkit-animation-last-displayed-v1:')),false);
      await ui.remount();assert.equal(ui.recoveryReads().length,0);
      await ui.generate();await ui.settle(()=>ui.instances[1].first());
      assert.equal(ui.visible().length,1);assert.doesNotMatch(ui.text(),/Previous proposal restored/);
    }finally{Object.defineProperty(globalThis,'sessionStorage',{configurable:true,value:original});}
  });
});

test('returning to earlier brief context coalesces the pending version and rejects an obsolete context publication',async()=>{
  await mountedAnimationRecovery(async ui=>{
    const value=ownedPlan(),a={facts:{audience:{status:'confirmed',summary:'One household.',source_message_ids:['audience-a']}},requirements:[]};
    const b={facts:{audience:{status:'confirmed',summary:'Independent households.',source_message_ids:['audience-b']}},requirements:[]};
    const c={facts:{audience:{status:'proposed',summary:'Separate operators across organizations.',source_message_ids:[]}},requirements:[]};
    await ui.mount(value,undefined,null,a);await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await ui.mount(value,undefined,null,b);await ui.generate();const obsolete=ui.instances[1];
    await ui.mount(value,undefined,null,c);await ui.mount(value,undefined,null,a);
    assert.equal([...ui.timers.values()].filter(timer=>timer.delay===650).length,1,'Only the latest context keeps a pending POST');
    await ui.settle(()=>obsolete.first());assert.deepEqual(ui.visible(),[previous]);
    await ui.generate();const returned=ui.instances[2];await ui.settle(()=>returned.first());
    assert.deepEqual(ui.visible(),[returned]);assert.equal(ui.posts.length,3,'The queued C context never posts');
    assert.deepEqual(ui.posts.map(body=>body.model.design_context),[a,b,a]);
    await ui.settle(()=>obsolete.first());assert.deepEqual(ui.visible(),[returned]);
  });
});

test('changing the authored presentation retires a late correction and reviews the replacement only after settling',async()=>{
  for(const selector of ['[aria-label="Replay simulation"]','[aria-pressed="false"]','[aria-label="Pause simulation"]','[aria-expanded="false"]']){
    await mountedAnimationRecovery(async ui=>{
      await ui.generate();await ui.settle(()=>ui.instances[0].first());
      const initial=await ui.waitForTimer(300);assert.ok(initial,'Initial presentation settles before review sampling');
      assert.equal(ui.reviewPosts.length,0);ui.timers.delete(initial[0]);await ui.settle(()=>initial[1].callback());
      assert.match(ui.text(),/Checking the visible animation/);
      for(let index=0;index<3;index++){
        const timer=await ui.waitForTimer(9000);
        assert.ok(timer,'Natural later-scene observation timer');ui.timers.delete(timer[0]);
        await ui.settle(()=>{ui.instances[0].time=(ui.instances[0].time+9)%30;timer[1].callback();});
      }
      await ui.waitForReview(1);
      assert.equal(ui.reviewPosts.length,1);
      await ui.settle(()=>ui.dom.window.document.querySelector(selector).click());
      assert.doesNotMatch(ui.text(),/Checking the visible animation|Sampled animation looks clear/);
      await ui.settle(()=>ui.resolveReview());
      assert.equal(ui.posts.length,1,'Late verdict cannot admit an automatic replacement');
      assert.equal(ui.reviewPosts.length,1,'No replacement review starts before the presentation settles');
      assert.ok(![...ui.timers.values()].some(value=>value.delay===650),'No corrective generation was queued');
      const settled=await ui.waitForTimer(300);assert.ok(settled,'The changed presentation schedules its settled replacement review');
      ui.timers.delete(settled[0]);await ui.settle(()=>settled[1].callback());
      if(!ui.instances[0].paused)for(let index=0;index<3;index++){
        const timer=await ui.waitForTimer(9000);assert.ok(timer,'Replacement observes naturally playing frames');ui.timers.delete(timer[0]);
        await ui.settle(()=>{ui.instances[0].time=(ui.instances[0].time+9)%30;timer[1].callback();});
      }
      await ui.waitForReview(2);
      assert.equal(ui.reviewPosts.length,2,'Exactly one review samples the settled replacement presentation');
      assert.deepEqual(ui.reviewPosts[1].binding,ui.reviewPosts[0].binding,'The same displayed source and publication remain authoritative');
      assert.equal(ui.reviewPosts[1].frames[0].show_tools,ui.instances[0].showPeers);
      assert.equal(ui.reviewPosts[1].frames[0].playing,!ui.instances[0].paused);
      await ui.settle(()=>ui.resolveReview('acceptable'));
      assert.match(ui.text(),/Sampled animation looks clear/);
      assert.equal(ui.posts.length,1,'An acceptable replacement presentation admits no new animation');
      assert.equal(ui.instances.length,1);assert.deepEqual(ui.visible(),[ui.instances[0]]);
      assert.ok(![...ui.timers.values()].some(value=>value.delay===650),'Neither the obsolete verdict nor the acceptable current verdict generates a replacement');
    },{visualReview:true});
  }
});

test('a replacement that fails after its first frame restores the exact prior proposal and inspector, with one repair only',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await ui.mount(ownedPlan('latest',3),undefined,{status:'running',activeNodeIds:['latest:1']});
    await ui.generate();const replacement=ui.instances[1];await ui.settle(()=>replacement.first());
    assert.equal(ui.count(),'3');assert.deepEqual(ui.visible(),[replacement]);
    assert.equal(previous.alive,true);assert.equal(previous.paused,true);
    await ui.settle(()=>replacement.fail());
    assert.deepEqual(ui.visible(),[previous]);assert.equal(ui.count(),'1');
    assert.equal(previous.runtimeActivity,null,'Current-proposal activity cannot mark a restored older proposal');
    assert.match(ui.dom.window.document.querySelector('.ps-runtime-activity').textContent,/latest owner 1 active/,
      'The separate actual-activity banner keeps the current proposal names');
    assert.match(ui.text(),/return program1/);assert.doesNotMatch(ui.text(),/return program2/);
    assert.match(ui.text(),/previous scene remains visible/);
    await ui.settle(()=>previous.options.onInspect('earlier:main'));
    assert.match(ui.text(),/earlier responsibilities/);assert.doesNotMatch(ui.text(),/latest responsibilities/);
    await ui.generate();assert.equal(ui.posts.length,3);const repair=ui.instances[2];
    await ui.settle(()=>repair.first());await ui.settle(()=>repair.fail());
    assert.deepEqual(ui.visible(),[previous]);assert.equal(ui.posts.length,3);
    assert.match(ui.text(),/does not include the latest proposal changes/);
    assert.match(ui.text(),/Retry agent/);
    assert.equal([...ui.timers.values()].some(timer=>timer.delay===650),false,'No automatic repair loop');
  });
});

test('a replacement startup failure preserves the displayed scene without swapping its binding',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await ui.mount(ownedPlan('latest',2));await ui.generate();
    await ui.settle(()=>ui.instances[1].fail());
    assert.deepEqual(ui.visible(),[previous]);assert.equal(ui.count(),'1');assert.equal(previous.alive,true);
    await ui.generate();await ui.settle(()=>ui.instances[2].fail());
    assert.deepEqual(ui.visible(),[previous]);assert.equal(ui.posts.length,3);
    assert.match(ui.text(),/previous animation is still shown/);
  });
});

test('same-proposal refinement rollback distinguishes the shown replacement from its bounded repair',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    const button=[...ui.dom.window.document.querySelectorAll('button')].find(item=>item.textContent==='Refine animation');
    assert.ok(button);await ui.settle(()=>button.click());
    const textarea=ui.dom.window.document.querySelector('.ps-animation-feedback textarea');
    await ui.settle(()=>{
      Object.getOwnPropertyDescriptor(ui.dom.window.HTMLTextAreaElement.prototype,'value').set.call(textarea,'Show the evidence handoff more clearly');
      textarea.dispatchEvent(new ui.dom.window.Event('input',{bubbles:true}));
    });
    await ui.settle(()=>textarea.form.dispatchEvent(new ui.dom.window.Event('submit',{bubbles:true,cancelable:true})));
    await ui.generate();assert.equal(ui.posts[1].feedback,'Show the evidence handoff more clearly');
    const replacement=ui.instances[1];await ui.settle(()=>replacement.first());await ui.settle(()=>replacement.fail());
    await ui.generate();const repair=ui.instances[2];await ui.settle(()=>repair.first());await ui.settle(()=>repair.fail());
    assert.deepEqual(ui.visible(),[previous]);assert.equal(ui.count(),'1');
    assert.match(ui.text(),/previous working animation is shown/);
    assert.match(ui.text(),/replacement stopped while running/);
    assert.doesNotMatch(ui.text(),/No replacement was applied|does not include the latest proposal changes/);
    assert.equal(ui.posts.length,3);assert.equal([...ui.timers.values()].some(timer=>timer.delay===650),false);
  });
});

test('only one paused previous scene is retained and a changed setup releases both owned layers',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();await ui.settle(()=>ui.instances[0].first());
    await ui.mount(ownedPlan('second',2));await ui.generate();await ui.settle(()=>ui.instances[1].first());
    await ui.mount(ownedPlan('third',3));await ui.generate();await ui.settle(()=>ui.instances[2].first());
    assert.equal(ui.instances[0].alive,false);
    assert.equal(ui.instances[1].alive,true);assert.equal(ui.instances[1].paused,true);
    assert.deepEqual(ui.visible(),[ui.instances[2]]);
    await ui.mount(ownedPlan('other',1),'another-setup');
    assert.equal(ui.instances[1].alive,false);assert.equal(ui.instances[2].alive,false);
    assert.deepEqual(ui.visible(),[]);assert.equal(ui.count(),'1');
  });
});

test('a retained scene that stops cannot be recursively restored or create an extra repair',async()=>{
  await mountedAnimationRecovery(async ui=>{
    await ui.generate();const previous=ui.instances[0];await ui.settle(()=>previous.first());
    await ui.mount(ownedPlan('latest',2));await ui.generate();const replacement=ui.instances[1];
    await ui.settle(()=>replacement.first());await ui.settle(()=>previous.fail());
    await ui.settle(()=>replacement.fail());assert.deepEqual(ui.visible(),[]);
    await ui.generate();await ui.settle(()=>ui.instances[2].fail());
    assert.equal(ui.posts.length,3);assert.deepEqual(ui.visible(),[]);
    assert.match(ui.text(),/automatic network preview is shown/);
    assert.equal([...ui.timers.values()].some(timer=>timer.delay===650),false);
  });
});
