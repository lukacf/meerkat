import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { FreeformAnimation, animationFrameDocument, animationFailureDetail, ANIMATION_CAPTURE_LIMITS } from './freeform-animation.mjs';

function host(t, callbacks={}, {observeResize=true,observeIntersection=false,clock=performance.now()}={}) {
  const listeners=new Map(),documentListeners=new Map(),timers=new Map(),frames=new Map(),messages=[],resizeObservers=[],intersectionObservers=[];
  let next=0,removed=0,now=clock;
  const rect={width:600,height:340};
  const frame={style:{},contentWindow:{postMessage:message=>messages.push(message)},getBoundingClientRect:()=>rect,remove(){removed++;}};
  class ResizeObserver {
    constructor(callback){this.callback=callback;resizeObservers.push(this);}
    observe(target){this.target=target;}
    disconnect(){this.disconnected=true;}
  }
  class IntersectionObserver {
    constructor(callback){this.callback=callback;intersectionObservers.push(this);}
    observe(target){this.target=target;}
    disconnect(){this.disconnected=true;}
  }
  const replacements={
    document:{hidden:false,createElement:()=>frame,
      addEventListener:(name,fn)=>documentListeners.set(name,fn),
      removeEventListener:(name,fn)=>{if(documentListeners.get(name)===fn)documentListeners.delete(name);}},
    window:{addEventListener:(name,fn)=>listeners.set(name,fn),removeEventListener:(name,fn)=>{if(listeners.get(name)===fn)listeners.delete(name);}},
    ResizeObserver:observeResize?ResizeObserver:undefined,IntersectionObserver:observeIntersection?IntersectionObserver:undefined,devicePixelRatio:1,performance:{now:()=>now},
    requestAnimationFrame:fn=>{const id=++next;frames.set(id,fn);return id;},
    cancelAnimationFrame:id=>frames.delete(id),
    setTimeout:(fn,delay)=>{const id=++next;timers.set(id,{fn,delay});return id;},
    clearTimeout:id=>timers.delete(id),
  };
  for(const [name,value] of Object.entries(replacements)){
    const descriptor=Object.getOwnPropertyDescriptor(globalThis,name);
    Object.defineProperty(globalThis,name,{value,configurable:true,writable:true});
    t.after(()=>descriptor?Object.defineProperty(globalThis,name,descriptor):delete globalThis[name]);
  }
  const instance=new FreeformAnimation({replaceChildren(value){assert.equal(value,frame);}},
    {code:'return {};',model:{nodes:[{id:'lead'}]},blueprint:{},...callbacks});
  const emit=(data,source=frame.contentWindow)=>listeners.get('message')?.({source,data:{token:instance.token,...data}});
  const tick=(milliseconds=16)=>{const [id,callback]=frames.entries().next().value;frames.delete(id);now+=milliseconds;callback(now);};
  // A resumed watchdog's delay is the remaining visible time, a clock difference with float error.
  const timeout=delay=>{for(const [id,timer] of timers){if(Math.abs(timer.delay-delay)<1e-9){timers.delete(id);now+=delay;timer.fn();return;}}throw Error(`No ${delay}ms timer`);};
  const visibility=hidden=>{replacements.document.hidden=hidden;documentListeners.get('visibilitychange')?.();};
  return {instance,emit,tick,timeout,visibility,advance:milliseconds=>{now+=milliseconds;},now:()=>now,messages,timers,listeners,documentListeners,
    rect,resizeObservers,intersectionObservers,surface:visible=>intersectionObservers[0].callback([{target:frame,isIntersecting:visible}]),document:replacements.document,removed:()=>removed};
}

function readyPaused(h) {
  h.instance.paused=true;
  h.emit({boot:true});h.emit({ready:true,duration:40,chapters:[]});
  h.emit({drawn:true,frameId:h.messages.at(-1).draw.frameId});
}

const draws=h=>h.messages.filter(message=>message.draw).map(message=>message.draw);
const expireDraw=(h,delay=2000)=>{h.timeout(delay);h.timeout(100);};

test('paused and reduced-motion scenes keep their first frame without repeated static draws',t=>{
  let first=0,frames=0;
  const h=host(t,{onFirstFrame:()=>first++,onFrame:()=>frames++});
  h.instance.reducedMotion=true;readyPaused(h);
  for(let i=0;i<120;i++)h.tick();
  assert.equal(draws(h).length,1);
  assert.equal(frames,1);
  assert.equal(first,1);
  assert.equal(h.instance.time,0);
  assert.equal([...h.timers.values()].some(timer=>timer.delay===2000),false);
});

test('static draw inputs coalesce while busy and redraw the latest controls after acknowledgement',t=>{
  const h=host(t);readyPaused(h);
  h.instance.selectedId='lead';h.tick();
  const selected=draws(h).at(-1);
  assert.equal(selected.selectedId,'lead');
  h.instance.showPeers=true;h.instance.reducedMotion=true;h.instance.seek(12);
  h.instance.runtimeActivity={activeNodeIds:['lead'],scope:'native-design-rehearsal'};
  h.tick();h.tick();
  assert.equal(draws(h).length,2);
  h.emit({drawn:true,frameId:selected.frameId+1});h.tick();
  assert.equal(draws(h).length,2);
  h.emit({drawn:true,frameId:selected.frameId});h.tick();
  const latest=draws(h).at(-1);
  assert.equal(latest.time,12);
  assert.equal(latest.showTools,true);
  assert.equal(latest.reducedMotion,true);
  assert.deepEqual(latest.runtimeActivity,{activeNodeIds:['lead'],scope:'native-design-rehearsal'});
  h.emit({drawn:true,frameId:latest.frameId});h.tick();
  assert.equal(draws(h).length,3);
});

test('seek, activity reassignment, resize and pixel ratio invalidate a settled static frame',t=>{
  const h=host(t);readyPaused(h);
  const update=callback=>{const count=draws(h).length;callback();h.tick();assert.equal(draws(h).length,count+1);h.emit({drawn:true,frameId:draws(h).at(-1).frameId});h.tick();assert.equal(draws(h).length,count+1);};
  update(()=>h.instance.seek(0));
  const activity={activeNodeIds:['lead']};update(()=>h.instance.runtimeActivity=activity);
  update(()=>{activity.activeNodeIds=[];h.instance.runtimeActivity=activity;});
  assert.deepEqual(draws(h).at(-1).runtimeActivity.activeNodeIds,[]);
  assert.equal(h.resizeObservers[0].target,h.instance.frame);
  update(()=>h.resizeObservers[0].callback());
  update(()=>h.listeners.get('resize')());
  update(()=>globalThis.devicePixelRatio=2);
  update(()=>h.instance.selectedId='lead');
  update(()=>h.emit({inspect:'lead'}));
  const count=draws(h).length;h.emit({inspect:'unknown'});h.emit({inspect:'lead'},{});h.tick();
  assert.equal(draws(h).length,count);
  update(()=>h.instance.showPeers=true);
  update(()=>h.instance.reducedMotion=true);
});

test('playing scenes keep drawing and hidden scenes freeze until actual state changes',t=>{
  const h=host(t);readyPaused(h);
  h.instance.reducedMotion=true;h.instance.paused=false;h.tick();
  const firstPlaying=draws(h).at(-1);
  assert.equal(firstPlaying.playing,true);
  assert.equal(firstPlaying.reducedMotion,true);
  assert.ok(firstPlaying.time>0);
  h.emit({drawn:true,frameId:firstPlaying.frameId});h.tick();
  const secondPlaying=draws(h).at(-1);assert.ok(secondPlaying.time>firstPlaying.time);
  h.emit({drawn:true,frameId:secondPlaying.frameId});
  h.visibility(true);
  for(let i=0;i<10;i++)h.tick();
  assert.equal(draws(h).length,3);assert.equal(h.instance.time,secondPlaying.time);
  h.instance.showPeers=true;h.tick();
  assert.equal(draws(h).length,3,'Ordinary control redraw waits while the document is hidden');
  h.visibility(false);
  assert.equal(draws(h).length,4);assert.equal(draws(h).at(-1).time,secondPlaying.time);
  assert.equal(draws(h).at(-1).showTools,true);
  h.emit({drawn:true,frameId:draws(h).at(-1).frameId});h.tick();
  assert.equal(draws(h).length,5);assert.ok(draws(h).at(-1).time>secondPlaying.time);
});

