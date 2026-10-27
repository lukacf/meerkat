import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act } from 'react';
import { captureGuideResumeInput, inputKey, manualKey, GUIDE_CREATION_KEY } from './wizard-state.mjs';
import { GUIDE_KEY } from './guided-state.mjs';

const { JSDOM }=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
const require=createRequire(import.meta.url);
const realSetTimeout=globalThis.setTimeout;
const settle=()=>new Promise(resolve=>realSetTimeout(resolve,5));
const bundled=await build({entryPoints:[fileURLToPath(new URL('guided.jsx',import.meta.url))],bundle:true,write:false,
  format:'esm',platform:'node',logLevel:'silent',jsx:'automatic',loader:{'.css':'empty'},
  nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
  alias:{'@console-core':fileURLToPath(new URL('../../packages/console-core/src',import.meta.url))},
  plugins:[{name:'shared-real-react',setup(builder){builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({
    path:pathToFileURL(require.resolve(args.path)).href,external:true}));
    // These mounted guide fixtures inspect checked inputs, not generated code or rendering.
    builder.onResolve({filter:/proposal-simulation\.jsx$/},()=>({path:'guide-input-preview',namespace:'inert'}));
    builder.onLoad({filter:/.*/,namespace:'inert'},()=>({contents:`import React from 'react';
      export function ProposalSimulation(props){return React.createElement('div',{'data-guide-input-preview':true,
        'data-proposal-name':props.plan?.project.name || ''},props.designBrief?.facts?.outcome?.summary || '');}`,loader:'js'}));}}]});
const { GuidedSetup,GuideRecoveryNotice }=await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);

const saved={id:'owned-synthetic-guide',version:5,status:'question',stage:'purpose',completed_stages:[],answers:[],messages:[],
  brief:{facts:{},requirements:[]},capabilities:{available:true},
  question:{stage:'purpose',title:'Saved server question',description:'Choose the next step.',multiple:true,
    options:[{id:'newer-choice',label:'Keep the newer choice',description:'A local selection.'}]},
  operation:{status:'completed',request_id:'a'.repeat(64),version:4,started_at:'2026-10-01T21:00:00Z',
    request:{version:4,action:'answer',text:'Already submitted',selected:['old-choice']}}};
const draft={key:inputKey(saved),draft:'  My unsent next thought\n\u00e5\u{1f9ed}  ',selected:['newer-choice'],mode:'chat'};

async function mountedGuide(storageDescriptor, verify, read, { allowedPosts = [], sessionValues = [], onBlueprint = () => {} } = {}){
  const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/',pretendToBeVisual:true});
  for(const [key,value] of sessionValues)dom.window.sessionStorage.setItem(key,value);
  const replacements={window:{value:dom.window},document:{value:dom.window.document},navigator:{value:dom.window.navigator},
    HTMLElement:{value:dom.window.HTMLElement},requestAnimationFrame:{value:dom.window.requestAnimationFrame.bind(dom.window)},
    cancelAnimationFrame:{value:dom.window.cancelAnimationFrame.bind(dom.window)},IS_REACT_ACT_ENVIRONMENT:{value:true},
    matchMedia:{value:()=>({matches:false,addEventListener(){},removeEventListener(){}})},
    localStorage:storageDescriptor,sessionStorage:{value:dom.window.sessionStorage}};
  const prior=new Map(Object.keys(replacements).map(key=>[key,Object.getOwnPropertyDescriptor(globalThis,key)]));
  const calls=[],serverBefore=structuredClone(saved);
  prior.set('fetch',Object.getOwnPropertyDescriptor(globalThis,'fetch'));
  replacements.fetch={value:async(url,options={})=>{
    const method=options.method || 'GET';
    calls.push({url,method,body:options.body && JSON.parse(options.body)});
    assert.ok(method==='GET' || method==='POST' && allowedPosts.includes(url),
      `Only explicitly allowed offline requests are admitted: ${method} ${url}`);
    const body=read ? await read(url,options) : url==='/api/guide/list'?{setups:[]}:
      url==='/api/guide/new' || url===`/api/guide/${saved.id}`?structuredClone(saved):
      url.startsWith(`/api/guide/${saved.id}/timeline`)?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
    assert.ok(body,`Unexpected offline fixture read: ${url}`);
    if(body.httpError)return {ok:false,status:body.httpError,json:async()=>({detail:body.detail})};
    return {ok:true,json:async()=>body};
  }};
  for(const [key,descriptor] of Object.entries(replacements))Object.defineProperty(globalThis,key,{configurable:true,...descriptor});
  dom.window.HTMLElement.prototype.scrollTo=function(){};
  dom.window.HTMLElement.prototype.scrollIntoView=function(){};
  // Focus and layout are browser checks, not part of these ownership fixtures.
  dom.window.HTMLElement.prototype.focus=function(){};
  const { createRoot }=await import('react-dom/client');
  const root=createRoot(dom.window.document.getElementById('root'));
  try{
    await act(async()=>root.render(React.createElement(GuidedSetup,{active:true,bp:null,Icon:()=>null,onBlueprint,config:{architect:{available:true}}})));
    await act(async()=>new Promise(resolve=>realSetTimeout(resolve,15)));
    await verify(dom.window.document,calls);
    assert.deepEqual(saved,serverBefore);
    assert.ok(calls.length);
    assert.ok(calls.every(call=>call.method==='GET' || allowedPosts.includes(call.url)));
  }finally{
    await act(async()=>root.unmount());
    dom.window.close();
    for(const [key,descriptor] of prior){if(descriptor)Object.defineProperty(globalThis,key,descriptor);else delete globalThis[key];}
  }
}


