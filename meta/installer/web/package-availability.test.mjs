import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
import { dirname, join, relative } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, access, mkdtemp, writeFile, rm } from 'node:fs/promises';

// This test mounts actual package/build controls. Animation generation is outside
// its scope; the simulation dependency is inert and every HTTP call is local data.
const web = process.env.FORGE_WEB_SOURCE || dirname(fileURLToPath(import.meta.url));
const overlay = process.env.FORGE_WEB_OVERLAY;
const require = createRequire(join(web, 'package.json'));
const { build } = require('esbuild');
const React = require('react'), { act } = React;
const { JSDOM } = createRequire(join(web, '../../console/package.json'))('jsdom');
const { renderToStaticMarkup } = require('react-dom/server');
const built = await build({ stdin: { contents: `
  export {GuidedSetup, NativeTeamPackage} from './guided.jsx';
  export {ImplementationPanel} from './implementation-panel.jsx';
  export {EvolvingPlan, AcquisitionReview} from './evolving-plan.jsx';
  export {setupProgress} from './setup-progress.mjs';
  export {nativeTeamCanInstall} from './deployment-panel.mjs';
  export {canonicalJson} from './state.mjs';`, resolveDir: web, sourcefile: 'package-controls-entry.mjs' },
  bundle: true, write: false, platform: 'node', format: 'esm', jsx: 'automatic', logLevel: 'silent',
  loader: {'.css': 'empty'}, nodePaths: [join(web, 'node_modules')],
  alias: {'@console-core': join(web, '../../packages/console-core/src')},
  plugins: [{ name: 'isolated-package-controls', setup(builder) {
    builder.onResolve({filter: /^react(?:\/.*)?$/}, args => ({path: pathToFileURL(require.resolve(args.path)).href, external: true}));
    builder.onResolve({filter: /proposal-simulation\.jsx$/}, () => ({path: 'inert-animation', namespace: 'inert'}));
    builder.onLoad({filter: /.*/, namespace: 'inert'}, () => ({contents: `import React from 'react'; export function ProposalSimulation(){return React.createElement('div',{'data-inert-animation':true});}`, loader: 'js'}));
    if (overlay) {
      builder.onResolve({filter: /package-availability\.mjs$/}, async () => ({path: join(web, 'package-availability.mjs')}));
      builder.onLoad({filter: /\.(jsx|mjs)$/}, async args => {
        const name = relative(web, args.path);
        if (name.startsWith('..')) return;
        const replacement = join(overlay, name);
        try { await access(replacement); } catch { return; }
        return {contents: await readFile(replacement, 'utf8'), resolveDir: dirname(args.path), loader: args.path.endsWith('.jsx') ? 'jsx' : 'js'};
      });
    }
  }}] });
const bundleDirectory = await mkdtemp(join(tmpdir(), 'forge-package-controls-'));
const bundlePath = join(bundleDirectory, 'controls.mjs');
await writeFile(bundlePath, built.outputFiles[0].text);
after(() => rm(bundleDirectory, {recursive:true, force:true}));
const { GuidedSetup, NativeTeamPackage, ImplementationPanel, AcquisitionReview, EvolvingPlan,
  setupProgress, nativeTeamCanInstall, canonicalJson } = await import(pathToFileURL(bundlePath).href);
