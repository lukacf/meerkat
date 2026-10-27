import test,{after} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createRequire} from 'node:module';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {build} from 'esbuild';
import React,{act} from 'react';
import {createRoot} from 'react-dom/client';
import {createSimulation} from './simulation-state.mjs';
import {toolSummary,setupFacts} from './evolving-plan-state.mjs';

const require=createRequire(import.meta.url);
const {JSDOM}=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
function realSection(file,start,end) {
  const source=readFileSync(new URL(file,import.meta.url),'utf8');
  const first=source.indexOf(start),last=source.indexOf(end,first);
  assert.ok(first>=0 && last>first,`Retain actual ${start} component source`);
  return source.slice(first,last);
}
// Mount actual presentation components without the App bootstrap, generated
// animation, native runtime or any provider. Shared controls remain real.
const rendered=await build({stdin:{sourcefile:'native-bundle-presentation.jsx',
  resolveDir:fileURLToPath(new URL('.',import.meta.url)),loader:'jsx',contents:`
    import React,{useState,useEffect,useMemo,useRef} from 'react';
    import {NativeBundleSelection} from './native-bundle-selection.jsx';
    import {toolSummary} from './evolving-plan-state.mjs';
    import {canResizeClass,classInstanceLimit,resizeClass} from './state.mjs';
    ${realSection('app.jsx','const paths =','function Modal(')}
    ${realSection('app.jsx','function Avatar(','function messageEntry(')}
    ${realSection('app.jsx','function AllowlistInput(','function ArchitectConversation(')}
    ${realSection('app.jsx','function toolCount(','function safeUrl(')}
    ${realSection('evolving-plan.jsx','function Arrow(','export function AcquisitionReview(')}
    ${realSection('proposal-simulation.jsx','function RelationshipGroup(','export function AnimationGenerationStatus(')}
    const IDENTITY_PAGE_SIZE=60;
    export {ClassCard,ClassInspector,Connections,AgentRow};`},
  bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent',jsx:'automatic',
  loader:{'.css':'empty'},nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
  plugins:[{name:'shared-installed-react',setup(builder){
    builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({path:pathToFileURL(require.resolve(args.path)).href,external:true}));
  }}]});
const bundleDirectory=await mkdtemp(join(tmpdir(),'forge-bundle-presentation-'));
after(()=>rm(bundleDirectory,{recursive:true,force:true}));
const bundlePath=join(bundleDirectory,'presentation.mjs');
await writeFile(bundlePath,rendered.outputFiles[0].text);
const components=await import(pathToFileURL(bundlePath).href);
const nativeName='meerkat_toolkit.setup';
const tools=()=>({workspace:false,notes:'none',builtins:false,shell:false,memory:false,
  workgraph:false,schedule:false,knowledge:false,mcp:[],rust_bundles:[nativeName]});
const agent=(id='assistant')=>({id,name:id==='assistant'?'Assistant':'Setup helper',description:'Keep this context.',
  instructions:'Use actual capabilities and preserve evidence.',instances:1,members:[],provider:'openai',model:'fixture',tools:tools()});
const plan=()=>({project:{name:'Bundle selection'},target:'mac',agent_classes:[agent()],connections:[],
  knowledge:{mode:'none'},topology:{mode:'manual',entrypoint:'assistant:main',edges:[]},flows:[]});

async function mounted(verify) {
  const dom=new JSDOM('<div id="root"></div>',{url:'http://offline.test/'}),saved=new Map(),requests=[];
  const replacements={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,
    HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true,
    fetch:(...args)=>{requests.push(args);throw new Error('A selection disclosure must not request tool discovery or admission.');}};
  for(const [key,value] of Object.entries(replacements)){
    saved.set(key,Object.getOwnPropertyDescriptor(globalThis,key));Object.defineProperty(globalThis,key,{value,configurable:true});}
  const root=createRoot(dom.window.document.getElementById('root'));
  const render=async(name,props)=>act(async()=>root.render(React.createElement(components[name],props)));
  const click=async(node)=>{assert.ok(node,'Find actual mounted action');await act(async()=>node.click());};
  try{await verify({render,click,document:dom.window.document,requests});assert.deepEqual(requests,[]);}
  finally{await act(async()=>root.unmount());dom.window.close();for(const [key,value] of saved){
    if(value)Object.defineProperty(globalThis,key,value);else delete globalThis[key];}}
}
function disclosure(document) {
  const body=document.querySelector('.native-bundle-selection');assert.ok(body);
  assert.equal(body.querySelector('code').textContent,nativeName);
  assert.match(body.textContent,/Native bundles \(selected\)/);
  assert.match(body.textContent,/host must register.*tool permissions must allow/);
  assert.match(body.textContent,/does not make it ready to run/);
  return body;
}

