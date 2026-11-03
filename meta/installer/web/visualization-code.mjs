import { composeTopology } from './topology-layout.mjs';

/** Validate generated geometry before it can affect the trusted network renderer. */
export function applyVisualizationLayout(base, layout, model) {
  if (!layout || !Array.isArray(layout.positions)) throw new Error('The animation did not return a layout.');
  if (layout.groups !== undefined && !Array.isArray(layout.groups)) throw new Error('The animation did not return valid groups.');
  if (layout.events !== undefined && !Array.isArray(layout.events)) throw new Error('The animation did not return valid events.');
  const expected = new Set(model.nodes.map(n => n.id));
  const positions = new Map();
  for(const p of layout.positions) {
    if(!p || !expected.has(p.id) || positions.has(p.id) || !Number.isFinite(p.x) || !Number.isFinite(p.y) || p.x<15 || p.x>985 || p.y<24 || p.y>525) throw new Error('The animation changed or misplaced a configured identity.');
    positions.set(p.id,p);
  }
  if(positions.size !== expected.size) throw new Error('The animation left out configured identities.');
  const seenGroups = new Set();
  const agentIds = new Set(model.nodes.filter(n=>n.kind==='agent').map(n=>n.id));
  const sourceNames = new Map(model.nodes.map(n=>[n.id,n.name]));
  const groups=(layout.groups || []).map((g,index)=>{
    if(!g || !Array.isArray(g.memberIds) || !g.memberIds.length || g.memberIds.some(id=>!agentIds.has(id) || seenGroups.has(id)) || new Set(g.memberIds).size!==g.memberIds.length) throw new Error('The animation invented or duplicated group members.');
    g.memberIds.forEach(id=>seenGroups.add(id));
    const xs=g.memberIds.map(id=>positions.get(id).x),ys=g.memberIds.map(id=>positions.get(id).y);
    const x=Math.min(...xs)-14,y=Math.min(...ys)-14,width=Math.max(...xs)-x+14,height=Math.max(...ys)-y+14;
    const membersSet=new Set(g.memberIds);
    const exactClass=base.groups?.find(item=>item.count===membersSet.size && item.memberIds.every(id=>membersSet.has(id)));
    return {id:`generated-group:${index}`,name:exactClass?.name || String(g.label || 'Agent group').slice(0,100),memberIds:[...g.memberIds],ids:[...g.memberIds],
      count:g.memberIds.length,renderedCount:g.memberIds.length,representedCount:g.memberIds.length,dense:g.memberIds.length>=12,
      shape:['ring','cluster','lane'].includes(g.shape)?g.shape:'cluster',x,y,width,height,bounds:{x,y,width,height},
      members:g.memberIds.map(id=>({id,name:sourceNames.get(id)}))};
  });
  const scene = structuredClone(base);
  const denseIds=new Set(groups.filter(g=>g.dense).flatMap(g=>g.memberIds));
  const nodes=scene.nodes.filter(n=>n.kind!=='text').map(n=>{
    const p=positions.get(n.id);
    const dense=n.semanticKind==='agent' && denseIds.has(n.id);
    return {...n,x:p.x,y:p.y,labelPos:['above','below','left','right'].includes(p.labelPos)?p.labelPos:n.labelPos,
      label: typeof p.displayLabel==='string' && p.displayLabel.trim() ? p.displayLabel.trim().slice(0,42) : n.label,
      dense,
      kind:n.semanticKind==='agent' && n.kind!=='coord' ? 'agent' : n.kind,
      r:dense && n.kind!=='coord' ? 2.6:n.r};
  });
  groups.filter(g=>g.dense).forEach(g=>nodes.push({id:`label:${g.id}`,kind:'text',x:g.x+g.width/2,y:Math.max(15,g.y-13),label:`${g.name} · ${g.count}`,groupId:g.id}));
  if(layout.annotations !== undefined && (!Array.isArray(layout.annotations) || layout.annotations.length>40)) throw new Error('The scene has invalid annotations.');
  for(const [i,a] of (layout.annotations || []).entries()) {
    if(!a || typeof a.text!=='string' || !a.text.trim() || a.text.length>65 || !Number.isFinite(a.x) || !Number.isFinite(a.y) || a.x<15 || a.x>985 || a.y<15 || a.y>540) throw new Error('The scene has an invalid section label.');
    nodes.push({id:`annotation:${i}`,kind:'text',semanticKind:'lane-label',label:a.text,x:a.x,y:a.y,align:['left','center','right'].includes(a.align)?a.align:'left'});
  }
  const routes={};
  if(layout.paths !== undefined && (!Array.isArray(layout.paths) || layout.paths.length>131072)) throw new Error('The scene has invalid connection routes.');
  const routeEdges=new Map(model.edges.map(e=>[e.id,e]));
  for(const path of layout.paths || []) {
    const edge=routeEdges.get(path?.edgeId);
    if(!edge || routes[path.edgeId] || !Array.isArray(path.points) || path.points.length<2 || path.points.length>8 || path.points.some(p=>!p || !Number.isFinite(p.x) || !Number.isFinite(p.y) || p.x<0 || p.x>1000 || p.y<0 || p.y>560)) throw new Error('The scene changed a connection route.');
    const first=path.points[0],last=path.points.at(-1),a=positions.get(edge.source),b=positions.get(edge.target);
    if(Math.hypot(first.x-a.x,first.y-a.y)>.1 || Math.hypot(last.x-b.x,last.y-b.y)>.1) throw new Error('The scene routed a connection to the wrong identity.');
    routes[path.edgeId]=path.points.map(p=>({x:p.x,y:p.y}));
  }
  let script=scene.script;
  if(layout.events?.length){
    if(layout.events.length>4096) throw new Error('Too many animation events.');
    const edgeMap=new Map(model.edges.map(e=>[e.id,e]));
    const additions=layout.events.map(ev=>{
      if(!ev) throw new Error('The animation invented a workflow transition.');
      const pi=model.phases.findIndex(p=>p.id===ev.phaseId),p=model.phases[pi],e=edgeMap.get(ev.edgeId);
      if(!p || !e || !p.activeEdgeIds.includes(ev.edgeId) || !Number.isFinite(ev.offset) || ev.offset<.1 || ev.offset>2.6) throw new Error('The animation invented a workflow transition.');
      return {t:pi*4.2+ev.offset,msg:[e.source,e.target],dur:1,c:e.kind==='tool'?'ink':e.kind==='peer'?'mid':'accent',
        phaseId:p.id,phaseIndex:pi,edgeId:e.id,representedEdgeIds:[e.id]};
    });
    script=[...scene.script.filter(e=>!e.msg),...additions].sort((a,b)=>a.t-b.t);
  }
  return {...scene,nodes,groups,script,routes,relationships:structuredClone(model.edges),selectedFlowId:model.selectedFlowId,nodeIndex:Object.fromEntries(nodes.map(n=>[n.id,{...n}]))};
}