test('hidden startup retains its full visible timeout and a safe startup failure phase',t=>{
  const errors=[],h=host(t,{onError:failure=>errors.push(failure)});
  h.visibility(true);h.emit({boot:true});h.advance(20000);
  assert.equal(h.timers.size,0);assert.equal(h.instance.alive,true);assert.deepEqual(errors,[]);
  h.visibility(false);assert.equal([...h.timers.values()][0].delay,4000);
  h.timeout(4000);assert.deepEqual(errors,[{kind:'timeout',phase:'startup'}]);
  assert.equal(h.instance.alive,false);assert.equal(h.documentListeners.has('visibilitychange'),false);
});

test('an in-flight draw resumes only its remaining visible time after a hidden interval',t=>{
  const errors=[],h=host(t,{onError:failure=>errors.push(failure)});readyPaused(h);
  h.instance.seek(4);h.tick();h.advance(600);h.visibility(true);
  h.advance(20000);h.tick();assert.equal(h.timers.size,0);assert.deepEqual(errors,[]);
  h.visibility(false);assert.ok(Math.abs([...h.timers.values()][0].delay-1400)<1e-9);
  expireDraw(h,1400);assert.equal(errors.length,1);const {diagnostics,...detail}=errors[0];assert.deepEqual(detail,{kind:'timeout',phase:'draw'});
  assert.equal(errors[0].diagnostics.frameId,draws(h).at(-1).frameId);assert.equal(errors[0].diagnostics.parentVisibility,'visible');
  assert.equal(errors[0].diagnostics.deadlineOvershootMs,0);
  assert.equal(h.instance.alive,false);
});

test('matching hidden readiness and draw acknowledgements clear suspended deadlines',t=>{
  let first=0;
  const h=host(t,{onFirstFrame:()=>first++});h.instance.paused=true;
  h.emit({boot:true});h.advance(600);h.visibility(true);
  h.emit({ready:true,duration:40,chapters:[]});h.advance(10000);
  assert.equal(draws(h).length,0);assert.equal(h.timers.size,0);
  h.visibility(false);const draw=draws(h).at(-1);h.advance(600);h.visibility(true);
  h.emit({rendering:true,frameId:draw.frameId});assert.equal(h.timers.size,0);
  h.emit({drawn:true,frameId:draw.frameId+1});assert.equal(first,0);
  h.emit({drawn:true,frameId:draw.frameId});assert.equal(first,1);
  h.advance(10000);h.visibility(false);h.tick();
  assert.equal(draws(h).length,1);assert.equal(h.timers.size,0);h.instance.destroy();
});

test('queued timeout callbacks cannot fail a hidden, acknowledged or newer draw',t=>{
  const errors=[],h=host(t,{onError:failure=>errors.push(failure)});readyPaused(h);
  h.instance.seek(4);h.tick();const [oldId,old]=[...h.timers.entries()][0];
  h.tick(600);
  // The timer can be queued before the browser delivers visibilitychange.
  h.document.hidden=true;h.advance(5000);h.timers.delete(oldId);old.fn();assert.deepEqual(errors,[]);
  h.visibility(false);assert.ok(Math.abs([...h.timers.values()].at(-1).delay-1400)<1e-9);
  h.emit({drawn:true,frameId:draws(h).at(-1).frameId});
  h.instance.seek(6);h.tick();old.fn();assert.deepEqual(errors,[]);
  expireDraw(h);assert.equal(errors.length,1);assert.equal(errors[0].kind,'timeout');assert.equal(errors[0].phase,'draw');
  assert.equal(errors[0].diagnostics.time,6);
});

test('real hidden errors stay fatal and disclose only bounded kind and phase',t=>{
  const errors=[],h=host(t,{onError:failure=>errors.push(failure)});readyPaused(h);
  h.instance.seek(4);h.tick();h.visibility(true);
  h.emit({error:true,failure:{kind:'program',phase:'render',message:'Authored source must stay private'}});
  assert.equal(errors.length,1);assert.equal(errors[0].kind,'program');assert.equal(errors[0].phase,'render');
  assert.equal(errors[0].diagnostics.parentVisibility,'hidden');assert.equal(errors[0].diagnostics.time,4);
  assert.equal(JSON.stringify(errors).includes('Authored source'),false);assert.equal(h.instance.alive,false);
  assert.equal(h.documentListeners.has('visibilitychange'),false);
  h.visibility(false);assert.equal(draws(h).length,2);
  assert.equal([...h.timers.values()].some(timer=>timer.delay===2000),false);
});

test('destruction while hidden cannot revive a suspended draw watchdog',t=>{
  const errors=[],h=host(t,{onError:failure=>errors.push(failure)});readyPaused(h);
  h.instance.seek(4);h.tick();const callback=[...h.timers.values()][0].fn;
  h.advance(600);h.visibility(true);h.instance.destroy();h.visibility(false);callback();
  assert.equal(h.documentListeners.has('visibilitychange'),false);
  assert.equal([...h.timers.values()].some(timer=>timer.delay===2000),false);
  assert.deepEqual(errors,[]);assert.equal(draws(h).length,2);
});

test('an explicit hidden capture keeps its independent wall deadline without stopping the scene',async t=>{
  const errors=[],h=host(t,{onError:failure=>errors.push(failure)});readyPaused(h);h.visibility(true);
  const capture=h.instance.capture({time:5}),rejected=assert.rejects(capture,{code:'timeout'});
  assert.equal(draws(h).length,2);assert.equal(h.timers.size,1);
  h.timeout(ANIMATION_CAPTURE_LIMITS.timeout);await rejected;
  assert.equal(h.instance.alive,true);assert.deepEqual(errors,[]);
  h.emit({drawn:true,frameId:draws(h).at(-1).frameId});h.visibility(false);h.instance.destroy();
});

test('static redraw retains watchdog and disconnects invalidation after destruction',t=>{
  let errors=0;
  const h=host(t,{onError:()=>errors++});readyPaused(h);
  h.instance.seek(4);h.tick();
  expireDraw(h);
  assert.equal(errors,1);assert.equal(h.instance.alive,false);
  assert.equal(h.resizeObservers[0].disconnected,true);
  assert.equal(h.listeners.has('resize'),false);
  const count=draws(h).length;h.resizeObservers[0].callback();
  h.emit({drawn:true,frameId:draws(h).at(-1).frameId});
  assert.equal(draws(h).length,count);
});

test('static resize fallback works when ResizeObserver is unavailable',t=>{
  const h=host(t,{}, {observeResize:false});readyPaused(h);
  h.tick();const count=draws(h).length;
  h.rect.width=390;h.rect.height=300;h.tick();
  assert.equal(draws(h).length,count+1);
  h.instance.destroy();
});

test('a hidden candidate waits for real geometry before first paint or its draw watchdog',t=>{
  let first=0,errors=0;
  const h=host(t,{onFirstFrame:()=>first++,onError:()=>errors++});
  Object.assign(h.rect,{width:0,height:0});h.instance.paused=true;
  h.emit({boot:true});h.emit({ready:true,duration:40,chapters:[]});
  for(let i=0;i<180;i++)h.tick();
  assert.equal(draws(h).length,0);
  assert.equal([...h.timers.values()].some(timer=>timer.delay===2000),false);
  assert.equal(first,0);assert.equal(errors,0);assert.equal(h.instance.alive,true);
  Object.assign(h.rect,{width:520,height:400});h.resizeObservers[0].callback();h.tick();
  assert.equal(draws(h).length,1);assert.equal(first,0);
  h.emit({drawn:true,frameId:draws(h).at(-1).frameId});
  assert.equal(first,1);h.instance.destroy();
});