const reason = 'This plan selects a connection setup helper. Installation support for that choice is not ready in this Forge build yet. Your proposed helper and tools are preserved. You can keep refining the design while support is completed.';
const Icon = () => null;
function fixture(selected = true, status = 'ready', mode = 'preparation') {
  const plan = {schema_version:2,project:{name:'Workshop',slug:'workshop',description:'A useful workshop'},provider:'openai',model:'gpt-6.1-sol',target:'mac',
    agent_classes:[{id:'connection_research',name:'Connection researcher',description:'Research connections',instances:1,instructions:'Prepare evidence for the request owner.',
      tools:{workspace:false,notes:'none',shell:false,builtins:false,workgraph:false,schedule:false,memory:false,knowledge:false,mcp:[]}}],
    connections:[],flows:[],topology:{mode:'hub',entrypoint:'connection_research:main',edges:[]},knowledge:{mode:'none'},advanced:{port:4110}};
  if (selected) plan.acquisition={helper_identity:'connection_research:main',helper_tools:mode,helper_profile:'exclusive_application_role'};
  const state = {id:'package-workshop',version:4,status,stage:'review',answers:[],messages:[],completed_stages:[],summary:'Workshop proposal',
    question:null,brief:{facts:{outcome:{summary:'Useful workshop',status:'confirmed'}},requirements:[]},
    readiness:{ready:true,context_ready:true,decision_blockers:[],implementation_blockers:[]},capabilities:{available:true},
    proposal:status==='review'?plan:null,blueprint:status==='review'?null:plan};
  bind(state, selected);
  return {state,plan};
}
function bind(state, blocked) {
  const plan=state.proposal||state.blueprint;
  state.package_availability={guide_id:state.id,guide_version:state.version,source:state.proposal?'proposal':'blueprint',
    blueprint_hash:createHash('sha256').update(canonicalJson(plan)).digest('hex'),available:!blocked,status:blocked?'pending-integration':'ready',reason:blocked?reason:null};
}
const byText=(document,text)=>[...document.querySelectorAll('button')].find(button=>button.textContent.includes(text));
async function mounted(node, read, verify, state) {
  const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/',pretendToBeVisual:true});
  if(state)dom.window.localStorage.setItem('mobkit-launch-guide-v1',state.id);
  const calls=[], prior=new Map();
  const values={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,HTMLElement:dom.window.HTMLElement,
    localStorage:dom.window.localStorage,sessionStorage:dom.window.sessionStorage,IS_REACT_ACT_ENVIRONMENT:true,
    requestAnimationFrame:dom.window.requestAnimationFrame.bind(dom.window),cancelAnimationFrame:dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia:()=>({matches:false,addEventListener(){},removeEventListener(){}}),fetch:async(url,options={})=>{
      const method=options.method||'GET'; calls.push({url,method,body:options.body?JSON.parse(options.body):null});
      const data=await read(url,options);
      assert.ok(data,`No live HTTP fallback: ${method} ${url}`);
      return {ok:!data.httpError,status:data.httpError||200,json:async()=>data.httpError?{detail:data.detail}:data};
    }};
  for(const [key,value] of Object.entries(values)){prior.set(key,Object.getOwnPropertyDescriptor(globalThis,key));Object.defineProperty(globalThis,key,{configurable:true,value});}
  dom.window.HTMLElement.prototype.scrollTo=function(){};
  dom.window.HTMLElement.prototype.scrollIntoView=function(){};
  const {createRoot}=await import(pathToFileURL(require.resolve('react-dom/client')).href);
  const root=createRoot(dom.window.document.getElementById('root'));
  try {
    await act(async()=>root.render(node));
    await act(async()=>new Promise(resolve=>setTimeout(resolve,20)));
    await verify(dom.window.document,calls,async next=>{await act(async()=>root.render(next));});
  } finally {
    await act(async()=>root.unmount());dom.window.close();
    for(const[key,value]of prior){if(value)Object.defineProperty(globalThis,key,value);else delete globalThis[key];}
  }
}
function readControls(state, {receipt, post}={}) {
  return async (url,options={})=>{
    const method=options.method||'GET';
    if(method==='POST')return post?post(url,options):null;
    if(url==='/api/guide/list')return {setups:[]};
    if(url===`/api/guide/${state.id}`||url==='/api/guide/new')return structuredClone(state);
    if(url==='/api/architect/status')return {available:true,state:'ready'};
    if(url==='/api/capabilities')return {rehearsal:{start_request_replay:false}};
    if(url==='/api/checker-setup')return {status:'ready',can_prepare:false,detail:'Inert checker readiness'};
    if(url.includes('/timeline'))return {frames:[],bindings:[],exhausted:true,has_more:false,build_id:receipt?.build_id};
    if(url===`/api/guide/${state.id}/implementation`)return receipt||{httpError:404,detail:'No saved build'};
    if(url===`/api/guide/${state.id}/deployment`||url===`/api/guide/${state.id}/rehearsal`)return {httpError:404,detail:'No saved operation'};
    return null;
  };
}

