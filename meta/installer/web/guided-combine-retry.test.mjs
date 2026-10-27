import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { inputKey, manualKey } from './wizard-state.mjs';
import { GUIDE_KEY } from './guided-state.mjs';
import { webcrypto } from 'node:crypto';

const { JSDOM }=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
const require=createRequire(import.meta.url);
const { build }=await import(pathToFileURL(require.resolve('esbuild')));
const ReactModule=await import(pathToFileURL(require.resolve('react')));
const React=ReactModule.default,{act}=ReactModule;
const realSetTimeout=globalThis.setTimeout;
const settle=()=>new Promise(resolve=>realSetTimeout(resolve,5));
const bundled=await build({entryPoints:[fileURLToPath(new URL('guided.jsx',import.meta.url))],bundle:true,write:false,
  format:'esm',platform:'node',logLevel:'silent',jsx:'automatic',loader:{'.css':'empty'},
  nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
  alias:{'@console-core':fileURLToPath(new URL('../../packages/console-core/src',import.meta.url))},
  plugins:[{name:'shared-real-react',setup(builder){
    // These parent conversation tests do not execute the animation sidebar.
    builder.onLoad({filter:/evolving-plan\.jsx$/},()=>({contents:
      'export const SetupPlan = () => null, ScopeDecisions = () => null, SetupRequirementGroups = () => null;',loader:'js'}));
    builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({
    path:pathToFileURL(require.resolve(args.path)).href,external:true}));}}]});
const { GuidedSetup }=await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);

const saved={id:'owned-synthetic-guide',version:5,status:'question',stage:'purpose',completed_stages:[],answers:[],messages:[],
  brief:{facts:{},requirements:[]},capabilities:{available:true},
  question:{stage:'purpose',title:'Saved server question',description:'Choose the next step.',multiple:true,
    options:[{id:'newer-choice',label:'Keep the newer choice',description:'A local selection.'}]},
  operation:{status:'completed',request_id:'a'.repeat(64),version:4,started_at:'2026-10-01T21:00:00Z',
    request:{version:4,action:'answer',text:'Already submitted',selected:['old-choice']}}};
const draft={key:inputKey(saved),draft:'  My unsent next thought\n\u00e5\u{1f9ed}  ',selected:['newer-choice'],mode:'chat'};

async function mountedGuide(storageDescriptor, verify, read, { allowedPosts = [], setupDom, schedule, downloads, digest,
  frames, focus = false, narrow = false, session } = {}){
  const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/',pretendToBeVisual:true});
  const replacements={window:{value:dom.window},document:{value:dom.window.document},navigator:{value:dom.window.navigator},
    HTMLElement:{value:dom.window.HTMLElement},requestAnimationFrame:{value:dom.window.requestAnimationFrame.bind(dom.window)},
    cancelAnimationFrame:{value:dom.window.cancelAnimationFrame.bind(dom.window)},IS_REACT_ACT_ENVIRONMENT:{value:true},
    matchMedia:{value:()=>({matches:narrow,addEventListener(){},removeEventListener(){}})},
    localStorage:storageDescriptor};
  if(session)replacements.sessionStorage={value:session};
  if(frames){
    let frameId=0;
    replacements.requestAnimationFrame={value:callback=>{const queued={id:++frameId,callback};frames.push(queued);return queued.id;}};
    replacements.cancelAnimationFrame={value:id=>{const queued=frames.find(frame=>frame.id===id);if(queued)queued.cancelled=true;}};
  }
  if(schedule)replacements.setTimeout={value:schedule};
  if(digest)replacements.crypto={value:{subtle:{digest}}};
  const prior=new Map(Object.keys(replacements).map(key=>[key,Object.getOwnPropertyDescriptor(globalThis,key)]));
  const originalCreateURL=URL.createObjectURL,originalRevokeURL=URL.revokeObjectURL;
  if(downloads){
    URL.createObjectURL=()=>`blob:offline-package-${downloads.length}`;
    URL.revokeObjectURL=()=>{};
    dom.window.HTMLAnchorElement.prototype.click=function(){downloads.push({href:this.href,name:this.download});};
  }
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
    return {ok:!body.httpError,status:body.httpError||200,json:async()=>body.fixtureJson?body.fixtureJson():body,
      blob:async()=>body.fixtureBlob?body.fixtureBlob():new Blob(['offline package fixture'])};
  }};
  for(const [key,descriptor] of Object.entries(replacements))Object.defineProperty(globalThis,key,{configurable:true,...descriptor});
  dom.window.HTMLElement.prototype.scrollTo=function(){};
  dom.window.HTMLElement.prototype.scrollIntoView=function(){};
  // Most ownership fixtures omit focus; navigation controls use real JSDOM focus.
  if(!focus)dom.window.HTMLElement.prototype.focus=function(){};
  setupDom?.(dom);
  const { createRoot }=await import(pathToFileURL(require.resolve('react-dom/client')));
  const root=createRoot(dom.window.document.getElementById('root'));
  const props={active:true,bp:null,Icon:()=>null,onBlueprint:()=>{},config:{architect:{available:true}}};
  try{
    await act(async()=>root.render(React.createElement(GuidedSetup,props)));
    await act(async()=>new Promise(resolve=>realSetTimeout(resolve,15)));
    await verify(dom.window.document,calls,{render:next=>root.render(React.createElement(GuidedSetup,{...props,...next}))});
    assert.deepEqual(saved,serverBefore);
    assert.ok(calls.length);
    assert.ok(calls.every(call=>call.method==='GET' || allowedPosts.includes(call.url)));
  }finally{
    await act(async()=>root.unmount());
    dom.window.close();
    if(downloads){URL.createObjectURL=originalCreateURL;URL.revokeObjectURL=originalRevokeURL;}
    for(const [key,descriptor] of prior){if(descriptor)Object.defineProperty(globalThis,key,descriptor);else delete globalThis[key];}
  }
}