test('parent validates caption values and exact identity inspection',t=>{
  const captions=[],inspected=[];
  const h=host(t,{onCaption:value=>captions.push(value),onInspect:id=>inspected.push(id)});
  h.emit({caption:{title:'Bad',description:{unexpected:'object'}}});
  h.emit({caption:{title:['Bad'],description:'Text'}});
  h.emit({caption:{title:'x'.repeat(300),description:'y'.repeat(3000)}});
  h.emit({caption:{title:'Valid'},inspect:'lead'});
  h.emit({inspect:'invented'});
  h.emit({inspect:'lead'},{});
  assert.equal(captions.length,2);
  assert.equal(captions[0].title.length,200);
  assert.equal(captions[0].description.length,2000);
  assert.deepEqual(captions[1],{title:'Valid',description:''});
  assert.deepEqual(inspected,['lead']);
});

test('custom animation receives observed rehearsal identity activity separately from illustrative progress',t=>{
  const h=host(t);
  const activity={rehearsalId:'trial-1',scope:'native-design-rehearsal',status:'running',activeNodeIds:['lead:main'],
    last:{identity:'lead:main',event:'tool_call_requested',cursor:'console:8'}};
  h.instance.runtimeActivity=activity;
  h.emit({boot:true});h.emit({ready:true,duration:40,chapters:[]});
  const draw=h.messages.at(-1).draw;
  assert.deepEqual(draw.runtimeActivity,activity);
  assert.equal(typeof draw.progress,'number');
  h.instance.runtimeActivity=null;
  h.emit({drawn:true,frameId:draw.frameId});h.tick();
  assert.equal(h.messages.at(-1).draw.runtimeActivity,null);
});

test('invalid ready metadata cannot reach React callbacks',t=>{
  let ready=0,errors=0;
  const h=host(t,{onReady:()=>ready++,onError:()=>errors++});
  h.emit({boot:true});
  h.emit({ready:true,duration:Infinity,chapters:[]});
  assert.equal(ready,0);
  assert.equal(errors,1);
  assert.equal(h.instance.alive,false);
  h.emit({caption:{title:'late'},ready:true,duration:1,chapters:[]});
  h.emit({stopped:true});
  assert.equal(ready,0);
  assert.equal(h.removed(),1);
});

test('invalid chapter fields fail before readiness',t=>{
  let ready=0,errors=0;
  const h=host(t,{onReady:()=>ready++,onError:()=>errors++});
  h.emit({boot:true});
  h.emit({ready:true,duration:40,chapters:[{at:0,title:'Chapter',description:{bad:true}}]});
  assert.equal(ready,0);
  assert.equal(errors,1);
});

test('first frame callback requires a matching draw acknowledgement and fires once',t=>{
  const ready=[],first=[];
  const h=host(t,{onReady:value=>ready.push(value),onFirstFrame:value=>first.push(value)});
  h.emit({boot:true});h.emit({boot:true});
  assert.equal(h.messages.filter(message=>message.start).length,1);
  h.emit({drawn:true,frameId:1});
  assert.equal(first.length,0);
  h.emit({ready:true,duration:40,chapters:[{at:20,title:'Later'},{at:0,title:'Start'}]});
  assert.deepEqual(ready[0].chapters.map(chapter=>chapter.at),[0,20]);
  // The first render starts immediately, even when background tabs suspend RAF.
  const draw=h.messages.at(-1).draw;
  assert.equal(draw.frameId,1);
  h.emit({drawn:true,frameId:draw.frameId+1});
  assert.equal(first.length,0);
  h.emit({drawn:true,frameId:draw.frameId});
  assert.deepEqual(first,[ready[0]]);
  h.tick();h.emit({drawn:true,frameId:h.messages.at(-1).draw.frameId});
  assert.equal(first.length,1);
  h.emit({ready:true,duration:99,chapters:[]});
  assert.equal(ready.length,1);
});

test('teardown waits briefly for acknowledgement and hard-removes an unresponsive frame',t=>{
  const h=host(t);
  h.instance.destroy();h.instance.destroy();
  assert.equal(h.messages.filter(message=>message.stop).length,1);
  assert.equal(h.removed(),0);
  h.timeout(250);
  assert.equal(h.removed(),1);
  assert.equal(h.listeners.has('message'),false);
});

const pointerProbeCode=`
let marks=[];
return {
  duration:40,
  render({ctx,width,height}){
    ctx.clearRect(0,0,width,height);
    marks=model.nodes.map((node,index)=>({id:node.id,x:width*(index+1)/(model.nodes.length+1),y:height/2}));
    for(const mark of marks){ctx.beginPath();ctx.arc(mark.x,mark.y,12,0,Math.PI*2);ctx.fill();}
  },
  hitTest(x,y){if(x<0)return 'unknown';return marks.find(mark=>Math.hypot(x-mark.x,y-mark.y)<=12)?.id;},
  destroy(){api.caption('Probe stopped','Unit fixture cleanup');}
};`;

function pngBytes(width,height,marks=0) {
  const bytes=new ArrayBuffer(40),prefix=new Uint8Array(bytes),view=new DataView(bytes);
  prefix.set([137,80,78,71,13,10,26,10]);view.setUint32(8,13);
  prefix.set([73,72,68,82],12);view.setUint32(16,width);view.setUint32(20,height);view.setUint32(33,marks);
  return bytes;
}

