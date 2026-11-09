import assert from 'node:assert/strict';
import test from 'node:test';
import {createRequire} from 'node:module';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {build} from 'esbuild';
import React,{act} from 'react';
import {createRoot} from 'react-dom/client';
import {createSimulation,simulationSourceKey} from './simulation-state.mjs';

const require=createRequire(import.meta.url);
const {JSDOM}=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
const rendered=await build({entryPoints:[fileURLToPath(new URL('evolving-plan.jsx',import.meta.url))],bundle:true,
  write:false,format:'esm',platform:'node',logLevel:'silent',jsx:'automatic',loader:{'.css':'empty'},
  nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],plugins:[{name:'inert-preview-and-shared-react',setup(builder){
    builder.onResolve({filter:/proposal-simulation\.jsx$/},()=>({path:'owned-inert-preview',namespace:'inert'}));
    builder.onLoad({filter:/.*/,namespace:'inert'},()=>({contents:`import React from 'react';
      export function ProposalSimulation(props){globalThis.__ownedPreviewProps=props;
        return React.createElement('div',{'data-inert-preview':true},'Inert proposal preview');}`,loader:'js'}));
    builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({path:pathToFileURL(require.resolve(args.path)).href,external:true}));
  }}]});
const {EvolvingPlan}=await import(`data:text/javascript;base64,${Buffer.from(rendered.outputFiles[0].text).toString('base64')}`);
const fact=(summary,source)=>({summary,status:'confirmed',source_message_ids:[source]});
const blueprint=name=>({project:{name,slug:'owned-preview'},agent_classes:[{id:'owner',name:'Context owner',
  description:'Own the useful work',instructions:'Keep the current context.',instances:1,members:[],
  provider:'openai',model:'fixture',tools:{memory:true,mcp:[],notes:'none'}}],
  topology:{mode:'manual',entrypoint:'owner:main',edges:[]},connections:[],knowledge:{mode:'none'},flows:[]});
const original=()=>({id:'owned-guide',version:4,status:'question',proposal:blueprint('Saved proposal'),
  brief:{facts:{outcome:fact('Saved earlier understanding','earlier-answer'),
    interfaces:fact('Saved earlier interface','earlier-answer')},requirements:[]},
  readiness:{context_ready:false},completed_stages:[],answers:[],messages:[]});
const running=()=>{const state=original();state.operation={status:'running',version:4,request_id:'a'.repeat(64)};
  state.live_design={guide_id:state.id,guide_version:4,request_id:state.operation.request_id,
    blueprint:blueprint('Checked evolving proposal'),brief:{facts:{outcome:fact('Checked new understanding','current-answer'),
      interfaces:fact('Checked new interface','current-answer')},requirements:[]}};return state;};
const previewSource=props=>simulationSourceKey(props.plan,createSimulation(props.plan,'__overview__',props.interfaceFact,props.designBrief));

async function mounted(verify){
  const dom=new JSDOM('<div id="root"></div>',{url:'http://offline.test/'});
  const descriptors=new Map(),replacements={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,
    HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true,
    fetch:()=>{throw new Error('Presentation projection must not admit or fetch any work.');}};
  descriptors.set('__ownedPreviewProps',Object.getOwnPropertyDescriptor(globalThis,'__ownedPreviewProps'));
  for(const [key,value] of Object.entries(replacements)){
    descriptors.set(key,Object.getOwnPropertyDescriptor(globalThis,key));Object.defineProperty(globalThis,key,{value,configurable:true});}
  const root=createRoot(dom.window.document.getElementById('root'));
  const render=async(state,extra={})=>act(async()=>root.render(React.createElement('div',null,
    React.createElement('textarea',{'aria-label':'Unsent guide draft',defaultValue:'  My next thought\n stays unsent  '}),
    React.createElement(EvolvingPlan,{state,busy:state.operation?.status==='running',...extra}))));
  try{await verify({render,document:dom.window.document,props:()=>globalThis.__ownedPreviewProps});}
  finally{await act(async()=>root.unmount());dom.window.close();for(const [key,descriptor] of descriptors){
    if(descriptor)Object.defineProperty(globalThis,key,descriptor);else delete globalThis[key];}}
}

