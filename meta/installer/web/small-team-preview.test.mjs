import assert from 'node:assert/strict';
import test from 'node:test';
import {createSimulation} from './simulation-state.mjs';
import {createSimulationScene,simulationViewport,smallTeamPreview,SMALL_TEAM_LIMIT} from './simulation-scene.mjs';
import {composeTopology} from './topology-layout.mjs';
import {applyVisualizationLayout} from './visualization-code.mjs';
import {build} from 'esbuild';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {createRequire} from 'node:module';
import {fileURLToPath,pathToFileURL} from 'node:url';

const plan=count=>({
  agent_classes:Array.from({length:count},(_,i)=>({id:`role${i}`,name:`Distinct context ${i}`,
    description:`Own continuing context ${i}`,instructions:`Keep the evidence and decisions for domain ${i}.`,
    instances:1,members:[],tools:{memory:true,notes:i===0?'write':'read',mcp:[]}})),
  topology:{mode:'manual',entrypoint:'role0:main',edges:count>1?[{a:'role0:main',b:'role1:main'}]:[]},
  connections:[],knowledge:{mode:'none'},flows:[],
});

test('small-team resize preserves every identity, grant and peer without inventing human routing',()=>{
  for(const count of [1,2,3,8]){
    const spec=plan(count),model=createSimulation(spec),before=structuredClone(model);
    const base=createSimulationScene(model,spec);
    const wide=applyVisualizationLayout(base,composeTopology(model),model);
    const narrow=applyVisualizationLayout(base,composeTopology(model,{narrow:true}),model);
    assert.deepEqual(model,before);
    assert.deepEqual(narrow.relationships,wide.relationships);
    assert.deepEqual(narrow.script,wide.script);
    assert.equal(narrow.nodes.filter(n=>n.semanticKind==='agent').length,count);
    assert.ok(narrow.relationships.every(e=>!e.source.startsWith('boundary:') && !e.target.startsWith('boundary:')));
    for(const [scene,width,height] of [[wide,700,350],[narrow,348,348]]){
      const v=simulationViewport(scene,width,height);
      for(const node of scene.nodes.filter(n=>n.kind!=='text')){
        const x=node.x*v.sx+v.ox,y=node.y*v.sy+v.oy;
        assert.ok(x>12 && x<width-12,`${count} ${node.id} x=${x}`);
        assert.ok(y>12 && y<height-12,`${count} ${node.id} y=${y}`);
      }
    }
    assert.notDeepEqual(narrow.nodeIndex['role0:main'],wide.nodeIndex['role0:main']);
  }
});

test('native flow and large-population geometry retain their existing equal-axis projection',()=>{
  const big=plan(49),model=createSimulation(big),scene=createSimulationScene(model,big);
  const v=simulationViewport(scene,700,350);
  assert.equal(v.sx,v.sy);
  const workflow={...scene,population:{count:2},selectedFlowId:'explicit-workflow'};
  assert.equal(simulationViewport(workflow,360,460).sx,simulationViewport(workflow,360,460).sy);
});

const require=createRequire(import.meta.url);
const sharedReact={name:'shared-react',setup(builder){
  builder.onResolve({filter:/^react(?:\/.*)?$/},args=>({path:pathToFileURL(require.resolve(args.path)).href,external:true}));
}};
// The automatic renderer over an inert drawing host, which records the scene it lays out.
const inertNetwork={name:'inert-network',setup(builder){
  builder.onResolve({filter:/network-renderer\.mjs$/},()=>({path:'inert-network',namespace:'inert'}));
  builder.onLoad({filter:/.*/,namespace:'inert'},()=>({loader:'js',contents:`export const COL={ink:[0,0,0],mid:[0,0,0],screen:[255,255,255],accent:[255,0,0]};
export const rgba=()=>'';export const SANS='sans-serif';
export class Net{constructor(canvas){this.canvas=canvas;this.selectedId=null;this.showPeers=false;this.t=0;}
setScene(factory){this.factory=factory;this.scene=factory(this.canvas.width,this.canvas.height);}seek(time){this.t=time;}destroy(){}}`}));
}};
const loadBundle=async(entry,plugins=[])=>{
  const built=await build({entryPoints:[fileURLToPath(new URL(entry,import.meta.url))],bundle:true,write:false,format:'esm',platform:'node',
    logLevel:'silent',loader:{'.css':'empty'},nodePaths:[fileURLToPath(new URL('node_modules',import.meta.url))],
    alias:{'@console-core':fileURLToPath(new URL('../../packages/console-core/src',import.meta.url))},plugins:[sharedReact,...plugins]});
  return import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);
};
const withFlow=(spec,flow)=>flow?{...spec,flows:[{id:'review',name:'Review',steps:[{id:'prepare',agent:'role0:main',prompt:'Prepare.',depends_on:[]}]}]}:spec;

test('the stage, layout, projection and renderer agree on which previews are a small team',async()=>{
  const {ProposalSimulation}=await loadBundle('proposal-simulation.jsx');
  const {MobSim}=await loadBundle('mob-sim-renderer.mjs',[inertNetwork]);
  const previous=Object.getOwnPropertyDescriptor(globalThis,'matchMedia');
  Object.defineProperty(globalThis,'matchMedia',{configurable:true,value:()=>({matches:false,addEventListener(){},removeEventListener(){}})});
  try{
    for(const count of [0,1,2,8,9])for(const flow of count?[false,true]:[false]){
      // One to eight agents outside a selected native flow make a small team.
      const expected=count>0 && count<=8 && !flow,label=`${count} agents${flow?' in a flow':''}`;
      const spec=withFlow(plan(count),flow),model=createSimulation(spec,flow?'review':undefined),topology=composeTopology(model);
      const scene=applyVisualizationLayout(createSimulationScene(model,spec),topology,model);
      const v=simulationViewport(scene,700,350);
      assert.equal(v.ox===0 && v.oy===0 && v.sx!==v.sy,expected,`projection: ${label}`);
      const sim=new MobSim({width:700,height:350});sim.setScene(scene);
      const labelled=sim.scene.nodes.filter(node=>node.label);
      assert.equal(labelled.length>0 && labelled.every(node=>node.labelSize===14),expected,`renderer: ${label}`);
      if(count){
        const input=model.nodes.find(node=>node.kind==='input');
        assert.equal(topology.positions.find(position=>position.id===input.id).x===145,expected,`layout: ${label}`);
      }
      // The stage starts on the team overview; its flow choice is local state.
      if(!flow)assert.equal(/ps-stage--small/.test(renderToStaticMarkup(React.createElement(ProposalSimulation,{plan:spec}))),
        expected,`stage: ${label}`);
    }
  }finally{
    if(previous)Object.defineProperty(globalThis,'matchMedia',previous);else delete globalThis.matchMedia;
  }
});

test('one small-team rule: one to eight agents outside a selected native flow',()=>{
  assert.equal(SMALL_TEAM_LIMIT,8);
  for(const [count,small] of [[0,false],[1,true],[8,true],[9,false],[-1,false],[2.5,false],[NaN,false],[undefined,false],['3',false]]){
    assert.equal(smallTeamPreview(count,null),small,String(count));
    assert.equal(smallTeamPreview(count,'review'),false,`${count} in a flow`);
  }
});

test('the layout keeps its small-team rule inside its own source for the author sandbox',()=>{
  // The sandbox runs composeTopology from its source text alone, so it cannot import the shared rule.
  const isolated=new Function(`return ${composeTopology.toString()}`)();
  for(const count of [0,1,8,9]){
    const model=createSimulation(plan(count));
    assert.deepEqual(isolated(model),composeTopology(model),String(count));
  }
});