// Execute the production candidate host, srcdoc, and emitted worker wrapper.
// Fake Canvas/ImageBitmap/PNG bytes verify their coupling and protocol only.
// These fixtures do not establish browser pixels, layout, or visual quality.
async function bitmapPath(t,{code=pointerProbeCode,initialFailure=false,renderCostMs=0,copyCostMs=0,clock}={}) {
  const inspected=[],captions=[],errors=[],queue=[],workers=[],canvasHandlers={},iframeHandlers={},iframeTimers=new Map(),revoked=[],encodes=[],copies=[],workerClock=[];
  const h=host(t,{code,model:{nodes:[{id:'lead'},{id:'worker:200'}]},
    onInspect:id=>inspected.push(id),onCaption:value=>captions.push(value),onError:failure=>errors.push(failure)},{clock});
  h.instance.paused=true;globalThis.devicePixelRatio=2;
  const rect={left:17,top:23,width:h.rect.width,height:h.rect.height};
  let holdCopies=false,failCopies=false,holdPixels=false,holdPaintAcks=false,holdRenderings=false,nextTimer=0,workerSource;
  const heldPixels=[],heldPaintAcks=[],heldRenderings=[];
  class ImageBitmap {
    constructor(canvas){this.width=canvas.width;this.height=canvas.height;this.marks=structuredClone(canvas.marks);this.background=canvas.background;this.closed=0;}
    close(){this.closed++;}
  }
  class Canvas {
    constructor(width=1,height=1){this._width=width;this._height=height;this.marks=[];this.background=null;this.transforms=[];
      this.ctx={setTransform:(...values)=>this.transforms.push(values),
        clearRect:()=>{this.marks=[];this.background=null;h.advance(renderCostMs);},beginPath(){},arc:(x,y,r)=>this.marks.push({x,y,r}),fill(){},
        fillRect:()=>{this.background=this.ctx.fillStyle;},
        drawImage:source=>{this.marks=structuredClone(source.marks);if(source.background)this.background=source.background;}};
    }
    get width(){return this._width;}set width(value){this._width=value;this.marks=[];this.background=null;}
    get height(){return this._height;}set height(value){this._height=value;this.marks=[];this.background=null;}
    getContext(type){assert.equal(type,'2d');return this.ctx;}
    getBoundingClientRect(){return rect;}
    addEventListener(name,callback){canvasHandlers[name]=callback;}
    toBlob(callback,type){assert.equal(type,'image/png');encodes.push({callback,width:this.width,height:this.height,marks:structuredClone(this.marks),background:this.background});}
  }
  const canvas=new Canvas();
  const parent={postMessage:value=>{
    const data=structuredClone(value);
    const deliver=()=>h.listeners.get('message')?.({source:h.instance.frame.contentWindow,data});
    if(data.drawn && holdPaintAcks)heldPaintAcks.push(deliver);
    else if(data.rendering && holdRenderings)heldRenderings.push(deliver);
    else queue.push(deliver);
  }};
  h.instance.frame.contentWindow.postMessage=value=>{
    h.messages.push(value);
    const data=structuredClone(value);
    queue.push(()=>iframeHandlers.message({source:parent,data}));
  };
  class Blob {constructor(parts){workerSource=parts.join('');}}
  class Worker {
    constructor(){
      this.messages=[];this.terminations=0;this.closed=false;workers.push(this);
      const context={OffscreenCanvas:Canvas,performance:{now:()=>{const value=h.now();workerClock.push(value);return value;}},
        createImageBitmap:source=>{if(failCopies)return Promise.reject(new Error('Do not disclose pixel-copy error text'));
          h.advance(copyCostMs);
          const bitmap=new ImageBitmap(source);copies.push(bitmap);return holdCopies
          ? new Promise(resolve=>queue.push(()=>{if(!holdCopies)resolve(bitmap);else this.releaseCopy=()=>resolve(bitmap);}))
          : Promise.resolve(bitmap);},
        postMessage:value=>{const {pixels,...rest}=value,data=structuredClone(rest);if(pixels)data.pixels=pixels;
          const deliver=()=>{if(!this.terminations)this.onmessage?.({data});};
          if(pixels && holdPixels)heldPixels.push(deliver);else queue.push(deliver);},close:()=>this.closed=true};
      context.self=context;this.context=context;vm.runInNewContext(workerSource,context);
    }
    postMessage(value){const data=structuredClone(value);this.messages.push(data);
      queue.push(()=>{if(!this.terminations&&!this.closed)this.context.onmessage({data});});}
    terminate(){this.terminations++;}
  }
  const srcdoc=h.instance.frame.srcdoc,script=srcdoc.slice(srcdoc.indexOf('<script>')+8,srcdoc.lastIndexOf('</script>'));
  vm.runInNewContext(script,{document:{querySelector:()=>canvas,createElement:()=>new Canvas()},parent,Blob,Worker,ImageBitmap,devicePixelRatio:2,
    URL:{createObjectURL:()=> 'blob:bitmap-path',revokeObjectURL:value=>revoked.push(value)},
    addEventListener:(name,callback)=>iframeHandlers[name]=callback,
    removeEventListener:(name,callback)=>{if(iframeHandlers[name]===callback)delete iframeHandlers[name];},
    setTimeout:(callback,delay)=>{const id=++nextTimer;iframeTimers.set(id,{callback,delay});return id;},
    clearTimeout:id=>iframeTimers.delete(id)});
  const flush=async()=>{
    let steps=0,empty=0;
    while(empty<2){
      while(queue.length){assert.ok(++steps<200,'Message transport did not settle');queue.shift()();empty=0;}
      await new Promise(resolve=>setImmediate(resolve));
      if(!queue.length)empty++;
    }
  };
  const flushStop=async()=>{await flush();};
  const click=(x,y)=>canvasHandlers.click({clientX:rect.left+x,clientY:rect.top+y});
  const sendFromParent=(data,{source=parent,token=h.instance.token}={})=>iframeHandlers.message({source,data:{token,...data}});
  const encode=async(index=encodes.length-1,blobOverrides={})=>{
    const encode=encodes[index];assert.ok(encode,'One trusted displayed-canvas encode');
    const bytes=pngBytes(encode.width,encode.height,encode.marks.length);
    encode.callback({type:'image/png',size:bytes.byteLength,arrayBuffer:async()=>bytes,...blobOverrides});await flush();return bytes;
  };
  await flush();if(!initialFailure){assert.equal(h.instance.ready,true);assert.equal(errors.length,0);}
  const stop=async()=>{h.instance.destroy();await flushStop();};
  return {h,inspected,captions,errors,rect,canvas,workers,workerClock,iframeTimers,revoked,click,flush,flushStop,sendFromParent,encodes,encode,copies,stop,
    resize:()=>iframeHandlers.resize?.(),
    makeBitmap:(width,height)=>new ImageBitmap(new Canvas(width,height)),
    holdPixels:()=>{holdPixels=true;},releasePixels:async()=>{holdPixels=false;queue.push(...heldPixels.splice(0));await flush();},
    holdPaintAcks:()=>{holdPaintAcks=true;},releasePaintAcks:async()=>{holdPaintAcks=false;queue.push(...heldPaintAcks.splice(0));await flush();},
    holdRenderings:()=>{holdRenderings=true;},releaseRenderings:async()=>{holdRenderings=false;queue.push(...heldRenderings.splice(0));await flush();},
    failCopies:()=>{failCopies=true;},hold:()=>{holdCopies=true;},release:async()=>{holdCopies=false;workers[0].releaseCopy?.();await flush();}};
}

test('production worker and presentation failures retain safe distinct phases',async t=>{
  const cases=[
    {name:'initialization',code:"throw Error('Do not disclose program source');",initialFailure:true,kind:'program',phase:'initialize',exception:'Error'},
    {name:'malformed quoted description',code:"const description='A participating department's population label'; return {duration:40,render(){}};",initialFailure:true,kind:'program',phase:'initialize',exception:'SyntaxError'},
    {name:'custom exception cannot impersonate a built-in class',code:"class PrivateFailure extends Error{}; throw Object.assign(new PrivateFailure('Do not disclose program source'),{name:'SyntaxError',stack:'Do not disclose the stack'});",initialFailure:true,kind:'program',phase:'initialize'},
    {name:'render',code:"return {duration:40,render({time}){if(time>0)throw Error('Do not disclose program source');}};",kind:'program',phase:'render',exception:'Error',trigger:p=>{p.h.instance.seek(1);p.h.tick();}},
    {name:'undefined render identifier',code:"return {duration:40,render({time}){if(time>0)missingRenderValue();}};",kind:'program',phase:'render',exception:'ReferenceError',trigger:p=>{p.h.instance.seek(1);p.h.tick();}},
    {name:'inspection',code:"return {duration:40,render(){},hitTest(){throw Error('Do not disclose program source');}};",kind:'program',phase:'inspect',exception:'Error',trigger:p=>p.click(10,10)},
    {name:'pixel copy',kind:'presentation',phase:'copy',trigger:p=>{p.failCopies();p.h.instance.seek(1);p.h.tick();}},
    {name:'surface bounds',kind:'presentation',phase:'bounds',trigger:p=>{p.rect.width=5000;p.h.instance.seek(1);p.h.tick();}},
    {name:'visible canvas paint',kind:'presentation',phase:'paint',trigger:p=>{p.canvas.ctx.drawImage=()=>{throw Error('Do not disclose presentation text');};p.h.instance.seek(1);p.h.tick();}},
    {name:'unexpected worker event',kind:'runtime',phase:'worker',trigger:p=>p.workers[0].onerror({message:'Do not disclose worker text'})},
    {name:'unsupported worker details',kind:'runtime',phase:'worker',trigger:p=>p.workers[0].onmessage({data:{error:true,failure:{kind:'constructor',phase:'Private source',message:'Do not disclose worker text'}}})},
    {name:'unknown exception and private details',kind:'program',phase:'initialize',trigger:p=>p.workers[0].onmessage({data:{error:true,failure:{kind:'program',phase:'initialize',exception:'PrivateFailure',message:'Do not disclose worker text',stack:'Do not disclose worker stack'}}})},
  ];
  for(const {name,kind,phase,exception,trigger,...options} of cases)await t.test(name,async t=>{
    const p=await bitmapPath(t,options);trigger?.(p);await p.flush();
    assert.equal(p.errors.length,1);const {diagnostics,...detail}=p.errors[0];
    assert.deepEqual(detail,{kind,phase,...(exception ? {exception}:{})});
    if(diagnostics)assert.equal(diagnostics.frameId,draws(p.h).at(-1).frameId);
    assert.equal(JSON.stringify(p.errors).includes('Do not disclose'),false);assert.equal(p.h.instance.alive,false);await p.stop();
  });
});