test('independent ordinary saved retry never borrows a newer combined draft intent',async()=>{
  const original=structuredClone(saved);
  original.question={id:'history',stage:'knowledge',title:'Which histories matter?',description:'Offered histories.',multiple:false,
    options:[{id:'personal',label:'Personal context',description:'Private personal history.'},
      {id:'shared',label:'Shared context',description:'Shared crew history.'}]};
  original.operation={status:'failed',version:5,request_id:'a'.repeat(64),
    request:{version:5,action:'answer',selected:['personal'],text:'The original answer.',stage:null,blueprint:null}};
  const input={key:inputKey(original),draft:'A later unsent draft.',selected:[],mode:'answer'};
  const values=new Map([[GUIDE_KEY,original.id],[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key),
    key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  const respondUrl=`/api/guide/${original.id}/respond`;
  await mountedGuide({value:durable},async(document,calls)=>{
    const click=async(label)=>{const button=[...document.querySelectorAll('button')].find(item=>item.textContent.includes(label));
      assert.ok(button,`Missing ${label}`);await act(async()=>button.click());await act(async()=>settle());};
    await click('More than one fits');
    assert.equal([...document.querySelectorAll('button')].some(item=>item.textContent.includes('Choose one instead')),true);
    const before=values.get(input.key);
    await click('Review your draft');
    assert.equal(values.get(input.key),before,'Reviewing the current draft does not reconstruct the saved answer');
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[],'Reviewing is navigation only');
    await click('Retry saved answer');
    const post=calls.find(call=>call.url===respondUrl && call.method==='POST');assert.ok(post,'Actual parent submit path sends the retry');
    assert.deepEqual(post.body.selected,['personal']);assert.equal(post.body.text,'The original answer.');
    assert.equal(Object.hasOwn(post.body,'combine_options'),false,'An absent saved override stays absent, even with a newer combined draft');
    assert.equal(document.querySelector('#setup-message').value,input.draft);
  },async(url)=>url==='/api/guide/list'?{setups:[]}:url===`/api/guide/${original.id}`||url==='/api/guide/new'||url===respondUrl?structuredClone(original):
    url.startsWith(`/api/guide/${original.id}/timeline`)?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null,
    {allowedPosts:[respondUrl]});
});

function failedDraftGuide(status='failed'){
  const guide=structuredClone(saved);
  guide.question={id:'history',stage:'knowledge',title:'Which histories matter?',description:'Offered histories.',multiple:false,
    options:[{id:'personal',label:'Personal context',description:'Private history.'},
      {id:'shared',label:'Shared context',description:'Shared history.'}]};
  guide.operation={status,version:guide.version,request_id:'a'.repeat(64),started_at:'2026-10-11T09:00:00Z',
    detail:'The submitted response could not finish.',
    request:{version:guide.version,action:'answer',selected:['personal'],text:'The original saved answer.',combine_options:false,
      attachments:[draftReviewFile(guide.id,'b')],stage:null,blueprint:null}};
  return guide;
}
function draftReviewFile(guideId,char){return {id:char.repeat(64),sha256:'c'.repeat(64),filename:'retained.txt',
  media_type:'text/plain',size:17,url:`/api/guide/${guideId}/attachments/${char.repeat(64)}`};}
function testStorage(entries=[]){
  const values=new Map(entries);
  return {getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key),
    key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
}
const storageSnapshot=storage=>Array.from({length:storage.length},(_,index)=>{
  const key=storage.key(index);return [key,storage.getItem(key)];
});
const reviewDraftButton=document=>[...document.querySelectorAll('.setup-error button')].find(button=>button.textContent==='Review your draft');
async function flushReviewFrames(frames){
  await act(async()=>{for(const frame of frames.splice(0))if(!frame.cancelled)frame.callback(0);await settle();});
}
function realFocusOptions(frames,focuses,options={}){
  return {frames,focus:true,...options,setupDom(dom){
    const focus=dom.window.HTMLElement.prototype.focus;
    dom.window.HTMLElement.prototype.focus=function(...args){focuses.push(this);return focus.apply(this,args);};
  }};
}

for(const [status,narrow,mode] of [['failed',false,'answer'],['interrupted',true,'chat']]){
  test(`saved failure draft review preserves all current intent and files without a POST (${status})`,async()=>{
    const guide=failedDraftGuide(status),original=structuredClone(guide),frames=[],focuses=[];
    const input={key:inputKey(guide),draft:'  My newer writing\nKeep this exact.  ',selected:['personal','shared'],mode,
      combine_options:true,starting_help:{version:2,audience_id:'team',kind_id:'organizing'}};
    const storage=testStorage([[GUIDE_KEY,guide.id],[input.key,JSON.stringify(input)]]);
    const refs=[draftReviewFile(guide.id,'d')],session=testStorage([[`mobkit-guide-attachments-v1:${guide.id}`,
      JSON.stringify({guide_id:guide.id,attachments:refs})]]);
    await mountedGuide({value:storage},async(document,calls)=>{
      await flushReviewFrames(frames);
      const editor=document.querySelector('#setup-message'),button=reviewDraftButton(document);
      assert.ok(button && !button.disabled,'Every saved failure offers current-draft navigation');
      assert.match(button.closest('.setup-operation-failure').querySelector('.setup-operation-failure-note').textContent,
        /Retry uses the saved answer\. Your current writing stays separate\./);
      if(narrow)await act(async()=>{[...document.querySelectorAll('.setup-mobile-tabs button')].find(node=>node.textContent.includes('Your team')).click();});
      const before=storageSnapshot(storage),fileBefore=storageSnapshot(session);
      button.focus();
      await act(async()=>button.click());
      await flushReviewFrames(frames);
      assert.equal(document.activeElement === editor, true,'The actual connected composer receives focus');
      assert.ok(focuses.includes(editor));
      assert.equal(editor.value,input.draft,'Saved request prose never replaces newer writing');
      assert.deepEqual(storageSnapshot(storage),before,'Text, choices, context, mode, combination and journals stay exact');
      assert.deepEqual(storageSnapshot(session),fileBefore,'Current file references and their ownership stay exact');
      assert.deepEqual(guide,original,'The retained request and operation are unchanged');
      assert.deepEqual([...document.querySelectorAll('.setup-attachments li a')].map(link=>link.getAttribute('href')),refs.map(ref=>ref.url));
      if(narrow)assert.ok(document.querySelector('.setup-shell').classList.contains('setup-mobile-chat'));
      assert.deepEqual(calls.filter(call=>call.method==='POST'),[]);
    },packageReads(guide),realFocusOptions(frames,focuses,{narrow,session}));
  });
}

test('delayed draft-review focus respects later human navigation, writing, choices, mode and inactivity',async t=>{
  for(const change of ['focus','writing','choice','mode','inactive','detached'])await t.test(change,async()=>{
    const guide=failedDraftGuide(),frames=[],focuses=[];
    const input={key:inputKey(guide),draft:'The newer draft.',selected:[],mode:'chat'};
    const storage=testStorage([[GUIDE_KEY,guide.id],[input.key,JSON.stringify(input)]]);
    await mountedGuide({value:storage},async(document,calls,control)=>{
      await flushReviewFrames(frames);
      const editor=document.querySelector('#setup-message'),button=reviewDraftButton(document);
      button.focus();await act(async()=>button.click());
      const atClick=focuses.filter(node=>node===editor).length;
      if(change==='focus')document.querySelector('.setup-history-control > button').focus();
      if(change==='writing')await act(async()=>{
        Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set.call(editor,'A more recent sentence.');
        editor.dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
      });
      if(change==='choice')await act(async()=>document.querySelector('.setup-choice').click());
      if(change==='mode')await act(async()=>[...document.querySelectorAll('.setup-composer-mode button')]
        .find(node=>node.textContent.includes('Answer this question')).click());
      if(change==='inactive')await act(async()=>control.render({active:false}));
      if(change==='detached')button.remove();
      const before=storageSnapshot(storage);
      await flushReviewFrames(frames);
      assert.equal(focuses.filter(node=>node===editor).length,atClick+(change==='mode'?1:0),
        'The queued review callback cannot add focus after later human action');
      assert.deepEqual(storageSnapshot(storage),before);
      assert.deepEqual(calls.filter(call=>call.method==='POST'),[]);
      if(change==='detached')document.querySelector('.setup-error').append(button);
    },packageReads(guide),realFocusOptions(frames,focuses));
  });
});

test('a retained draft-review click and queued focus refuse a pending or changed setup selection',async()=>{
  const guide=failedDraftGuide(),next={...structuredClone(guide),id:'next-saved-guide',version:6},frames=[],focuses=[];
  let finishRead,switched=false;
  const read=packageReads(guide,null,{
    '/api/guide/list':()=>({setups:[{id:next.id,name:'Next saved setup',status:next.status}]}),
    [`/api/guide/${next.id}`]:()=>new Promise(resolve=>{finishRead=()=>resolve(structuredClone(next));}),
  });
  const input={key:inputKey(guide),draft:'Keep this current draft.',selected:[],mode:'chat'};
  const storage=testStorage([[GUIDE_KEY,guide.id],[input.key,JSON.stringify(input)]]);
  await mountedGuide({value:storage},async(document,calls)=>{
    await flushReviewFrames(frames);
    const button=reviewDraftButton(document),editor=document.querySelector('#setup-message');
    const propsKey=Object.keys(button).find(key=>key.startsWith('__reactProps$')),retainedClick=button[propsKey].onClick;
    button.focus();await act(async()=>button.click());
    await act(async()=>{document.querySelector('.setup-history-control > button').click();await settle();});
    const selection=[...document.querySelectorAll('.setup-history-menu button')].find(node=>node.textContent.includes('Next saved setup'));
    assert.ok(selection);
    await act(async()=>{selection.click();await settle();});
    assert.equal(typeof finishRead,'function','The new setup read is genuinely pending');
    const focusedBefore=focuses.filter(node=>node===editor).length;
    await act(async()=>retainedClick({currentTarget:button}));
    await flushReviewFrames(frames);
    assert.equal(focuses.filter(node=>node===editor).length,focusedBefore);
    await act(async()=>{finishRead();await settle();});switched=true;
    await flushReviewFrames(frames);
    const currentEditor=document.querySelector('#setup-message'),currentFocuses=focuses.length,before=storageSnapshot(storage);
    await act(async()=>retainedClick({currentTarget:button}));
    await flushReviewFrames(frames);
    assert.equal(focuses.length,currentFocuses,'A retained detached callback cannot focus the replacement composer');
    assert.ok(currentEditor && switched);
    assert.deepEqual(storageSnapshot(storage),before);
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[]);
  },read,realFocusOptions(frames,focuses));
});

test('draft-review queued focus cannot return after a deliberate new submission starts',async()=>{
  const guide=failedDraftGuide(),frames=[],focuses=[];
  let finishPost;
  const respondUrl=`/api/guide/${guide.id}/respond`,read=packageReads(guide,null,{
    [respondUrl]:options=>new Promise(resolve=>{finishPost=()=>resolve({...structuredClone(guide),operation:{
      ...structuredClone(guide.operation),request_id:'f'.repeat(64),started_at:'2026-10-11T09:01:00Z',
      request:JSON.parse(options.body)}});}),
  });
  const input={key:inputKey(guide),draft:'Send my new clarification.',selected:[],mode:'chat'};
  const storage=testStorage([[GUIDE_KEY,guide.id],[input.key,JSON.stringify(input)]]);
  await mountedGuide({value:storage},async(document,calls)=>{
    await flushReviewFrames(frames);
    const editor=document.querySelector('#setup-message'),button=reviewDraftButton(document);
    const propsKey=Object.keys(button).find(key=>key.startsWith('__reactProps$')),retainedClick=button[propsKey].onClick;
    button.focus();await act(async()=>button.click());
    await act(async()=>{document.querySelector('#setup-send').click();await settle();});
    assert.equal(typeof finishPost,'function','The new request is held in the actual parent send path');
    const atSend=focuses.filter(node=>node===editor).length;
    await act(async()=>retainedClick({currentTarget:button}));
    await flushReviewFrames(frames);
    assert.equal(focuses.filter(node=>node===editor).length,atSend);
    assert.deepEqual(calls.filter(call=>call.method==='POST').map(call=>call.body),[
      {version:guide.version,action:'chat',selected:[],text:input.draft}],
      'Only the separate deliberate Send is admitted');
    await act(async()=>{finishPost();await settle();});
    await flushReviewFrames(frames);
    const nextButton=reviewDraftButton(document),atCompletion=focuses.filter(node=>node===editor).length;
    assert.ok(nextButton,'The replacement failed request has its own recovery card');
    await act(async()=>retainedClick({currentTarget:nextButton}));
    await flushReviewFrames(frames);
    assert.equal(focuses.filter(node=>node===editor).length,atCompletion,
      'The old callback cannot navigate the newer request even on its connected current card');
    assert.equal(calls.filter(call=>call.method==='POST').length,1);
  },read,realFocusOptions(frames,focuses,{allowedPosts:[respondUrl]}));
});

