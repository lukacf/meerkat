import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
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
    builder.onLoad({filter:/\/guided\.jsx$/},async args=>{
      if(!process.env.FORGE_GUIDE_STORAGE_SOURCE)return;
      return {contents:await readFile(process.env.FORGE_GUIDE_STORAGE_SOURCE,'utf8'),
        resolveDir:fileURLToPath(new URL('.',import.meta.url)),loader:'jsx'};
    });
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
    if(typeof body.ok==='boolean' && typeof body.json==='function')return body;
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

for(const originalFiles of [undefined,[],['original']])test(`saved retry preserves only its original files (${originalFiles === undefined ? 'omitted' : originalFiles.length ? 'attached' : 'empty'})`,async()=>{
  const before={...structuredClone(saved),id:'owned-exact-file-retry'};
  const file=(id,filename)=>({id:id.repeat(64),filename,media_type:'text/plain',sha256:id.repeat(64),size:20,
    url:`/api/guide/${before.id}/attachments/${id.repeat(64)}`});
  const pendingFile=file('b','unsent-next-message.txt'),originalFile=file('c','original-request.txt');
  const expectedFiles=originalFiles?.length ? [originalFile] : [];
  const text='Discuss the selected historical request without sending my unfinished next message.';
  before.operation={status:'failed',request_id:'d'.repeat(64),version:before.version,
    detail:'The response was not saved.',request:{version:before.version,action:'chat',selected:[],text,
      ...(originalFiles === undefined ? {} : {attachments:expectedFiles})}};
  const input={key:inputKey(before),draft:'My separate unsent note.',selected:['newer-choice'],mode:'chat'};
  const values=new Map([[GUIDE_KEY,before.id],[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  assert.equal(captureGuideResumeInput(durable,before,input,text),true);
  const respondUrl=`/api/guide/${before.id}/respond`;
  let server=before;
  const read=(url,options)=>{
    if(url===respondUrl && options.method==='POST')return server={...structuredClone(before),version:before.version+1,
      operation:{status:'completed',request_id:'e'.repeat(64),version:before.version,
        request:{version:before.version,...JSON.parse(options.body)}}};
    return url===`/api/guide/${before.id}`?structuredClone(server):url==='/api/guide/list'?{setups:[]}:
      url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  };
  const sessionValues=[[`mobkit-guide-attachments-v1:${before.id}`,JSON.stringify({guide_id:before.id,attachments:[pendingFile]})]];
  await mountedGuide({value:durable},async(document,calls)=>{
    assert.equal(document.querySelector('.setup-attachments a').textContent,pendingFile.filename);
    const retry=[...document.querySelectorAll('button')].find(button=>button.textContent==='Retry saved answer');
    assert.ok(retry);
    await act(async()=>retry.click());
    const posts=calls.filter(call=>call.method==='POST');
    assert.equal(posts.length,1);
    assert.equal(posts[0].body.text,text);
    assert.deepEqual(posts[0].body.attachments,expectedFiles);
    assert.deepEqual(posts[0].body.selected,[]);
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length,1);
    assert.equal(document.querySelector('.setup-attachments a').textContent,pendingFile.filename);
    assert.deepEqual(JSON.parse(values.get(input.key)),input);
  },read,{allowedPosts:[respondUrl],sessionValues});
});

test('the real guide renders completed server state when its browser completion marker hits quota, retaining the newer draft and choices',async()=>{
  const values=new Map([[GUIDE_KEY,saved.id],[draft.key,JSON.stringify(draft)]]);
  const durable={getItem:key=>values.get(key) ?? null,setItem(key,value){
    if(key.startsWith('mobkit-setup-completed-v1:'))throw new Error('Quota exceeded');values.set(key,value);
  },removeItem:key=>values.delete(key),key:index=>[...values.keys()][index] ?? null,get length(){return values.size;}};
  await mountedGuide({value:durable},(document,calls)=>{
    assert.match(document.body.textContent,/Saved server question/);
    assert.match(document.body.textContent,/Browser recovery is unavailable/);
    assert.doesNotMatch(document.body.textContent,/Connection interrupted|Quota exceeded|Opening your conversation/);
    assert.equal(document.querySelector('#setup-message').value,draft.draft);
    assert.ok(document.querySelector('.setup-choice.selected'));
    assert.equal(values.has(`mobkit-setup-completed-v1:${saved.id}`),false);
    assert.equal(calls.filter(call=>call.url===`/api/guide/${saved.id}`).length,1);
  });
});

test('a denied storage getter still opens the real guide with an editable composer, without claiming reload recovery',async()=>{
  await mountedGuide({get(){throw new Error('Storage denied');}},document=>{
    assert.match(document.body.textContent,/Saved server question/);
    assert.match(document.body.textContent,/draft and choices remain in this open page/);
    assert.doesNotMatch(document.body.textContent,/Storage denied|Connection interrupted/);
    const composer=document.querySelector('#setup-message');
    assert.equal(composer.value,'');
    assert.equal(composer.disabled,false);
    assert.equal(GuideRecoveryNotice({persistent:true}),null);
  });
});

test('Help me decide sends tentative current meanings and the full unsent note as chat, preserving local choices and input',async()=>{
  const before={...structuredClone(saved),id:'owned-tentative-help',operation:undefined,
    question:{id:'histories',stage:'knowledge',title:'Which histories matter?',description:'Consider the context boundaries.',
      multiple:true,allow_text:true,options:[
        {id:'personal',label:'Personal context',description:'Each person keeps their continuing decisions.'},
        {id:'shared',label:'Shared context',description:'A common record of the group decisions.'}]}};
  const input={key:inputKey(before),draft:'  I might use both.\nKeep my caveat unchanged: Å.  ',
    selected:['shared','personal'],mode:'chat'};
  const values=new Map([[GUIDE_KEY,before.id],[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  const respondUrl=`/api/guide/${before.id}/respond`;
  let server=before;
  const read=(url,options)=>{
    if(url===respondUrl && options.method==='POST')return server={...structuredClone(before),version:before.version+1,
      operation:{status:'completed',request_id:'f'.repeat(64),version:before.version,
        request:{version:before.version,...JSON.parse(options.body)}}};
    return url===`/api/guide/${before.id}`?structuredClone(server):
      url==='/api/guide/list'?{setups:[]}:
      url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    assert.equal(composer().value,input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length,2);
    const help=[...document.querySelectorAll('button')].find(button=>button.textContent==='Help me decide');
    assert.ok(help);
    await act(async()=>help.click());
    const posts=calls.filter(call=>call.method==='POST');
    assert.equal(posts.length,1);
    assert.equal(posts[0].url,respondUrl);
    const body=posts[0].body;
    assert.equal(body.version,before.version);
    assert.equal(body.action,'chat');
    assert.deepEqual(body.selected,[]);
    assert.equal(Object.hasOwn(body,'combine_options'),false);
    assert.match(body.text,/Briefly recommend a sensible choice or combination and explain the main tradeoff/);
    assert.match(body.text,/Tentative context for this question, not a confirmed answer/);
    for(const option of before.question.options)assert.ok(body.text.includes(`${option.label}: ${option.description}`));
    assert.ok(body.text.includes(`Full unsent note:\n${input.draft}\n`),'the note retains all whitespace and characters');
    assert.match(body.text,/not confirmation or permission to change tool grants/);
    assert.equal(composer().value,input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length,2);
    assert.deepEqual(JSON.parse(values.get(input.key)),input,'completion leaves the local human draft and choices unchanged');
    assert.match(document.body.textContent,/Ask or suggest a change/);
  },read,{allowedPosts:[respondUrl]});
  await mountedGuide({value:durable},(document,calls)=>{
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length,2);
    assert.deepEqual(JSON.parse(values.get(input.key)),input,'remount restores the exact tentative draft and choices');
    assert.equal(calls.filter(call=>call.method==='POST').length,0,'restoring the tentative context never sends an answer');
  },read);
});

test('a pending single choice keeps its note through help and submits only through Send answer',async()=>{
  const before={...structuredClone(saved),id:'owned-pending-single-choice',operation:undefined,
    question:{id:'histories',stage:'knowledge',title:'Which histories matter?',description:'Consider the context boundaries.',
      multiple:false,allow_text:true,options:[
        {id:'personal',label:'Personal context',description:'Each person keeps their continuing decisions.'},
        {id:'shared',label:'Shared context',description:'A common record of the group decisions.'}]}};
  const input={key:inputKey(before),draft:'  I am leaning toward a personal record.\nKeep my caveat unchanged: \u00e5 \u{1f9ed}.  ',
    selected:[],mode:'chat'};
  const values=new Map([[GUIDE_KEY,before.id],[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  const respondUrl=`/api/guide/${before.id}/respond`;
  let server=before;
  const read=(url,options)=>{
    if(url===respondUrl && options.method==='POST'){
      const request=JSON.parse(options.body),version=server.version;
      server={...structuredClone(server),version:version+1,
        operation:{status:'completed',request_id:(request.action==='chat'?'f':'e').repeat(64),version,request}};
      return structuredClone(server);
    }
    return url===`/api/guide/${before.id}`?structuredClone(server):url==='/api/guide/list'?{setups:[]}:
      url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    const posts=()=>calls.filter(call=>call.method==='POST');
    const choice=label=>[...document.querySelectorAll('.setup-choice')].find(button=>button.textContent.includes(label));
    assert.match(document.querySelector('#setup-send').getAttribute('aria-label')||document.querySelector('#setup-send').textContent,/Send message/);
    await act(async()=>choice('Personal context').click());
    assert.equal(posts().length,0,'selecting a card does not send an answer');
    assert.equal(composer().value,input.draft);
    assert.match(document.querySelector('#setup-send').getAttribute('aria-label')||document.querySelector('#setup-send').textContent,/Send answer/);
    assert.deepEqual(JSON.parse(values.get(input.key)),{...input,selected:['personal'],mode:'answer'});
    const help=[...document.querySelectorAll('button')].find(button=>button.textContent==='Help me decide');
    await act(async()=>help.click());
    assert.equal(posts().length,1);
    assert.equal(posts()[0].body.action,'chat');
    assert.deepEqual(posts()[0].body.selected,[]);
    assert.ok(posts()[0].body.text.includes('Personal context: Each person keeps their continuing decisions.'));
    assert.ok(!posts()[0].body.text.includes('Shared context: A common record of the group decisions.'));
    assert.ok(posts()[0].body.text.includes(`Full unsent note:\n${input.draft}\n`));
    assert.match(posts()[0].body.text,/not a confirmed answer/);
    assert.equal(composer().value,input.draft);
    assert.equal(choice('Personal context').getAttribute('aria-pressed'),'true');
    assert.deepEqual(JSON.parse(values.get(input.key)),{...input,selected:['personal'],mode:'answer'});
    const send=document.querySelector('#setup-send');
    assert.match(send.getAttribute('aria-label')||send.textContent,/Send answer/);
    await act(async()=>send.click());
    assert.equal(posts().length,2,'only help and the explicit answer are admitted');
    assert.deepEqual(posts()[1].body,{version:before.version+1,action:'answer',selected:['personal'],text:input.draft});
    assert.equal(composer().value,'');
    assert.equal(document.querySelectorAll('.setup-choice.selected').length,0);
  },read,{allowedPosts:[respondUrl]});
});

test('Help me decide refuses oversized assembled context without truncation and uses code points for the existing request limit',async()=>{
  for(const [note,tooLong] of [['\u{1f9ed}'.repeat(3000),false],['x'.repeat(6000),true]]){
    const before={...structuredClone(saved),id:'owned-help-limit',operation:undefined,
      question:{id:'context',stage:'knowledge',title:'Which context?',description:'Consider how the shared context is used.',
        multiple:true,allow_text:true,options:[{id:'shared',label:'Shared record',description:'The group records its continuing decisions.'}]}};
    const input={key:inputKey(before),draft:note,selected:['shared'],mode:'chat'};
    const values=new Map([[GUIDE_KEY,before.id],[input.key,JSON.stringify(input)]]);
    const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
      removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
    const respondUrl=`/api/guide/${before.id}/respond`;
    const read=(url,options)=>{
      if(url===respondUrl && options.method==='POST')return {...structuredClone(before),version:before.version+1,
        operation:{status:'completed',request_id:'e'.repeat(64),version:before.version,
          request:{version:before.version,...JSON.parse(options.body)}}};
      return url===`/api/guide/${before.id}`?structuredClone(before):url==='/api/guide/list'?{setups:[]}:
        url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
    };
    await mountedGuide({value:durable},async(document,calls)=>{
      const help=[...document.querySelectorAll('button')].find(button=>button.textContent==='Help me decide');
      assert.ok(help);
      await act(async()=>help.click());
      const posts=calls.filter(call=>call.method==='POST');
      if(tooLong){
        assert.equal(posts.length,0,'a valid-length raw note plus its option details is rejected before any POST');
        assert.ok(document.body.textContent.includes('Your note and selected option details are too long to send together. Shorten your note and try again. Your draft and choices are kept.'));
      }else{
        assert.equal(posts.length,1);
        assert.equal(posts[0].body.action,'chat');
        assert.deepEqual(posts[0].body.selected,[]);
        assert.ok(posts[0].body.text.length>6000,'UTF-16 length exceeds the bound but is not the server count');
        assert.ok([...posts[0].body.text].length<=6000);
        assert.ok(posts[0].body.text.includes(`Full unsent note:\n${note}\n`),'every note character is included');
      }
      assert.equal(document.querySelector('#setup-message').value,note);
      assert.equal(document.querySelectorAll('.setup-choice.selected').length,1);
      assert.deepEqual(JSON.parse(values.get(input.key)),input);
    },read,{allowedPosts:[respondUrl]});
  }
});

test('a help discussion keeps original-question choices after changed description or option meanings without selecting the new decision',async()=>{
  for(const change of [question=>{question.description='Consider this recommendation before choosing.';},
    question=>{question.options[0].description='Personal context now includes a revised sharing boundary.';}]){
    const before={...structuredClone(saved),id:'owned-help-question-change',operation:undefined,
      question:{id:'histories',stage:'knowledge',title:'Which histories matter?',description:'Consider the context boundaries.',
        multiple:true,allow_text:true,options:[
          {id:'personal',label:'Personal context',description:'Each person keeps their continuing decisions.'},
          {id:'shared',label:'Shared context',description:'A common record of the group decisions.'}]}};
    const input={key:inputKey(before),draft:'  Keep my tentative context\n\u{1f9ed}  ',selected:['personal','shared'],mode:'chat'};
    const values=new Map([[GUIDE_KEY,before.id],[input.key,JSON.stringify(input)]]);
    const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
      removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
    const respondUrl=`/api/guide/${before.id}/respond`;
    let server=before;
    const read=(url,options)=>{
      if(url===respondUrl && options.method==='POST'){
        server={...structuredClone(before),version:before.version+1,
          operation:{status:'completed',request_id:'d'.repeat(64),version:before.version,
            request:{version:before.version,...JSON.parse(options.body)}}};
        change(server.question);
        return structuredClone(server);
      }
      return url===`/api/guide/${before.id}`?structuredClone(server):url==='/api/guide/list'?{setups:[]}:
        url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
    };
    const verify=(document)=>{
      assert.notEqual(inputKey(server),input.key);
      assert.deepEqual(JSON.parse(values.get(input.key)),input,'the old full question key keeps its exact tentative selections');
      assert.equal(document.querySelector('#setup-message').value,input.draft);
      assert.equal(document.querySelectorAll('.setup-choice.selected').length,0,'old IDs are not answers to changed meanings');
      assert.deepEqual(JSON.parse(values.get(inputKey(server))),{key:inputKey(server),draft:input.draft,selected:[],mode:'chat'});
    };
    await mountedGuide({value:durable},async(document,calls)=>{
      const help=[...document.querySelectorAll('button')].find(button=>button.textContent==='Help me decide');
      assert.ok(help);
      await act(async()=>help.click());
      const posts=calls.filter(call=>call.method==='POST');
      assert.equal(posts.length,1);
      assert.equal(posts[0].body.action,'chat');
      assert.deepEqual(posts[0].body.selected,[]);
      assert.equal(Object.hasOwn(posts[0].body,'stage'),false);
      assert.equal(Object.hasOwn(posts[0].body,'blueprint'),false);
      verify(document);
    },read,{allowedPosts:[respondUrl]});
    await mountedGuide({value:durable},(document,calls)=>{
      verify(document);
      assert.equal(calls.filter(call=>call.method==='POST').length,0,'remount only observes the saved discussion');
    },read);
  }
});

test('the anonymous opening keeps a starting-point preview unsent until explicit send creates its owned setup',async()=>{
  const opening={...structuredClone(saved),id:null,version:0,operation:undefined,
    question:{id:'purpose',stage:'purpose',title:'What would you like to build?',description:'Describe the work.',
      multiple:false,allow_text:true,options:[]}};
  const started={...structuredClone(opening),id:'owned-first-help',version:1};
  const input={key:inputKey(opening),draft:'  My first unsent idea\n\u{1f9ed}  ',selected:[],mode:'chat'};
  const values=new Map([[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  const respondUrl=`/api/guide/${started.id}/respond`;
  const read=(url,options)=>{
    if(url===respondUrl && options.method==='POST')return {...structuredClone(started),version:2,
      operation:{status:'completed',request_id:'c'.repeat(64),version:1,
        request:{version:1,...JSON.parse(options.body)}}};
    return url==='/api/guide/new'?structuredClone(opening):url==='/api/guide/start'?structuredClone(started):
      url==='/api/guide/list'?{setups:[]}:url.includes('/timeline')?
        {frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    const preview=[...document.querySelectorAll('button')].find(button=>button.textContent.includes('Answer two questions'));
    assert.ok(preview);
    await act(async()=>preview.click());
    assert.equal(calls.filter(call=>call.method==='POST').length,0,'Preview never creates a setup or sends the existing idea');
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    const back=[...document.querySelectorAll('button')].find(button=>button.textContent==='Back to my words');
    assert.ok(back); await act(async()=>back.click());
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    await act(async()=>document.querySelector('#setup-send').click());
    assert.equal(calls.filter(call=>call.url==='/api/guide/start'&&call.method==='POST').length,1);
    const posts=calls.filter(call=>call.url===respondUrl&&call.method==='POST');
    assert.equal(posts.length,1);
    assert.equal(posts[0].body.action,'chat');
    assert.deepEqual(posts[0].body.selected,[]);
    assert.equal(posts[0].body.text,input.draft);
    assert.equal(document.querySelector('#setup-message').value,'');
    assert.deepEqual(JSON.parse(values.get(inputKey(started))),{...input,key:inputKey(started),draft:'',mode:'answer'});
  },read,{allowedPosts:['/api/guide/start',respondUrl]});
});

test('a lost setup creation reply and page reload reuse one saved setup before sending the retained idea',async()=>{
  const opening={...structuredClone(saved),id:null,version:0,operation:undefined,
    question:{id:'purpose',stage:'purpose',title:'What would you like to build?',description:'Describe the work.',
      multiple:false,allow_text:true,options:[]}};
  const input={key:inputKey(opening),draft:'  My first idea\nKeep this context.  ',selected:[],mode:'answer'};
  const values=new Map([[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  const created=new Map(); let lost=true, firstRequest, sessionValues;
  const respondUrl='/api/guide/owned-recovered-creation/respond';
  const read=(url,options)=>{
    if(url==='/api/guide/start' && options.method==='POST'){
      const request=JSON.parse(options.body).start_request_id;
      if(!created.has(request))created.set(request,{...structuredClone(opening),id:'owned-recovered-creation',version:1});
      if(lost){lost=false;throw new TypeError('Connection lost after creation committed');}
      return structuredClone(created.get(request));
    }
    if(url.startsWith('/api/guide/creation/'))return structuredClone(created.get(url.split('/').at(-1)));
    if(url===respondUrl && options.method==='POST')return {...structuredClone(opening),id:'owned-recovered-creation',version:2,
      operation:{status:'completed',request_id:'c'.repeat(64),version:1,request:JSON.parse(options.body)}};
    return url==='/api/guide/new'?structuredClone(opening):url==='/api/guide/list'?{setups:[]}:
      url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    await act(async()=>document.querySelector('#setup-send').click());
    firstRequest=calls.find(call=>call.url==='/api/guide/start').body.start_request_id;
    assert.match(firstRequest,/^[0-9a-f]{8}-[0-9a-f-]{27}$/);
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    assert.equal(calls.filter(call=>call.url===respondUrl).length,0,'No idea is sent before creation is acknowledged');
    const storage=document.defaultView.sessionStorage;
    sessionValues=Array.from({length:storage.length},(_,i)=>[storage.key(i),storage.getItem(storage.key(i))]);
    assert.ok(sessionValues.some(([,value])=>value===firstRequest),'The same tab retains creation ownership before sending');
  },read,{allowedPosts:['/api/guide/start',respondUrl]});
  await mountedGuide({value:durable},async(document,calls)=>{
    assert.equal(calls.filter(call=>call.method==='POST').length,0,'Reload observes without automatically replaying work');
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    const check=[...document.querySelectorAll('button')].find(button=>button.textContent==='Check saved conversation');
    assert.ok(check); await act(async()=>check.click());
    assert.ok(calls.some(call=>call.url===`/api/guide/creation/${firstRequest}`&&call.method==='GET'));
    assert.equal(calls.filter(call=>call.url==='/api/guide/start').length,0,'Creation recovery observes the original request without posting it again');
    assert.equal(calls.filter(call=>call.url===respondUrl).length,0,'Checking alone never sends the retained idea');
    await act(async()=>document.querySelector('#setup-send').click());
    assert.equal(created.size,1);
    const replies=calls.filter(call=>call.url===respondUrl);
    assert.equal(replies.length,1);assert.equal(replies[0].body.text,input.draft);
    assert.equal(document.querySelector('#setup-message').value,'');
    assert.equal(values.get(GUIDE_KEY),'owned-recovered-creation');
  },read,{allowedPosts:['/api/guide/start',respondUrl],sessionValues});
});

test('delayed real guide navigation never copies an anonymous or another saved setup draft into its destination',async()=>{
  const opening={id:null,version:0,status:'question',stage:'purpose',completed_stages:[],answers:[],messages:[],
    brief:{facts:{},requirements:[]},capabilities:{available:true},question:{id:'purpose',stage:'purpose',
      title:'What do you want to build?',description:'Describe your system.',multiple:false,options:[]}};
  const telescope={...structuredClone(opening),id:'owned-telescope',version:2,
    question:{...opening.question,id:'telescope',title:'Which telescope?',description:'Describe your equipment.'}};
  const clinic={...structuredClone(opening),id:'owned-clinic',version:3,
    question:{...opening.question,id:'clinic',title:'Who works at the clinic?',description:'Describe the staff.'}};
  const anonymousDraft={key:inputKey(opening),draft:' My workshop should coordinate repairs.\n\u00e5 ',selected:[],mode:'answer'};
  const telescopeDraft={key:inputKey(telescope),draft:' My telescope should remember targets. ',selected:[],mode:'chat'};
  const clinicDraft={key:inputKey(clinic),draft:' My clinic needs shared intake. ',selected:[],mode:'answer'};
  const values=new Map([anonymousDraft,telescopeDraft,clinicDraft].map(input=>[input.key,JSON.stringify(input)]));
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let releaseTelescope,releaseClinic,delayTelescope=true,delayClinic=true;
  const telescopeRead=new Promise(resolve=>{releaseTelescope=()=>resolve(structuredClone(telescope));});
  const clinicRead=new Promise(resolve=>{releaseClinic=()=>resolve(structuredClone(clinic));});
  const read=url=>url==='/api/guide/new'?structuredClone(opening):url==='/api/guide/list'?{setups:[
    {id:telescope.id,name:'Smart telescope companion',status:'question'},
    {id:clinic.id,name:'Community clinic',status:'question'}]}:
    url===`/api/guide/${telescope.id}`?(delayTelescope?telescopeRead:structuredClone(telescope)):
    url===`/api/guide/${clinic.id}`?(delayClinic?clinicRead:structuredClone(clinic)):
    url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    const click=async text=>{
      const button=[...document.querySelectorAll('button')].find(node=>node.textContent.includes(text));
      assert.ok(button,`Missing navigation button: ${text}`);
      await act(async()=>button.click());
    };
    assert.equal(composer().value,anonymousDraft.draft);
    await click('Answer two questions');
    assert.equal(composer().value,anonymousDraft.draft);
    await click('Back to my words');
    await click('Your setups');
    await click('Smart telescope companion');
    assert.equal(composer().value,anonymousDraft.draft,'the current setup keeps its draft during a pending read');
    assert.ok(document.body.textContent.includes('What do you want to build?'));
    await act(async()=>{delayTelescope=false;releaseTelescope();await new Promise(resolve=>setTimeout(resolve,5));});
    assert.ok(document.body.textContent.includes('Which telescope?'));
    assert.equal(composer().value,telescopeDraft.draft);
    assert.equal(JSON.parse(values.get(anonymousDraft.key)).draft,anonymousDraft.draft);
    await click('Your setups');
    await click('Community clinic');
    assert.equal(composer().value,telescopeDraft.draft);
    await act(async()=>{delayClinic=false;releaseClinic();await new Promise(resolve=>setTimeout(resolve,5));});
    assert.ok(document.body.textContent.includes('Who works at the clinic?'));
    assert.equal(composer().value,clinicDraft.draft);
    await click('Your setups');
    await click('Smart telescope companion');
    assert.equal(composer().value,telescopeDraft.draft);
    assert.equal(JSON.parse(values.get(clinicDraft.key)).draft,clinicDraft.draft);
    assert.equal(JSON.parse(values.get(anonymousDraft.key)).draft,anonymousDraft.draft);
    await click('Your setups');
    await click('Start a new setup');
    assert.equal(document.defaultView.sessionStorage.getItem(GUIDE_CREATION_KEY),null,'A deliberate new setup discards the prior anonymous creation owner');
    assert.equal(composer().value,anonymousDraft.draft,'the real new-setup action restores the unfinished anonymous draft');
    assert.equal(JSON.parse(values.get(telescopeDraft.key)).draft,telescopeDraft.draft);
    await click('Your setups');
    await click('Smart telescope companion');
    assert.equal(composer().value,telescopeDraft.draft);
    assert.ok(calls.every(call=>call.method==='GET'));
  },read,{sessionValues:[[GUIDE_CREATION_KEY,'7b35020b-a7f9-49e1-bbe6-0d9531b3b061']]});
});

test('a delayed anonymous creation cannot publish or remove drafts after navigating to a saved setup',async()=>{
  const opening={id:null,version:0,status:'question',stage:'purpose',completed_stages:[],answers:[],messages:[],
    brief:{facts:{},requirements:[]},capabilities:{available:true},question:{id:'purpose',stage:'purpose',
      title:'Describe your workshop',description:'What should it do?',multiple:false,options:[]}};
  const target={...structuredClone(opening),id:'owned-existing-telescope',version:2,
    question:{...opening.question,id:'equipment',title:'Your telescope equipment'}};
  const created={...structuredClone(opening),id:'owned-late-created',version:1};
  const anonymous={key:inputKey(opening),draft:'  Coordinate workshop repairs.\n\u00e5  ',selected:[],mode:'answer'};
  const telescope={key:inputKey(target),draft:'  Keep observing windows short.  ',selected:[],mode:'chat'};
  const values=new Map([anonymous,telescope].map(input=>[input.key,JSON.stringify(input)]));
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let releaseStart;
  const start=new Promise(resolve=>{releaseStart=()=>resolve(structuredClone(created));});
  const read=(url,options)=>url==='/api/guide/start' && options.method==='POST'?start:
    url==='/api/guide/new'?structuredClone(opening):url===`/api/guide/${target.id}`?structuredClone(target):
    url==='/api/guide/list'?{setups:[{id:target.id,name:'Existing telescope',status:'question'}]}:
    url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    const click=async text=>{
      const button=[...document.querySelectorAll('button')].find(node=>node.textContent.includes(text));
      assert.ok(button,`Missing navigation button: ${text}`);
      await act(async()=>button.click());
    };
    assert.equal(composer().value,anonymous.draft);
    const send=document.querySelector('#setup-send');
    assert.equal(send.disabled,false);
    await act(async()=>send.click());
    assert.equal(calls.filter(call=>call.url==='/api/guide/start').length,1);
    await click('Your setups');
    await click('Existing telescope');
    assert.equal(composer().value,telescope.draft);
    assert.equal(values.get(GUIDE_KEY),target.id);
    await act(async()=>{releaseStart();await new Promise(resolve=>setTimeout(resolve,5));});
    assert.match(document.body.textContent,/Your telescope equipment/);
    assert.equal(composer().value,telescope.draft);
    assert.equal(values.get(GUIDE_KEY),target.id,'the stale start cannot replace the selected setup');
    assert.equal(JSON.parse(values.get(telescope.key)).draft,telescope.draft);
    assert.equal(JSON.parse(values.get(anonymous.key)).draft,anonymous.draft);
    assert.equal([...values.keys()].some(key=>key.startsWith(`mobkit-setup-input-v2:${created.id}:`)),false);
    assert.equal(calls.some(call=>call.url.endsWith('/respond')),false,'the stale creation never sends the original answer');
    await click('Your setups');
    await click('Start a new setup');
    assert.equal(composer().value,anonymous.draft);
  },read,{allowedPosts:['/api/guide/start']});
});

test('reload consumes a newly completed ambiguous send without erasing a later repeated unsent draft',async()=>{
  const text='I want help coordinating requests and recurring work for a team. Help me work out who needs support, what gets handed off, and where useful context lives.';
  const before={id:'owned-ambiguous-send',version:3,status:'question',stage:'purpose',completed_stages:[],answers:[],messages:[],
    brief:{facts:{},requirements:[]},capabilities:{available:true},question:{id:'before',stage:'purpose',
      title:'Describe your office',description:'What should it do?',multiple:false,options:[]}};
  const completed={...structuredClone(before),version:4,question:{...before.question,id:'after',title:'Who is the system for?'},
    operation:{status:'completed',request_id:'b'.repeat(64),version:3,
      request:{version:3,action:'answer',text,selected:[]}}};
  const input={key:inputKey(before),draft:text,selected:[],mode:'answer'};
  const values=new Map([[GUIDE_KEY,before.id],[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let serverCompleted=false;
  const respondUrl=`/api/guide/${before.id}/respond`;
  const read=(url,options)=>{
    if(url===respondUrl && options.method==='POST'){
      serverCompleted=true;
      throw new Error('The response was interrupted after server completion.');
    }
    return url===`/api/guide/${before.id}`?structuredClone(serverCompleted?completed:before):
      url==='/api/guide/list'?{setups:[{id:before.id,name:'Office setup',status:'question'}]}:
      url.includes('/timeline')?{frames:[],bindings:[],next_cursor:null,latest_cursor:null,exhausted:true}:null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    const click=async text=>{
      const button=[...document.querySelectorAll('button')].find(node=>node.textContent.includes(text));
      assert.ok(button,`Missing guide button: ${text}`);
      await act(async()=>button.click());
    };
    assert.equal(composer().value,text);
    await act(async()=>document.querySelector('#setup-send').click());
    assert.equal(composer().value,text,'an ambiguous response preserves input until actual admission evidence returns');
    await click('Check saved conversation');
    assert.match(document.body.textContent,/Who is the system for\?/);
    assert.equal(composer().value,'','the newly acknowledged send is not resurrected by live-input migration');
    assert.equal(JSON.parse(values.get(inputKey(completed))).draft,'');
    assert.equal(values.get(`mobkit-setup-completed-v1:${before.id}`),completed.operation.request_id);
    await act(async()=>{
      const setter=Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set;
      setter.call(composer(),text); composer().dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
    });
    assert.equal(composer().value,text,'the user deliberately writes a later draft with the same text');
    await click('Your setups');
    await click('Office setup');
    assert.equal(composer().value,text,'a historical completed operation cannot consume that new draft');
    assert.equal(calls.filter(call=>call.url===respondUrl).length,1,'reload only observes the original request');
  },read,{allowedPosts:[respondUrl]});
});

test('the mounted guide retains saved choices beside a timed wait and editable draft through reconnect and completion',async t=>{
  const now=Date.parse('2026-10-02T15:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const before={...structuredClone(saved),id:'owned-long-guide',version:5,messages:[],operation:undefined,
    question:{id:'context',stage:'purpose',title:'Who should this help?',description:'Describe the people and work.',
      multiple:false,allow_text:true,options:[
        {id:'workshop',label:'Workshop staff',description:'Coordinate the people who own repair requests.'},
        {id:'volunteers',label:'Volunteer group',description:'Help volunteers find their next useful contribution.'}]}};
  const submitted='Help a community workshop coordinate repair requests.';
  const pending={...structuredClone(before),operation:{status:'running',request_id:'c'.repeat(64),version:5,
    started_at:new Date(now).toISOString(),request:{version:5,action:'answer',text:submitted,selected:['workshop']}}};
  const nextDraft='  Preserve my next thought\n\u00e5\u{1f9ed}  ';
  const original={key:inputKey(before),draft:submitted,selected:['workshop'],mode:'answer'};
  const values=new Map([[GUIDE_KEY,before.id],[original.key,JSON.stringify(original)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let server=before,outage=false;
  const respondUrl=`/api/guide/${before.id}/respond`;
  const read=(url,options)=>{
    if(url===respondUrl && options.method==='POST'){server=pending;return structuredClone(pending);}
    if(url===`/api/guide/${before.id}`)return structuredClone(server);
    if(url==='/api/guide/list')return {setups:[{id:before.id,name:'Workshop',status:'question'}]};
    if(url==='/api/architect/status')return {state:'working'};
    if(url.includes('/timeline')){
      if(outage)throw Error('Synthetic timeline observation interrupted');
      return {available:true,frames:[],bindings:[],exhausted:true};
    }
    return null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    const strip=()=>document.querySelector('.setup-guide-wait');
    const question=()=>document.querySelector('.setup-current-question');
    const advance=async ms=>act(async()=>{t.mock.timers.tick(ms);await settle();});
    assert.equal(strip() === null,true);
    await act(async()=>document.querySelector('#setup-send').click());
    assert.equal(composer().value,'','only the admitted text is consumed');
    assert.equal(strip().dataset.requestId,pending.operation.request_id);
    assert.ok(strip().closest('.setup-composer-area'));
    assert.equal(strip().closest('.setup-chat-scroll') === null,true,'the status remains outside transcript scrolling');
    assert.match(question().textContent,/SAVED QUESTION/);
    assert.match(question().textContent,/Your guide is responding\. Choices are paused/);
    assert.match(question().textContent,/Who should this help\?/);
    for(const option of before.question.options){
      assert.ok(question().textContent.includes(option.label));
      assert.ok(question().textContent.includes(option.description));
    }
    assert.ok([...question().querySelectorAll('button')].every(button=>button.disabled));
    assert.equal(question().querySelectorAll('.setup-choice.selected').length,0,'admitted choices are consumed, not selected again');
    assert.equal(composer().disabled,false);
    const setter=Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set;
    await act(async()=>{
      setter.call(composer(),nextDraft);
      composer().dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
    });
    assert.equal(composer().value,nextDraft);
    assert.equal(document.querySelector('#setup-send').disabled,true);
    await act(async()=>{
      document.querySelector('#setup-send').click();
      for(const button of question().querySelectorAll('button'))button.click();
      composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
    });
    assert.equal(calls.filter(call=>call.url===respondUrl).length,1);
    await advance(75000);
    assert.match(strip().textContent,/1m 15s elapsed/);
    assert.match(strip().textContent,/Your submitted answer is saved.*keep writing/s);
    assert.equal(strip().querySelector('[role="status"]').textContent,'Guide request running');
    assert.equal(strip().querySelector('[aria-live="off"]').textContent,'1m 15s elapsed');
    assert.ok(!strip().querySelector('[role="status"]').textContent.includes('elapsed'),'the clock is outside the live announcement');
    outage=true;
    await advance(2500);
    assert.match(strip().querySelector('[role="status"]').textContent,/Reconnecting/);
    assert.match(question().textContent,/SAVED QUESTION/);
    assert.ok([...question().querySelectorAll('button')].every(button=>button.disabled));
    assert.equal(composer().value,nextDraft);
    outage=false;
    await advance(2500);
    assert.equal(strip().querySelector('[role="status"]').textContent,'Guide request running');
    await advance(57000);
    assert.match(strip().textContent,/2m 17s elapsed/);
    server={...structuredClone(pending),version:6,
      question:{...before.question,id:'scale',title:'How many people will use it?',options:[
        {id:'small',label:'A small group',description:'One shared place to coordinate a few people.'},
        {id:'several',label:'Several groups',description:'Keep each group context distinct.'}]},
      messages:[{id:'submitted-user',role:'user',text:submitted,created_at:pending.operation.started_at},
        {id:'published-guide',role:'assistant',text:'Let us clarify the scale.',action:'answer',created_at:new Date(Date.now()).toISOString()}],
      operation:{...pending.operation,status:'completed'}};
    await advance(2500);
    assert.equal(strip() === null,true);
    assert.match(document.body.textContent,/How many people will use it/);
    assert.doesNotMatch(question().textContent,/SAVED QUESTION|Choices are paused|Workshop staff|Volunteer group/);
    assert.ok([...question().querySelectorAll('.setup-choice')].every(button=>!button.disabled));
    assert.equal(question().querySelectorAll('.setup-choice.selected').length,0);
    assert.equal(composer().value,nextDraft,'completion preserves the later unsent text exactly');
    assert.equal(JSON.parse(values.get(inputKey(server))).draft,nextDraft);
    assert.deepEqual(JSON.parse(values.get(inputKey(server))).selected,[],'old answer choices do not cross into the new question');
    assert.equal(calls.filter(call=>call.url===respondUrl).length,1,'elapsed ticks and recovery never resubmit');
  },read,{allowedPosts:[respondUrl]});
});

test('mounted navigation hides the outgoing wait and rejects its late receipt and reconnect error',async t=>{
  const now=Date.parse('2026-10-02T16:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const first={...structuredClone(saved),id:'owned-wait-first',version:5,messages:[],
    operation:{status:'running',request_id:'d'.repeat(64),version:5,started_at:new Date(now-75000).toISOString(),
      request:{version:5,action:'chat',text:'Submitted first context',selected:[]}}};
  const second={...structuredClone(first),id:'owned-wait-second',
    question:{...first.question,id:'second-question',title:'The second setup question'},
    operation:{...first.operation,request_id:'e'.repeat(64),started_at:new Date(now-5000).toISOString()}};
  const firstDraft={key:inputKey(first),draft:'  Unsent workshop details\n\u00e5  ',selected:[],mode:'chat'};
  const secondDraft={key:inputKey(second),draft:'  Unsent observatory details  ',selected:[],mode:'chat'};
  const values=new Map([[GUIDE_KEY,first.id],...[firstDraft,secondDraft].map(input=>[input.key,JSON.stringify(input)])]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let firstReads=0,firstTimelines=0,releaseFirst,rejectTimeline,releaseSecond,returning=false;
  const lateFirst=new Promise(resolve=>{releaseFirst=resolve;});
  const lateTimeline=new Promise((resolve,reject)=>{rejectTimeline=reject;});
  const nextRead=new Promise(resolve=>{releaseSecond=resolve;});
  const completed={...structuredClone(first),operation:{...first.operation,status:'completed'},version:6};
  const read=url=>{
    if(url===`/api/guide/${first.id}`)return returning?structuredClone(completed):++firstReads===1?structuredClone(first):lateFirst;
    if(url===`/api/guide/${second.id}`)return nextRead;
    if(url.startsWith(`/api/guide/${first.id}/timeline`) && ++firstTimelines>1)return lateTimeline;
    if(url.includes('/timeline'))return {available:true,frames:[],bindings:[],exhausted:true};
    if(url==='/api/architect/status')return {state:'working'};
    if(url==='/api/guide/list')return {setups:[{id:first.id,name:'First workshop',status:'question'},
      {id:second.id,name:'Second observatory',status:'question'}]};
    return null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message');
    const strip=()=>document.querySelector('.setup-guide-wait');
    const click=async text=>{
      const button=[...document.querySelectorAll('button')].find(node=>node.textContent.includes(text));
      assert.ok(button,`Missing navigation control: ${text}`);
      await act(async()=>button.click());
    };
    assert.equal(strip().dataset.requestId,first.operation.request_id);
    assert.match(strip().textContent,/1m 15s elapsed/);
    await act(async()=>{t.mock.timers.tick(2500);await settle();});
    await click('Your setups');
    await click('Second observatory');
    assert.equal(strip() === null,true,'a pending selection does not display the old request as current');
    await act(async()=>{releaseSecond(structuredClone(second));await settle();});
    assert.equal(strip().dataset.guideId,second.id);
    assert.equal(strip().dataset.requestId,second.operation.request_id);
    assert.match(strip().textContent,/7s elapsed/);
    assert.equal(composer().value,secondDraft.draft);
    await act(async()=>{releaseFirst(structuredClone(completed));rejectTimeline(Error('Late first timeline failure'));await settle();});
    assert.equal(strip().dataset.requestId,second.operation.request_id);
    assert.doesNotMatch(strip().textContent,/Reconnecting|1m/);
    assert.equal(composer().value,secondDraft.draft);
    returning=true;
    await click('Your setups');
    await click('First workshop');
    assert.equal(strip() === null,true);
    assert.equal(composer().value,firstDraft.draft);
    assert.equal(JSON.parse(values.get(secondDraft.key)).draft,secondDraft.draft);
    assert.ok(calls.every(call=>call.method==='GET'));
  },read);
});

function acceptanceGuide(id,name,version=5){
  const proposal={project:{name,slug:id},target:'mac',
    agent_classes:[{id:'owner',name:'Context owner',description:'Own the supplied context',
      instructions:'Use the current evidence.',instances:1,members:[],provider:'openai',model:'fixture',
      tools:{memory:true,mcp:[],notes:'none'}}],
    topology:{mode:'manual',entrypoint:'owner:main',edges:[]},connections:[],knowledge:{mode:'none'},
    flows:[],advanced:{port:4110}};
  return {id,version,status:'refining',stage:'review',question:null,proposal,blueprint:null,
    messages:[],answers:[],completed_stages:[],brief:{facts:{},requirements:[]},capabilities:{available:true},
    readiness:{ready:true,context_ready:true,decision_blockers:[],implementation_blockers:[]}};
}
const acceptedGuide=guide=>({...structuredClone(guide),version:guide.version+1,status:'ready',
  blueprint:structuredClone(guide.proposal),proposal:null});
const usePlanButton=document=>[...document.querySelectorAll('button')].find(button=>button.textContent.includes('Use this plan'));
async function clickGuideButton(document,text){
  const button=[...document.querySelectorAll('button')].find(node=>node.textContent.includes(text));
  assert.ok(button,`Missing guide control: ${text}`);
  await act(async()=>{button.click();await settle();});
}
function acceptanceReads(guides,read){
  return (url,options)=>{
    const specific=read(url,options);
    if(specific!==undefined)return specific;
    if(url==='/api/guide/list')return {setups:guides.map((guide,index)=>({id:guide.id,
      name:index===0?'First workshop':'Second observatory',status:guide.status}))};
    if(url==='/api/architect/status')return {state:'ready',available:true};
    if(url==='/api/capabilities')return {rehearsal:{start_request_replay:false}};
    if(url==='/api/checker-setup')return {status:'ready',can_prepare:false,detail:'Offline fixture checks ready.'};
    for(const guide of guides){
      if(url.startsWith(`/api/guide/${guide.id}/timeline`))return {available:true,frames:[],bindings:[],exhausted:true};
      if(['implementation','deployment','rehearsal'].some(path=>url===`/api/guide/${guide.id}/${path}`))
        return {httpError:404,detail:'No saved activity in this offline fixture.'};
    }
    return null;
  };
}
function acceptanceStorage(guides){
  const inputs=guides.map(guide=>({key:inputKey(guide),draft:`  Unsent ${guide.id} details\nkeep this note  `,
    selected:[],mode:'chat'}));
  const values=new Map([[GUIDE_KEY,guides[0].id],...inputs.map(input=>[input.key,JSON.stringify(input)])]);
  return {inputs,values,storage:{getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}}};
}

for(const outcome of ['success','failure'])for(const selection of ['running B','idle B','A-B-A','same-guide newer version','pending B'])
test(`delayed plan ${outcome} belongs to its original navigation (${selection})`,async t=>{
  const now=Date.parse('2026-10-06T17:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const first=acceptanceGuide('owned-plan-first','First proposal'),second=acceptanceGuide('owned-plan-second','Second proposal');
  if(selection==='running B'){
    second.status='question';
    second.question={...structuredClone(saved.question),id:'second-question',title:'The second setup question'};
    second.operation={status:'running',request_id:'e'.repeat(64),version:second.version,started_at:new Date(now).toISOString(),
      request:{version:second.version,action:'chat',text:'Saved second work',selected:[]}};
  }
  const {inputs,values,storage}=acceptanceStorage([first,second]);
  const applyUrl=`/api/guide/${first.id}/apply`,secondApply=`/api/guide/${second.id}/apply`,secondRespond=`/api/guide/${second.id}/respond`;
  let resolveApply,rejectApply,resolveSelection,firstServer=first,secondServer=second,applyCalls=0,secondReadFails=false;
  const delayedApply=new Promise((resolve,reject)=>{resolveApply=resolve;rejectApply=reject;});
  const delayedSelection=new Promise(resolve=>{resolveSelection=resolve;});
  const blueprints=[];
  const read=acceptanceReads([first,second],(url,options)=>{
    if(url===applyUrl && options.method==='POST')return ++applyCalls===1?delayedApply:acceptedGuide(firstServer);
    if(url===secondApply && options.method==='POST')return acceptedGuide(secondServer);
    if(url===secondRespond && options.method==='POST')return {...structuredClone(secondServer),operation:{status:'running',
      request_id:'f'.repeat(64),version:secondServer.version,started_at:new Date(now).toISOString(),request:JSON.parse(options.body)}};
    if(url===`/api/guide/${first.id}`)return structuredClone(firstServer);
    if(url===`/api/guide/${second.id}`){
      if(secondReadFails)throw Error('Second operation observation interrupted');
      return selection==='pending B'?delayedSelection:structuredClone(secondServer);
    }
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message'),send=()=>document.querySelector('#setup-send');
    const posts=()=>calls.filter(call=>call.method==='POST');
    assert.ok(usePlanButton(document) && !usePlanButton(document).disabled);
    await clickGuideButton(document,'Use this plan');
    assert.equal(send().disabled,true);
    assert.equal(composer().disabled,false,'accepting does not make the unsent note read-only');
    await act(async()=>{send().click();composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));});
    assert.deepEqual(posts(),[{url:applyUrl,method:'POST',body:{version:first.version}}]);
    await clickGuideButton(document,'Your setups');
    if(selection==='same-guide newer version'){
      firstServer={...structuredClone(first),version:6,proposal:{...structuredClone(first.proposal),project:{...first.proposal.project,name:'First proposal revision 6'}}};
      await clickGuideButton(document,'First workshop');
    }else{
      await clickGuideButton(document,'Second observatory');
      if(selection==='A-B-A'){
        await clickGuideButton(document,'Your setups');
        await clickGuideButton(document,'First workshop');
      }
    }
    const selectedFirst=['A-B-A','same-guide newer version'].includes(selection),input=inputs[selectedFirst?0:1];
    if(selection!=='pending B')assert.equal(composer().value,input.draft);
    if(selection==='running B'){
      secondReadFails=true;
      await act(async()=>{t.mock.timers.tick(2500);await settle();});
      assert.match(document.querySelector('.setup-error').textContent,/Second operation observation interrupted/);
      assert.equal(document.querySelector('.setup-guide-wait').dataset.requestId,second.operation.request_id);
    }
    const publishedBefore=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')resolveApply(acceptedGuide(first));
      else rejectApply(Object.assign(Error('First apply rejected after navigation'),{status:409}));
      await settle();
    });
    assert.deepEqual(blueprints,publishedBefore,'the outgoing apply cannot publish a blueprint after any navigation');
    assert.doesNotMatch(document.body.textContent,/First apply rejected after navigation|ACCEPTED PLAN/);
    assert.equal(posts().length,1,'settling an old apply never submits another request');
    if(selection==='pending B'){
      await act(async()=>{resolveSelection(structuredClone(second));await settle();});
      assert.equal(composer().value,inputs[1].draft);
    }
    if(selection==='running B'){
      assert.equal(send().disabled,true,'the old finally cannot unlock B while its saved guide request is running');
      assert.match(document.querySelector('.setup-current-question').textContent,/Choices are paused/);
      assert.ok([...document.querySelectorAll('.setup-current-question button')].every(button=>button.disabled));
      assert.match(document.querySelector('.setup-error').textContent,/Second operation observation interrupted/);
      assert.doesNotMatch(document.querySelector('.setup-error').textContent,/First apply/);
      assert.equal(composer().value,input.draft);
      assert.deepEqual(JSON.parse(values.get(input.key)),input);
      await act(async()=>{send().click();composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));});
      assert.equal(posts().length,1);
      secondReadFails=false;
      secondServer={...structuredClone(second),version:6,operation:{...second.operation,status:'completed'}};
      await act(async()=>{t.mock.timers.tick(2500);await settle();});
      assert.equal(send().disabled,false,'B becomes actionable only when its own saved request completes');
      assert.equal(composer().value,input.draft);
      await act(async()=>{send().click();await settle();});
      assert.equal(posts().length,2,'the outgoing request releases its processing latch for an explicit current-owner request');
      assert.equal(posts()[1].url,secondRespond);
      assert.equal(posts()[1].body.version,6);
      assert.equal(posts()[1].body.text,input.draft);
    }else{
      assert.equal(document.querySelector('.setup-error') === null,true);
      const needsReadback=selection==='A-B-A' && outcome==='success';
      assert.ok(usePlanButton(document));
      assert.equal(usePlanButton(document).disabled,needsReadback);
      assert.equal(send().disabled,needsReadback);
      const expected=selection==='same-guide newer version'?firstServer:second;
      if(selection==='A-B-A'){
        assert.equal(usePlanButton(document).closest('.setup-review-card').querySelector('h2').textContent,first.proposal.project.name);
        assert.equal(values.get(GUIDE_KEY),first.id);
        assert.deepEqual(blueprints.at(-1),first.proposal);
        if(needsReadback){
          assert.ok(document.querySelector('[aria-label="Plan save status"]'),
            'discarded navigation-fenced success cannot unlock the old submitted version');
          assert.equal(composer().value,inputs[0].draft);
          firstServer=acceptedGuide(first);
          await clickGuideButton(document,'Check saved plan');
          assert.match(document.body.textContent,/ACCEPTED PLAN/);
          assert.equal(send().disabled,false,'only explicit newer readback retires the old success fence');
          assert.equal(composer().value,inputs[0].draft);
          assert.equal(posts().length,1,'checking the saved accepted plan does not repeat acceptance');
        }
        await clickGuideButton(document,'Your setups');
        await clickGuideButton(document,'Second observatory');
      }
      assert.equal(composer().value,inputs[expected.id===first.id?0:1].draft);
      await clickGuideButton(document,'Use this plan');
      assert.match(document.body.textContent,/ACCEPTED PLAN/);
      assert.equal(posts().length,2,'the released latch admits only the deliberate selected-owner acceptance');
      assert.deepEqual(posts()[1],{url:`/api/guide/${expected.id}/apply`,method:'POST',body:{version:expected.version}});
      assert.equal(send().disabled,false,'the selected acceptance cleans up after its own version advances');
    }
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
  },read,{allowedPosts:[applyUrl,secondApply,secondRespond],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

for(const outcome of ['success','failure'])test(`same-owner plan ${outcome} releases busy and keeps the unsent note`,async()=>{
  const before=acceptanceGuide('owned-plan-control','Control proposal'),applyUrl=`/api/guide/${before.id}/apply`;
  const {inputs,values,storage}=acceptanceStorage([before]);
  let resolveApply,rejectApply,attempts=0;
  const delayed=new Promise((resolve,reject)=>{resolveApply=resolve;rejectApply=reject;});
  const read=acceptanceReads([before],(url,options)=>{
    if(url===`/api/guide/${before.id}`)return structuredClone(before);
    if(url===applyUrl && options.method==='POST')return ++attempts===1?delayed:acceptedGuide(before);
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message'),send=()=>document.querySelector('#setup-send');
    await clickGuideButton(document,'Use this plan');
    assert.equal(send().disabled,true);
    await act(async()=>{send().click();composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));});
    assert.equal(calls.filter(call=>call.method==='POST').length,1);
    await act(async()=>{
      if(outcome==='success')resolveApply(acceptedGuide(before));
      else rejectApply(Object.assign(Error('Current plan acceptance refused'),{status:409}));
      await settle();
    });
    assert.equal(composer().value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
    assert.equal(send().disabled,false,'own cleanup must release busy even after successful version advancement');
    if(outcome==='success'){
      assert.match(document.body.textContent,/ACCEPTED PLAN/);
      assert.equal(document.querySelector('.setup-error') === null,true);
      assert.equal(usePlanButton(document) === undefined, true);
    }else{
      assert.match(document.querySelector('.setup-error').textContent,/Current plan acceptance refused/);
      assert.ok(usePlanButton(document) && !usePlanButton(document).disabled);
      assert.equal(calls.filter(call=>call.method==='POST').length,1,'an error never retries acceptance automatically');
      await clickGuideButton(document,'Use this plan');
      assert.match(document.body.textContent,/ACCEPTED PLAN/);
      assert.equal(document.querySelector('.setup-error') === null,true);
      assert.equal(send().disabled,false);
    }
    assert.deepEqual(calls.filter(call=>call.method==='POST'),Array.from({length:outcome==='success'?1:2},()=>({
      url:applyUrl,method:'POST',body:{version:before.version}})));
  },read,{allowedPosts:[applyUrl]});
});

for(const held of ['headers','successful body','refused body'])
test(`plan acceptance response timeout preserves the draft and requires newer readback (${held})`,async t=>{
  const now=Date.parse('2026-10-09T15:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const before=acceptanceGuide('owned-apply-readback','Readback proposal');
  const {inputs,values,storage}=acceptanceStorage([before]);
  const applyUrl=`/api/guide/${before.id}/apply`,respondUrl=`/api/guide/${before.id}/respond`;
  let releaseApply,server=before,readFails=false;
  const delayed=new Promise(resolve=>{releaseApply=resolve;});
  const blueprints=[];
  const read=acceptanceReads([before],(url,options)=>{
    if(url===`/api/guide/${before.id}`){
      if(readFails)throw Error('Saved plan read interrupted');
      return structuredClone(server);
    }
    if(url===applyUrl && options.method==='POST')return held==='headers'?delayed:
      {ok:held==='successful body',status:held==='refused body'?409:200,json:()=>delayed};
    if(url===respondUrl && options.method==='POST')return {...structuredClone(server),operation:{status:'running',
      request_id:'7'.repeat(64),version:server.version,started_at:new Date(now+30000).toISOString(),
      request:JSON.parse(options.body)}};
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message'),send=()=>document.querySelector('#setup-send');
    const posts=()=>calls.filter(call=>call.method==='POST');
    const status=()=>document.querySelector('[aria-label="Plan save status"]');
    await clickGuideButton(document,'Use this plan');
    assert.equal(composer().disabled,false,'acceptance keeps the unfinished draft editable');
    const edited='  A revised unsent thought\nKeep this exact draft after checking.  ';
    await act(async()=>{
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set.call(composer(),edited);
      composer().dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
      await settle();
    });
    await act(async()=>{t.mock.timers.tick(30000);await settle();});
    assert.ok(status(),'an uncertain save retains a visible status separate from dismissible errors');
    assert.equal(status().dataset.guideId,before.id);
    assert.equal(composer().value,edited);
    assert.equal(composer().disabled,false);
    assert.equal(send().disabled,true,'a timed-out apply cannot admit another guide turn');
    assert.equal(usePlanButton(document).disabled,true,'a timeout never becomes permission to apply again');
    await act(async()=>{send().click();usePlanButton(document).click();
      composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));await settle();});
    assert.deepEqual(posts(),[{url:applyUrl,method:'POST',body:{version:before.version}}]);
    await clickGuideButton(document,'Dismiss');
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.ok(status(),'Dismiss removes the error, not uncertainty about the saved plan');
    assert.equal(send().disabled,true);
    await clickGuideButton(document,'Check saved plan');
    assert.ok(status(),'a same-version read cannot exclude the original compare-and-swap write');
    assert.equal(send().disabled,true);
    assert.equal(usePlanButton(document).disabled,true);
    assert.equal(composer().value,edited);
    readFails=true;
    await clickGuideButton(document,'Check saved plan');
    assert.match([...document.querySelectorAll('.setup-error')].map(node=>node.textContent).join('\n'),/Saved plan read interrupted/);
    assert.ok(status(),'a failed GET cannot retire the apply fence');
    await clickGuideButton(document,'Dismiss');
    assert.ok(status());
    assert.equal(send().disabled,true);
    assert.equal(usePlanButton(document).disabled,true);
    assert.equal(posts().length,1,'checking, dismissing and failed observation never repeat acceptance');

    readFails=false;server=acceptedGuide(before);
    await clickGuideButton(document,'Check saved plan');
    assert.equal(status() === null,true,'a newer same-guide snapshot excludes the old version-CAS write');
    assert.match(document.body.textContent,/ACCEPTED PLAN/);
    assert.equal(send().disabled,false);
    assert.equal(composer().value,edited);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),{...inputs[0],draft:edited});
    assert.equal(posts().length,1,'readback does not send the retained draft');
    const published=structuredClone(blueprints);
    await act(async()=>{
      releaseApply(held==='headers'?{ok:true,json:async()=>acceptedGuide(before)}
        :held==='refused body'?{detail:'Late acceptance refusal'}:acceptedGuide(before));
      await settle();
    });
    assert.deepEqual(blueprints,published,'late original presentation cannot replace checked state');
    assert.equal(send().disabled,false);
    assert.equal(composer().value,edited);
    await act(async()=>{send().click();await settle();});
    assert.equal(posts().length,2,'only an explicit action admits the next guide turn');
    assert.equal(posts()[1].url,respondUrl);
    assert.equal(posts()[1].body.version,server.version);
    assert.equal(posts()[1].body.text,edited);
    assert.equal(send().disabled,true,'the new guide turn retains its own running owner');
  },read,{allowedPosts:[applyUrl,respondUrl],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

for(const outcome of ['success','failure'])
test(`plan acceptance response timeout cannot replace B or release B's later owner (${outcome})`,async t=>{
  const now=Date.parse('2026-10-09T16:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const first=acceptanceGuide('owned-apply-timeout-a','First proposal'),second=acceptanceGuide('owned-apply-timeout-b','Second proposal');
  const {inputs,values,storage}=acceptanceStorage([first,second]);
  const firstApply=`/api/guide/${first.id}/apply`,secondApply=`/api/guide/${second.id}/apply`;
  let releaseFirst,rejectFirst,releaseSecond;
  const firstBody=new Promise((resolve,reject)=>{releaseFirst=resolve;rejectFirst=reject;});
  const secondBody=new Promise(resolve=>{releaseSecond=resolve;});
  const blueprints=[];
  const read=acceptanceReads([first,second],(url,options)=>{
    if(url===`/api/guide/${first.id}`)return structuredClone(first);
    if(url===`/api/guide/${second.id}`)return structuredClone(second);
    if(url===firstApply && options.method==='POST')return {ok:true,json:()=>firstBody};
    if(url===secondApply && options.method==='POST')return {ok:true,json:()=>secondBody};
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message'),send=()=>document.querySelector('#setup-send');
    const posts=()=>calls.filter(call=>call.method==='POST');
    await clickGuideButton(document,'Use this plan');
    await act(async()=>{t.mock.timers.tick(30000);await settle();});
    await clickGuideButton(document,'Your setups');
    await clickGuideButton(document,'Second observatory');
    assert.equal(composer().value,inputs[1].draft);
    assert.equal(send().disabled,false,'A uncertainty does not freeze a different selected guide');
    assert.equal(usePlanButton(document).disabled,false);
    assert.equal(document.querySelector('[aria-label="Plan save status"]') === null,true,'A status is not displayed as B status');
    await clickGuideButton(document,'Use this plan');
    assert.equal(send().disabled,true);
    assert.equal(composer().disabled,false);
    const published=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')releaseFirst(acceptedGuide(first));
      else rejectFirst(Error('Late first acceptance body failed'));
      await settle();
    });
    assert.deepEqual(blueprints,published);
    assert.equal(values.get(GUIDE_KEY),second.id);
    assert.equal(composer().value,inputs[1].draft);
    assert.equal(send().disabled,true,'the original A completion cannot release B processing');
    assert.ok(!usePlanButton(document) || usePlanButton(document).disabled,'busy B has no admitting acceptance control');
    assert.equal(document.querySelector('[aria-label="Plan save status"]').dataset.guideId,second.id);
    assert.doesNotMatch(document.body.textContent,/Late first acceptance body failed|ACCEPTED PLAN/);
    await act(async()=>{send().click();usePlanButton(document)?.click();await settle();});
    assert.deepEqual(posts(),[{url:firstApply,method:'POST',body:{version:first.version}},
      {url:secondApply,method:'POST',body:{version:second.version}}]);
    await act(async()=>{releaseSecond(acceptedGuide(second));await settle();});
    assert.equal(send().disabled,false,'only B completion releases B processing');
    assert.match(document.body.textContent,/ACCEPTED PLAN/);
    assert.equal(composer().value,inputs[1].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
    assert.deepEqual(JSON.parse(values.get(inputs[1].key)),inputs[1]);
  },read,{allowedPosts:[firstApply,secondApply],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

for(const outcome of ['success','failure'])
test(`plan acceptance response timeout survives A-B-A and cannot release a later A owner (${outcome})`,async t=>{
  const now=Date.parse('2026-10-09T17:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const first=acceptanceGuide('owned-apply-return-a','First proposal'),second=acceptanceGuide('owned-apply-return-b','Second proposal');
  const {inputs,values,storage}=acceptanceStorage([first,second]);
  const firstApply=`/api/guide/${first.id}/apply`;
  let releaseOld,rejectOld,releaseNew,server=first,applies=0;
  const oldBody=new Promise((resolve,reject)=>{releaseOld=resolve;rejectOld=reject;});
  const newBody=new Promise(resolve=>{releaseNew=resolve;});
  const blueprints=[];
  const read=acceptanceReads([first,second],(url,options)=>{
    if(url===`/api/guide/${first.id}`)return structuredClone(server);
    if(url===`/api/guide/${second.id}`)return structuredClone(second);
    if(url===firstApply && options.method==='POST')return {ok:true,json:()=>++applies===1?oldBody:newBody};
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message'),send=()=>document.querySelector('#setup-send');
    const posts=()=>calls.filter(call=>call.method==='POST');
    await clickGuideButton(document,'Use this plan');
    await act(async()=>{t.mock.timers.tick(30000);await settle();});
    await clickGuideButton(document,'Your setups');
    await clickGuideButton(document,'Second observatory');
    assert.equal(send().disabled,false);
    assert.equal(composer().value,inputs[1].draft);
    await clickGuideButton(document,'Your setups');
    await clickGuideButton(document,'First workshop');
    assert.equal(send().disabled,true,'returning to the same version cannot forget A uncertainty');
    assert.equal(usePlanButton(document).disabled,true);
    assert.equal(composer().value,inputs[0].draft);
    assert.equal(composer().disabled,false);
    assert.equal(posts().length,1);
    server={...structuredClone(first),version:first.version+1,
      proposal:{...structuredClone(first.proposal),project:{...first.proposal.project,name:'Newer first review'}}};
    await clickGuideButton(document,'Check saved plan');
    assert.equal(send().disabled,false,'a newer same-guide reviewed plan excludes the original CAS');
    assert.equal(usePlanButton(document).disabled,false);
    assert.equal(document.querySelector('[aria-label="Plan save status"]') === null,true);
    assert.equal(posts().length,1,'observing a newer review does not apply it automatically');
    await clickGuideButton(document,'Use this plan');
    assert.equal(send().disabled,true);
    const published=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')releaseOld(acceptedGuide(first));
      else rejectOld(Error('Late original acceptance body failed'));
      await settle();
    });
    assert.deepEqual(blueprints,published,'late original success cannot publish over the newer review');
    assert.equal(send().disabled,true,'old finally cannot release a later A request token');
    assert.ok(!usePlanButton(document) || usePlanButton(document).disabled,'the later A owner has no admitting acceptance control');
    assert.equal(document.querySelector('[aria-label="Plan save status"]').dataset.guideVersion,String(server.version));
    assert.equal(composer().value,inputs[0].draft);
    assert.doesNotMatch(document.body.textContent,/Late original acceptance body failed|ACCEPTED PLAN/);
    await act(async()=>{send().click();usePlanButton(document)?.click();await settle();});
    assert.deepEqual(posts(),[{url:firstApply,method:'POST',body:{version:first.version}},
      {url:firstApply,method:'POST',body:{version:server.version}}]);
    await act(async()=>{releaseNew(acceptedGuide(server));await settle();});
    assert.match(document.body.textContent,/ACCEPTED PLAN/);
    assert.equal(send().disabled,false);
    assert.equal(composer().value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
    assert.deepEqual(JSON.parse(values.get(inputs[1].key)),inputs[1]);
  },read,{allowedPosts:[firstApply],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

for(const outcome of ['success','failure'])
test(`plan acceptance deliberate retry requires an exact saved check and retains its own response owner (${outcome})`,async t=>{
  const now=Date.parse('2026-10-09T18:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const before=acceptanceGuide('owned-apply-cas-retry','Retry proposal');
  const {inputs,values,storage}=acceptanceStorage([before]);
  const applyUrl=`/api/guide/${before.id}/apply`;
  let releaseOriginal,rejectOriginal,releaseRetry,server=before,readFails=false,applies=0;
  const original=new Promise((resolve,reject)=>{releaseOriginal=resolve;rejectOriginal=reject;});
  const retryBody=new Promise(resolve=>{releaseRetry=resolve;});
  const blueprints=[];
  const read=acceptanceReads([before],(url,options)=>{
    if(url===`/api/guide/${before.id}`){
      if(readFails)throw Error('Retry eligibility read interrupted');
      return structuredClone(server);
    }
    if(url===applyUrl && options.method==='POST')return ++applies===1?original:{ok:true,json:()=>retryBody};
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const composer=()=>document.querySelector('#setup-message'),send=()=>document.querySelector('#setup-send');
    const posts=()=>calls.filter(call=>call.method==='POST');
    const retry=()=>[...document.querySelectorAll('button')].find(button=>button.textContent==='Try saving this plan again');
    const status=()=>document.querySelector('[aria-label="Plan save status"]');
    await clickGuideButton(document,'Use this plan');
    assert.equal(retry() === undefined, true,'admission alone does not authorize a duplicate acceptance');
    await act(async()=>{t.mock.timers.tick(30000);await settle();});
    assert.ok(status());
    assert.equal(retry() === undefined, true,'an elapsed response deadline does not prove the original plan remains current');
    assert.equal(composer().value,inputs[0].draft);
    assert.equal(composer().disabled,false);
    assert.equal(send().disabled,true);
    assert.equal(usePlanButton(document).disabled,true);
    assert.deepEqual(posts(),[{url:applyUrl,method:'POST',body:{version:before.version}}]);

    readFails=true;
    await clickGuideButton(document,'Check saved plan');
    assert.equal(retry() === undefined, true,'a failed check cannot authorize the explicit retry');
    assert.equal(send().disabled,true);
    assert.equal(posts().length,1);
    readFails=false;
    server={...structuredClone(before),proposal:{...structuredClone(before.proposal),
      project:{...before.proposal.project,name:'Different proposal at the same version'}}};
    await clickGuideButton(document,'Check saved plan');
    assert.equal(retry() === undefined, true,'a same-version read must preserve the exact reviewed proposal');
    assert.equal(send().disabled,true);
    assert.equal(posts().length,1);

    server=before;
    await clickGuideButton(document,'Check saved plan');
    assert.ok(retry() && !retry().disabled,'only a matching explicit saved check offers deliberate CAS retry');
    assert.equal(send().disabled,true,'retry eligibility does not admit a new guide turn');
    assert.equal(usePlanButton(document).disabled,true);
    assert.equal(composer().value,inputs[0].draft);
    assert.equal(posts().length,1,'checking the saved plan does not accept it automatically');
    const capturedRetry=retry();
    await clickGuideButton(document,'Try saving this plan again');
    assert.deepEqual(posts(),Array.from({length:2},()=>({url:applyUrl,method:'POST',body:{version:before.version}})),
      'the deliberate retry repeats only the original version-CAS acceptance body');
    assert.equal(send().disabled,true);
    assert.equal(retry() === undefined, true,'the fresh response owner requires its own check before any further retry');
    const published=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')releaseOriginal(acceptedGuide(before));
      else rejectOriginal(Error('Late original retry response failed'));
      capturedRetry.click();send().click();usePlanButton(document)?.click();
      composer().dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
      await settle();
    });
    assert.deepEqual(blueprints,published,'late original response cannot replace the checked proposal during retry');
    assert.equal(send().disabled,true,'late original settlement cannot release the deliberate retry owner');
    assert.ok(status());
    assert.equal(status().dataset.guideVersion,String(before.version));
    assert.equal(composer().value,inputs[0].draft);
    assert.doesNotMatch(document.body.textContent,/Late original retry response failed|ACCEPTED PLAN/);
    assert.equal(posts().length,2,'a captured retired retry control cannot submit a third acceptance');

    await act(async()=>{releaseRetry(acceptedGuide(before));await settle();});
    assert.equal(status() === null,true);
    assert.match(document.body.textContent,/ACCEPTED PLAN/);
    assert.equal(send().disabled,false,'only the deliberate retry confirmation admits subsequent explicit work');
    assert.equal(composer().value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
    assert.equal(posts().length,2,'confirmation does not submit the retained draft');
  },read,{allowedPosts:[applyUrl],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

test('a slow guide poll stays single-flight so an earlier checked pair cannot arrive after the latest pair',async t=>{
  const now=Date.parse('2026-10-04T10:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const before={...structuredClone(saved),id:'owned-continuous-guide',messages:[],operation:{status:'running',
    request_id:'9'.repeat(64),version:saved.version,started_at:new Date(now).toISOString(),
    request:{version:saved.version,action:'chat',selected:[],text:'Keep the current context while revising the plan.'}}};
  const plan=name=>({project:{name,slug:'continuous-guide'},agent_classes:[{id:'owner',name:'Context owner',
    description:'Own the supplied context',instructions:'Use the current evidence.',instances:1,members:[],
    provider:'openai',model:'fixture',tools:{memory:true,mcp:[],notes:'none'}}],
    topology:{mode:'manual',entrypoint:'owner:main',edges:[]},connections:[],knowledge:{mode:'none'},flows:[]});
  const staged=name=>({...structuredClone(before),live_design:{guide_id:before.id,guide_version:before.version,
    request_id:before.operation.request_id,blueprint:plan(name),brief:{facts:{outcome:{summary:name,
      status:'proposed',source_message_ids:['current-answer']}},requirements:[]}}});
  const earlier=staged('Earlier checked pair'),latest=staged('Latest checked pair');
  const terminal={...structuredClone(before),operation:{...before.operation,status:'failed'}};
  const input={key:inputKey(before),draft:'  Keep my unsent correction\n\u00e5\u{1f9ed}  ',selected:[],mode:'chat'};
  const values=new Map([[GUIDE_KEY,before.id],[input.key,JSON.stringify(input)]]);
  const durable={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),
    removeItem:key=>values.delete(key),key:index=>[...values.keys()][index]??null,get length(){return values.size;}};
  let reads=0,releaseEarlier;
  const slowEarlier=new Promise(resolve=>{releaseEarlier=resolve;});
  const read=url=>{
    if(url===`/api/guide/${before.id}`){
      reads+=1;
      if(reads===1)return structuredClone(before);
      if(reads===2)return slowEarlier;
      if(reads===4)throw Error('Synthetic guide read interrupted');
      return structuredClone(reads>=6?terminal:latest);
    }
    if(url==='/api/guide/list')return {setups:[]};
    if(url==='/api/architect/status')return {state:'working'};
    if(url.includes('/timeline'))return {available:true,frames:[],bindings:[],exhausted:true};
    return null;
  };
  await mountedGuide({value:durable},async(document,calls)=>{
    const preview=()=>document.querySelector('[data-guide-input-preview]');
    const composer=()=>document.querySelector('#setup-message');
    const advance=async ms=>act(async()=>{t.mock.timers.tick(ms);await settle();});
    await advance(2500);
    assert.equal(reads,2);
    await advance(7500);
    assert.equal(reads,2,'the slow earlier snapshot owns the only in-flight guide read');
    assert.equal(preview().dataset.proposalName,'');
    await act(async()=>{releaseEarlier(structuredClone(earlier));await settle();});
    assert.equal(preview().dataset.proposalName,'Earlier checked pair');
    await advance(2500);
    assert.equal(preview().dataset.proposalName,'Latest checked pair');
    assert.equal(preview().textContent,'Latest checked pair','preview understanding follows the same checked pair');
    assert.match(document.body.textContent,/Checked draft preview - guide response still in progress/);
    await advance(2500);
    assert.match(document.body.textContent,/Connection interrupted/);
    assert.equal(preview().dataset.proposalName,'Latest checked pair','a failed read does not discard the checked pair');
    await advance(2500);
    assert.equal(reads,5,'a failed read releases the polling slot for the same saved operation');
    assert.doesNotMatch(document.body.textContent,/Connection interrupted/);
    assert.equal(preview().dataset.proposalName,'Latest checked pair');
    await advance(2500);
    assert.equal(reads,6);
    assert.equal(preview().dataset.proposalName,'','a failed guide turn cannot keep its staged preview as current');
    await advance(10000);
    assert.equal(reads,6,'the terminal operation retires its guide watch');
    assert.equal(composer().value,input.draft);
    assert.deepEqual(JSON.parse(values.get(input.key)),input);
    assert.ok(calls.every(call=>call.method==='GET'),'observing checked proposals never resubmits a guide request');
  },read);
});

const configModal=document=>document.querySelector('[aria-labelledby="setup-config-title"]');
const configSend=document=>configModal(document)?.querySelector('footer .setup-action');
const configText=document=>document.querySelector('#setup-config-source');
const configCandidate=(guide,name)=>({...structuredClone(guide.proposal),project:{...guide.proposal.project,name}});
async function waitConfig(predicate){
  for(let attempt=0;attempt<60 && !predicate();attempt++)await act(async()=>{await settle();});
  assert.ok(predicate(),'The exact mounted configuration request reached its expected boundary');
}
async function typeConfig(document,text){
  const source=configText(document);
  assert.ok(source,'The current configuration editor is open');
  await act(async()=>{
    Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set.call(source,text);
    source.dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
    await settle();
  });
}
async function sendConfig(document){
  const button=configSend(document);
  assert.ok(button && !button.disabled,'Only the current editor can deliberately submit');
  await act(async()=>{button.click();await settle();});
}
function configReads(guides,read){
  return acceptanceReads(guides,(url,options)=>{
    const result=read(url,options);
    if(result!==undefined)return result;
    if(url==='/api/guide/new')return structuredClone(guides[0]);
    const guide=guides.find(item=>item.id && url===`/api/guide/${item.id}`);
    if(guide)return structuredClone(guide);
  });
}
const configRunning=(guide,body)=>({...structuredClone(guide),operation:{status:'running',request_id:'6'.repeat(64),
  version:guide.version,started_at:'2026-10-06T20:00:00Z',request:body}});

for(const outcome of ['success','failure'])for(const selection of ['running B','idle B','A-B-A','same-guide newer version','pending B'])
test(`manual import ${outcome} cannot publish across navigation (${selection})`,async t=>{
  const now=Date.parse('2026-10-06T20:00:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const first=acceptanceGuide('owned-config-first','First manual proposal'),second=acceptanceGuide('owned-config-second','Second manual proposal');
  if(selection==='running B'){
    second.status='question';
    second.question={...structuredClone(saved.question),id:'current-second-question',title:'Current second setup question'};
    second.operation={status:'running',request_id:'7'.repeat(64),version:second.version,started_at:new Date(now).toISOString(),
      request:{version:second.version,action:'chat',selected:[],text:'Saved second guide request'}};
  }
  const {storage,values,inputs}=acceptanceStorage([first,second]),blueprints=[];
  const originalCandidate=configCandidate(first,'Outgoing imported plan');
  let releaseImport,rejectImport,releaseSelection,imports=0,firstServer=first,secondReadFails=false;
  const delayedImport=new Promise((resolve,reject)=>{releaseImport=resolve;rejectImport=reject;});
  const delayedSelection=new Promise(resolve=>{releaseSelection=resolve;});
  const respondFirst=`/api/guide/${first.id}/respond`,respondSecond=`/api/guide/${second.id}/respond`;
  const read=configReads([first,second],(url,options)=>{
    if(url==='/api/blueprint/import')return ++imports===1?delayedImport:{blueprint:JSON.parse(options.body)};
    if(url===`/api/guide/${first.id}`)return structuredClone(firstServer);
    if(url===`/api/guide/${second.id}`){
      if(secondReadFails)throw Error('Current second observation interrupted');
      return selection==='pending B'?delayedSelection:structuredClone(second);
    }
    if(url===respondFirst && options.method==='POST')return configRunning(firstServer,JSON.parse(options.body));
    if(url===respondSecond && options.method==='POST')return configRunning(second,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const posts=()=>calls.filter(call=>call.method==='POST');
    await clickGuideButton(document,'Configuration');
    await typeConfig(document,JSON.stringify(originalCandidate,null,2));
    const submit=configSend(document),propsKey=Object.keys(submit).find(key=>key.startsWith('__reactProps$'));
    const retainedSubmit=submit[propsKey].onClick;
    await act(async()=>{retainedSubmit();retainedSubmit();await settle();});
    assert.deepEqual(posts(),[{url:'/api/blueprint/import',method:'POST',body:originalCandidate}],
      'synchronous duplicate editor clicks admit only one import');
    assert.equal(configSend(document).disabled,true);
    await clickGuideButton(document,'Your setups');
    if(selection==='same-guide newer version'){
      firstServer={...structuredClone(first),version:6,proposal:configCandidate(first,'Current manual revision 6')};
      await clickGuideButton(document,'First workshop');
    }else{
      await clickGuideButton(document,'Second observatory');
      if(selection==='A-B-A'){
        await clickGuideButton(document,'Your setups');
        await clickGuideButton(document,'First workshop');
      }
    }
    assert.equal(configModal(document) === null,true,'navigation cannot retain the outgoing configuration editor');
    let currentEditorError;
    if(selection==='running B'){
      secondReadFails=true;
      await act(async()=>{t.mock.timers.tick(2500);await settle();});
      assert.match(document.querySelector('.setup-error').textContent,/Current second observation interrupted/);
    }else if(selection!=='pending B'){
      await clickGuideButton(document,'Configuration');
      await typeConfig(document,'{');
      await sendConfig(document);
      currentEditorError=configModal(document).querySelector('.setup-error').textContent;
      assert.ok(currentEditorError);
    }
    const publishedBefore=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')releaseImport({blueprint:originalCandidate});
      else rejectImport(Error('Outgoing configuration import refused'));
      await settle();
    });
    assert.deepEqual(blueprints,publishedBefore,'old imported configuration cannot publish on the selected guide');
    assert.equal(values.get(manualKey(first.id)),undefined);
    assert.equal(values.get(manualKey(second.id)),undefined);
    assert.equal(posts().length,1,'old settlement cannot write or submit manual feedback');
    assert.doesNotMatch(document.body.textContent,/Outgoing configuration import refused|Outgoing imported plan/);
    if(selection==='pending B'){
      await act(async()=>{releaseSelection(structuredClone(second));await settle();});
      assert.equal(configModal(document) === null,true);
    }else if(currentEditorError){
      assert.equal(configText(document).value,'{','the selected editor draft survives old completion');
      assert.equal(configModal(document).querySelector('.setup-error').textContent,currentEditorError);
      assert.equal(configSend(document).disabled,false,'old cleanup cannot change selected editor eligibility');
    }
    const current=['A-B-A','same-guide newer version'].includes(selection)?firstServer:second;
    const input=inputs[current.id===first.id?0:1];
    assert.equal(values.get(GUIDE_KEY),current.id);
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    assert.deepEqual(JSON.parse(values.get(input.key)),input);
    if(selection==='running B'){
      assert.equal(document.querySelector('#setup-send').disabled,true);
      assert.equal(document.querySelector('.setup-guide-wait').dataset.requestId,second.operation.request_id);
      assert.ok([...document.querySelectorAll('.setup-current-question button')].every(button=>button.disabled));
      assert.match(document.querySelector('.setup-error').textContent,/Current second observation interrupted/);
      assert.equal([...document.querySelectorAll('button')].find(button=>button.textContent.trim()==='Configuration').disabled,true);
    }else{
      if(!configModal(document))await clickGuideButton(document,'Configuration');
      const nextCandidate=configCandidate(current,'Selected owner manual change');
      await typeConfig(document,JSON.stringify(nextCandidate,null,2));
      await sendConfig(document);
      await waitConfig(()=>calls.some(call=>call.url===`/api/guide/${current.id}/respond`));
      assert.equal(configModal(document) === null,true);
      assert.deepEqual(posts().map(call=>call.url),['/api/blueprint/import','/api/blueprint/import',`/api/guide/${current.id}/respond`]);
      const response=posts()[2];
      assert.equal(response.body.version,current.version);
      assert.equal(response.body.action,'manual');
      assert.deepEqual(response.body.blueprint,nextCandidate);
      assert.deepEqual(JSON.parse(values.get(manualKey(current.id))).blueprint,nextCandidate);
      assert.equal(values.get(manualKey(current.id===first.id?second.id:first.id)),undefined);
      assert.equal(document.querySelector('#setup-message').value,input.draft);
    }
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
  },read,{allowedPosts:['/api/blueprint/import',respondFirst,respondSecond],
    onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

for(const outcome of ['success','failure'])for(const replacement of ['Cancel','Close configuration','Escape','backdrop','edit','edit A-B-A'])
test(`manual import ${outcome} cannot settle a newer editor (${replacement})`,async()=>{
  const guide=acceptanceGuide('owned-editor-lifetime','Editor lifetime proposal');
  const {storage,values,inputs}=acceptanceStorage([guide]),blueprints=[];
  const originalCandidate=configCandidate(guide,'Imported editor intent'),originalText=JSON.stringify(originalCandidate,null,2);
  let releaseFirst,rejectFirst,releaseSecond,imports=0;
  const firstImport=new Promise((resolve,reject)=>{releaseFirst=resolve;rejectFirst=reject;});
  const secondImport=new Promise(resolve=>{releaseSecond=resolve;});
  const respond=`/api/guide/${guide.id}/respond`;
  const read=configReads([guide],(url,options)=>{
    if(url==='/api/blueprint/import')return ++imports===1?firstImport:imports===2?secondImport:{blueprint:JSON.parse(options.body)};
    if(url===respond && options.method==='POST')return configRunning(guide,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const posts=()=>calls.filter(call=>call.method==='POST');
    await clickGuideButton(document,'Configuration');
    await typeConfig(document,originalText);
    await sendConfig(document);
    assert.equal(configSend(document).disabled,true);
    let nextCandidate=originalCandidate;
    if(replacement.startsWith('edit')){
      nextCandidate=configCandidate(guide,'Current edited intent');
      await typeConfig(document,JSON.stringify(nextCandidate,null,2));
      if(replacement==='edit A-B-A'){
        nextCandidate=originalCandidate;
        await typeConfig(document,originalText);
      }
    }else{
      await act(async()=>{
        if(replacement==='Escape')document.dispatchEvent(new document.defaultView.KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
        else if(replacement==='backdrop')configModal(document).parentElement.click();
        else if(replacement==='Close configuration')configModal(document).querySelector('[aria-label="Close configuration"]').click();
        else [...configModal(document).querySelectorAll('footer button')].find(button=>button.textContent==='Cancel').click();
        await settle();
      });
      assert.equal(configModal(document) === null,true);
      await clickGuideButton(document,'Configuration');
      await typeConfig(document,originalText);
    }
    const retainedText=JSON.stringify(nextCandidate,null,2);
    await sendConfig(document);
    assert.equal(configSend(document).disabled,true);
    assert.equal(posts().length,2,'a new explicit editor request can proceed before old import settles');
    const publicationsBefore=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')releaseFirst({blueprint:originalCandidate});
      else rejectFirst(Error('Canceled editor import refused'));
      await settle();
    });
    assert.deepEqual(blueprints,publicationsBefore);
    assert.equal(configText(document).value,retainedText);
    assert.equal(configSend(document).disabled,true,'old finally cannot unlock the newer pending request');
    assert.equal(configModal(document).querySelector('.setup-error') === null,true);
    assert.equal(values.get(manualKey(guide.id)),undefined);
    assert.equal(posts().length,2);
    await act(async()=>{releaseSecond({httpError:422,detail:'Current editor import refused'});await settle();});
    await waitConfig(()=>configSend(document)?.disabled===false);
    assert.match(configModal(document).querySelector('.setup-error').textContent,/Current editor import refused/);
    assert.equal(configText(document).value,retainedText);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
    assert.equal(posts().length,2,'the current refusal is not automatically retried');
    await sendConfig(document);
    await waitConfig(()=>calls.some(call=>call.url===respond));
    assert.equal(configModal(document) === null,true);
    assert.deepEqual(posts().slice(0,3).map(call=>call.body),[originalCandidate,nextCandidate,nextCandidate]);
    assert.deepEqual(posts().map(call=>call.url),['/api/blueprint/import','/api/blueprint/import','/api/blueprint/import',respond]);
    assert.equal(posts()[3].body.version,guide.version);
    assert.equal(posts()[3].body.action,'manual');
    assert.deepEqual(posts()[3].body.blueprint,nextCandidate);
    assert.deepEqual(JSON.parse(values.get(manualKey(guide.id))).blueprint,nextCandidate);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
  },read,{allowedPosts:['/api/blueprint/import',respond],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

test('unchanged current configuration closes only its own editor without manual feedback',async()=>{
  const guide=acceptanceGuide('owned-unchanged-config','Unchanged proposal'),{storage,values,inputs}=acceptanceStorage([guide]);
  const read=configReads([guide],(url,options)=>url==='/api/blueprint/import'?{blueprint:JSON.parse(options.body)}:undefined);
  await mountedGuide({value:storage},async(document,calls)=>{
    await clickGuideButton(document,'Configuration');
    assert.deepEqual(JSON.parse(configText(document).value),guide.proposal);
    await sendConfig(document);
    await waitConfig(()=>configModal(document)===null);
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[{url:'/api/blueprint/import',method:'POST',body:guide.proposal}]);
    assert.equal(values.get(manualKey(guide.id)),undefined);
    assert.equal(document.querySelector('#setup-send').disabled,false);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
  },read,{allowedPosts:['/api/blueprint/import']});
});

test('current pending manual configuration remains the base through honest refusal and explicit retry',async()=>{
  const guide=acceptanceGuide('owned-pending-config','Original guide plan'),{storage,values,inputs}=acceptanceStorage([guide]);
  const pending={blueprint:{...structuredClone(guide.proposal),target:'linux'},text:'My saved pending destination choice.',changes:['Deployment settings']};
  values.set(manualKey(guide.id),JSON.stringify(pending));
  const candidate={...structuredClone(pending.blueprint),advanced:{port:4550}};
  let imports=0;
  const respond=`/api/guide/${guide.id}/respond`;
  const read=configReads([guide],(url,options)=>{
    if(url==='/api/blueprint/import')return ++imports===1?{httpError:422,detail:'Current pending choice needs correction'}:{blueprint:JSON.parse(options.body)};
    if(url===respond)return configRunning(guide,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    await clickGuideButton(document,'Configuration');
    assert.deepEqual(JSON.parse(configText(document).value),pending.blueprint,'pending manual intent remains editable');
    await typeConfig(document,JSON.stringify(candidate,null,2));
    await sendConfig(document);
    assert.match(configModal(document).querySelector('.setup-error').textContent,/Current pending choice needs correction/);
    assert.deepEqual(JSON.parse(values.get(manualKey(guide.id))),pending,'import refusal cannot replace the retained manual feedback');
    assert.equal(calls.filter(call=>call.method==='POST').length,1);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    await sendConfig(document);
    await waitConfig(()=>calls.some(call=>call.url===respond));
    assert.equal(configModal(document) === null,true);
    const submitted=calls.find(call=>call.url===respond).body;
    assert.equal(submitted.version,guide.version);
    assert.equal(submitted.action,'manual');
    assert.deepEqual(submitted.blueprint,candidate);
    assert.equal(submitted.blueprint.target,'linux','retry cannot fall back to the older guide blueprint');
    assert.deepEqual(JSON.parse(values.get(manualKey(guide.id))).blueprint,candidate);
    assert.deepEqual(calls.filter(call=>call.url==='/api/blueprint/import').map(call=>call.body),[candidate,candidate]);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
  },read,{allowedPosts:['/api/blueprint/import',respond]});
});

test('manual configuration in an anonymous setup retains supported startup and exact new-owner feedback',async()=>{
  const anonymous={...acceptanceGuide('anonymous-config','Anonymous plan'),id:null,version:0};
  const started={...structuredClone(anonymous),id:'owned-started-config',version:1};
  const {storage,values,inputs}=acceptanceStorage([anonymous]);
  const candidate=configCandidate(anonymous,'Anonymous operator edit'),respond=`/api/guide/${started.id}/respond`;
  const read=configReads([anonymous,started],(url,options)=>{
    if(url==='/api/guide/list')return {setups:[]};
    if(url==='/api/blueprint/import')return {blueprint:JSON.parse(options.body)};
    if(url==='/api/guide/start')return structuredClone(started);
    if(url===respond)return configRunning(started,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    await clickGuideButton(document,'Configuration');
    await typeConfig(document,JSON.stringify(candidate,null,2));
    await sendConfig(document);
    await waitConfig(()=>calls.some(call=>call.url===respond));
    assert.equal(configModal(document) === null,true);
    assert.deepEqual(calls.filter(call=>call.method==='POST').map(call=>call.url),['/api/blueprint/import','/api/guide/start',respond]);
    const creation=calls.find(call=>call.url==='/api/guide/start').body;
    assert.deepEqual(Object.keys(creation),['start_request_id']);
    assert.match(creation.start_request_id,/^[0-9a-f]{8}-[0-9a-f-]{27}$/);
    const submitted=calls.find(call=>call.url===respond).body;
    assert.equal(submitted.version,started.version);
    assert.equal(submitted.action,'manual');
    assert.deepEqual(submitted.blueprint,candidate);
    assert.equal(values.get(GUIDE_KEY),started.id);
    assert.equal(values.get(manualKey(null)),undefined);
    assert.deepEqual(JSON.parse(values.get(manualKey(started.id))).blueprint,candidate);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.equal(JSON.parse(values.get(inputKey(started))).draft,inputs[0].draft);
  },read,{allowedPosts:['/api/blueprint/import','/api/guide/start',respond]});
});

for(const outcome of ['success','failure'])test(`current saved manual import ${outcome} preserves its draft and deliberate retry`,async()=>{
  const guide=acceptanceGuide('owned-config-control','Current manual proposal'),{storage,values,inputs}=acceptanceStorage([guide]);
  const candidate=configCandidate(guide,'Current operator adjustment'),text=JSON.stringify(candidate,null,2);
  let release,reject,imports=0;
  const deferred=new Promise((resolve,fail)=>{release=resolve;reject=fail;});
  const respond=`/api/guide/${guide.id}/respond`;
  const read=configReads([guide],(url,options)=>{
    if(url==='/api/blueprint/import')return ++imports===1?deferred:{blueprint:JSON.parse(options.body)};
    if(url===respond)return configRunning(guide,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    await clickGuideButton(document,'Configuration');
    await typeConfig(document,text);
    await sendConfig(document);
    assert.equal(configSend(document).disabled,true);
    await act(async()=>{
      if(outcome==='success')release({blueprint:candidate});
      else reject(Error('Current manual import refused'));
      await settle();
    });
    if(outcome==='failure'){
      assert.match(configModal(document).querySelector('.setup-error').textContent,/Current manual import refused/);
      assert.equal(configText(document).value,text);
      assert.equal(configSend(document).disabled,false);
      assert.equal(values.get(manualKey(guide.id)),undefined);
      assert.deepEqual(calls.filter(call=>call.method==='POST'),[{url:'/api/blueprint/import',method:'POST',body:candidate}]);
      assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
      await sendConfig(document);
    }
    await waitConfig(()=>calls.some(call=>call.url===respond));
    assert.equal(configModal(document) === null,true);
    const posts=calls.filter(call=>call.method==='POST');
    assert.deepEqual(posts.map(call=>call.url),[...Array(outcome==='success'?1:2).fill('/api/blueprint/import'),respond]);
    assert.ok(posts.filter(call=>call.url==='/api/blueprint/import').every(call=>JSON.stringify(call.body)===JSON.stringify(candidate)));
    assert.equal(posts.at(-1).body.action,'manual');
    assert.equal(posts.at(-1).body.version,guide.version);
    assert.deepEqual(posts.at(-1).body.blueprint,candidate);
    const retained=JSON.parse(values.get(manualKey(guide.id)));
    assert.deepEqual(retained.blueprint,candidate);
    assert.equal(retained.text,posts.at(-1).body.text);
    assert.ok(retained.changes.length>0);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
    assert.equal(document.querySelector('.setup-guide-wait').dataset.requestId,'6'.repeat(64));
    assert.equal(document.querySelector('#setup-send').disabled,true,'only the admitted manual guide operation owns busy state');
  },read,{allowedPosts:['/api/blueprint/import',respond]});
});

for(const outcome of ['success','failure'])test(`same-guide response revision fences pending manual import ${outcome} without navigation`,async()=>{
  const guide=acceptanceGuide('owned-config-revision','Original editor base'),next={...structuredClone(guide),version:6,
    proposal:configCandidate(guide,'Current revised guide base')};
  const {storage,values}=acceptanceStorage([guide]),blueprints=[];
  const originalCandidate=configCandidate(guide,'Superseded imported adjustment'),originalText=JSON.stringify(originalCandidate,null,2);
  const laterDraft='  Later unsent revision note\nretain this exact text  ';
  let releaseImport,rejectImport,releaseResponse,imports=0,responses=0;
  const delayedImport=new Promise((resolve,reject)=>{releaseImport=resolve;rejectImport=reject;});
  const delayedResponse=new Promise(resolve=>{releaseResponse=resolve;});
  const respond=`/api/guide/${guide.id}/respond`;
  const read=configReads([guide],(url,options)=>{
    if(url==='/api/blueprint/import')return ++imports===1?delayedImport:{blueprint:JSON.parse(options.body)};
    if(url===respond)return ++responses===1?delayedResponse:configRunning(next,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    await clickGuideButton(document,'Configuration');
    await typeConfig(document,originalText);
    await sendConfig(document);
    const composer=document.querySelector('#setup-message');
    await act(async()=>{document.querySelector('#setup-send').click();await settle();});
    assert.equal(calls.filter(call=>call.url===respond).length,1);
    assert.equal(calls.find(call=>call.url===respond).body.action,'chat');
    await act(async()=>{
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set.call(composer,laterDraft);
      composer.dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
      await settle();
      releaseResponse(structuredClone(next));
      await settle();
    });
    assert.equal(configText(document).value,originalText);
    assert.equal(configSend(document).disabled,true);
    assert.match(configModal(document).querySelector('.setup-error').textContent,/The setup changed/);
    const currentPublications=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')releaseImport({blueprint:originalCandidate});
      else rejectImport(Error('Superseded import refused after guide revision'));
      await settle();
    });
    assert.deepEqual(blueprints,currentPublications);
    assert.deepEqual(blueprints.at(-1),next.proposal);
    assert.equal(values.get(manualKey(guide.id)),undefined);
    assert.equal(configText(document).value,originalText);
    assert.equal(configSend(document).disabled,true,'releasing import state does not authorize the old guide revision');
    assert.match(configModal(document).querySelector('.setup-error').textContent,/The setup changed/);
    assert.doesNotMatch(document.body.textContent,/Superseded import refused/);
    assert.equal(calls.filter(call=>call.method==='POST').length,2);
    assert.equal(document.querySelector('#setup-message').value,laterDraft);
    assert.equal(JSON.parse(values.get(inputKey(next))).draft,laterDraft);
    await act(async()=>{configModal(document).querySelector('[aria-label="Close configuration"]').click();await settle();});
    await clickGuideButton(document,'Configuration');
    assert.deepEqual(JSON.parse(configText(document).value),next.proposal);
    const currentCandidate=configCandidate(next,'Deliberate current revision adjustment');
    await typeConfig(document,JSON.stringify(currentCandidate,null,2));
    await sendConfig(document);
    await waitConfig(()=>calls.filter(call=>call.url===respond).length===2);
    const manual=calls.filter(call=>call.url===respond)[1].body;
    assert.equal(manual.action,'manual');
    assert.equal(manual.version,6);
    assert.deepEqual(manual.blueprint,currentCandidate);
    assert.deepEqual(JSON.parse(values.get(manualKey(guide.id))).blueprint,currentCandidate);
    assert.equal(document.querySelector('#setup-message').value,laterDraft);
  },read,{allowedPosts:['/api/blueprint/import',respond],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

const moreCount=document=>document.querySelector('.ep-stepper [aria-label="Ask guide for more Context owner agents"]');
const countValue=document=>document.querySelector('.ep-stepper span')?.textContent;
function twoOwnerPlan(guide){
  const plan=structuredClone(guide.proposal || guide.blueprint);
  plan.agent_classes[0].instances=2;
  plan.topology.entrypoint='owner:1';
  return plan;
}
async function requestMoreCount(document){
  const button=moreCount(document);
  assert.ok(button && !button.disabled,'The current visible class-count control is enabled');
  await act(async()=>{button.click();await settle();});
}

for(const outcome of ['success','failure'])for(const selection of ['running B','idle B','A-B-A','same-guide newer version',
  'same-guide changed plan','pending B','pending manual B'])
test(`count import ${outcome} cannot publish across navigation (${selection})`,async t=>{
  const now=Date.parse('2026-10-06T20:30:00Z');
  t.mock.timers.enable({apis:['Date','setInterval','setTimeout'],now});
  const first=acceptanceGuide('owned-count-first','First count proposal'),second=acceptanceGuide('owned-count-second','Second count proposal');
  if(selection==='running B'){
    second.status='question';
    second.question={...structuredClone(saved.question),id:'second-count-question',title:'The second count setup question'};
    second.operation={status:'running',request_id:'8'.repeat(64),version:second.version,started_at:new Date(now).toISOString(),
      request:{version:second.version,action:'chat',selected:[],text:'Saved second count guide work'}};
  }
  const {storage,values,inputs}=acceptanceStorage([first,second]),blueprints=[];
  const pendingManual={blueprint:{...structuredClone(second.proposal),target:'linux'},
    text:'My separately saved second destination choice.',changes:['Deployment settings']};
  if(selection==='pending manual B')values.set(manualKey(second.id),JSON.stringify(pendingManual));
  const outgoing=twoOwnerPlan(first);
  let releaseImport,rejectImport,releaseSelection,imports=0,firstServer=first,secondReadFails=false;
  const deferred=new Promise((resolve,reject)=>{releaseImport=resolve;rejectImport=reject;});
  const pendingSelection=new Promise(resolve=>{releaseSelection=resolve;});
  const firstRespond=`/api/guide/${first.id}/respond`,secondRespond=`/api/guide/${second.id}/respond`;
  const read=configReads([first,second],(url,options)=>{
    if(url==='/api/blueprint/import')return ++imports===1?deferred:{blueprint:JSON.parse(options.body)};
    if(url===`/api/guide/${first.id}`)return structuredClone(firstServer);
    if(url===`/api/guide/${second.id}`){
      if(secondReadFails)throw Error('Current count setup observation interrupted');
      return selection==='pending B'?pendingSelection:structuredClone(second);
    }
    if(url===firstRespond)return configRunning(firstServer,JSON.parse(options.body));
    if(url===secondRespond)return configRunning(second,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const posts=()=>calls.filter(call=>call.method==='POST');
    assert.equal(countValue(document),'1');
    const button=moreCount(document),propsKey=Object.keys(button).find(key=>key.startsWith('__reactProps$'));
    assert.ok(button && !button.disabled,'This uses the actual evolving-plan class-count control');
    const retainedClick=button[propsKey].onClick;
    await act(async()=>{retainedClick();retainedClick();await settle();});
    assert.deepEqual(posts(),[{url:'/api/blueprint/import',method:'POST',body:outgoing}],
      'two synchronous clicks cannot admit duplicate count imports');
    assert.equal(moreCount(document).disabled,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    await clickGuideButton(document,'Your setups');
    if(selection.startsWith('same-guide')){
      firstServer={...structuredClone(first),version:selection==='same-guide newer version'?6:first.version,
        proposal:configCandidate(first,'Selected current count plan')};
      await clickGuideButton(document,'First workshop');
    }else{
      await clickGuideButton(document,'Second observatory');
      if(selection==='A-B-A'){
        await clickGuideButton(document,'Your setups');
        await clickGuideButton(document,'First workshop');
      }
    }
    if(selection==='running B'){
      secondReadFails=true;
      await act(async()=>{t.mock.timers.tick(2500);await settle();});
      assert.match(document.querySelector('.setup-error').textContent,/Current count setup observation interrupted/);
    }
    const publications=structuredClone(blueprints);
    await act(async()=>{
      if(outcome==='success')releaseImport({blueprint:outgoing});
      else rejectImport(Error('Outgoing count import refused'));
      await settle();
    });
    assert.deepEqual(blueprints,publications,'old count settlement cannot publish another selected blueprint');
    assert.equal(posts().length,1,'old count settlement cannot submit a manual guide operation');
    assert.equal(values.get(manualKey(first.id)),undefined);
    if(selection==='pending manual B')assert.deepEqual(JSON.parse(values.get(manualKey(second.id))),pendingManual);
    else assert.equal(values.get(manualKey(second.id)),undefined);
    assert.doesNotMatch(document.body.textContent,/Outgoing count import refused/);
    if(selection==='pending B'){
      assert.equal(document.querySelector('.setup-guide-wait') === null,true,'a pending selection does not borrow an old operation');
      // The old plan can remain visible while loading, but it cannot admit another count import.
      const pendingControl=moreCount(document);
      assert.ok(pendingControl,'The displayed count control still belongs to the outgoing plan during loading');
      await act(async()=>{pendingControl.click();await settle();});
      assert.equal(posts().length,1);
      await act(async()=>{releaseSelection(structuredClone(second));await settle();});
    }
    const current=selection==='A-B-A' || selection.startsWith('same-guide')?firstServer:second;
    const input=inputs[current.id===first.id?0:1];
    assert.equal(values.get(GUIDE_KEY),current.id);
    assert.equal(document.querySelector('#setup-message').value,input.draft);
    assert.deepEqual(JSON.parse(values.get(input.key)),input);
    assert.equal(countValue(document),'1','the selected owner never receives the outgoing count');
    if(selection==='running B'){
      assert.equal(moreCount(document).disabled,true);
      assert.equal(document.querySelector('#setup-send').disabled,true);
      assert.equal(document.querySelector('.setup-guide-wait').dataset.requestId,second.operation.request_id);
      assert.match(document.querySelector('.setup-error').textContent,/Current count setup observation interrupted/);
      assert.ok([...document.querySelectorAll('.setup-current-question button')].every(node=>node.disabled));
    }else if(selection==='pending manual B'){
      assert.equal(moreCount(document).disabled,true,'saved manual intent keeps its own count block after old cleanup');
      assert.equal(document.querySelector('#setup-send').disabled,true);
      assert.match(document.querySelector('.setup-manual-note').textContent,/Deployment settings/);
      assert.deepEqual(blueprints.at(-1),pendingManual.blueprint);
    }else{
      assert.equal(document.querySelector('.setup-error') === null,true);
      assert.equal(moreCount(document).disabled,false,'the old count request releases only its package latch');
      assert.equal(document.querySelector('#setup-send').disabled,false);
      assert.deepEqual(blueprints.at(-1),current.proposal);
      await requestMoreCount(document);
      await waitConfig(()=>calls.some(call=>call.url===`/api/guide/${current.id}/respond`));
      const expected=twoOwnerPlan(current),submitted=posts().at(-1);
      assert.deepEqual(posts().map(call=>call.url),['/api/blueprint/import','/api/blueprint/import',`/api/guide/${current.id}/respond`]);
      assert.deepEqual(posts()[1].body,expected,'the deliberate next count uses the actual selected base');
      assert.equal(submitted.body.version,current.version);
      assert.equal(submitted.body.action,'manual');
      assert.deepEqual(submitted.body.blueprint,expected);
      assert.deepEqual(JSON.parse(values.get(manualKey(current.id))).blueprint,expected);
      assert.equal(values.get(manualKey(current.id===first.id?second.id:first.id)),undefined);
      assert.equal(document.querySelector('#setup-message').value,input.draft);
    }
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
  },read,{allowedPosts:['/api/blueprint/import',firstRespond,secondRespond],
    onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

for(const outcome of ['success','failure'])test(`current count import ${outcome} preserves draft and explicit retry`,async()=>{
  const guide=acceptanceGuide('owned-count-control','Current count plan'),{storage,values,inputs}=acceptanceStorage([guide]);
  const candidate=twoOwnerPlan(guide),respond=`/api/guide/${guide.id}/respond`,blueprints=[];
  let release,reject,imports=0;
  const deferred=new Promise((resolve,fail)=>{release=resolve;reject=fail;});
  const read=configReads([guide],(url,options)=>{
    if(url==='/api/blueprint/import')return ++imports===1?deferred:{blueprint:JSON.parse(options.body)};
    if(url===respond)return configRunning(guide,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    const posts=()=>calls.filter(call=>call.method==='POST');
    await requestMoreCount(document);
    assert.equal(moreCount(document).disabled,true);
    assert.equal(document.querySelector('#setup-send').disabled,true);
    await act(async()=>{
      if(outcome==='success')release({blueprint:candidate});
      else reject(Error('Current count adjustment refused'));
      await settle();
    });
    if(outcome==='failure'){
      assert.match(document.querySelector('.setup-error').textContent,/Current count adjustment refused/);
      assert.equal(moreCount(document).disabled,false);
      assert.equal(document.querySelector('#setup-send').disabled,false);
      assert.equal(countValue(document),'1');
      assert.equal(values.get(manualKey(guide.id)),undefined);
      assert.deepEqual(blueprints.at(-1),guide.proposal);
      assert.deepEqual(posts(),[{url:'/api/blueprint/import',method:'POST',body:candidate}]);
      assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
      await requestMoreCount(document);
    }
    await waitConfig(()=>calls.some(call=>call.url===respond));
    assert.deepEqual(posts().map(call=>call.url),[...Array(outcome==='success'?1:2).fill('/api/blueprint/import'),respond]);
    assert.ok(posts().filter(call=>call.url==='/api/blueprint/import').every(call=>JSON.stringify(call.body)===JSON.stringify(candidate)));
    const submitted=posts().at(-1).body,retained=JSON.parse(values.get(manualKey(guide.id)));
    assert.equal(submitted.version,guide.version);
    assert.equal(submitted.action,'manual');
    assert.deepEqual(submitted.blueprint,candidate);
    assert.deepEqual(retained.blueprint,candidate);
    assert.equal(retained.text,submitted.text);
    assert.ok(retained.changes.includes('Update Context owner'));
    assert.equal(countValue(document),'2');
    assert.deepEqual(blueprints.at(-1),candidate);
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.equal(document.querySelector('.setup-guide-wait').dataset.requestId,'6'.repeat(64));
    assert.equal(moreCount(document).disabled,true,'the admitted guide operation owns the current busy state');
    assert.equal(document.querySelector('#setup-send').disabled,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
  },read,{allowedPosts:['/api/blueprint/import',respond],onBlueprint:blueprint=>blueprints.push(structuredClone(blueprint))});
});

test('an unchanged checked count releases its own latch without a manual request',async()=>{
  const guide=acceptanceGuide('owned-count-unchanged','Unchanged checked count'),{storage,values,inputs}=acceptanceStorage([guide]);
  const read=configReads([guide],url=>url==='/api/blueprint/import'?{blueprint:structuredClone(guide.proposal)}:undefined);
  await mountedGuide({value:storage},async(document,calls)=>{
    await requestMoreCount(document);
    assert.deepEqual(calls.filter(call=>call.method==='POST'),[{url:'/api/blueprint/import',method:'POST',body:twoOwnerPlan(guide)}]);
    assert.equal(values.get(manualKey(guide.id)),undefined);
    assert.equal(countValue(document),'1');
    assert.equal(moreCount(document).disabled,false);
    assert.equal(document.querySelector('#setup-send').disabled,false);
    assert.equal(document.querySelector('.setup-error') === null,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.deepEqual(JSON.parse(values.get(inputs[0].key)),inputs[0]);
  },read,{allowedPosts:['/api/blueprint/import']});
});

test('count changes in an anonymous setup retain supported startup and exact new-owner feedback',async()=>{
  const anonymous={...acceptanceGuide('anonymous-count','Anonymous count plan'),id:null,version:0};
  const started={...structuredClone(anonymous),id:'owned-started-count',version:1};
  const {storage,values,inputs}=acceptanceStorage([anonymous]),candidate=twoOwnerPlan(anonymous),respond=`/api/guide/${started.id}/respond`;
  const read=configReads([anonymous,started],(url,options)=>{
    if(url==='/api/guide/list')return {setups:[]};
    if(url==='/api/blueprint/import')return {blueprint:JSON.parse(options.body)};
    if(url==='/api/guide/start')return structuredClone(started);
    if(url===respond)return configRunning(started,JSON.parse(options.body));
  });
  await mountedGuide({value:storage},async(document,calls)=>{
    await requestMoreCount(document);
    await waitConfig(()=>calls.some(call=>call.url===respond));
    assert.deepEqual(calls.filter(call=>call.method==='POST').map(call=>call.url),['/api/blueprint/import','/api/guide/start',respond]);
    assert.deepEqual(calls.find(call=>call.url==='/api/blueprint/import').body,candidate);
    const creation=calls.find(call=>call.url==='/api/guide/start').body;
    assert.deepEqual(Object.keys(creation),['start_request_id']);
    assert.match(creation.start_request_id,/^[0-9a-f]{8}-[0-9a-f-]{27}$/);
    const submitted=calls.find(call=>call.url===respond).body;
    assert.equal(submitted.version,started.version);
    assert.equal(submitted.action,'manual');
    assert.deepEqual(submitted.blueprint,candidate);
    assert.equal(values.get(GUIDE_KEY),started.id);
    assert.equal(values.get(manualKey(null)),undefined);
    assert.deepEqual(JSON.parse(values.get(manualKey(started.id))).blueprint,candidate);
    assert.equal(countValue(document),'2');
    assert.equal(moreCount(document).disabled,true);
    assert.equal(document.querySelector('#setup-message').value,inputs[0].draft);
    assert.equal(JSON.parse(values.get(inputKey(started))).draft,inputs[0].draft);
  },read,{allowedPosts:['/api/blueprint/import','/api/guide/start',respond]});
});