test('a size race retains the iframe draw until layout returns without replacing its candidate',async t=>{
  const p=await bitmapPath(t),worker=p.workers[0],before=worker.messages.filter(message=>message.draw).length;
  Object.assign(p.rect,{width:0,height:0});p.h.instance.seek(7);p.h.tick();await p.flush();
  assert.equal(p.h.instance.alive,true);assert.deepEqual(p.errors,[]);
  assert.equal(worker.messages.filter(message=>message.draw).length,before);
  assert.equal([...p.h.timers.values()].some(timer=>timer.delay===2000),false);
  Object.assign(p.rect,{width:520,height:400});p.resize();await p.flush();
  const draw=worker.messages.filter(message=>message.draw).at(-1).draw;
  assert.equal(draw.time,7);assert.equal(draw.width,520);assert.equal(draw.height,400);
  assert.equal(p.canvas.width,1040);assert.equal(p.canvas.height,800);assert.deepEqual(p.errors,[]);
  await p.stop();
});

test('a deferred capture keeps its requested controls and measures only the restored surface',async t=>{
  const p=await bitmapPath(t);
  p.h.instance.selectedId='worker:200';p.h.instance.showPeers=true;
  Object.assign(p.rect,{width:0,height:0});
  const result=p.h.instance.capture({time:12});await p.flush();
  assert.equal(p.h.instance.alive,true);assert.equal(p.encodes.length,0);
  p.h.instance.selectedId='lead';p.h.instance.showPeers=false;
  Object.assign(p.rect,{width:520,height:400});p.resize();await p.flush();await p.encode();
  const capture=await result;
  assert.equal(capture.time,12);assert.equal(capture.selectedId,'worker:200');assert.equal(capture.showTools,true);
  assert.deepEqual([capture.width,capture.height,capture.pixelWidth,capture.pixelHeight],[520,400,1040,800]);
  assert.equal(p.h.instance.time,0);assert.deepEqual(p.errors,[]);await p.stop();
});

test('a hidden capture times out without stopping the candidate or publishing a later stale sample',async t=>{
  const p=await bitmapPath(t);Object.assign(p.rect,{width:0,height:0});
  const result=p.h.instance.capture({time:12}),rejected=assert.rejects(result,{code:'timeout'});await p.flush();
  p.h.timeout(ANIMATION_CAPTURE_LIMITS.timeout);await rejected;await p.flush();
  assert.equal(p.h.instance.alive,true);assert.deepEqual(p.errors,[]);
  Object.assign(p.rect,{width:520,height:400});p.resize();await p.flush();p.h.tick();await p.flush();
  const draw=p.workers[0].messages.filter(message=>message.draw).at(-1).draw;
  assert.equal(draw.time,0);assert.equal(draw.captureId,undefined);assert.equal(p.encodes.length,0);
  await p.stop();
});

test('registered canvas clicks traverse bitmap presentation and worker in CSS coordinates at DPR 2',async t=>{
  const p=await bitmapPath(t),mark=p.canvas.marks[1],worker=p.workers[0];
  assert.equal(p.canvas.width,1200);assert.equal(p.canvas.height,680);assert.equal(p.copies[0].closed,1);
  p.click(mark.x,mark.y);await p.flush();assert.deepEqual(p.inspected,['worker:200']);
  assert.deepEqual(worker.messages.at(-1).pointer,{x:mark.x,y:mark.y});
  p.click(mark.x+30,mark.y+30);await p.flush();assert.deepEqual(p.inspected,['worker:200']);await p.stop();
});

test('actual resized bitmap presentation replaces hit geometry before the next registered canvas click',async t=>{
  const p=await bitmapPath(t),oldMark={...p.canvas.marks[1]};
  Object.assign(p.rect,{left:53,top:31,width:900,height:500});Object.assign(p.h.rect,{width:900,height:500});
  p.h.resizeObservers[0].callback();p.h.tick();await p.flush();
  const mark=p.canvas.marks[1],draw=p.workers[0].messages.filter(message=>message.draw).at(-1).draw;
  assert.deepEqual({width:draw.width,height:draw.height,ratio:draw.ratio},{width:900,height:500,ratio:2});
  assert.equal(p.canvas.width,1800);assert.equal(p.canvas.height,1000);assert.notDeepEqual(mark,oldMark);
  p.click(oldMark.x,oldMark.y);await p.flush();assert.deepEqual(p.inspected,[]);
  p.click(mark.x,mark.y);await p.flush();assert.deepEqual(p.inspected,['worker:200']);await p.stop();
});

test('iframe and parent reject foreign sources and stale tokens across bitmap transport',async t=>{
  const p=await bitmapPath(t),worker=p.workers[0],count=worker.messages.length;
  p.sendFromParent({draw:{frameId:99,width:999,height:999}},{source:{}});
  p.sendFromParent({draw:{frameId:99,width:999,height:999}},{token:'stale-token'});await p.flush();assert.equal(worker.messages.length,count);
  const receive=p.h.listeners.get('message');
  receive({source:{},data:{token:p.h.instance.token,inspect:'lead'}});
  receive({source:p.h.instance.frame.contentWindow,data:{token:'stale-token',inspect:'lead'}});
  assert.deepEqual(p.inspected,[]);p.click(p.canvas.marks[0].x,p.canvas.marks[0].y);await p.flush();assert.deepEqual(p.inspected,['lead']);await p.stop();
});

test('unknown identity from authored hit test cannot reach the host inspector',async t=>{
  const p=await bitmapPath(t);p.click(-1,20);await p.flush();assert.deepEqual(p.inspected,[]);assert.deepEqual(p.errors,[]);
  p.click(p.canvas.marks[1].x,p.canvas.marks[1].y);await p.flush();assert.deepEqual(p.inspected,['worker:200']);await p.stop();
});

test('queued and later clicks cannot inspect a stopped host and graceful cleanup disposes workers and URLs',async t=>{
  const p=await bitmapPath(t),worker=p.workers[0],mark=p.canvas.marks[1];
  p.click(mark.x,mark.y);p.h.instance.destroy();await p.flush();assert.deepEqual(p.inspected,[]);
  const count=worker.messages.filter(message=>message.pointer).length;p.click(mark.x,mark.y);await p.flush();
  assert.equal(worker.messages.filter(message=>message.pointer).length,count);assert.equal(worker.closed,true);assert.equal(worker.terminations,1);
  assert.equal(p.h.removed(),1);assert.equal(p.h.listeners.has('message'),false);assert.equal(p.iframeTimers.size,0);assert.equal(p.h.timers.size,0);
  assert.deepEqual(p.revoked,['blob:bitmap-path']);
});

test('trusted displayed PNG captures the exact requested draw, controls, measured geometry and paper background',async t=>{
  const p=await bitmapPath(t);
  p.h.instance.selectedId='worker:200';p.h.instance.showPeers=true;p.h.instance.reducedMotion=true;
  p.h.instance.runtimeActivity={scope:'native-design-rehearsal',activeNodeIds:['lead']};
  const result=p.h.instance.capture({time:12});await p.flush();
  assert.equal(p.encodes.length,1);assert.equal(p.encodes[0].background,'#fafaf7');
  const requested=p.h.messages.filter(message=>message.draw).at(-1).draw;
  await p.encode();const captured=await result;
  assert.equal(captured.frameId,requested.frameId);assert.equal(captured.captureId,requested.captureId);assert.equal(captured.time,12);
  assert.equal(captured.progress,.3);assert.equal(captured.playing,false);assert.equal(captured.selectedId,'worker:200');
  assert.equal(captured.showTools,true);assert.equal(captured.reducedMotion,true);
  assert.deepEqual(captured.runtimeActivity,{scope:'native-design-rehearsal',activeNodeIds:['lead']});
  assert.deepEqual([captured.width,captured.height,captured.ratio,captured.pixelWidth,captured.pixelHeight],[600,340,2,1200,680]);
  assert.equal(captured.mime,'image/png');assert.equal(new DataView(captured.bytes).getUint32(33),2);
  assert.equal(p.h.instance.time,0,'A review sample does not seek playback');await p.stop();
});

test('capture waits for busy draw then freezes controls at request time and allows later playback during encoding',async t=>{
  const p=await bitmapPath(t);p.h.instance.paused=false;p.h.tick();
  const result=p.h.instance.capture();const duplicate=p.h.instance.capture();await assert.rejects(duplicate,{code:'busy'});
  const time=p.h.instance.time;p.h.instance.selectedId='lead';await p.flush();
  p.h.tick();await p.flush();assert.ok(p.h.instance.time>time);
  const capture=p.encodes[0];assert.ok(capture);await p.encode();const packet=await result;
  assert.equal(packet.time,time);assert.equal(packet.playing,true);assert.equal(packet.selectedId,null);await p.stop();
});