for(const multiple of [true,false]){
  test(`${multiple?'published multiple':'combined single'} choices keep a typed Ask/change note through composer Send`,async()=>{
    const current=structuredClone(saved);
    current.question={id:'history',stage:'knowledge',title:'Which histories matter?',description:'Offered histories.',multiple,
      options:[{id:'personal',label:'Personal context',description:'Private personal history.'},
        {id:'shared',label:'Shared context',description:'Shared crew history.'}]};
    delete current.operation;
    const note='  Keep personal history separate.\nUse the shared record for handoff.  ';
    const input={key:inputKey(current),draft:'',selected:[],mode:'chat'};
    const values=new Map([[GUIDE_KEY,current.id],[input.key,JSON.stringify(input)]]);
    const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key),
      key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
    const respondUrl=`/api/guide/${current.id}/respond`;
    await mountedGuide({value:durable},async(document,calls)=>{
      const click=async(label)=>{
        const button=[...document.querySelectorAll('button')].find(item=>item.textContent.includes(label));
        assert.ok(button,`Missing ${label}`);
        await act(async()=>{button.click();await settle();});
      };
      const mode=label=>[...document.querySelectorAll('.setup-composer-mode button')].find(button=>button.textContent.includes(label));
      const textarea=document.querySelector('#setup-message');
      assert.equal(mode('Ask or suggest a change').getAttribute('aria-pressed'),'true');
      assert.equal(document.querySelector('#setup-send').getAttribute('aria-label'),'Send message');
      await act(async()=>{
        Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set.call(textarea,note);
        textarea.dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
        await settle();
      });
      assert.equal(textarea.value,note);
      assert.equal(JSON.parse(values.get(input.key)).draft,note,'The real composer change saves the exact typed note');
      if(!multiple)await click('More than one fits');
      await click('Personal context');
      assert.equal(mode('Answer this question').getAttribute('aria-pressed'),'true','Selecting a choice switches the parent composer to answer');
      assert.equal(mode('Ask or suggest a change').getAttribute('aria-pressed'),'false');
      assert.equal(document.querySelector('#setup-send').getAttribute('aria-label'),'Send answer');
      assert.equal(textarea.value,note,'Selecting a choice retains the unsent note');
      await click('Shared context');
      assert.deepEqual([...document.querySelectorAll('.setup-choice.selected strong')].map(label=>label.textContent),
        ['Personal context','Shared context']);
      assert.equal(textarea.value,note);
      assert.deepEqual(calls.filter(call=>call.method==='POST'),[],'Selection does not send the answer');
      assert.equal([...document.querySelectorAll('.setup-choice-actions button')].some(button=>button.textContent.includes('Continue')),false);
      const submitButton=document.querySelector('#setup-send');
      assert.ok(submitButton && !submitButton.disabled,'The single composer Send admits the selected answer');
      await act(async()=>{submitButton.click();await settle();});
      assert.deepEqual(calls.filter(call=>call.method==='POST'),[{url:respondUrl,method:'POST',body:{
        version:current.version,action:'answer',selected:['personal','shared'],text:note,
        ...(!multiple?{combine_options:true}:{})}}]);
      assert.equal(current.question.multiple,multiple,'The combined answer does not rewrite the published question');
    },async(url)=>url==='/api/guide/list'?{setups:[]}:
      url===`/api/guide/${current.id}`||url==='/api/guide/new'||url===respondUrl?structuredClone(current):
      url.startsWith(`/api/guide/${current.id}/timeline`)?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null,
      {allowedPosts:[respondUrl]});
  });
}