test('mounted staged preview and understanding share the exact checked pair without replacing saved facts or unsent draft',async()=>{
  await mounted(async({render,document,props})=>{
    const state=running(),untouched=structuredClone(state);
    await render(state);
    assert.equal(props().plan,state.live_design.blueprint);
    assert.equal(props().designBrief,state.live_design.brief);
    assert.equal(props().interfaceFact,state.live_design.brief.facts.interfaces);
    assert.match(document.body.textContent,/Checked draft preview - guide response still in progress/);
    assert.match(document.querySelector('.ep-brief').textContent,/Checked new understanding/);
    assert.match(document.querySelector('.ep-brief').textContent,/Checked new interface/);
    assert.doesNotMatch(document.querySelector('.ep-brief').textContent,/Saved earlier understanding/);
    assert.match(document.querySelector('.ep-brief').textContent,/Checked draft understanding for this proposal preview/);
    assert.equal(document.querySelector('textarea').value,'  My next thought\n stays unsent  ');
    const stagedSource=previewSource(props());
    const completed={...state,version:5,proposal:state.live_design.blueprint,brief:state.live_design.brief,
      operation:{...state.operation,status:'completed'}};
    await render(completed);
    assert.equal(previewSource(props()),stagedSource,'An unchanged saved pair does not schedule a different factual source');
    assert.doesNotMatch(document.body.textContent,/Checked draft preview - guide response still in progress/);
    assert.match(document.body.textContent,/Checked new understanding/);
    assert.doesNotMatch(document.body.textContent,/Checked draft understanding for this proposal preview/);
    assert.equal(document.querySelector('textarea').value,'  My next thought\n stays unsent  ');
    assert.deepEqual(state,untouched);
  });
});

test('mounted early topology excludes older context until the checked pair arrives and retains saved authority',async()=>{
  await mounted(async({render,document,props})=>{
    const early=running();early.live_design.brief=null;early.live_design.context_pending=true;
    early.status='ready';early.completed_stages=['destination','knowledge','autonomy'];
    early.brief.facts.authority=fact('Earlier permission preference','earlier-answer');
    early.readiness.decision_blockers=['Saved audience decision'];
    early.brief.requirements=[{id:'saved-check',outcome:'Saved application prerequisite',disposition:'prerequisite'}];
    const untouched=structuredClone(early);
    await render(early);
    assert.equal(props().plan,early.live_design.blueprint);
    assert.equal(props().designBrief,null);assert.equal(props().interfaceFact,undefined);
    assert.match(document.body.textContent,/Provisional topology - guide context still in progress/);
    assert.doesNotMatch(document.body.textContent,/Accepted plan - ready to package|Saved earlier understanding|Saved earlier interface|Earlier permission preference/);
    assert.equal(document.querySelector('.ep-brief') === null,true);
    assert.equal(document.querySelector('.ep-status-dot').classList.contains('is-ready'),false);
    const facts=[...document.querySelectorAll('.ep-facts > div')];
    for(const row of facts.filter(row=>row.querySelector('dt').textContent!=='Current access'))
      assert.equal(row.querySelector('dd').classList.contains('is-undecided'),true);
    const checks=document.querySelector('.ep-requirements');
    assert.match(checks.textContent,/Last saved design checks|Saved audience decision|Saved application prerequisite/);
    assert.match(checks.textContent,/These checks belong to your last saved design/);
    assert.ok([...document.querySelectorAll('.ep-count button')].every(button=>button.disabled));
    const earlySource=previewSource(props());
    assert.equal(Object.hasOwn(createSimulation(props().plan,'__overview__',props().interfaceFact,props().designBrief),'design_context'),false);
    const paired=running();paired.readiness=early.readiness;paired.brief=early.brief;
    await render(paired);
    assert.equal(props().plan,paired.live_design.blueprint);
    assert.equal(props().designBrief,paired.live_design.brief);
    assert.notEqual(previewSource(props()),earlySource);
    assert.match(document.querySelector('.ep-brief').textContent,/Checked new understanding/);
    assert.doesNotMatch(document.body.textContent,/Provisional topology/);
    const pairedSource=previewSource(props());
    const complete={...paired,version:5,proposal:paired.live_design.blueprint,brief:paired.live_design.brief,
      operation:{...paired.operation,status:'completed'}};
    await render(complete);assert.equal(previewSource(props()),pairedSource);
    const failed={...early,operation:{...early.operation,status:'failed'}};
    await render(failed);assert.equal(props().plan,failed.proposal);assert.equal(props().designBrief,failed.brief);
    assert.doesNotMatch(document.body.textContent,/Provisional topology/);
    assert.equal(document.querySelector('textarea').value,'  My next thought\n stays unsent  ');
    assert.deepEqual(early,untouched);
  });
});