test('worker-forged acknowledgements and capture packets do not bypass trusted bitmap paint',async t=>{
  const code="return {duration:10,render(){postMessage({drawn:true,frameId:1,captured:true,captureId:'forged',bytes:new ArrayBuffer(40)});}};";
  const p=await bitmapPath(t,{code});const before=draws(p.h).length;
  p.h.instance.seek(2);p.h.tick();p.hold();await p.flush();
  const worker=p.workers[0];worker.onmessage({data:{drawn:true,frameId:draws(p.h).at(-1).frameId}});
  worker.onmessage({data:{deferred:true,frameId:draws(p.h).at(-1).frameId}});
  worker.onmessage({data:{rendering:true,frameId:draws(p.h).at(-1).frameId}});
  assert.equal([...p.h.timers.values()].some(timer=>timer.delay===2000),true,'Authored packets cannot suspend execution timeout');
  p.h.tick();assert.equal(draws(p.h).length,before+1,'Worker acknowledgement cannot clear host draw busy state');
  worker.onmessage({data:{captured:true,captureId:'forged',frameId:2,bytes:pngBytes(1,1)}});assert.equal(p.encodes.length,0);
  await p.release();p.h.tick();await p.flush();await p.stop();
});

test('incremental worker backing pixels survive createImageBitmap copies',async t=>{
  const p=await bitmapPath(t,{code:"return {duration:40,render({ctx,time}){ctx.beginPath();ctx.arc(10+time,20,2,0,6);ctx.fill();}};"});
  assert.equal(p.canvas.marks.length,1);p.h.instance.seek(3);p.h.tick();await p.flush();assert.equal(p.canvas.marks.length,2);
  assert.ok(p.copies.every(bitmap=>bitmap.closed===1));await p.stop();
});

test('in-flight resize rejects capture and recovers ordinary drawing without scene failure',async t=>{
  const p=await bitmapPath(t);p.hold();const result=p.h.instance.capture({time:5});const rejected=assert.rejects(result,{code:'resized'});await p.flush();
  Object.assign(p.rect,{width:900,height:500});Object.assign(p.h.rect,{width:900,height:500});p.h.resizeObservers[0].callback();
  await p.release();await rejected;assert.equal(p.h.instance.alive,true);assert.deepEqual(p.errors,[]);assert.equal(p.encodes.length,0);
  p.h.tick();await p.flush();assert.equal(p.canvas.width,1800);assert.equal(p.canvas.height,1000);
  assert.equal([...p.h.timers.values()].some(timer=>timer.delay===2000),false);await p.stop();
});

test('oversized capture surface and encoding responses fail with bounded errors',async t=>{
  const p=await bitmapPath(t);p.rect.width=5000;
  const oversized=p.h.instance.capture();const rejected=assert.rejects(oversized,{code:'bounds'});await p.flush();await rejected;
  assert.equal(p.encodes.length,0);assert.equal(p.h.instance.alive,true);
  p.rect.width=600;const bytes=p.h.instance.capture();const bad=assert.rejects(bytes,{code:'bounds'});await p.flush();
  await p.encode(0,{size:ANIMATION_CAPTURE_LIMITS.bytes+1});await bad;assert.equal(p.h.instance.alive,true);await p.stop();
});

test('encoding timeout and destruction cancel captures and late encoder completion cannot publish',async t=>{
  const p=await bitmapPath(t);const result=p.h.instance.capture();const rejected=assert.rejects(result,{code:'timeout'});await p.flush();
  const timer=[...p.iframeTimers.values()].find(value=>value.delay===ANIMATION_CAPTURE_LIMITS.encodeTimeout);assert.ok(timer);timer.callback();await p.flush();await rejected;
  await p.encode();assert.equal(p.h.instance.alive,true);
  const next=p.h.instance.capture();const stopped=assert.rejects(next,{code:'destroyed'});await p.flush();p.h.instance.destroy();await stopped;await p.flush();
  await p.encode(1);assert.equal(p.h.listeners.has('message'),false);assert.equal(p.iframeTimers.size,0);
});

test('capture parent fences reject foreign/stale envelopes, invalid PNG and mismatched sample controls',async t=>{
  const h=host(t);readyPaused(h);
  for(const wrong of ['controls','png']){
    const result=h.instance.capture({time:4}),draw=draws(h).at(-1),frame={...draw,width:600,height:340,ratio:1};
    const response={captured:true,captureId:draw.captureId,frameId:draw.frameId,frame,mime:'image/png',bytes:pngBytes(600,340)};
    h.emit(response,{});h.listeners.get('message')({source:h.instance.frame.contentWindow,data:{token:'old',...response}});
    h.emit({...response,captureId:'other'});h.emit({...response,frameId:draw.frameId+1});
    const rejected=assert.rejects(result,{code:'invalid'});
    h.emit(wrong==='controls'?{...response,frame:{...frame,time:5}}:{...response,bytes:new ArrayBuffer(40)});
    await rejected;h.emit({drawn:true,frameId:draw.frameId});
  }
  h.instance.destroy();
});

test('capture rejects unavailable, invalid sample time and stopped owners before allocating a request',async t=>{
  const h=host(t);await assert.rejects(h.instance.capture(),{code:'unavailable'});readyPaused(h);
  for(const time of [-1,40,NaN,Infinity,'5'])await assert.rejects(h.instance.capture({time}),{code:'time'});
  await assert.rejects(h.instance.capture({invented:true}),{code:'invalid'});assert.equal(draws(h).length,1);
  h.instance.destroy();await assert.rejects(h.instance.capture(),{code:'destroyed'});
});

test('failure diagnostics retain bounded numbers and enums without authored text or nested payloads',()=>{
  const diagnostics={frameId:7,time:2.5,width:668,height:501,ratio:2,pixelWidth:1336,pixelHeight:1002,
    receivedPixelWidth:1337,receivedPixelHeight:1002,lastStage:'iframe-pixels',stageSource:'iframe',
    parentRafAgeMs:81100,deadlineOvershootMs:79000,parentVisibility:'visible',renderMs:3,copyMs:5,
    workerTimingSource:'untrusted-worker-report'};
  const failure=animationFailureDetail({kind:'presentation',phase:'copy',diagnostics:{...diagnostics,
    source:'Private program',identity:'Private owner',message:'Private provider',stack:{private:true}}});
  assert.deepEqual(failure,{kind:'presentation',phase:'copy',diagnostics});
  assert.deepEqual(animationFailureDetail(failure),failure,'Consumer sanitization retains the same diagnostic envelope');
  const invalid=animationFailureDetail({kind:'timeout',phase:'draw',diagnostics:{frameId:7.5,time:-1,width:Infinity,
    height:'501',ratio:3,pixelWidth:1.5,pixelHeight:4294967296,renderMs:NaN,copyMs:-1,parentRafAgeMs:86400001,
    deadlineOvershootMs:{private:true},lastStage:'Private program',stageSource:'native-authority',
    parentVisibility:'frozen',workerTimingSource:'trusted',message:'Private source'}});
  assert.deepEqual(invalid,{kind:'timeout',phase:'draw'});
  assert.deepEqual(animationFailureDetail({kind:'timeout',phase:'draw',diagnostics:[diagnostics]}),{kind:'timeout',phase:'draw'});
});

test('a delayed visible watchdog records parent scheduler age and overshoot for its exact pending draw',t=>{
  const errors=[],h=host(t,{onError:value=>errors.push(value)});readyPaused(h);
  h.instance.seek(2);h.tick();const draw=draws(h).at(-1);
  h.advance(79000);expireDraw(h);
  assert.equal(errors.length,1);
  assert.deepEqual(errors[0],{kind:'timeout',phase:'draw',diagnostics:{frameId:draw.frameId,time:2,
    width:600,height:340,ratio:1,pixelWidth:600,pixelHeight:340,lastStage:'draw-sent',stageSource:'host',
    parentRafAgeMs:81100,deadlineOvershootMs:79000,parentVisibility:'visible'}});
});