test('current-question navigation preserves the unsent note and selection without submitting',async()=>{
  const current=structuredClone(saved);
  current.question={id:'history',stage:'knowledge',title:'Which histories matter?',description:'Offered histories.',multiple:true,
    options:[{id:'personal',label:'Personal context',description:'Private personal history.'},
      {id:'shared',label:'Shared context',description:'Shared crew history.'}]};
  delete current.operation;
  const input={key:inputKey(current),draft:'  My unsent history note.\nKeep it for the answer.  ',selected:['personal'],mode:'answer'};
  const values=new Map([[GUIDE_KEY,current.id],[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key),
    key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  await mountedGuide({value:durable},async(document,calls)=>{
    const navigation=document.querySelector('.setup-question-return');
    assert.ok(navigation,'The current saved question has a persistent return action');
    assert.equal(navigation.getAttribute('aria-label'),`Show current question: ${current.question.title}`);
    const before=values.get(input.key);
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    assert.deepEqual([...document.querySelectorAll('.setup-choice.selected strong')].map(label=>label.textContent),['Personal context']);
    await act(async()=>{navigation.click();await settle();});
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    assert.deepEqual([...document.querySelectorAll('.setup-choice.selected strong')].map(label=>label.textContent),['Personal context']);
    assert.equal(values.get(input.key),before,'Navigation preserves the exact saved unsent input');
    assert.equal(document.querySelector('.setup-composer-mode button').getAttribute('aria-pressed'),'true');
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[]);
  },async(url)=>url==='/api/guide/list'?{setups:[]}:
    url===`/api/guide/${current.id}`||url==='/api/guide/new'?structuredClone(current):
    url.startsWith(`/api/guide/${current.id}/timeline`)?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null);
});

test('the current-question shortcut disappears in view and returns on history navigation without changing the draft',async()=>{
  const values=new Map([[GUIDE_KEY,saved.id],[draft.key,JSON.stringify(draft)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key),
    key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  await mountedGuide({value:durable},async(document,calls)=>{
    const before=values.get(draft.key),scroll=document.querySelector('.setup-chat-scroll');
    assert.ok(document.querySelector('.setup-question-return'),'An off-screen decision has a return shortcut');
    await act(async()=>{
      scroll.scrollTop=860;
      scroll.dispatchEvent(new document.defaultView.Event('scroll',{bubbles:true}));
    });
    assert.equal(document.querySelector('.setup-question-return') === null,true,'The visible heading and first choice do not need duplicate navigation');
    await act(async()=>{
      scroll.scrollTop=0;
      scroll.dispatchEvent(new document.defaultView.Event('scroll',{bubbles:true}));
    });
    assert.ok(document.querySelector('.setup-question-return'),'Scrolling into history restores the shortcut');
    assert.equal(document.querySelector('#setup-message').value,draft.draft);
    assert.equal(values.get(draft.key),before);
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[]);
  },undefined,{setupDom(dom){
    const prototype=dom.window.HTMLElement.prototype;
    Object.defineProperty(prototype,'clientHeight',{get(){return this.classList.contains('setup-chat-scroll')?600:0;},configurable:true});
    Object.defineProperty(prototype,'clientWidth',{get(){return this.classList.contains('setup-chat-scroll')?700:0;},configurable:true});
    prototype.getBoundingClientRect=function(){
      const top=900-(dom.window.document.querySelector('.setup-chat-scroll')?.scrollTop||0);
      if(this.classList.contains('setup-current-question'))return {top,height:420,bottom:top+420};
      if(this.classList.contains('setup-choice'))return {top:top+140,height:100,bottom:top+240};
      return {top:0,bottom:0,height:0};
    };
  }});
});

test('question visibility can settle during a slow initial saved-guide load',async()=>{
  const values=new Map([[GUIDE_KEY,saved.id]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key),
    key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  await mountedGuide({value:durable},async(document,calls)=>{
    await act(async()=>new Promise(resolve=>realSetTimeout(resolve,65)));
    assert.equal(document.querySelector('.setup-current-question h1')?.textContent,saved.question.title,
      'The saved wizard still renders after visibility was measured before its guide arrived');
    assert.ok(document.querySelector('#setup-message'));
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[]);
  },async(url)=>{
    if(url==='/api/guide/list')return {setups:[]};
    if(url===`/api/guide/${saved.id}`||url==='/api/guide/new'){
      await new Promise(resolve=>realSetTimeout(resolve,45));return structuredClone(saved);
    }
    if(url.startsWith(`/api/guide/${saved.id}/timeline`))return {frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true};
    return null;
  });
});

test('busy following keeps current native activity visible above long saved setup panels and respects human navigation',async()=>{
  const current=structuredClone(saved);
  current.proposal={target:'mac',project:{name:'Saved working plan',slug:'saved-working-plan'},
    agent_classes:[{id:'owner',name:'Context owner',instances:1,members:[],instructions:'Own the current work.',
      tools:{memory:true,mcp:[],notes:'none'}}],
    topology:{mode:'manual',entrypoint:'owner:main',edges:[]},connections:[],knowledge:{mode:'none'},flows:[]};
  current.messages=[{id:'older-user',role:'user',text:'Keep this plan.'},
    {id:'older-answer',role:'assistant',text:'Your earlier plan is saved.'}];
  current.operation={status:'running',version:5,request_id:'b'.repeat(64),started_at:'2026-10-04T09:00:00Z',
    request:{version:5,action:'chat',selected:[],text:'Check the supplied evidence.'}};
  const values=new Map([[GUIDE_KEY,current.id]]),polls=[],movements=[],navigationFrames=[];
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key),
    key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let nativeTail=900,page=0,asynchronousNavigation=false;
  const nextFrame=(kind,payload)=>({id:`live-${++page}`,kind,payload,interaction_id:'current-turn',identity:'architect:main',
    timestamp_ms:Date.parse(current.operation.started_at)+page,cursor:`console:${page}`,frame_version:1,source:{kind:'console_event'}});
  let frames=[nextFrame('user_input',{content:'Check the supplied evidence.'}),
    nextFrame('tool_call_requested',{id:'inspect',name:'inspect_blueprint',args:{}})];
  await mountedGuide({value:durable},async(document,calls)=>{
    const scroll=document.querySelector('.setup-chat-scroll'),native=document.querySelector('[data-native-console-chat]');
    const follow=()=>document.querySelector('.setup-guide-follow');
    assert.ok(document.querySelector('#setup-trial') && document.querySelector('.implementation-panel') &&
      document.querySelector('#setup-deployment'),'The actual saved Try/Build/Run panels remain below the conversation');
    assert.match(native.textContent,/Using tools/);
    assert.equal(scroll.scrollTop,416);
    assert.equal(follow() === null,true,'Following the live transcript needs no redundant navigation');
    assert.ok(movements.some(move=>move.behavior==='smooth' && move.top===416),'The initial busy reveal follows native activity');
    const advance=async(tail)=>{
      nativeTail=tail;
      frames=[nextFrame('text_delta',{delta:`Live update ${page}.`})];
      const timer=polls.shift();assert.ok(timer,'The normal timeline poll is retained');clearTimeout(timer.id);
      await act(async()=>{await timer.callback();await settle();});
    };
    await advance(1200);
    assert.equal(scroll.scrollTop,716,'New stream content follows its own tail, not the end of the saved panels');
    assert.equal(native.getBoundingClientRect().bottom,524,'Current native activity remains within the 500px viewport');
    assert.ok(scroll.scrollTop<scroll.scrollHeight-scroll.clientHeight-2000);
    for(const gesture of ['wheel','pointerdown']){
      await act(async()=>{
        scroll.dispatchEvent(new document.defaultView.Event(gesture,{bubbles:true}));
        scroll.scrollTop=100;
        scroll.dispatchEvent(new document.defaultView.Event('scroll',{bubbles:true}));
      });
      const count=movements.length;
      await advance(gesture==='wheel'?1500:1700);
      assert.equal(scroll.scrollTop,100,`${gesture} releases following`);
      assert.equal(movements.length,count,'A new native update does not pull the reader away');
      assert.equal(follow()?.textContent.trim(),'Follow live activity','A deliberate return action remains beside the composer');
    }
    const requestsBeforeFollow=calls.length;
    asynchronousNavigation=true;
    await act(async()=>follow().click());
    // Model a browser's deferred smooth-scroll positions rather than treating
    // scrollTo as synchronous. Native activity arrives between those positions.
    await act(async()=>{navigationFrames.shift()?.();await settle();});
    assert.equal(calls.length,requestsBeforeFollow,'Returning to activity does not admit or poll new work');
    await advance(2100);
    await act(async()=>{while(navigationFrames.length)navigationFrames.shift()();await settle();});
    assert.equal(scroll.scrollTop,1616,'Explicit following survives asynchronous scroll events and follows the newly grown native tail');
    assert.equal(follow() === null,true,'Following resumes without creating a false new-activity notice');
    assert.ok(movements.some(move=>move.behavior==='instant' && move.top===1216),
      'The explicit return has no intermediate smooth-scroll positions');
    await act(async()=>{
      scroll.scrollTop=1516;
      scroll.dispatchEvent(new document.defaultView.Event('scroll',{bubbles:true}));
    });
    await advance(2200);
    assert.equal(scroll.scrollTop,1716,'Returning near the native tail also resumes following despite the long panels below');
    assert.ok(calls.every(call=>call.method==='GET'),'Viewing progress admits no new work');
  },async(url)=>url==='/api/guide/list'?{setups:[]}:url===`/api/guide/${current.id}`||url==='/api/guide/new'?structuredClone(current):
    url.startsWith(`/api/guide/${current.id}/timeline`)?{frames,bindings:[{interaction_id:'current-turn',cursor:'console:1',
      user_message_id:'current-user',user_text:'Check the supplied evidence.',
      operation_request_id:current.operation.request_id,operation_started_at:current.operation.started_at}],next_cursor:`console:${page}`,latest_cursor:`console:${page}`,exhausted:true}:
    url==='/api/capabilities'?{rehearsal:{start_request_replay:true}}:
    {httpError:404,detail:'No saved application activity.'},{
      schedule(callback,delay,...args){
        if(delay!==1000)return realSetTimeout(callback,delay,...args);
        const id=realSetTimeout(()=>{},60000);polls.push({id,callback});return id;
      },
      setupDom(dom){
        const proto=dom.window.HTMLElement.prototype;
        Object.defineProperty(proto,'clientHeight',{configurable:true,get(){return this.classList.contains('setup-chat-scroll')?500:0;}});
        Object.defineProperty(proto,'scrollHeight',{configurable:true,get(){return this.classList.contains('setup-chat-scroll')?5000:0;}});
        Object.defineProperty(proto,'scrollTop',{configurable:true,get(){return this._top||0;},set(value){
          this._top=value;
          if(this.classList.contains('setup-chat-scroll')){
            movements.push({top:value});
            queueMicrotask(()=>this.dispatchEvent(new dom.window.Event('scroll',{bubbles:true})));
          }
        }});
        proto.getBoundingClientRect=function(){
          const scroll=dom.window.document.querySelector('.setup-chat-scroll');
          if(this===scroll)return {top:40,bottom:540,height:500};
          if(this.matches('[data-native-console-chat]'))return {top:40-scroll.scrollTop,bottom:40+nativeTail-scroll.scrollTop,height:nativeTail};
          return {top:40+nativeTail+200-scroll?.scrollTop,bottom:5040-scroll?.scrollTop,height:3000};
        };
        proto.scrollTo=function(options){
          movements.push(options);
          if(asynchronousNavigation && options.behavior==='smooth'){
            const intermediate=Math.min(options.top,this.scrollTop+100);
            navigationFrames.push(()=>{this.scrollTop=intermediate;},()=>{this.scrollTop=options.top;});
          }else this.scrollTop=options.top;
        };
      },
    });
});


const packagePlan = { project: { name: 'Sample team', slug: 'sample-team' }, target: 'mac',
  agent_classes: [], connections: [], knowledge: { mode: 'none' }, advanced: { port: 4110 } };
const packageGuide = { id: 'package-guide', version: 2, status: 'question', stage: 'purpose',
  proposal: packagePlan, blueprint: null, messages: [], answers: [], completed_stages: [],
  brief: { facts: {}, requirements: [{ id: 'citations', summary: 'Check citation provenance.', status: 'open' }] },
  readiness: { context_ready: true, decision_blockers: [], implementation_blockers: ['Check citation provenance.'] },
  capabilities: { available: true } };
function packageStorage(state, manual = false) {
  const values = new Map([[GUIDE_KEY, state.id]]);
  if (manual) values.set(manualKey(state.id), JSON.stringify({ blueprint: { ...packagePlan, target: 'linux' },
    text: 'My unsent deployment change.', changes: ['Deployment settings'] }));
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
}
function packageReads(state, implementation, overrides = {}) {
  return async (url, options) => {
    if (overrides[url]) return overrides[url](options);
    if (url === '/api/guide/list') return { setups: [] };
    if (url === '/api/guide/new' || url === `/api/guide/${state.id}`) return structuredClone(state);
    if (url === '/api/blueprint/validate') return { valid: true, files: [], checks: [] };
    if (url === '/api/blueprint/package') return {};
    if (url === `/api/guide/${state.id}/implementation` && implementation) return structuredClone(implementation);
    if (url.startsWith(`/api/guide/${state.id}/timeline`))
      return { frames: [], bindings: [], exhausted: true, next_cursor: null, latest_cursor: null };
    return { httpError: 404, detail: 'No saved activity in this isolated fixture.' };
  };
}
const packageOptions = downloads => ({ downloads, allowedPosts: ['/api/blueprint/validate', '/api/blueprint/package'],
  schedule: (callback, delay, ...args) => delay === 10000 ? 0 : realSetTimeout(callback, delay, ...args) });
const teamDownload = document => document.querySelector('.setup-team-package button');
async function waitPackage(predicate) {
  for (let attempt = 0; attempt < 60 && !predicate(); attempt++) await act(async () => { await settle(); });
  assert.ok(predicate(), 'The exact offline package request reached the expected boundary');
}

test('an initial design question leads while optional build readiness stays in the closed action disclosure', async () => {
  const state = { ...structuredClone(packageGuide), question: { title: 'Which rehearsal trade-off matters?',
    description: 'Choose how competing feasible plans should be compared.', multiple: false, allow_text: true,
    options: [{ id: 'time', label: 'Volunteer time', description: 'Avoid unnecessary calls.' }] } };
  const input = { key: inputKey(state), draft: 'My unsent rehearsal context', selected: ['time'], mode: 'answer' };
  for (const build of [null, { guide_id: state.id, guide_version: state.version, current: true,
    build_id: 'owned-build', status: 'running', ready_to_package: false }]) {
    const storage = packageStorage(state);
    storage.setItem(input.key, JSON.stringify(input));
    await mountedGuide({ value: storage }, async (document, calls) => {
      const question = document.querySelector('.setup-current-question');
      const progress = document.querySelector('.setup-progress-card');
      assert.ok(question && progress);
      const disclosure = document.querySelector('.setup-proposal-actions');
      if (!build) {
        assert.equal(disclosure.open, false);
        assert.equal(progress.closest('details') === disclosure, true);
        assert.equal(question.querySelector('h1')?.textContent, state.question.title);
        assert.equal(document.querySelector('.setup-chat-top small')?.textContent, 'One decision at a time');
        assert.deepEqual([...document.querySelectorAll('.setup-stage-shortcuts a')].map(link => link.textContent), ['Try, build and run']);
      } else {
        assert.equal(progress.closest('details') === null, true, 'Actual owned work remains prominent');
        assert.match(progress.textContent, /being built and checked/);
        assert.equal(question.querySelector('h2')?.textContent, state.question.title);
        assert.ok(progress.compareDocumentPosition(question) & document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING);
      }
      assert.equal(document.querySelector('#setup-message').value, input.draft);
      assert.equal(document.querySelectorAll('.setup-choice.selected').length, 1);
      assert.equal(calls.some(call => call.method === 'POST'), false);
    }, packageReads(state, build));
  }
});

test('a saved design refinement without a question keeps an unstarted build optional', async () => {
  const state = { ...structuredClone(packageGuide), status: 'refining', question: null };
  const input = { key: inputKey(state), draft: 'Keep designing before we build.', selected: [], mode: 'chat' };
  const storage = packageStorage(state);
  storage.setItem(input.key, JSON.stringify(input));
  await mountedGuide({ value: storage }, async (document, calls) => {
    const disclosure = document.querySelector('.setup-proposal-actions');
    assert.equal(disclosure.open, false);
    assert.equal(document.querySelector('.setup-progress-card').closest('details') === disclosure, true);
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(calls.some(call => call.method === 'POST'), false);
  }, packageReads(state, null));
});

test('an accepted native team offers Install navigation to private setup without a build or installation POST', async () => {
  const state = { ...structuredClone(packageGuide), status: 'ready', proposal: null, blueprint: packagePlan,
    question: null, readiness: { ready: true, context_ready: true, decision_blockers: [], implementation_blockers: [] } };
  await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
    const button = [...document.querySelectorAll('.setup-team-package button')].find(item => item.textContent.startsWith('Install agent team'));
    assert.ok(button);
    assert.equal(button.disabled, false);
    await act(async () => { button.click(); await settle(); });
    assert.equal(document.querySelector('#setup-deployment input[name="deployment-source"][value="native-team"]').checked, true);
    assert.match(document.querySelector('#setup-deployment').textContent, /No custom application build is needed/);
    assert.equal(calls.some(call => call.method === 'POST'), false);
  }, packageReads(state, null));
});