test('mounted delayed foreign stage, failed turn and pending manual override cannot borrow checked context',async()=>{
  await mounted(async({render,document,props})=>{
    const state=running();await render(state);
    for(const changed of [{...state,operation:{...state.operation,request_id:'b'.repeat(64)}},
      {...state,id:'different-guide'}, {...state,version:5},
      {...state,operation:{...state.operation,status:'failed'}}]){
      await render(changed);
      assert.equal(props().plan,changed.proposal);assert.equal(props().designBrief,changed.brief);
      assert.doesNotMatch(document.body.textContent,/Checked draft preview - guide response still in progress/);
      assert.match(document.querySelector('.ep-brief').textContent,/Saved earlier understanding/);
      assert.doesNotMatch(document.querySelector('.ep-brief').textContent,/Checked new understanding|Checked draft understanding/);
    }
    const manual=blueprint('My unsent manual adjustment');await render(state,{plan:manual,pending:true});
    assert.equal(props().plan,manual);assert.equal(props().designBrief,state.brief);
    assert.match(document.body.textContent,/Your changes are waiting for guide review/);
    assert.doesNotMatch(document.body.textContent,/Checked draft preview - guide response still in progress/);
    assert.match(document.querySelector('.ep-brief').textContent,/Saved earlier understanding/);
    assert.doesNotMatch(document.querySelector('.ep-brief').textContent,/Checked new understanding|Checked draft understanding/);
    assert.equal(document.querySelector('textarea').value,'  My next thought\n stays unsent  ');
  });
});

test('mounted checked draft context labels saved readiness without borrowing draft decision authority',async()=>{
  await mounted(async({render,document})=>{
    const state=running();
    state.readiness.decision_blockers=['Saved audience decision'];
    state.brief.requirements=[{id:'saved-check',outcome:'Saved application prerequisite',disposition:'prerequisite'}];
    state.live_design.brief.requirements=[{id:'draft-decision',outcome:'Draft-only decision',disposition:'open'}];
    const untouched=structuredClone(state);
    await render(state);
    const checks=document.querySelector('.ep-requirements');
    assert.match(checks.textContent,/Last saved design checks/);
    assert.match(checks.textContent,/These checks belong to your last saved design/);
    assert.match(checks.textContent,/Saved audience decision/);
    assert.match(checks.textContent,/Saved application prerequisite/);
    assert.doesNotMatch(checks.textContent,/Draft-only decision/);
    assert.match(document.querySelector('.ep-brief').textContent,/Checked new understanding/);
    assert.deepEqual(state,untouched);
  });
});

test('mounted checked draft knowledge stays proposed while manual and saved context keep their own status',async()=>{
  await mounted(async({render,document})=>{
    const state=running();
    state.brief.facts.knowledge=fact('Conversation memory only','earlier-answer');
    state.live_design.blueprint.knowledge={mode:'local'};
    state.live_design.brief.facts.knowledge={...fact('An optional local knowledge service','current-answer'),status:'proposed'};
    const untouched=structuredClone(state);
    const knowledge=()=>[...document.querySelectorAll('.ep-facts > div')]
      .find(row=>row.querySelector('dt').textContent==='Knowledge').querySelector('dd');
    await render(state);
    assert.equal(knowledge().textContent,'Proposed: New knowledge service');
    assert.equal(knowledge().classList.contains('is-undecided'),true);
    await render(state,{plan:structuredClone(state.live_design.blueprint),pending:true});
    assert.equal(knowledge().textContent,'New knowledge service');
    assert.equal(knowledge().classList.contains('is-undecided'),false);
    const completed={...state,version:5,proposal:state.live_design.blueprint,brief:state.live_design.brief,
      operation:{...state.operation,status:'completed'}};
    await render(completed);
    assert.equal(knowledge().textContent,'Proposed: New knowledge service');
    assert.equal(knowledge().classList.contains('is-undecided'),true);
    assert.deepEqual(state,untouched);
  });
});