test('selected review warns before acceptance and keeps Use this plan available',async()=>{
  const {state}=fixture(true,'review');let accepted=false;
  const read=readControls(state,{post:async(url,options)=>{
    assert.equal(url,`/api/guide/${state.id}/apply`);assert.deepEqual(JSON.parse(options.body),{version:4});
    accepted=true;state.blueprint=state.proposal;state.proposal=null;state.status='ready';state.version++;bind(state,true);return structuredClone(state);
  }});
  await mounted(React.createElement(GuidedSetup,{active:true,bp:null,Icon,onBlueprint:()=>{},config:{architect:{available:true}}}),read,async(document,calls)=>{
    const card=document.querySelector('.setup-review-card:not(.setup-team-package)');
    assert.ok(card.textContent.includes(reason),'the present compiler limitation is visible before acceptance');
    assert.ok(card.textContent.includes('Accepting saves this design'));
    const accept=byText(card,'Use this plan');assert.equal(accept.disabled,false);
    const draft='Keep my exact question while I review this limitation.';
    const composer=document.querySelector('#setup-message');
    const setValue=Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value').set;
    await act(async()=>{setValue.call(composer,draft);composer.dispatchEvent(new window.Event('input',{bubbles:true}));});
    await act(async()=>accept.click());assert.equal(accepted,true);
    assert.ok(document.querySelector('.ep-plan-status').textContent.includes('package preparation is pending'));
    assert.ok(document.body.textContent.includes('Your design is saved'));
    assert.ok(!document.body.textContent.includes('Accepted plan - ready to package'));
    assert.equal(byText(document,'Download agent team package').disabled,true);
    assert.ok(!byText(document,'Install agent team'));
    const status=document.querySelector('.setup-visually-hidden[role="status"]');
    const shortcuts=document.querySelector('nav[aria-label="Try, build and run this proposal"]');
    assert.deepEqual({
      status:status.textContent,
      buildShortcut:shortcuts.querySelector('a[href="#setup-implementation"]')?.textContent||null,
      caption:document.querySelector('.setup-chat-top small').textContent,
    },{
      status:setupProgress(state,{},{}).title,
      buildShortcut:null,
      caption:setupProgress(state,{},{}).title,
    },'accepted package-pending plans must describe the available revision path, not invite a blocked build');
    assert.equal(byText(document.querySelector('.setup-team-package'),'Revise with guide').disabled,false);
    assert.equal(document.querySelector('#setup-message').value,draft);
    assert.equal(calls.filter(call=>call.method==='POST').length,1);
  },state);
});

test('selected fresh builder shows reason, prevents admission, and preserves the note',async()=>{
  const {state,plan}=fixture();const revisions=[];
  await mounted(React.createElement(ImplementationPanel,{state,plan,Icon,onRevise:(...value)=>revisions.push(value)}),readControls(state),async(document,calls)=>{
    const textarea=document.querySelector('textarea');
    const setter=Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value').set;
    await act(async()=>{setter.call(textarea,'My retained builder note');textarea.dispatchEvent(new window.Event('input',{bubbles:true}));});
    const start=byText(document,'Build application');assert.equal(start.disabled,true);
    assert.ok(document.body.textContent.includes(reason));
    assert.ok(!document.body.textContent.includes('or start building below'));
    await act(async()=>start.click());assert.equal(calls.filter(call=>call.method==='POST').length,0);
    await act(async()=>byText(document,'Revise with guide').click());assert.equal(revisions.length,1);
    assert.deepEqual(revisions[0][1],{id:state.id,version:state.version});assert.equal(textarea.value,'My retained builder note');
  });
});