test('a decision-clear saved proposal exports its actual team despite pending application work and an available checked build', async () => {
  for (const build of [{ status: 'failed', ready_to_package: false }, { status: 'completed', ready_to_package: true }]) {
    const state = structuredClone(packageGuide), downloads = [];
    const implementation = { guide_id: state.id, guide_version: state.version, build_id: 'custom-build', current: true, ...build };
    await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
      const button = teamDownload(document);
      assert.ok(button && !button.disabled, 'The team export remains actionable without application acceptance');
      const optionalPackage = button.closest('details');
      assert.ok(optionalPackage, 'Pending application work keeps the team-only export optional');
      assert.equal(optionalPackage.open, false);
      assert.match(optionalPackage.querySelector('summary').textContent, /Agent team package \(optional\)/);
      await act(async () => { optionalPackage.querySelector('summary').click(); await settle(); });
      assert.equal(optionalPackage.open, true);
      assert.match(document.querySelector('.setup-team-package').textContent, /current proposal.*proposed defaults/i);
      assert.match(document.body.textContent, /Use the builder when your requested outcomes need custom screens/);
      assert.match(document.body.textContent, /Check citation provenance/);
      await act(async () => { button.click(); await settle(); });
      await waitPackage(() => downloads.length === 1);
      assert.deepEqual(calls.filter(call => call.method === 'POST'), [
        { url: '/api/blueprint/validate', method: 'POST', body: state.proposal },
        { url: '/api/blueprint/package', method: 'POST', body: state.proposal }]);
      assert.equal(calls.some(call => call.url.includes('/implementation/package')), false);
      assert.deepEqual(downloads.map(item => item.name), ['sample-team.zip']);
      assert.match(document.querySelector('.setup-team-package [role="status"]').textContent, /Package download started/);
      assert.equal(state.status, 'question');
    }, packageReads(state, implementation), packageOptions(downloads));
  }
});

test('compiler validation refusal preserves the proposal without starting a package or builder', async () => {
  const state = structuredClone(packageGuide), downloads = [];
  await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
    await act(async () => { teamDownload(document).click(); await settle(); });
    await waitPackage(() => document.body.textContent.includes('Selected tool is unavailable'));
    assert.match(document.body.textContent, /Selected tool is unavailable/);
    assert.deepEqual(calls.filter(call => call.method === 'POST').map(call => call.url), ['/api/blueprint/validate']);
    assert.deepEqual(downloads, []);
  }, packageReads(state, null, { '/api/blueprint/validate': () => ({ httpError: 422, detail: 'Selected tool is unavailable.' }) }), packageOptions(downloads));
});

test('a late compiled ZIP never downloads after the displayed guide, version, plan or brief changes', async () => {
  for (const changed of [{ id: 'another-guide' }, { version: 3 },
    {proposal:{...packagePlan,project:{...packagePlan.project,name:'Changed current proposal'}}},
    { brief: { facts: {}, requirements: [] } }]) {
    const state = structuredClone(packageGuide), next = { ...structuredClone(state), ...changed }, downloads = [];
    let finishBlob, requested = false, switched = false;
    const blob = new Promise(resolve => { finishBlob = resolve; });
    const read = packageReads(state, null, {
      '/api/guide/list': () => ({ setups: [{ id: next.id, name: 'Reloaded setup', status: next.status }] }),
      [`/api/guide/${next.id}`]: () => structuredClone(switched ? next : state),
      '/api/blueprint/package': () => { requested = true; return { fixtureBlob: () => blob }; },
    });
    const storage=packageStorage(state);
    const retained={key:inputKey(next),draft:'  Keep the selected setup note  ',selected:[],mode:'chat'};
    storage.setItem(retained.key,JSON.stringify(retained));
    await mountedGuide({ value: storage }, async (document, calls) => {
      const oldButton = teamDownload(document);
      await act(async () => { oldButton.click(); await settle(); });
      await waitPackage(() => requested);
      assert.equal(requested, true);
      await act(async () => { document.querySelector('.setup-history-control > button').click(); await settle(); });
      switched = true;
      const reload = [...document.querySelectorAll('.setup-history-menu button')].find(button => button.textContent.includes('Reloaded setup'));
      await act(async () => { reload.click(); await settle(); });
      await act(async () => { finishBlob(new Blob(['late offline package'])); await settle(); });
      await waitPackage(() => teamDownload(document)?.disabled===false);
      assert.deepEqual(downloads, []);
      assert.equal(document.querySelector('.setup-team-package [role="status"]') === null, true);
      assert.equal(document.querySelector('.setup-error') === null,true,'the outgoing ZIP cannot report an error on the selected setup');
      assert.equal(storage.getItem(GUIDE_KEY),next.id);
      assert.equal(document.querySelector('#setup-message').value,retained.draft);
      assert.deepEqual(JSON.parse(storage.getItem(retained.key)),retained);
      assert.equal(calls.filter(call => call.method === 'POST').length, 2);
    }, read, packageOptions(downloads));
  }
});

test('an unsent manual change leaves the team package visible but blocks export', async () => {
  const state = structuredClone(packageGuide), downloads = [];
  await mountedGuide({ value: packageStorage(state, true) }, async (document, calls) => {
    const button = teamDownload(document);
    assert.ok(button && button.disabled);
    await act(async () => { button.click(); await settle(); });
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
    assert.deepEqual(downloads, []);
  }, packageReads(state), packageOptions(downloads));
});


test('a successful native package acknowledgment belongs only to its guide, version, plan and brief', async () => {
  for (const changed of [{ id: 'other-package-guide' }, { version: 3 },
    { proposal: { ...packagePlan, project: { ...packagePlan.project, name: 'Changed team' } } },
    { brief: { facts: {}, requirements: [] } }]) {
    const state = structuredClone(packageGuide), next = { ...structuredClone(state), ...changed }, downloads = [];
    let switched = false;
    const read = packageReads(state, null, {
      '/api/guide/list': () => ({ setups: [{ id: next.id, name: 'Current setup', status: next.status }] }),
      [`/api/guide/${next.id}`]: () => structuredClone(switched ? next : state),
    });
    await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
      assert.equal(document.querySelector('.setup-team-package [role="status"]') === null, true);
      await act(async () => { teamDownload(document).click(); await settle(); });
      await waitPackage(() => downloads.length === 1 && document.querySelector('.setup-team-package [role="status"]'));
      await act(async () => { document.querySelector('.setup-history-control > button').click(); await settle(); });
      switched = true;
      const reload = [...document.querySelectorAll('.setup-history-menu button')].find(button => button.textContent.includes('Current setup'));
      await act(async () => { reload.click(); await settle(); });
      assert.equal(document.querySelector('.setup-team-package [role="status"]') === null, true);
      assert.equal(downloads.length, 1);
      assert.equal(calls.filter(call => call.method === 'POST').length, 2, 'Changing the displayed setup never automatically exports again');
    }, read, packageOptions(downloads));
  }
});


function packageJourneyGuide(route){
  const guide=structuredClone(packageGuide);
  if(route!=='native')Object.assign(guide,{status:'ready',question:null,blueprint:guide.proposal,proposal:null});
  return guide;
}
const packageFiles={valid:true,files:[{path:'owned-package.txt',content:'Exact current package fixture.'}],checks:[]};
const packageBuild=guide=>({guide_id:guide.id,guide_version:guide.version,build_id:'owned-checked-build',
  status:'completed',current:true,ready_to_package:true,source_digest:'a'.repeat(64),package_sha256:'b'.repeat(64)});