test('parent scheduler diagnostics are whole milliseconds when a plain clock difference is not',t=>{
  const errors=[],h=host(t,{onError:value=>errors.push(value)},{clock:1000.1});readyPaused(h);
  h.instance.seek(2);h.tick();h.advance(79000.4);expireDraw(h);
  assert.equal(errors.length,1);const {parentRafAgeMs,deadlineOvershootMs}=errors[0].diagnostics;
  assert.deepEqual([parentRafAgeMs,deadlineOvershootMs],[81100,79000]);
});

test('actual worker reports distinguish a completed nonzero render from an unresolved bitmap copy',async t=>{
  const p=await bitmapPath(t,{renderCostMs:3,copyCostMs:5});p.hold();p.h.instance.seek(2);p.h.tick();await p.flush();
  const pending=draws(p.h).at(-1);expireDraw(p.h);
  assert.equal(p.errors.length,1);const failure=p.errors[0];
  assert.equal(failure.kind,'timeout');assert.equal(failure.phase,'draw');
  assert.equal(failure.diagnostics.frameId,pending.frameId);assert.equal(failure.diagnostics.time,2);
  assert.equal(failure.diagnostics.lastStage,'worker-rendered');assert.equal(failure.diagnostics.renderMs,3);
  assert.equal(failure.diagnostics.copyMs,undefined);assert.equal(failure.diagnostics.workerTimingSource,'untrusted-worker-report');
  assert.equal(failure.diagnostics.stageSource,'untrusted-worker-report');assert.equal(failure.diagnostics.parentVisibility,'visible');
  assert.ok(failure.diagnostics.parentRafAgeMs>=2000);await p.release();await p.stop();
});

test('an actual nonzero render exception retains its incomplete render stage and pending time',async t=>{
  const p=await bitmapPath(t,{code:"return {duration:40,render({time}){if(time>0)throw Error('Private source');}};"});
  p.h.instance.seek(2);p.h.tick();await p.flush();
  assert.equal(p.errors.length,1);const {kind,phase,exception,diagnostics}=p.errors[0];
  assert.deepEqual({kind,phase,exception},{kind:'program',phase:'render',exception:'Error'});
  assert.equal(diagnostics.time,2);assert.equal(diagnostics.lastStage,'worker-render-started');
  assert.equal(diagnostics.stageSource,'untrusted-worker-report');assert.equal(diagnostics.renderMs,undefined);
  assert.equal(JSON.stringify(p.errors).includes('Private source'),false);await p.stop();
});

test('completed bitmap-copy reports do not attest iframe receipt or extend its pending deadline',async t=>{
  const p=await bitmapPath(t,{renderCostMs:3,copyCostMs:5});p.holdPixels();p.h.instance.seek(4);p.h.tick();await p.flush();
  const pending=draws(p.h).at(-1);expireDraw(p.h);
  assert.equal(p.errors.length,1);const diagnostic=p.errors[0].diagnostics;
  assert.equal(diagnostic.frameId,pending.frameId);assert.equal(diagnostic.time,4);
  assert.equal(diagnostic.lastStage,'worker-copied');assert.equal(diagnostic.stageSource,'untrusted-worker-report');
  assert.equal(diagnostic.renderMs,3);assert.equal(diagnostic.copyMs,5);
  assert.equal(diagnostic.receivedPixelWidth,undefined);assert.equal(diagnostic.workerTimingSource,'untrusted-worker-report');
  await p.releasePixels();await p.stop();
});

test('worker render and copy reports are whole milliseconds when a plain clock difference is not',async t=>{
  // The first two clocks start the timed render or copy within its cost below 256. The sum needs
  // one more integer bit and drops the clock's last bit, so the plain difference misses the cost.
  // The third case costs fractions of a millisecond.
  const cases=[
    {name:'render crosses 256',clock:227.1,costs:[3,5],plain:[3.0000000000000284,5],reported:[3,5]},
    {name:'copy crosses 256',clock:223.1,costs:[3,5],plain:[3,5.000000000000028],reported:[3,5]},
    {name:'sub-millisecond costs',clock:1000.1,costs:[2.6,4.4],plain:[2.599999999999909,4.400000000000091],reported:[3,4]},
  ];
  for(const {name,clock,costs:[renderCostMs,copyCostMs],plain,reported} of cases)await t.test(name,async t=>{
    const p=await bitmapPath(t,{renderCostMs,copyCostMs,clock});p.holdPixels();p.h.instance.seek(4);p.h.tick();await p.flush();
    const [renderStarted,rendered,copyStarted,copied]=p.workerClock.slice(-4);
    assert.deepEqual([rendered-renderStarted,copied-copyStarted],plain,'Plain differences of the worker clock');
    expireDraw(p.h);assert.equal(p.errors.length,1);
    const {lastStage,renderMs,copyMs}=p.errors[0].diagnostics;
    assert.deepEqual([lastStage,renderMs,copyMs],['worker-copied',...reported]);
    await p.releasePixels();await p.stop();
  });
});

test('matching malformed bitmap dimensions fail explicitly while preserving expected and received geometry',async t=>{
  const p=await bitmapPath(t),before=structuredClone(p.canvas.marks);p.hold();p.h.instance.seek(2);p.h.tick();await p.flush();
  const bitmap=p.copies.at(-1);bitmap.width+=1;await p.release();
  assert.equal(p.errors.length,1);const {kind,phase,diagnostics}=p.errors[0];
  assert.deepEqual({kind,phase},{kind:'presentation',phase:'copy'});
  assert.equal(diagnostics.frameId,draws(p.h).at(-1).frameId);assert.equal(diagnostics.time,2);
  assert.equal(diagnostics.lastStage,'iframe-pixels');assert.equal(diagnostics.stageSource,'iframe');
  assert.deepEqual([diagnostics.width,diagnostics.height,diagnostics.ratio,diagnostics.pixelWidth,diagnostics.pixelHeight,
    diagnostics.receivedPixelWidth,diagnostics.receivedPixelHeight],[600,340,2,1200,680,1201,680]);
  assert.equal(bitmap.closed,1);assert.deepEqual(p.canvas.marks,before);await p.stop();
});

test('stale bitmap and diagnostic packets cannot fail or supersede a newer pending frame',async t=>{
  const p=await bitmapPath(t);p.hold();p.h.instance.seek(2);p.h.tick();await p.flush();
  const frameId=draws(p.h).at(-1).frameId,worker=p.workers[0],timer=[...p.h.timers.keys()][0];
  const stale=p.makeBitmap(1,1);worker.onmessage({data:{pixels:stale,frameId:frameId-1}});
  worker.onmessage({data:{diagnostic:true,frameId:frameId-1,lastStage:'worker-copied',renderMs:900,copyMs:900}});
  worker.onmessage({data:{diagnostic:true,frameId:frameId+1,lastStage:'worker-copied',renderMs:900,copyMs:900}});
  worker.onmessage({data:{diagnostic:true,frameId,lastStage:'iframe-painted',renderMs:900,copyMs:900}});
  await p.flush();assert.equal(stale.closed,1);assert.deepEqual(p.errors,[]);
  assert.deepEqual([...p.h.timers.keys()],[timer],'Reports cannot replace the existing draw deadline');
  expireDraw(p.h);const diagnostic=p.errors[0].diagnostics;
  assert.equal(diagnostic.frameId,frameId);assert.equal(diagnostic.lastStage,'worker-rendered');assert.equal(diagnostic.renderMs,0);
  assert.equal(diagnostic.copyMs,undefined);await p.release();await p.stop();
});