test('default unselected plan still offers package/install and admits its builder once',async()=>{
  const {state,plan}=fixture(false);let posts=0;
  assert.equal(nativeTeamCanInstall(state),true);
  const progress=setupProgress(state,{},{});assert.equal(progress.title,'Your agent team is ready to install');
  const html=renderToStaticMarkup(React.createElement(NativeTeamPackage,{state,Icon,onPackage:()=>{},onInstall:()=>{}}));
  assert.ok(html.includes('Download agent team package'));assert.ok(html.includes('Install agent team'));assert.ok(!html.includes(reason));
  await mounted(React.createElement(ImplementationPanel,{state,plan,Icon}),readControls(state,{post:async(url,options)=>{
    assert.equal(url,`/api/guide/${state.id}/implementation`);assert.deepEqual(JSON.parse(options.body),{version:4,request:'',previous_build_id:null});posts++;
    return {guide_id:state.id,guide_version:state.version,build_id:'inert-default-build',current:true,status:'queued'};
  }}),async(document)=>{
    const button=byText(document,'Build application');assert.equal(button.disabled,false);await act(async()=>button.click());assert.equal(posts,1);
  });
});

test('accepted available team keeps the header Build shortcut and optional package paths',async()=>{
  const {state}=fixture(false);
  const read=readControls(state);
  await mounted(React.createElement(GuidedSetup,{active:true,bp:null,Icon,onBlueprint:()=>{},config:{architect:{available:true}}}),async(url,options)=>{
    if(url.startsWith(`/api/guide/${state.id}/deployment/options?`))return {
      source_kind:'native-team',source_id:'a'.repeat(64),source_digest:'a'.repeat(64),build_id:null,
      current:true,guide_version:state.version,current_guide_version:state.version,
      target:'mac',host_target:'mac',supported:false,
    };
    return read(url,options);
  },async(document,calls)=>{
    const shortcuts=document.querySelector('nav[aria-label="Try, build and run this proposal"]');
    assert.equal(shortcuts.querySelector('a[href="#setup-implementation"]').textContent,'Build');
    assert.equal(byText(document,'Download agent team package').disabled,false);
    assert.equal(byText(document,'Install agent team').disabled,false);
    assert.match(document.querySelector('.setup-chat-top small').textContent,/ready to install/);
    assert.ok(!document.querySelector('.setup-visually-hidden[role="status"]').textContent.includes('package preparation is pending'));
    assert.equal(calls.filter(call=>call.method==='POST').length,0);
  },state);
});

test('saved availability never describes another version, source, guide or preview object',()=>{
  for(const change of ['version','source','guide','preview']) {
    const {state,plan}=fixture();let displayed=plan;
    if(change==='version')state.package_availability.guide_version++;
    if(change==='source')state.package_availability.source='proposal';
    if(change==='guide')state.package_availability.guide_id='other';
    if(change==='preview')displayed=structuredClone(plan);
    const html=renderToStaticMarkup(React.createElement(AcquisitionReview,{state,plan:displayed,disabled:true}));
    assert.ok(!html.includes(reason),`${change} must not reuse saved package availability`);
  }
  const {state,plan}=fixture();assert.ok(renderToStaticMarkup(React.createElement(AcquisitionReview,{state,plan})).includes(reason));
});

test('selected source blocks package actions and progress invites revision, not an impossible build',()=>{
  const {state}=fixture();assert.equal(nativeTeamCanInstall(state),false);
  const progress=setupProgress(state,{},{});assert.equal(progress.kind,'package-pending');assert.equal(progress.detail,reason);assert.deepEqual(progress.actions,[]);
  const html=renderToStaticMarkup(React.createElement(NativeTeamPackage,{state,Icon,onPackage:()=>{},onInstall:()=>{},onRevise:()=>{}}));
  assert.ok(html.includes(reason));assert.ok(html.includes('Revise with guide'));
  assert.ok(!html.includes('Install agent team'));assert.ok(!html.includes('Install and start the package'));
});