// This case uses the existing mounted guide harness and real draft/navigation owners.
// Only generated animation is inert; Send, choice, Help and history controls are real.
test('a pending saved-guide switch pauses answer controls without consuming either guide draft',async()=>{
  const guide=(id,title)=>({id,version:id==='owned-switch-a'?5:8,status:'question',stage:'audience',
    completed_stages:[],answers:[],messages:[],brief:{facts:{},requirements:[]},
    capabilities:{available:true},question:{id:`audience-${id}`,stage:'audience',title,
      description:'Choose the people this system should help.',multiple:true,allow_text:true,
      options:[{id:'solo',label:'Just me',description:'One continuing personal context.'},
        {id:'crew',label:'A shared crew',description:'Separate people with shared work.'}]}});
  const a=guide('owned-switch-a','Who should the workshop help?');
  const b=guide('owned-switch-b','Who should the observing system help?');
  const draftA={key:inputKey(a),draft:'  Keep my workshop idea unsent.\nExact whitespace stays.  ',selected:['solo'],mode:'answer'};
  const editedA={...draftA,draft:'  Keep my workshop idea unsent.\nI can still refine it while another setup opens.  '};
  const draftB={key:inputKey(b),draft:'  Separate observing crew context.  ',selected:['crew'],mode:'answer'};
  const values=new Map([[GUIDE_KEY,a.id],...[draftA,draftB].map(input=>[input.key,JSON.stringify(input)])]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let releaseFirst,releaseSecond,readsB=0;
  const first=new Promise(resolve=>{releaseFirst=resolve;});
  const second=new Promise(resolve=>{releaseSecond=resolve;});
  const read=url=>url===`/api/guide/${a.id}`?structuredClone(a):
    url===`/api/guide/${b.id}`?(++readsB===1?first:second):
    url==='/api/guide/list'?{setups:[{id:a.id,name:'Workshop setup A',status:'question'},
      {id:b.id,name:'Observing setup B',status:'question'}]}:
    url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    const send=()=>document.querySelector('#setup-send');
    const question=()=>document.querySelector('.setup-current-question');
    const help=()=>[...question().querySelectorAll('button')].find(node=>node.textContent==='Help me decide');
    const choices=()=>[...question().querySelectorAll('.setup-choice')];
    const selectedAnswer=()=>document.querySelector('.setup-selected-answer button');
    const props=node=>node[Object.keys(node).find(key=>key.startsWith('__reactProps$'))];
    const click=async text=>{
      const button=[...document.querySelectorAll('button')].find(node=>node.textContent.trim()===text || node.querySelector('span')?.textContent===text);
      assert.ok(button,`Missing actual navigation control: ${text}`);
      assert.equal(button.disabled,false,`Navigation remains usable: ${text}`);
      await act(async()=>{button.click();await settle();});
    };
    const controls=disabled=>{
      assert.equal(send().disabled,disabled,`Send matches the saved-guide selection boundary (${disabled})`);
      assert.equal(help().disabled,disabled,`Help matches the saved-guide selection boundary (${disabled})`);
      assert.equal(selectedAnswer().disabled,disabled,`Removing a selected answer matches the saved-guide selection boundary (${disabled})`);
      assert.equal([...question().querySelectorAll('button')].some(node=>node.textContent.trim()==='Continue'),false,'Composer Send is the only answer submission');
      assert.equal(choices().length,2);
      assert.ok(choices().every(button=>button.disabled===disabled),'Choice controls match the same selection boundary');
      assert.equal(composer().disabled,false,'The current guide draft remains editable during selection');
      assert.equal(composer().readOnly,false);
    };
    const owned=(state,input)=>{
      assert.equal(question().dataset.guideId,state.id);
      assert.ok(question().textContent.includes(state.question.title));
      assert.equal(composer().value,input.draft);
      assert.equal(document.querySelectorAll('.setup-choice.selected').length,1);
      assert.ok(document.querySelector('.setup-choice.selected').textContent.includes(
        state.question.options.find(option=>option.id===input.selected[0]).label));
    };
    owned(a,draftA);
    controls(false);
    const staleSend=props(send()).onClick,staleHelp=props(help()).onClick,staleRemove=props(selectedAnswer()).onClick;
    await click('Your setups');
    await click('Observing setup B');
    assert.equal(readsB,1,'The actual B GET is held before completion');
    owned(a,draftA);
    controls(true);
    assert.equal(values.get(GUIDE_KEY),a.id,'An unaccepted selection does not change the saved guide');
    await act(async()=>{
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set.call(composer(),editedA.draft);
      composer().dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
      await settle();
    });
    owned(a,editedA);
    assert.deepEqual(JSON.parse(values.get(draftA.key)),editedA,'Editing remains owned by A during B loading');
    assert.deepEqual(JSON.parse(values.get(draftB.key)),draftB,'B is not overwritten by the visible A composer');
    await act(async()=>{
      send().click();help().click();selectedAnswer().click();
      for(const button of choices())button.click();
      composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
      // Previously captured enabled handlers still hit the existing synchronous fence.
      staleSend();staleHelp();
      await settle();
    });
    assert.equal(calls.filter(call=>call.method==='POST').length,0);
    owned(a,editedA);
    await act(async()=>{releaseFirst({httpError:503,detail:'Synthetic saved setup read failed.'});await settle();});
    assert.match(document.body.textContent,/That setup did not open/);
    owned(a,editedA);
    controls(false);
    assert.equal(values.get(GUIDE_KEY),a.id);
    assert.deepEqual(JSON.parse(values.get(draftA.key)),editedA);
    assert.deepEqual(JSON.parse(values.get(draftB.key)),draftB);
    await click('Retry selected setup');
    assert.equal(readsB,2,'Retry uses the same real selection owner');
    owned(a,editedA);
    controls(true);
    await act(async()=>{releaseSecond(structuredClone(b));await settle();});
    owned(b,draftB);
    controls(false);
    await act(async()=>{staleRemove();await settle();});
    owned(b,draftB);
    assert.deepEqual(JSON.parse(values.get(draftB.key)),draftB,'A retained removal handler cannot change the newly selected guide answer');
    assert.equal(values.get(GUIDE_KEY),b.id,'Only the accepted B read changes the selected guide');
    assert.deepEqual(JSON.parse(values.get(draftA.key)),editedA);
    assert.deepEqual(JSON.parse(values.get(draftB.key)),draftB);
    await click('Your setups');
    await click('Workshop setup A');
    owned(a,editedA);
    controls(false);
    assert.equal(values.get(GUIDE_KEY),a.id);
    assert.deepEqual(JSON.parse(values.get(draftB.key)),draftB);
    assert.ok(calls.every(call=>call.method==='GET'),'Navigation and disabled answer controls never send a request');
  },read);
});