const checkedPackageUrl=guide=>`/api/guide/${guide.id}/implementation/package?build_id=owned-checked-build`;
function packageAction(document,route){
  const selector=route==='native'?'.setup-team-package button':'.setup-manual-export button';
  const label=route==='native'?'Download agent team package':route==='inspect'?'Inspect native setup files':'Download checked application';
  return [...document.querySelectorAll(selector)].find(button=>button.textContent.includes(label));
}
async function clickPackageNavigation(document,name){
  const button=[...document.querySelectorAll('.setup-history-control button')].find(node=>node.textContent.includes(name));
  assert.ok(button,`Missing package navigation: ${name}`);
  await act(async()=>{button.click();await settle();});
}
function packageJourneyStorage(first,second){
  const storage=packageStorage(first),inputs=[first,second].filter(Boolean).map(guide=>({key:inputKey(guide),
    draft:`  Unsent ${guide.id} package note\nkeep the whitespace  `,selected:[],mode:'chat'}));
  for(const input of inputs)storage.setItem(input.key,JSON.stringify(input));
  return {storage,inputs};
}
function packageJourneyReads(first,second,read){
  const fallback=packageReads(first,null);
  return (url,options)=>{
    const specific=read(url,options);
    if(specific!==undefined)return specific;
    if(url==='/api/guide/list')return {setups:[{id:first.id,name:'First package setup',status:first.status},
      ...(second?[{id:second.id,name:'Second package setup',status:second.status}]:[])]};
    if(url==='/api/architect/status')return {state:'working',available:true};
    if(url==='/api/capabilities')return {rehearsal:{start_request_replay:false}};
    if(second && url===`/api/guide/${second.id}`)return structuredClone(second);
    if(second && url.startsWith(`/api/guide/${second.id}/timeline`))return {frames:[],bindings:[],exhausted:true};
    if(url==='/api/blueprint/validate')return structuredClone(packageFiles);
    return fallback(url,options);
  };
}

for(const route of ['native','inspect','checked'])for(const outcome of ['success','failure'])for(const navigation of ['running B','A-B-A'])
test(`package ${route} delayed ${outcome} respects ${navigation} ownership`,async t=>{
  const now=Date.parse('2026-10-06T19:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const first=packageJourneyGuide(route),second={...structuredClone(saved),id:'owned-package-second',operation:undefined};
  if(navigation==='running B')second.operation={status:'running',request_id:'c'.repeat(64),version:second.version,
    started_at:new Date(now).toISOString(),request:{version:second.version,action:'chat',selected:[],text:'Saved second request'}};
  const {storage,inputs}=packageJourneyStorage(first,second),downloads=[];
  let release,blocked=false,intercepted=false,secondFails=false;
  const deferred=new Promise(resolve=>{release=resolve;});
  const read=packageJourneyReads(first,second,(url,options)=>{
    if(url===`/api/guide/${first.id}/implementation` && route!=='native')return packageBuild(first);
    if(url===`/api/guide/${second.id}` && secondFails)throw Error('Second saved request observation interrupted');
    if(!intercepted && url==='/api/blueprint/validate' && (route==='inspect' || outcome==='failure' && route==='native')){
      intercepted=true;blocked=true;return deferred;
    }
    if(!intercepted && (url==='/api/blueprint/package' && route==='native' || url===checkedPackageUrl(first) && route==='checked')){
      intercepted=true;
      return route==='checked' && outcome==='failure'
        ?{httpError:422,fixtureJson:()=>{blocked=true;return deferred;}}
        :{fixtureBlob:()=>{blocked=true;return deferred;}};
    }
    if(url===checkedPackageUrl(first))return {};
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message'),send=()=>document.querySelector('#setup-send');
    await waitPackage(()=>packageAction(document,route)?.disabled===false);
    await act(async()=>{packageAction(document,route).click();await settle();});
    await waitPackage(()=>blocked);
    const captured=calls.filter(call=>call.method==='POST' || call.url.includes('/implementation/package'));
    await act(async()=>{send().click();composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));});
    assert.deepEqual(calls.filter(call=>call.method==='POST' || call.url.includes('/implementation/package')),captured);
    await clickPackageNavigation(document,'Your setups');
    await clickPackageNavigation(document,'Second package setup');
    if(navigation==='A-B-A'){
      await clickPackageNavigation(document,'Your setups');
      await clickPackageNavigation(document,'First package setup');
    }else{
      secondFails=true;
      await act(async()=>{t.mock.timers.tick(2500);await settle();});
      assert.match(document.querySelector('.setup-error').textContent,/Second saved request observation interrupted/);
    }
    await act(async()=>{
      release(outcome==='failure'?{detail:'Outgoing package failed.',httpError:422}:
        route==='inspect'?structuredClone(packageFiles):new Blob(['outgoing package bytes']));
      await settle();
    });
    await waitPackage(()=>composer()?.disabled===false);
    assert.deepEqual(downloads,[]);
    assert.equal(document.querySelector('.setup-team-package [role="status"]') === null,true);
    assert.equal(document.querySelector('.setup-file-modal') === null,true);
    assert.doesNotMatch(document.body.textContent,/Outgoing package failed|The setup changed|Package download started/);
    assert.deepEqual(calls.filter(call=>call.method==='POST' || call.url.includes('/implementation/package')),captured,
      'late settlement never validates, fetches another package or resends a guide request');
    const selected=navigation==='A-B-A'?first:second,input=inputs[navigation==='A-B-A'?0:1];
    assert.equal(storage.getItem(GUIDE_KEY),selected.id);
    assert.equal(composer().value,input.draft);
    assert.deepEqual(JSON.parse(storage.getItem(input.key)),input);
    if(navigation==='running B'){
      assert.equal(send().disabled,true);
      assert.match(document.querySelector('.setup-current-question').textContent,/Choices are paused/);
      assert.ok([...document.querySelectorAll('.setup-current-question button')].every(button=>button.disabled));
      assert.equal(document.querySelector('.setup-guide-wait').dataset.requestId,second.operation.request_id);
      assert.match(document.querySelector('.setup-error').textContent,/Second saved request observation interrupted/);
    }else{
      assert.equal(document.querySelector('.setup-error') === null,true);
      assert.ok(packageAction(document,route) && !packageAction(document,route).disabled);
      await act(async()=>{packageAction(document,route).click();await settle();});
      await waitPackage(()=>route==='inspect'?!!document.querySelector('.setup-file-modal'):downloads.length===1);
      if(route==='inspect')assert.match(document.querySelector('.setup-file-modal pre').textContent,/Exact current package fixture/);
      else assert.equal(downloads[0].name,route==='checked'?'sample-team-application.zip':'sample-team.zip');
    }
    assert.deepEqual(JSON.parse(storage.getItem(inputs[0].key)),inputs[0]);
    if(route==='checked'){
      assert.equal(captured.length,1);
      assert.equal(captured[0].url,checkedPackageUrl(first));
      assert.equal(captured[0].method,'GET');
    }else{
      assert.deepEqual(captured.map(call=>({url:call.url,body:call.body})),
        (route==='native' && outcome==='success'?['/api/blueprint/validate','/api/blueprint/package']:['/api/blueprint/validate'])
          .map(url=>({url,body:route==='native'?first.proposal:first.blueprint})));
    }
  },read,packageOptions(downloads));
});

for(const stage of [1,2,3,4,5,6,7])test(`native package fingerprint ${stage} cannot outlive pending or returned navigation`,async()=>{
  const first=packageJourneyGuide('native'),second={...structuredClone(saved),id:'owned-digest-destination',operation:undefined};
  const {storage,inputs}=packageJourneyStorage(first,second),downloads=[];
  let callsToDigest=0,releaseDigest,releaseSelection,blocked=false,pending=true;
  const selection=new Promise(resolve=>{releaseSelection=resolve;});
  const digest=async(algorithm,bytes)=>{
    const result=await webcrypto.subtle.digest(algorithm,bytes);
    if(++callsToDigest===stage){blocked=true;return new Promise(resolve=>{releaseDigest=()=>resolve(result);});}
    return result;
  };
  const read=packageJourneyReads(first,second,url=>url===`/api/guide/${second.id}` && pending?selection:undefined);
  await mountedGuide({value:storage},async(document,calls)=>{
    await act(async()=>{packageAction(document,'native').click();await settle();});
    await waitPackage(()=>blocked);
    const before=calls.filter(call=>call.method==='POST');
    assert.equal(before.length,stage<=3?0:stage<=5?1:2);
    await clickPackageNavigation(document,'Your setups');
    await clickPackageNavigation(document,'Second package setup');
    if(stage%2===0){
      pending=false;
      await act(async()=>{releaseSelection(structuredClone(second));await settle();});
      await clickPackageNavigation(document,'Your setups');
      await clickPackageNavigation(document,'First package setup');
    }
    await act(async()=>{releaseDigest();await settle();});
    assert.deepEqual(calls.filter(call=>call.method==='POST'),before,'a digest cannot renew the captured navigation lifetime');
    assert.deepEqual(downloads,[]);
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.equal(document.querySelector('.setup-file-modal') === null,true);
    if(stage%2!==0){
      pending=false;
      await act(async()=>{releaseSelection(structuredClone(second));await settle();});
      assert.equal(document.querySelector('#setup-message').value,inputs[1].draft);
      assert.equal(storage.getItem(GUIDE_KEY),second.id);
    }else assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    await waitPackage(()=>document.querySelector('#setup-message').disabled===false);
    assert.deepEqual(JSON.parse(storage.getItem(inputs[0].key)),inputs[0]);
  },read,{...packageOptions(downloads),digest});
});

