import assert from 'node:assert/strict';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
const source = process.env.FORGE_ESCAPE_SOURCE
  ? pathToFileURL(process.env.FORGE_ESCAPE_SOURCE)
  : new URL('./freeform-animation.mjs', import.meta.url);
const { FreeformAnimation } = await import(source);

function host(t, callbacks={}, {observeResize=true}={}) {
  const listeners=new Map(),documentListeners=new Map(),timers=new Map(),frames=new Map(),messages=[],resizeObservers=[];
  let next=0,removed=0,now=performance.now();
  const rect={width:600,height:340};
  const frame={style:{},contentWindow:{postMessage:message=>messages.push(message)},getBoundingClientRect:()=>rect,remove(){removed++;}};
  class ResizeObserver {
    constructor(callback){this.callback=callback;resizeObservers.push(this);}
    observe(target){this.target=target;}
    disconnect(){this.disconnected=true;}
  }
  const replacements={
    document:{hidden:false,createElement:()=>frame,
      addEventListener:(name,fn)=>documentListeners.set(name,fn),
      removeEventListener:(name,fn)=>{if(documentListeners.get(name)===fn)documentListeners.delete(name);}},
    window:{addEventListener:(name,fn)=>listeners.set(name,fn),removeEventListener:(name,fn)=>{if(listeners.get(name)===fn)listeners.delete(name);}},
    ResizeObserver:observeResize?ResizeObserver:undefined,devicePixelRatio:1,performance:{now:()=>now},
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
  const timeout=delay=>{for(const [id,timer] of timers){if(timer.delay===delay){timers.delete(id);now+=delay;timer.fn();return;}}throw Error(`No ${delay}ms timer`);};
  const visibility=hidden=>{replacements.document.hidden=hidden;documentListeners.get('visibilitychange')?.();};
  return {instance,emit,tick,timeout,visibility,advance:milliseconds=>{now+=milliseconds;},now:()=>now,messages,timers,listeners,documentListeners,
    rect,resizeObservers,document:replacements.document,removed:()=>removed};
}

test('only the live owned animation frame can request Escape', t => {
  let escapes = 0;
  const h = host(t, { onEscape: () => escapes++ });
  h.emit({ escape: true }, {});
  h.emit({ escape: true, token: 'another-animation' });
  h.emit({ escape: 'true' });
  assert.equal(escapes, 0, 'Foreign frames, tokens and nonliteral events cannot close the view.');
  h.emit({ escape: true });
  assert.equal(escapes, 1, 'The owned live frame forwards Escape to its host.');
  h.instance.destroy();
  h.emit({ escape: true });
  assert.equal(escapes, 1, 'A destroyed animation cannot issue a later keyboard action.');
});