test('forged current worker progress stays untrusted, bounded and unable to suspend the watchdog',async t=>{
  const p=await bitmapPath(t);p.hold();p.h.instance.seek(3);p.h.tick();await p.flush();
  const frameId=draws(p.h).at(-1).frameId,worker=p.workers[0],timer=[...p.h.timers.keys()][0];
  for(let i=0;i<3;i++){
    p.h.advance(200);worker.onmessage({data:{diagnostic:true,frameId,lastStage:'worker-rendered',renderMs:12,
      copyMs:Infinity,workerTimingSource:'trusted',parentVisibility:'hidden',parentRafAgeMs:0,source:'Private source'}});await p.flush();
    assert.deepEqual([...p.h.timers.keys()],[timer]);assert.equal(p.h.instance.alive,true);
  }
  expireDraw(p.h);const diagnostic=p.errors[0].diagnostics;
  assert.equal(diagnostic.renderMs,12);assert.equal(diagnostic.copyMs,undefined);
  assert.equal(diagnostic.workerTimingSource,'untrusted-worker-report');assert.equal(diagnostic.stageSource,'untrusted-worker-report');
  assert.equal(diagnostic.parentVisibility,'visible');assert.ok(diagnostic.parentRafAgeMs>=2600);
  assert.equal(JSON.stringify(p.errors).includes('Private source'),false);await p.release();await p.stop();
});

test('worker error envelopes cannot impersonate trusted stage, geometry or parent scheduling diagnostics',async t=>{
  const p=await bitmapPath(t);p.hold();p.h.instance.seek(3);p.h.tick();await p.flush();
  const frameId=draws(p.h).at(-1).frameId;
  p.workers[0].onmessage({data:{error:true,failure:{kind:'program',phase:'render',diagnostics:{frameId,time:999,
    width:999,height:999,lastStage:'iframe-painted',stageSource:'iframe',parentVisibility:'hidden',parentRafAgeMs:0,
    workerTimingSource:'trusted',renderMs:999,copyMs:999,message:'Private source'}}}});await p.flush();
  assert.equal(p.errors.length,1);const diagnostic=p.errors[0].diagnostics;
  assert.equal(diagnostic.time,3);assert.equal(diagnostic.width,600);assert.equal(diagnostic.height,340);
  assert.equal(diagnostic.lastStage,'worker-rendered');assert.equal(diagnostic.stageSource,'untrusted-worker-report');
  assert.equal(diagnostic.renderMs,0);assert.equal(diagnostic.copyMs,undefined);assert.equal(diagnostic.parentVisibility,'visible');
  assert.equal(JSON.stringify(p.errors).includes('Private source'),false);await p.release();await p.stop();
});

test('a queued trusted painted ACK can settle its exact draw after the original deadline',async t=>{
  const code="return {duration:40,render({ctx,time}){ctx.clearRect(0,0,1000,1000);ctx.beginPath();ctx.arc(20+time,20,2,0,6);ctx.fill();}};";
  const p=await bitmapPath(t,{code});p.holdPaintAcks();p.h.instance.seek(2);p.h.tick();await p.flush();
  const pending=draws(p.h).at(-1),count=draws(p.h).length;
  assert.equal(p.canvas.marks[0].x,22,'The production iframe already painted this exact pending draw');
  p.h.timeout(2000);
  assert.deepEqual(p.errors,[],'The due timer must allow the already-queued matching painted acknowledgement');
  assert.equal(p.h.instance.alive,true);
  assert.deepEqual([...p.h.timers.values()].map(timer=>timer.delay),[100]);
  const finalCallback=[...p.h.timers.values()][0].fn;
  p.h.emit({drawn:true,frameId:pending.frameId+1});p.h.tick();
  assert.equal(draws(p.h).length,count,'Pixels and a wrong-frame ACK do not clear busy');
  await p.releasePaintAcks();assert.equal(p.h.timers.size,0);assert.deepEqual(p.errors,[]);
  p.h.instance.seek(3);p.h.tick();finalCallback();assert.deepEqual(p.errors,[],'An old final callback cannot fail a newer draw');
  await p.flush();await p.stop();
});

test('a missing painted ACK fails after one bounded opportunity despite worker progress',async t=>{
  const p=await bitmapPath(t);p.hold();p.h.instance.seek(2);p.h.tick();await p.flush();
  const pending=draws(p.h).at(-1),worker=p.workers[0];
  p.h.timeout(2000);assert.deepEqual(p.errors,[]);
  const [id,final]=[...p.h.timers.entries()][0];assert.equal(final.delay,100);
  for(let index=0;index<3;index++){
    worker.onmessage({data:{diagnostic:true,frameId:pending.frameId,lastStage:'worker-copied',renderMs:0,copyMs:0}});
    worker.onmessage({data:{drawn:true,frameId:pending.frameId}});
    await p.flush();assert.equal(p.h.timers.get(id),final,'Untrusted worker reports cannot replace the final timer');
  }
  p.h.timeout(100);assert.equal(p.errors.length,1);assert.equal(p.h.instance.alive,false);
  assert.equal(p.errors[0].kind,'timeout');assert.equal(p.errors[0].phase,'draw');
  assert.equal(p.errors[0].diagnostics.frameId,pending.frameId);
  await p.release();await p.stop();
});

test('the final ACK opportunity retains only its remaining visible time and cannot revive disposal',t=>{
  const errors=[],h=host(t,{onError:value=>errors.push(value)});readyPaused(h);h.instance.seek(2);h.tick();
  h.timeout(2000);assert.deepEqual(errors,[]);h.advance(40);h.visibility(true);
  h.advance(10000);assert.equal(h.timers.size,0);h.visibility(false);
  const [id,final]=[...h.timers.entries()][0];assert.ok(Math.abs(final.delay-60)<1e-9);
  h.timers.delete(id);h.advance(60);final.fn();assert.equal(errors.length,1);assert.equal(h.instance.alive,false);
  h.visibility(false);final.fn();assert.equal(errors.length,1,'A queued final callback cannot revive or re-fail disposal');
});

test('a queued iframe dispatch cannot renew an expired draw but its exact painted ACK can settle',async t=>{
  for(const acknowledged of [false,true])await t.test(acknowledged?'matching queued ACK still settles':'missing ACK still expires',async t=>{
    const p=await bitmapPath(t);p.holdRenderings();p.holdPaintAcks();p.h.instance.seek(2);p.h.tick();await p.flush();
    p.h.timeout(2000);assert.deepEqual(p.errors,[]);
    const [id,final]=[...p.h.timers.entries()][0];assert.equal(final.delay,100);
    await p.releaseRenderings();
    assert.equal(p.h.timers.get(id),final,'Late trusted rendering cannot replace the final delivery timer');
    assert.deepEqual([...p.h.timers.values()].map(timer=>timer.delay),[100]);
    if(acknowledged){await p.releasePaintAcks();assert.equal(p.h.timers.size,0);final.fn();assert.deepEqual(p.errors,[]);}
    else{p.h.timeout(100);assert.equal(p.errors.length,1);assert.equal(p.errors[0].kind,'timeout');await p.releasePaintAcks();assert.equal(p.errors.length,1);}
    await p.stop();
  });
});


test('an offscreen animation freezes and resumes without replacing a healthy program',t=>{
  let errors=0;
  const h=host(t,{onError:()=>errors++},{observeIntersection:true});
  readyPaused(h);
  h.instance.paused=false;
  h.surface(false);
  const before=h.instance.time,count=draws(h).length;
  for(let i=0;i<20;i++)h.tick(500);
  assert.equal(h.instance.time,before);
  assert.equal(draws(h).length,count);
  assert.equal(errors,0);
  h.surface(true);h.tick();
  assert.ok(h.instance.time>before);
  assert.equal(draws(h).length,count+1);
  h.emit({drawn:true,frameId:draws(h).at(-1).frameId});
  assert.equal(errors,0);
  h.instance.destroy();assert.equal(h.intersectionObservers[0].disconnected,true);
});

test('offscreen waiting does not consume a pending draw deadline; a visible stall still fails',t=>{
  let failure;
  const h=host(t,{onError:value=>failure=value},{observeIntersection:true});
  readyPaused(h);h.instance.seek(3);h.tick();
  h.advance(750);h.surface(false);
  assert.equal([...h.timers.values()].some(timer=>timer.delay===2000),false);
  h.advance(10000);h.surface(true);
  h.timeout(1250);assert.equal(failure,undefined);
  h.timeout(100);assert.equal(failure.kind,'timeout');assert.equal(failure.phase,'draw');
});