for(const route of ['native','checked'])for(const failure of [false,true])
test(`${route} package fetch ${failure?'refusal':'success'} does not start body reads after navigation`,async()=>{
  const first=packageJourneyGuide(route),second={...structuredClone(saved),id:'owned-fetch-destination',operation:undefined};
  const {storage,inputs}=packageJourneyStorage(first,second),downloads=[];
  let releaseFetch,releaseSelection,blocked=false,jsonReads=0,blobReads=0;
  const response=new Promise(resolve=>{releaseFetch=resolve;}),selection=new Promise(resolve=>{releaseSelection=resolve;});
  const read=packageJourneyReads(first,second,url=>{
    if(url===`/api/guide/${first.id}/implementation` && route==='checked')return packageBuild(first);
    if(url===(route==='native'?'/api/blueprint/package':checkedPackageUrl(first))){blocked=true;return response;}
    if(url===`/api/guide/${second.id}`)return selection;
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    await waitPackage(()=>packageAction(document,route)?.disabled===false);
    await act(async()=>{packageAction(document,route).click();await settle();});
    await waitPackage(()=>blocked);
    const captured=calls.filter(call=>call.method==='POST' || call.url.includes('/implementation/package'));
    await clickPackageNavigation(document,'Your setups');
    await clickPackageNavigation(document,'Second package setup');
    await act(async()=>{releaseFetch({...(failure?{httpError:422}:{}),
      fixtureJson:()=>{jsonReads++;return {detail:'Outgoing HTTP refusal'};},
      fixtureBlob:()=>{blobReads++;return new Blob(['outgoing response bytes']);}});await settle();});
    assert.equal(jsonReads,0);
    assert.equal(blobReads,0);
    assert.deepEqual(downloads,[]);
    assert.equal(document.querySelector('.setup-error') === null,true);
    await act(async()=>{releaseSelection(structuredClone(second));await settle();});
    await waitPackage(()=>document.querySelector('#setup-message').disabled===false);
    assert.equal(document.querySelector('#setup-message').value,inputs[1].draft);
    assert.equal(storage.getItem(GUIDE_KEY),second.id);
    assert.deepEqual(calls.filter(call=>call.method==='POST' || call.url.includes('/implementation/package')),captured);
  },read,packageOptions(downloads));
});

test('a saved manual change selected during validation retains its own package block and draft',async()=>{
  const first=packageJourneyGuide('native'),{storage,inputs}=packageJourneyStorage(first),downloads=[];
  let release,blocked=false;
  const validation=new Promise(resolve=>{release=resolve;});
  const manual={blueprint:{...packagePlan,target:'linux'},text:'Review my new private destination.',changes:['Deployment settings']};
  const read=packageJourneyReads(first,null,url=>url==='/api/blueprint/validate'?(blocked=true,validation):undefined);
  await mountedGuide({value:storage},async(document,calls)=>{
    await act(async()=>{packageAction(document,'native').click();await settle();});
    await waitPackage(()=>blocked);
    storage.setItem(manualKey(first.id),JSON.stringify(manual));
    await clickPackageNavigation(document,'Your setups');
    await clickPackageNavigation(document,'First package setup');
    const adjustments=[...document.querySelectorAll('.setup-manual-note')]
      .find(node=>node.querySelector('strong')?.textContent==='Your adjustments');
    assert.ok(adjustments);
    assert.match(adjustments.textContent,/Deployment settings/);
    await act(async()=>{release(structuredClone(packageFiles));await settle();});
    await waitPackage(()=>document.querySelector('#setup-message').disabled===false);
    assert.equal(packageAction(document,'native').disabled,true);
    assert.equal(document.querySelector('#setup-send').disabled,true);
    assert.match(document.body.textContent,/Your adjustments.*Deployment settings/s);
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(storage.getItem(manualKey(first.id))),manual);
    assert.deepEqual(JSON.parse(storage.getItem(inputs[0].key)),inputs[0]);
    assert.deepEqual(downloads,[]);
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[{url:'/api/blueprint/validate',method:'POST',body:first.proposal}]);
  },read,packageOptions(downloads));
});

for(const changed of [
  {ready_to_package:false},{current:false},{cleanup_pending:true},{build_id:'replacement-build'},
  {source_digest:'c'.repeat(64)},{package_sha256:'d'.repeat(64)},
])test(`checked package rechecks eligibility and exact identity after blob (${Object.keys(changed)[0]})`,async()=>{
  const first=packageJourneyGuide('checked'),{storage,inputs}=packageJourneyStorage(first),downloads=[],polls=[];
  let build=packageBuild(first),blocked=false,release;
  const blob=new Promise(resolve=>{release=resolve;});
  const read=packageJourneyReads(first,null,url=>{
    if(url===`/api/guide/${first.id}/implementation`)return structuredClone(build);
    if(url===checkedPackageUrl(first))return {fixtureBlob:()=>{blocked=true;return blob;}};
  });
  const schedule=(callback,delay,...args)=>delay===6000?(polls.push(()=>callback(...args)),0):
    delay===10000?0:realSetTimeout(callback,delay,...args);
  await mountedGuide({value:storage},async(document,calls)=>{
    await waitPackage(()=>packageAction(document,'checked')?.disabled===false && polls.length>0);
    await act(async()=>{packageAction(document,'checked').click();await settle();});
    await waitPackage(()=>blocked);
    build={...build,...changed};
    await act(async()=>{for(const poll of polls.splice(0))await poll();await settle();});
    await act(async()=>{release(new Blob(['previous checked source']));await settle();});
    await waitPackage(()=>document.querySelector('#setup-message').disabled===false);
    assert.deepEqual(downloads,[]);
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.equal(document.querySelector('.setup-manual-export .setup-download-success') === null,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(storage.getItem(inputs[0].key)),inputs[0]);
    assert.deepEqual(calls.filter(call=>call.url.includes('/implementation/package')).map(call=>call.url),[checkedPackageUrl(first)]);
    assert.equal(calls.filter(call=>call.method==='POST').length,0);
  },read,{...packageOptions(downloads),schedule});
});

test('checked package identity cannot change while its final fingerprint is pending',async()=>{
  const first=packageJourneyGuide('checked'),{storage,inputs}=packageJourneyStorage(first),downloads=[],polls=[];
  let build=packageBuild(first),digests=0,blocked=false,release;
  const digest=async(algorithm,bytes)=>{
    const result=await webcrypto.subtle.digest(algorithm,bytes);
    if(++digests===5){blocked=true;return new Promise(resolve=>{release=()=>resolve(result);});}
    return result;
  };
  const read=packageJourneyReads(first,null,url=>{
    if(url===`/api/guide/${first.id}/implementation`)return structuredClone(build);
    if(url===checkedPackageUrl(first))return {};
  });
  const schedule=(callback,delay,...args)=>delay===6000?(polls.push(()=>callback(...args)),0):
    delay===10000?0:realSetTimeout(callback,delay,...args);
  await mountedGuide({value:storage},async(document,calls)=>{
    await waitPackage(()=>packageAction(document,'checked')?.disabled===false && polls.length>0);
    await act(async()=>{packageAction(document,'checked').click();await settle();});
    await waitPackage(()=>blocked);
    build={...build,source_digest:'e'.repeat(64)};
    await act(async()=>{for(const poll of polls.splice(0))await poll();await settle();});
    await act(async()=>{release();await settle();});
    await waitPackage(()=>document.querySelector('#setup-message').disabled===false);
    assert.deepEqual(downloads,[]);
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.equal(document.querySelector('.setup-manual-export .setup-download-success') === null,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(storage.getItem(inputs[0].key)),inputs[0]);
    assert.deepEqual(calls.filter(call=>call.url.includes('/implementation/package')).map(call=>call.url),[checkedPackageUrl(first)]);
    assert.equal(calls.filter(call=>call.method==='POST').length,0);
  },read,{...packageOptions(downloads),schedule,digest});
});