test('checked installation is a desired reviewed capability while package and fresh-build admission remain pending',async()=>{
  const {state,plan}=fixture(true,'ready','checked_installation');
  const original=structuredClone(plan);
  const changes=[];
  await mounted(React.createElement(React.Fragment,null,
    React.createElement(AcquisitionReview,{state,plan,onRequestChange:text=>changes.push(text)}),
    React.createElement(NativeTeamPackage,{state,Icon,onPackage:()=>assert.fail('No package admission'),onInstall:()=>assert.fail('No installation admission')}),
    React.createElement(ImplementationPanel,{state,plan,Icon})),readControls(state),async(document,calls)=>{
    const review=document.querySelector('section[aria-label="Connection preparation"]');
    assert.ok(review.textContent.includes('install requested software after its exact review'));
    assert.ok(review.textContent.includes('The exact software installation must be reviewed before it can run.'));
    assert.ok(review.textContent.includes('Choosing this capability installs nothing and connects no account.'));
    assert.ok(!review.textContent.includes('native implementation'));
    assert.ok(review.textContent.includes(reason));
    assert.equal(byText(document,'Download agent team package').disabled,true);
    assert.ok(!byText(document,'Install agent team'));
    const build=byText(document,'Build application');assert.equal(build.disabled,true);
    await act(async()=>build.click());
    await act(async()=>byText(review,'Discuss removing this setup helper').click());
    assert.equal(calls.filter(call=>call.method==='POST').length,0);
    assert.equal(changes.length,1);
    assert.ok(changes[0].includes('Discuss removing connection preparation'));
    assert.ok(changes[0].includes('ask before changing the plan'));
    assert.deepEqual(plan,original,'capability rendering and revision intent do not mutate identities or grants');
    assert.equal(nativeTeamCanInstall(state),false);
    assert.equal(setupProgress(state,{},{}).kind,'package-pending');
  });
});

test('legacy preparation and inspection summaries retain their original access boundary',()=>{
  for(const [mode,summary] of [
    ['preparation','Find connections and prepare their setup.'],
    ['inert_acquisition','Find connections, prepare setup, and download and inspect candidate packages.']]) {
    const {state,plan}=fixture(true,'ready',mode);
    const html=renderToStaticMarkup(React.createElement(AcquisitionReview,{state,plan}));
    assert.ok(html.includes(summary));
    assert.ok(html.includes('Preparing a connection does not activate it or grant account access.'));
    assert.ok(!html.includes('install requested software'));
  }
});

test('retained current candidate can continue, but stale candidate and current-runtime refresh stay blocked',async()=>{
  const {state,plan}=fixture();
  const receipt={guide_id:state.id,guide_version:state.version,current_guide_version:state.version,current:true,build_id:'retained-source',
    status:'failed',worker_stop_confirmed:true,cleanup_pending:false,source_digest:'b'.repeat(64),framework:{runtime_candidate:{schema:1,source:'inert exact-source specimen'}}};
  let posts=0;
  await mounted(React.createElement(ImplementationPanel,{state,plan,Icon}),readControls(state,{receipt,post:async(url)=>{
    assert.equal(url,`/api/guide/${state.id}/implementation`);posts++;return {...receipt,status:'queued'};
  }}),async(document)=>{
    assert.equal(byText(document,'Resume saved build').disabled,false);
    assert.equal(byText(document,'Rebuild with updated runtime').disabled,true);
    await act(async()=>byText(document,'Resume saved build').click());assert.equal(posts,1);
  });
  receipt.current=false;
  await mounted(React.createElement(ImplementationPanel,{state,plan,Icon}),readControls(state,{receipt}),async(document,calls)=>{
    assert.equal(byText(document,'Build updated proposal').disabled,true);assert.equal(calls.filter(call=>call.method==='POST').length,0);
  });
});