// Generated JavaScript only runs in a disposable worker inside an opaque-origin frame.
// The frame cannot access the application, storage, network, or navigation.
export function runVisualizationCode(code, model) {
  return new Promise((resolve,reject)=>{
    const frame=document.createElement('iframe');
    const token=crypto.randomUUID();
    frame.sandbox='allow-scripts';frame.hidden=true;
    const cleanup=()=>{clearTimeout(timer);window.removeEventListener('message',receive);frame.remove();};
    const receive=event=>{
      if(event.source!==frame.contentWindow || event.data?.token!==token)return;
      if(event.data.ready){frame.contentWindow.postMessage({token,code,model},'*');return;}
      cleanup();event.data.error?reject(new Error('The animation code could not run.')):resolve(event.data.result);
    };
    window.addEventListener('message',receive);
    const timer=setTimeout(()=>{cleanup();reject(new Error('The animation took too long.'));},4000);
    const workerSource=`const composeTopology=${composeTopology.toString()};self.onmessage = e => { try { const api = Object.freeze({width:1000,height:560,TAU:Math.PI*2,compose:options=>composeTopology(e.data.model,options)}); const result = new Function('model','api',e.data.code)(e.data.model,api); self.postMessage({result:JSON.parse(JSON.stringify(result))}); } catch(error){self.postMessage({error:true});} };`;
    frame.srcdoc=`<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; worker-src blob:; connect-src 'none'"><script>const token=${JSON.stringify(token)};addEventListener('message',e=>{if(e.source!==parent||e.data.token!==token)return;const worker=new Worker(URL.createObjectURL(new Blob([${JSON.stringify(workerSource)}],{type:'text/javascript'})));const watchdog=setTimeout(()=>{worker.terminate();parent.postMessage({token,error:true},'*');},2500);worker.onmessage=event=>{clearTimeout(watchdog);parent.postMessage({token,...event.data},'*');worker.terminate();};worker.onerror=()=>{clearTimeout(watchdog);parent.postMessage({token,error:true},'*');worker.terminate();};worker.postMessage(e.data);});parent.postMessage({token,ready:true},'*');</script>`;
    document.body.appendChild(frame);
  });
}