for(const navigation of [false,true])
test(`retained selected export uses source-only fallback ${navigation?'without surviving navigation':'with its selected acknowledgment'}`,async()=>{
  const first=packageJourneyGuide('checked'),second={...structuredClone(saved),id:'owned-selected-fallback-destination',operation:undefined};
  const {storage,inputs}=packageJourneyStorage(first,second),downloads=[],polls=[];
  let build=packageBuild(first),blocked=false,release;
  const blob=new Promise(resolve=>{release=resolve;});
  const read=packageJourneyReads(first,second,url=>{
    if(url===`/api/guide/${first.id}/implementation`)return structuredClone(build);
    if(url==='/api/blueprint/package' && navigation)return {fixtureBlob:()=>{blocked=true;return blob;}};
  });
  const schedule=(callback,delay,...args)=>delay===6000?(polls.push(()=>callback(...args)),0):
    delay===10000?0:realSetTimeout(callback,delay,...args);
  await mountedGuide({value:storage},async(document,calls)=>{
    await waitPackage(()=>packageAction(document,'checked')?.disabled===false && polls.length>0);
    const button=packageAction(document,'checked');
    const propsKey=Object.keys(button).find(key=>key.startsWith('__reactProps$'));
    const retainedClick=button[propsKey].onClick;
    build={...build,ready_to_package:false};
    await act(async()=>{for(const poll of polls.splice(0))await poll();await settle();});
    assert.equal(packageAction(document,'checked') === undefined, true,
      'this preserves an existing retained handler, not a newly visible legacy export control');
    await act(async()=>{retainedClick();await settle();});
    if(navigation){
      await waitPackage(()=>blocked);
      await clickPackageNavigation(document,'Your setups');
      await clickPackageNavigation(document,'Second package setup');
      await act(async()=>{release(new Blob(['outgoing source-only setup']));await settle();});
      await waitPackage(()=>document.querySelector('#setup-message').disabled===false);
      assert.deepEqual(downloads,[]);
      assert.equal(storage.getItem(GUIDE_KEY),second.id);
      assert.equal(document.querySelector('#setup-message').value,inputs[1].draft);
      assert.deepEqual(JSON.parse(storage.getItem(inputs[1].key)),inputs[1]);
      assert.equal(document.querySelector('.setup-download-success') === null,true);
    }else{
      await waitPackage(()=>downloads.length===1 && document.querySelector('#setup-message').disabled===false);
      assert.equal(downloads[0].name,'sample-team.zip');
      build=packageBuild(first);
      await act(async()=>{for(const poll of polls.splice(0))await poll();await settle();});
      await waitPackage(()=>!!document.querySelector('.setup-manual-export .setup-download-success'));
      assert.match(document.querySelector('.setup-manual-export .setup-download-success').textContent,/Package download started/);
      assert.equal(document.querySelector('.setup-team-package [role="status"]') === null,true,
        'the selected fallback must not acknowledge a native-scope export');
      assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
      assert.equal(storage.getItem(GUIDE_KEY),first.id);
    }
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.deepEqual(JSON.parse(storage.getItem(inputs[0].key)),inputs[0]);
    assert.deepEqual(calls.filter(call=>call.method==='POST'),['/api/blueprint/validate','/api/blueprint/package']
      .map(url=>({url,method:'POST',body:first.blueprint})));
    assert.equal(calls.filter(call=>call.url.includes('/implementation/package')).length,0);
  },read,{...packageOptions(downloads),schedule});
});

for(const route of ['native','inspect','checked'])test(`current ${route} package failure keeps its note and permits only deliberate recovery`,async()=>{
  const first=packageJourneyGuide(route),{storage,inputs}=packageJourneyStorage(first),downloads=[];
  let release,blocked=false,intercepted=false;
  const failure=new Promise(resolve=>{release=resolve;});
  const read=packageJourneyReads(first,null,url=>{
    if(url===`/api/guide/${first.id}/implementation` && route!=='native')return packageBuild(first);
    if(!intercepted && url===(route==='checked'?checkedPackageUrl(first):'/api/blueprint/validate')){
      intercepted=true;blocked=true;return failure;
    }
    if(url===checkedPackageUrl(first))return {};
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    await waitPackage(()=>packageAction(document,route)?.disabled===false);
    await act(async()=>{packageAction(document,route).click();await settle();});
    await waitPackage(()=>blocked);
    const admitted=calls.filter(call=>call.method==='POST' || call.url.includes('/implementation/package'));
    await act(async()=>{release({httpError:422,detail:'Current package refusal: review the saved setup.'});await settle();});
    await waitPackage(()=>packageAction(document,route)?.disabled===false);
    assert.match(document.querySelector('.setup-error').textContent,/Current package refusal/);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(storage.getItem(inputs[0].key)),inputs[0]);
    assert.deepEqual(downloads,[]);
    assert.deepEqual(calls.filter(call=>call.method==='POST' || call.url.includes('/implementation/package')),admitted);
    await act(async()=>{packageAction(document,route).click();await settle();});
    await waitPackage(()=>route==='inspect'?!!document.querySelector('.setup-file-modal'):downloads.length===1);
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.equal(calls.filter(call=>call.method==='POST').length,route==='checked'?0:route==='inspect'?2:3);
    if(route==='checked')assert.equal(calls.filter(call=>call.url===checkedPackageUrl(first)).length,2);
  },read,packageOptions(downloads));
});

const readyRefinement = { ...structuredClone(packageGuide), status: 'refining', question: null,
  readiness: { ready: true, context_ready: true, decision_blockers: [], implementation_blockers: [] } };
const usePlan = document => [...document.querySelectorAll('button')].find(button => button.textContent.includes('Use this plan'));

test('a fully ready saved refinement offers explicit acceptance with one exact-version apply and no guide turn', async () => {
  const state = structuredClone(readyRefinement), apply = `/api/guide/${state.id}/apply`;
  const accepted = { ...state, version: state.version + 1, status: 'ready', blueprint: state.proposal, proposal: null };
  await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
    const button = usePlan(document);
    assert.ok(button && !button.disabled);
    assert.match(document.body.textContent, /PROPOSAL TO REVIEW/);
    assert.deepEqual(calls.filter(call => call.method === 'POST'), [], 'Displaying a ready refinement never automatically accepts it');
    await act(async () => { button.click(); await settle(); });
    await waitPackage(() => document.body.textContent.includes('ACCEPTED PLAN'));
    assert.deepEqual(calls.filter(call => call.method === 'POST'), [{ url: apply, method: 'POST', body: { version: state.version } }]);
    assert.equal(state.status, 'refining');
  }, packageReads(state, null, { [apply]: () => structuredClone(accepted) }), { allowedPosts: [apply] });
});

test('accepting a complete plan preserves unfinished application work and offers Build without installation or checked download', async () => {
  const state = { ...structuredClone(readyRefinement),
    readiness: { ...readyRefinement.readiness, ready: false, implementation_blockers: ['Implement the requested view.'] } };
  const apply = `/api/guide/${state.id}/apply`;
  const accepted = { ...structuredClone(state), version: state.version + 1,
    status: 'ready', blueprint: state.proposal, proposal: null };
  await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
    const button = usePlan(document);
    assert.ok(button && !button.disabled);
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
    await act(async () => { button.click(); await settle(); });
    await waitPackage(() => document.body.textContent.includes('ACCEPTED PLAN'));
    await waitPackage(() => [...document.querySelectorAll('#setup-implementation button')]
      .some(item => item.textContent.includes('Build application') && !item.disabled));
    assert.deepEqual(calls.filter(call => call.method === 'POST'), [{ url: apply, method: 'POST', body: { version: state.version } }]);
    assert.match(document.body.textContent, /Implement the requested view/);
    assert.ok(![...document.querySelectorAll('button')].some(item => item.textContent.includes('Install agent team')));
    assert.ok(![...document.querySelectorAll('button')].some(item => item.textContent.includes('Download checked application')));
    assert.equal(accepted.readiness.ready, false);
    assert.deepEqual(accepted.brief.requirements, state.brief.requirements);
  }, packageReads(state, null, {
    [apply]: () => structuredClone(accepted),
    '/api/checker-setup': () => ({ status: 'ready', can_prepare: false, detail: 'App checks ready.' }),
  }), { allowedPosts: [apply] });
});

test('a decision-blocked, questioned or running refinement cannot offer acceptance', async () => {
  for (const changed of [
    { readiness: { ...readyRefinement.readiness, ready: false, decision_blockers: ['Agree how to handle the requested view.'] } },
    { readiness: { ...readyRefinement.readiness, ready: false, context_ready: false } },
    { question: { id: 'authority', title: 'Choose authority', options: [] } },
    { operation: { status: 'running', request_id: 'c'.repeat(64), version: 2, started_at: '2026-10-04T08:00:00Z',
      request: { action: 'chat', text: 'Current pending turn', selected: [] } } },
  ]) {
    const state = { ...structuredClone(readyRefinement), ...changed };
    await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
      assert.equal(usePlan(document) === undefined, true);
      assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
    }, packageReads(state));
  }
});

test('a ready refinement with unsent manual changes keeps acceptance disabled and preserves the draft', async () => {
  const state = structuredClone(readyRefinement), storage = packageStorage(state, true);
  await mountedGuide({ value: storage }, async (document, calls) => {
    const button = usePlan(document);
    assert.ok(button && button.disabled);
    await act(async () => { button.click(); await settle(); });
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
    assert.match(storage.getItem(manualKey(state.id)), /My unsent deployment change/);
  }, packageReads(state));
});

test('a retained ready-refinement click cannot accept after the saved version becomes blocked', async () => {
  const state = structuredClone(readyRefinement), next = { ...state, version: 3,
    readiness: { ...state.readiness, ready: false, decision_blockers: ['Choose authority.'] } };
  let switched = false, retainedClick;
  await mountedGuide({ value: packageStorage(state) }, async (document, calls) => {
    const button = usePlan(document);
    const propsKey = Object.keys(button).find(key => key.startsWith('__reactProps$'));
    retainedClick = button[propsKey].onClick;
    await act(async () => { document.querySelector('.setup-history-control > button').click(); await settle(); });
    switched = true;
    const reload = [...document.querySelectorAll('.setup-history-menu button')].find(item => item.textContent.includes('Current refinement'));
    await act(async () => { reload.click(); await settle(); });
    assert.equal(usePlan(document) === undefined, true);
    await act(async () => { retainedClick(); await settle(); });
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
  }, packageReads(state, null, {
    '/api/guide/list': () => ({ setups: [{ id: state.id, name: 'Current refinement', status: 'refining' }] }),
    [`/api/guide/${state.id}`]: () => structuredClone(switched ? next : state),
  }));
});