test('package-pending plan keeps current saved-build navigation and continuation',async()=>{
  const {state}=fixture();
  const receipt={guide_id:state.id,guide_version:state.version,current_guide_version:state.version,current:true,build_id:'retained-source',
    status:'failed',worker_stop_confirmed:true,cleanup_pending:false,source_digest:'b'.repeat(64),framework:{runtime_candidate:{schema:1,source:'inert exact-source specimen'}}};
  await mounted(React.createElement(GuidedSetup,{active:true,bp:null,Icon,onBlueprint:()=>{},config:{architect:{available:true}}}),readControls(state,{receipt}),async(document,calls)=>{
    const progress=setupProgress(state,{build:receipt},{});
    assert.notEqual(progress.kind,'package-pending');
    assert.equal(document.querySelector('.setup-visually-hidden[role="status"]').textContent,progress.title);
    const shortcuts=document.querySelector('nav[aria-label="Try, build and run this proposal"]');
    assert.equal(shortcuts.querySelector('a[href="#setup-implementation"]').textContent,'Build');
    assert.equal(byText(document,'Resume saved build').disabled,false);
    assert.equal(byText(document,'Rebuild with updated runtime').disabled,true);
    assert.equal(byText(document,'Download agent team package').disabled,true);
    assert.equal(byText(document.querySelector('.setup-team-package'),'Revise with guide').disabled,false);
    assert.equal(calls.filter(call=>call.method==='POST').length,0);
  },state);
});

test('selected new-package hold preserves an active build Stop action and activity',async()=>{
  const {state,plan}=fixture();const receipt={guide_id:state.id,guide_version:state.version,current:true,build_id:'active-source',status:'running'};
  await mounted(React.createElement(ImplementationPanel,{state,plan,Icon}),readControls(state,{receipt}),async(document)=>{
    assert.equal(byText(document,'Stop build').disabled,false);assert.ok(document.body.textContent.includes('Building your application'));
    assert.ok(document.body.textContent.includes(reason));assert.ok(!byText(document,'Build application'));
  });
});


test('passive status observation keeps loading/unknown guidance without promising saved continuation',async()=>{
  // The existing load lifetime rechecks the same saved guide without Build or
  // guide mutation. Last-known receipt data remains available for history.
  for (const candidate of [true,false]) {
    const {state,plan}=fixture();
    const receipt={guide_id:state.id,guide_version:state.version,current_guide_version:state.version,current:true,
      build_id:'last-known-source',status:'failed',worker_stop_confirmed:true,cleanup_pending:false,source_digest:'c'.repeat(64)};
    if(candidate)receipt.framework={runtime_candidate:{schema:1,source:'inert exact-source specimen'}};
    let token=1, phase='known', completeRead;
    const scope={token:()=>token,capture:(guide,expected)=>()=>guide.id===state.id && (expected===undefined||expected===token)};
    const ordinary=readControls(state,{receipt});
    const read=async(url,options={})=>{
      if(url===`/api/guide/${state.id}/implementation` && (options.method||'GET')==='GET') {
        if(phase==='waiting')return new Promise(resolve=>{completeRead=resolve;});
        if(phase==='unavailable')return {httpError:503,detail:'Inert status observation is unavailable'};
      }
      return ordinary(url,options);
    };
    const element=()=>React.createElement(ImplementationPanel,{state,plan,Icon,selectionScope:scope});
    await mounted(element(),read,async(document,calls,render)=>{
      assert.equal(byText(document,'Resume saved build').disabled,!candidate);
      phase='waiting';token++;
      await render(element());
      assert.ok(completeRead,'the new passive owner read is pending');
      assert.ok(document.body.textContent.includes('Checking your saved build'));
      assert.ok(document.body.textContent.includes('Forge is checking for saved application work before starting a build.'));
      assert.ok(document.body.textContent.includes('You can keep editing your builder note while this is checked.'));
      assert.ok(!document.body.textContent.includes('You can continue this saved application.'));
      assert.ok(document.body.textContent.includes(reason));
      phase='unavailable';
      await act(async()=>{completeRead({httpError:503,detail:'Inert status observation is unavailable'});await new Promise(resolve=>setTimeout(resolve,5));});
      assert.ok(document.body.textContent.includes('Build status is unavailable'));
      assert.ok(document.body.textContent.includes('Forge could not check the latest build status.'));
      assert.ok(document.body.textContent.includes('Refresh build status to continue. Your builder note is kept while you wait.'));
      assert.ok(!document.body.textContent.includes('You can continue this saved application.'));
      assert.equal(byText(document,'Resume saved build').disabled,true);
      assert.equal(byText(document,'Refresh build status').disabled,false);
      assert.equal(calls.filter(call=>call.method==='POST').length,0);
    });
  }
});