test('mounted bundle-only class counts selections without calling them usable tool grants',async()=>{
  await mounted(async({render,document})=>{
    const c=agent(),before=structuredClone(c);
    await render('ClassCard',{c,index:0,onClick(){},onDuplicate(){}});
    assert.match(document.body.textContent,/1 selection/);
    assert.doesNotMatch(document.body.textContent,/0 grants|1 grants/);
    assert.deepEqual(c,before);
    const old={...c,tools:{...c.tools}};delete old.tools.rust_bundles;
    await render('ClassCard',{c:old,index:0,onClick(){},onDuplicate(){}});
    assert.match(document.body.textContent,/0 grants/);
  });
});

test('mounted class tool details retain exact native selections through an unrelated policy edit',async()=>{
  await mounted(async({render,click,document})=>{
    const bp=plan(),c=bp.agent_classes[0],before=structuredClone(bp),changes=[];
    const props={c,bp,onChange:patch=>changes.push(patch),onClose(){},onDelete(){},onDuplicate(){},onResize(){}};
    await render('ClassInspector',props);
    await click([...document.querySelectorAll('button')].find(button=>button.textContent==='tools'));
    disclosure(document);
    await click([...document.querySelectorAll('label.toggle-row')].find(label=>label.textContent.includes('Native read-only policy')).querySelector('input'));
    assert.deepEqual(changes,[{tools:{...c.tools,read_only:true}}]);
    await render('ClassInspector',{...props,c:{...c,tools:changes[0].tools}});
    disclosure(document);assert.deepEqual(bp,before);
    assert.equal(document.querySelector('.native-bundle-selection button') === null,true,'No acquisition/invocation control is invented');
  });
});

test('mounted guided role details disclose bundles without a false empty or read-access summary',async()=>{
  await mounted(async({render,click,document})=>{
    const bp=plan(),c=bp.agent_classes[0],before=structuredClone(c);
    await render('AgentRow',{agent:c,disabled:false,instanceLimit:10,canDecrease:false});
    await click(document.querySelector('.ep-tools-toggle'));
    disclosure(document);
    assert.match(document.querySelector('.ep-tool-tags').textContent,/1 native bundle selected/);
    assert.doesNotMatch(document.body.textContent,/No optional tools granted/);
    assert.equal(setupFacts({brief:{facts:{}},completed_stages:[]},bp).find(row=>row.label==='Current access').value,
      'Native bundles selected (setup required)');
    assert.deepEqual(toolSummary({...c.tools,read_only:true}),['Native read-only policy (declared)','1 native bundle selected']);
    assert.deepEqual(c,before);
  });
});

test('mounted Connections view separates class-local native selections from MCP connections',async()=>{
  await mounted(async({render,document})=>{
    const bp=plan(),helper=agent('setup');delete helper.tools.rust_bundles;bp.agent_classes.push(helper);
    const before=structuredClone(bp);
    await render('Connections',{bp,update(){throw new Error('Read-only selection must not change plan');},setModal(){}});
    disclosure(document);
    assert.equal(document.querySelectorAll('.native-bundle-selection').length,1);
    assert.equal(document.querySelector('.native-bundle-selection').closest('article').querySelector('h3').textContent,'Assistant');
    assert.match(document.body.textContent,/Native bundle selections/);
    assert.equal(document.querySelectorAll('.connection-address').length,0);
    assert.deepEqual(bp,before);
  });
});

test('mounted identity details disclose actual retained bundles without fabricating tool relationships',async()=>{
  await mounted(async({render,document})=>{
    const bp=plan(),model=createSimulation(bp),selected=model.nodes.find(node=>node.id==='assistant:main');
    const before=structuredClone(model);assert.ok(selected);
    assert.equal(model.edges.filter(edge=>edge.kind==='tool').length,0);
    await render('OwnerRelationships',{selected,model});disclosure(document);
    assert.doesNotMatch(document.body.textContent,/No tools or source access configured/);
    assert.deepEqual(model,before);
    const old={...selected,tools:{...selected.tools}};delete old.tools.rust_bundles;
    await render('OwnerRelationships',{selected:old,model});
    assert.match(document.body.textContent,/No tools or source access configured/);
  });
});
