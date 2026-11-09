/* Adapt the detailed reference renderer to the proposal's semantic layers. */
import { Net, COL, rgba, SANS } from './network-renderer.mjs';
import { simulationViewport, smallTeamPreview } from './simulation-scene.mjs';
import { createRelationshipProjection } from './mob-sim-relationships.mjs';

export class MobSim extends Net {
  constructor(canvas, els = {}) {
    super(canvas, {labels:true, max:0, onFrame:(time, duration) => {
      els.onFrame?.(time, duration);
      if (els.progress) els.progress.style.width = `${100 * time / duration}%`;
    }});
  }
  setScene(source) {
    this.source = source;
    const relationships = createRelationshipProjection(source), simulation = this;
    super.setScene((width, height) => {
      const smallTeam = smallTeamPreview(source.population?.count, source.selectedFlowId);
      const {sc,sx,sy,ox,oy} = simulationViewport(source,width,height);
      this.transform = {sc,sx,sy,ox,oy};
      const point=p=>({x:p.x*sx+ox,y:p.y*sy+oy});
      const annotations=source.nodes.filter(n=>n.kind==='text');
      const convert = n => ({...n, ...point(n),
        r:Math.max(n.dense ? 1.35 : smallTeam ? n.kind==='coord' ? 7 : 5 : n.kind==='coord'?4.5:2.8,(n.r||5)*sc),
        kind:n.kind === 'ext' ? n.semanticKind === 'input' ? 'person' : 'source' : n.kind === 'mem' ? 'store' : n.kind === 'worker' ? 'agent' : n.kind,
        label:n.dense || n.kind==='text' ? '' : n.label,
        labelSize:smallTeam ? 14 : undefined, labelWidth:smallTeam ? Math.min(150,width*.4) : undefined, mem:n.mem || false,
      });
      const routed=Object.fromEntries(Object.entries(source.routes || {}).map(([id,points])=>[id,points.map(point)]));
      const phase=net=>source.phaseOffsets[Math.min(source.phaseOffsets.length-1,Math.floor(net.t/4.2))];
      const activeEdge=(g,net)=>phase(net)?.activeEdgeIds.includes(g.edgeId);
      const incident=(g,net)=>net.selectedId && (g.source===net.selectedId || g.target===net.selectedId);
      const annotationBoxes=[];
      const drawAnnotations=ctx=>{
        annotationBoxes.length=0;
        for(const n of annotations){
          const p=point(n),fs=Math.max(9,11*sc);
          ctx.font=`600 ${fs}px ${SANS}`;ctx.textAlign=n.align || 'center';ctx.textBaseline='middle';
          const text=n.label.toUpperCase(),tw=ctx.measureText(text).width;
          const left=p.x-(ctx.textAlign==='center'?tw/2:ctx.textAlign==='right'?tw:0);
          const box={x:left-4,y:p.y-fs/2-4,w:tw+8,h:fs+8};
          if(annotationBoxes.some(b=>box.x<b.x+b.w&&box.x+box.w>b.x&&box.y<b.y+b.h&&box.y+box.h>b.y))continue;
          annotationBoxes.push(box);
          ctx.fillStyle=rgba(COL.screen,1);ctx.fillRect(box.x,box.y,box.w,box.h);
          ctx.fillStyle=rgba(COL.mid,.9);ctx.fillText(text,p.x,p.y);
        }
      };
      return {
        nodes:source.nodes.filter(n=>n.kind!=='text').map(convert), duration:source.duration, script:source.script,
        get edges(){return relationships.select(simulation.selectedId);},
        edgeVisible:(g,net)=>{
          if(g.kind==='tool')return !!(net.showPeers || incident(g,net) || activeEdge(g,net));
          if(g.kind==='peer')return !source.selectedFlowId || !!(incident(g,net));
          return true;
        },
        edgeAlpha:(g,net)=>incident(g,net)?.5:activeEdge(g,net)?.35:g.kind==='tool'?.06:g.kind==='peer'?.19:.16,
        routePoints:(id,a,b)=>{const path=routed[id];return path ? [a,...path.slice(1,-1),b]:[a,b];},
        labelObstacles:()=>annotationBoxes,
        decor:ctx=>{
          ctx.strokeStyle=rgba(COL.ink,.07);ctx.lineWidth=1;
          for(const n of annotations.filter(n=>n.semanticKind==='lane-label')){
            const p=point(n);ctx.beginPath();ctx.moveTo(ox+30*sc,p.y+19*sc);ctx.lineTo(ox+970*sc,p.y+19*sc);ctx.stroke();
          }
          drawAnnotations(ctx);
        },
        overlay:(ctx)=>{
          for(const id of this.runtimeActivity?.activeNodeIds || []){
            const n=this.node(id);if(!n)continue;
            ctx.strokeStyle=rgba(COL.accent,.9);ctx.lineWidth=2;ctx.beginPath();ctx.arc(n.x,n.y,n.r+9,0,Math.PI*2);ctx.stroke();
          }
          if(this.selectedId){ const n=this.node(this.selectedId); if(n){
            ctx.strokeStyle=rgba(COL.accent,.6);ctx.lineWidth=1;ctx.beginPath();ctx.arc(n.x,n.y,n.r+6,0,Math.PI*2);ctx.stroke();
          }}
        },
      };
    });
    this.seek(.05);
  }
}