test('an unreadable initial saved setup can be left deliberately without losing its draft or duplicating navigation',async()=>{
  const values=new Map([[GUIDE_KEY,saved.id],[draft.key,JSON.stringify(draft)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  const blank={id:null,version:0,status:'question',stage:'purpose',completed_stages:[],answers:[],messages:[],
    brief:{facts:{},requirements:[]},capabilities:{available:true},
    question:{stage:'purpose',title:'What do you want to build?',description:'Describe your idea.',options:[]}};
  let releaseNew,savedReads=0;
  const newRead=new Promise(resolve=>{releaseNew=resolve;});
  const read=url=>url===`/api/guide/${saved.id}`?(++savedReads===1?
    {httpError:503,detail:'This saved conversation could not be read.'}:structuredClone(saved)):
    url==='/api/guide/new'?newRead:
    url==='/api/guide/list'?{setups:[{id:saved.id,name:'Original saved setup',status:'question'}]}:
    url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  await mountedGuide({value:durable},async(document,calls)=>{
    const button=text=>[...document.querySelectorAll('button')].find(node=>node.textContent.trim()===text || node.querySelector('span')?.textContent===text);
    const props=node=>node[Object.keys(node).find(key=>key.startsWith('__reactProps$'))];
    assert.match(document.body.textContent,/This saved conversation could not be read/);
    assert.equal(calls.filter(call=>call.url==='/api/guide/new').length,0,'Read failure must not silently replace a saved conversation');
    const start=button('Start a new setup');
    assert.ok(start,'A deliberate escape is available alongside retry');
    const retainedStart=props(start).onClick;
    await act(async()=>{start.click();retainedStart();await settle();});
    assert.equal(calls.filter(call=>call.url==='/api/guide/new').length,1,'Repeated or retained clicks share the pending selection fence');
    assert.equal(values.get(GUIDE_KEY),saved.id,'Keep the original selection until another setup opens');
    assert.equal(values.get(draft.key),JSON.stringify(draft));
    await act(async()=>{releaseNew(structuredClone(blank));await settle();});
    assert.ok(document.querySelector('#setup-message'),'The new conversation is usable');
    assert.equal(values.get(GUIDE_KEY),undefined,'The new unsaved conversation no longer reopens the unreadable selection');
    assert.equal(values.get(draft.key),JSON.stringify(draft),'Starting again does not delete the original draft');
    await act(async()=>{retainedStart();await settle();});
    assert.equal(calls.filter(call=>call.url==='/api/guide/new').length,1,'A retained error action cannot replace the now-open conversation');
    await act(async()=>{button('Your setups').click();await settle();});
    await act(async()=>{button('Original saved setup').click();await settle();});
    assert.equal(document.querySelector('#setup-message').value,draft.draft,'The original saved conversation and exact draft can be reopened');
    assert.equal(values.get(GUIDE_KEY),saved.id);
    assert.equal(calls.filter(call=>call.method==='POST').length,0,'Escaping the read failure does not create or send an agent request');
  },read);
});

test('a failed fresh opening retains the original selection and anonymous request tracking for retry',async()=>{
  const values=new Map([[GUIDE_KEY,saved.id],[draft.key,JSON.stringify(draft)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  const anonymousTracking='12345678-1234-4123-8123-123456789abc';
  let releaseRetry,savedReads=0;
  const retryRead=new Promise(resolve=>{releaseRetry=resolve;});
  const read=url=>url===`/api/guide/${saved.id}`?(++savedReads===1?
    {httpError:503,detail:'Saved setup read failed.'}:retryRead):
    url==='/api/guide/new'?{httpError:503,detail:'A fresh setup could not open either.'}:
    url==='/api/guide/list'?{setups:[]}:
    url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  await mountedGuide({value:durable},async(document,calls)=>{
    const button=text=>[...document.querySelectorAll('button')].find(node=>node.textContent.trim()===text);
    const start=button('Start a new setup');
    assert.ok(start,'The saved read failure offers a new conversation');
    const retainedStart=start[Object.keys(start).find(key=>key.startsWith('__reactProps$'))].onClick;
    await act(async()=>{start.click();await settle();});
    assert.match(document.body.textContent,/A fresh setup could not open either/);
    assert.equal(values.get(GUIDE_KEY),saved.id,'A failed escape must retain the original selected setup');
    assert.equal(values.get(draft.key),JSON.stringify(draft));
    assert.equal(document.defaultView.sessionStorage.getItem(GUIDE_CREATION_KEY),anonymousTracking,'Opening failure must not abandon prior anonymous request recovery');
    await act(async()=>{button('Try again').click();retainedStart();await settle();});
    assert.equal(savedReads,2,'Retry returns to the retained original selection');
    assert.equal(calls.filter(call=>call.url==='/api/guide/new').length,1,'A stale escape cannot race the retry');
    await act(async()=>{releaseRetry(structuredClone(saved));await settle();});
    assert.equal(document.querySelector('#setup-message').value,draft.draft);
    assert.equal(values.get(GUIDE_KEY),saved.id);
    assert.equal(document.defaultView.sessionStorage.getItem(GUIDE_CREATION_KEY),anonymousTracking);
    assert.equal(calls.filter(call=>call.method==='POST').length,0);
  },read,{sessionValues:[[GUIDE_CREATION_KEY,anonymousTracking]]});
});
