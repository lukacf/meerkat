/** Run an authored canvas animation away from the wizard's DOM and event loop. */
export const ANIMATION_CAPTURE_LIMITS=Object.freeze({
  cssDimension:4096,pixelDimension:8192,pixels:8000000,bytes:1500000,
  timeout:4500,encodeTimeout:2000,
});

const CAPTURE_MESSAGES=Object.freeze({
  unavailable:'The animation has not started.',busy:'An animation capture is already in progress.',
  invalid:'The animation capture response was invalid.',time:'Choose a time within the animation.',
  timeout:'The animation capture took too long.',encoding:'The animation capture could not be encoded.',
  bounds:'The animation capture exceeds its supported size.',destroyed:'The animation capture was cancelled because its owner stopped.',
  resized:'The animation resized before its capture completed.',
  cancelled:'The animation capture was cancelled.',
});
const captureError=code=>Object.assign(new Error(CAPTURE_MESSAGES[code] || CAPTURE_MESSAGES.invalid),{code});
const record=value=>!!value && typeof value==='object' && !Array.isArray(value);
const DRAW_ACK_WAIT_MS=100;

export function animationFailureDetail(value) {
  const phases={timeout:['startup','draw'],program:['initialize','metadata','render','inspect'],
    presentation:['initialize','surface','copy','bounds','paint'],runtime:['worker']};
  const detail=value && typeof value.kind==='string' && Object.hasOwn(phases,value.kind) && phases[value.kind].includes(value.phase)
    ? {kind:value.kind,phase:value.phase}:{kind:'runtime',phase:'worker'};
  if(detail.kind==='program' && ['Error','EvalError','RangeError','ReferenceError','SyntaxError','TypeError','URIError','AggregateError'].includes(value.exception))
    detail.exception=value.exception;
  // Diagnostics are observations, never acknowledgements or execution authority.
  // Keep authored text, source, identities and arbitrary objects out of failures.
  const input=value?.diagnostics;
  if(input && typeof input==='object' && !Array.isArray(input)){
    const diagnostics={};
    const numeric={frameId:[1,Number.MAX_SAFE_INTEGER,true],time:[0,Number.MAX_VALUE],
      width:[0,65536],height:[0,65536],ratio:[0,2],
      pixelWidth:[0,4294967295,true],pixelHeight:[0,4294967295,true],
      receivedPixelWidth:[0,4294967295,true],receivedPixelHeight:[0,4294967295,true],
      parentRafAgeMs:[0,86400000],deadlineOvershootMs:[0,86400000],renderMs:[0,86400000],copyMs:[0,86400000]};
    for(const [key,[min,max,integer]] of Object.entries(numeric)){
      const number=input[key];
      if(Number.isFinite(number) && number>=min && number<=max && (!integer || Number.isSafeInteger(number)))diagnostics[key]=number;
    }
    const enums={lastStage:['draw-sent','iframe-dispatched','worker-render-started','worker-rendered','worker-copied','iframe-pixels','iframe-painted'],
      stageSource:['host','iframe','untrusted-worker-report'],parentVisibility:['visible','hidden'],
      workerTimingSource:['untrusted-worker-report']};
    for(const [key,values] of Object.entries(enums))if(values.includes(input[key]))diagnostics[key]=input[key];
    if(Object.keys(diagnostics).length)detail.diagnostics=diagnostics;
  }
  return detail;
}

function captureGeometry(frame) {
  const limits=ANIMATION_CAPTURE_LIMITS;
  if(!record(frame) || !Number.isFinite(frame.width) || frame.width<=0 || frame.width>limits.cssDimension ||
      !Number.isFinite(frame.height) || frame.height<=0 || frame.height>limits.cssDimension ||
      !Number.isFinite(frame.ratio) || frame.ratio<=0 || frame.ratio>2)return null;
  const pixelWidth=Math.max(1,Math.round(frame.width*frame.ratio)),pixelHeight=Math.max(1,Math.round(frame.height*frame.ratio));
  if(pixelWidth>limits.pixelDimension || pixelHeight>limits.pixelDimension || pixelWidth*pixelHeight>limits.pixels)return null;
  return {width:frame.width,height:frame.height,ratio:frame.ratio,pixelWidth,pixelHeight};
}

function matchingCaptureFrame(expected,actual) {
  if(!record(actual))return false;
  for(const key of ['frameId','time','progress','playing','selectedId','showTools','reducedMotion','captureId'])
    if(!Object.is(expected[key],actual[key]))return false;
  try{return JSON.stringify(expected.runtimeActivity)===JSON.stringify(actual.runtimeActivity);}catch{return false;}
}

function pngMatches(bytes,geometry) {
  if(!(bytes instanceof ArrayBuffer) || bytes.byteLength<33 || bytes.byteLength>ANIMATION_CAPTURE_LIMITS.bytes)return false;
  const prefix=new Uint8Array(bytes,0,24),signature=[137,80,78,71,13,10,26,10];
  if(signature.some((value,index)=>prefix[index]!==value) ||
      prefix[12]!==73 || prefix[13]!==72 || prefix[14]!==68 || prefix[15]!==82)return false;
  const view=new DataView(bytes);
  return view.getUint32(8)===13 && view.getUint32(16)===geometry.pixelWidth && view.getUint32(20)===geometry.pixelHeight;
}

export class FreeformAnimation {
  constructor(container, {code,model,blueprint,onFrame,onReady,onFirstFrame,onError,onInspect,onCaption,onEscape}) {
    this.alive=true;
    this.paused=false;
    this.reducedMotion=false;
    this.selectedId=null;
    this.showPeers=false;
    this.time=0;
    this.duration=1;
    this.chapters=[];
    this.token=crypto.randomUUID();
    const ids=new Set(model.nodes.map(node=>node.id));
    this.frame=document.createElement('iframe');
    this.frame.title='Custom animation of your proposed mob';
    this.frame.sandbox='allow-scripts';
    this.frame.className='ps-authored-frame';
    let busy=false,last=performance.now(),lastRaf=last,watchdog,raf,booted=false,firstFrame=false,drawId=0,stopTimer,removed=false,drawDiagnostics=null;
    let invalidated=true,renderedInputs=null,runtimeActivity=null,pendingCapture=null,surfaceVisible=true;
    const invalidate=()=>{invalidated=true;};
    Object.defineProperty(this,'runtimeActivity',{enumerable:true,configurable:true,get:()=>runtimeActivity,set:value=>{runtimeActivity=value;invalidate();}});
    const resizeObserver=typeof ResizeObserver==='function' ? new ResizeObserver(invalidate):null;
    resizeObserver?.observe(this.frame);
    window.addEventListener('resize',invalidate);
    const updateDiagnostics=value=>{
      const safe=animationFailureDetail({kind:'timeout',phase:'draw',diagnostics:value}).diagnostics;
      if(!busy || !drawDiagnostics || safe?.frameId!==drawId)return;
      for(const key of ['width','height','ratio','pixelWidth','pixelHeight','receivedPixelWidth','receivedPixelHeight',
        'lastStage','stageSource','renderMs','copyMs','workerTimingSource'])if(Object.hasOwn(safe,key))drawDiagnostics[key]=safe[key];
    };
    const fail=detail=>{
      if(!this.alive)return;
      updateDiagnostics(detail?.diagnostics);
      // Timing diagnostics, here and in the worker, are whole milliseconds. A plain clock
      // difference carries floating-point error, such as 81099.99999999999 for 81100 ms.
      const diagnostics=busy && drawDiagnostics ? {...drawDiagnostics,
        parentRafAgeMs:Math.round(Math.max(0,performance.now()-lastRaf)),parentVisibility:document.hidden?'hidden':'visible',
        ...(detail?.kind==='timeout' && watchdog?.started!==null && watchdog?.started!==undefined
          ? {deadlineOvershootMs:Math.round((watchdog.expiredOvershootMs || 0)+Math.max(0,performance.now()-watchdog.started-watchdog.remaining))}:{})}:null;
      this.destroy();onError?.(animationFailureDetail({...detail,...(diagnostics ? {diagnostics}:{})}));
    };
    const clearWatchdog=()=>{if(watchdog)clearTimeout(watchdog.timer);watchdog=null;};
    const resumeWatchdog=()=>{
      const current=watchdog;
      if(!this.alive || !current || current.timer!==null || document.hidden || !surfaceVisible)return;
      current.started=performance.now();
      current.observed=0;
      const timer=setTimeout(()=>{
        if(watchdog!==current || current.timer!==timer)return;
        current.timer=null;
        // A queued timer can run before the visibility event. Hidden browser
        // scheduling is not evidence that the program exceeded visible time.
        if(document.hidden || !surfaceVisible){
          current.remaining=Math.max(0,current.remaining-current.observed);
          current.started=null;current.observed=0;return;
        }
        if(current.failure.phase==='draw' && !current.ackOpportunity){
          // The iframe's painted ACK may already be queued behind this timer.
          // Allow one fixed delivery window, independent of progress reports.
          // Only the matching ACK can settle the draw; a stall still expires.
          current.ackOpportunity=true;
          current.expiredOvershootMs=Math.max(0,performance.now()-current.started-current.remaining);
          current.remaining=DRAW_ACK_WAIT_MS;current.started=null;current.observed=0;
          resumeWatchdog();return;
        }
        fail(current.failure);
      },current.remaining);
      current.timer=timer;
    };
    const pauseWatchdog=()=>{
      if(!watchdog)return;
      if(watchdog.started!==null)
        watchdog.remaining=Math.max(0,watchdog.remaining-(performance.now()-watchdog.started));
      clearTimeout(watchdog.timer);watchdog.timer=null;watchdog.started=null;
    };
    const armWatchdog=(remaining,phase)=>{
      clearWatchdog();watchdog={remaining,started:null,observed:0,timer:null,failure:{kind:'timeout',phase}};
      resumeWatchdog();
    };
    const send=value=>this.frame.contentWindow?.postMessage({token:this.token,...value},'*');
    const remove=()=>{if(removed)return;removed=true;clearTimeout(stopTimer);window.removeEventListener('message',this.receive);this.frame.remove();};
    const settleCapture=(error,result)=>{
      const capture=pendingCapture;
      if(!capture)return;
      pendingCapture=null;clearTimeout(capture.timer);
      if(error){send({cancelCapture:capture.id});capture.reject(error);}else capture.resolve(result);
    };
    const controls=time=>({time,progress:time/this.duration,playing:!this.paused,selectedId:this.selectedId,
      showTools:this.showPeers,reducedMotion:this.reducedMotion,runtimeActivity:this.runtimeActivity || null});
    this.seek=time=>{this.time=Math.max(0,Math.min(Number(time)||0,this.duration-.001));invalidate();onFrame?.(this.time,this.duration);};
    const render=()=>{
      if(this.alive && this.ready && !busy){
        const capture=pendingCapture && !pendingCapture.draw ? pendingCapture:null;
        if((document.hidden || !surfaceVisible) && !capture)return;
        // Public controls can change while a draw is in flight. Compare only
        // the inputs actually sent, then preserve any later invalidation.
        const rect=this.frame.getBoundingClientRect?.();
        // A pane can be mounted before it has layout. Keep the candidate and
        // its pending capture, without starting an execution timeout yet.
        if(rect && (rect.width===0 || rect.height===0))return;
        const inputs=[this.time,this.paused,this.selectedId,this.showPeers,this.reducedMotion,this.runtimeActivity || null,Math.min(globalThis.devicePixelRatio || 1,2),rect?.width,rect?.height];
        if(!capture && !invalidated && renderedInputs?.every((value,index)=>Object.is(value,inputs[index])))return;
        invalidated=!!capture;renderedInputs=capture ? null:inputs;
        busy=true;
        armWatchdog(2000,'draw');
        const frame={frameId:++drawId,...(capture ? capture.controls:controls(this.time)),...(capture ? {captureId:capture.id}:{})};
        const ratio=Math.min(globalThis.devicePixelRatio || 1,2);
        drawDiagnostics={frameId:frame.frameId,time:frame.time,lastStage:'draw-sent',stageSource:'host',
          ...(rect ? {width:rect.width,height:rect.height,ratio,pixelWidth:Math.max(1,Math.round(rect.width*ratio)),pixelHeight:Math.max(1,Math.round(rect.height*ratio))}:{})};
        if(capture)capture.draw=frame;
        send({draw:frame});
        // A review sample never seeks the user's playback timeline.
        if(!capture)onFrame?.(this.time,this.duration);
      }
    };
    this.capture=(options={})=>{
      if(!this.alive)return Promise.reject(captureError('destroyed'));
      if(!this.ready)return Promise.reject(captureError('unavailable'));
      if(pendingCapture)return Promise.reject(captureError('busy'));
      if(!record(options) || Object.keys(options).some(key=>key!=='time'))return Promise.reject(captureError('invalid'));
      const time=Object.hasOwn(options,'time') ? options.time:this.time;
      if(!Number.isFinite(time) || time<0 || time>=this.duration)return Promise.reject(captureError('time'));
      let selected;
      try{selected=structuredClone(controls(time));}catch{return Promise.reject(captureError('invalid'));}
      return new Promise((resolve,reject)=>{
        const capture={id:crypto.randomUUID(),controls:selected,resolve,reject,draw:null,timer:null};
        pendingCapture=capture;
        capture.timer=setTimeout(()=>{if(pendingCapture===capture)settleCapture(captureError('timeout'));},ANIMATION_CAPTURE_LIMITS.timeout);
        render();
      });
    };
    this.cancelCapture=()=>{if(pendingCapture){settleCapture(captureError('cancelled'));invalidate();}};
    const draw=now=>{
      if(!this.alive)return;
      lastRaf=performance.now();
      // Retain observed visible progress even if a queued timer precedes the
      // hidden transition event. Unobserved background time stays uncharged.
      if(!document.hidden && surfaceVisible && watchdog && watchdog.started!==null)
        watchdog.observed=Math.max(watchdog.observed,now-watchdog.started);
      const dt=Math.min((now-last)/1000,.1);last=now;
      if(this.ready && !this.paused && !document.hidden && surfaceVisible)this.time=(this.time+dt)%this.duration;
      render();
      raf=requestAnimationFrame(draw);
    };
    const visibilityChanged=()=>{
      if(document.hidden || !surfaceVisible){pauseWatchdog();return;}
      last=performance.now();resumeWatchdog();render();
    };
    this.receive=event=>{
      if(event.source!==this.frame.contentWindow || event.data?.token!==this.token)return;
      const data=event.data;
      if(!record(data))return;
      if(!this.alive){if(data.stopped===true)remove();return;}
      if(data.escape===true){onEscape?.();return;}
      // Progress reports do not reset or clear any deadline. Worker timings
      // remain explicitly untrusted even when relayed by the isolated iframe.
      if((data.diagnostic===true || data.rendering===true || data.drawn===true) && data.frameId===drawId)
        updateDiagnostics(data.diagnostics);
      if(data.boot===true && !booted){booted=true;armWatchdog(4000,'startup');send({start:{code,model,blueprint}});}
      if(data.ready===true && booted && !this.ready){
        const metadata=animationMetadata(data);
        if(!metadata){fail({kind:'program',phase:'metadata'});return;}
        clearWatchdog();this.ready=true;this.duration=metadata.duration;this.chapters=metadata.chapters;
        onReady?.({duration:this.duration,chapters:this.chapters});
        render();
      }
      if(data.deferred===true && this.ready && busy && data.frameId===drawId)clearWatchdog();
      // A delayed dispatch notice cannot renew an already-expired draw.
      if(data.rendering===true && this.ready && busy && data.frameId===drawId && !watchdog?.ackOpportunity){
        armWatchdog(2000,'draw');
      }
      if(data.drawn===true && this.ready && busy && data.frameId===drawId){
        busy=false;clearWatchdog();
        if(!firstFrame){firstFrame=true;onFirstFrame?.({duration:this.duration,chapters:this.chapters});}
        // A suspended parent RAF must not strand an already-requested capture.
        if(pendingCapture && !pendingCapture.draw)render();
      }
      if((data.captured===true || data.captureError===true) && pendingCapture &&
          data.captureId===pendingCapture.id && data.frameId===pendingCapture.draw?.frameId){
        if(data.captureError===true){
          if(busy && data.frameId===drawId){busy=false;clearWatchdog();invalidate();}
          settleCapture(captureError(['bounds','encoding','timeout','resized'].includes(data.reason)?data.reason:'invalid'));
        }
        else{
          const geometry=captureGeometry(data.frame);
          if(!matchingCaptureFrame(pendingCapture.draw,data.frame) || !geometry || data.mime!=='image/png' || !pngMatches(data.bytes,geometry))
            settleCapture(captureError('invalid'));
          else settleCapture(null,{captureId:pendingCapture.id,...structuredClone(pendingCapture.draw),...geometry,mime:'image/png',bytes:data.bytes});
        }
      }
      if(data.error===true){fail(data.failure);return;}
      if(typeof data.inspect==='string' && ids.has(data.inspect)){invalidate();onInspect?.(data.inspect);}
      const caption=animationCaption(data.caption);
      if(caption)onCaption?.(caption);
    };
    window.addEventListener('message',this.receive);
    document.addEventListener('visibilitychange',visibilityChanged);
    // Browsers can defer an offscreen child frame while the parent remains
    // visible. Its missing paint ACK is not an authored-program timeout.
    const surfaceObserver=typeof IntersectionObserver==='function' ? new IntersectionObserver(entries=>{
      const entry=entries.find(item=>item.target===this.frame);
      if(!entry)return;
      const visible=entry.isIntersecting===true;
      if(visible===surfaceVisible)return;
      surfaceVisible=visible;visibilityChanged();
    }):null;
    surfaceObserver?.observe(this.frame);
    this.destroy=()=>{
      if(!this.alive)return;
      this.alive=false;settleCapture(captureError('destroyed'));clearWatchdog();cancelAnimationFrame(raf);
      resizeObserver?.disconnect();surfaceObserver?.disconnect();window.removeEventListener('resize',invalidate);
      document.removeEventListener('visibilitychange',visibilityChanged);
      this.frame.style.visibility='hidden';
      send({stop:true});stopTimer=setTimeout(remove,250);
    };
    this.frame.srcdoc=animationFrameDocument(this.token);
    container.replaceChildren(this.frame);
    raf=requestAnimationFrame(draw);
  }
}

function animationCaption(value) {
  if(!record(value) || typeof value.title!=='string' || (value.description!==undefined && typeof value.description!=='string'))return null;
  return {title:value.title.slice(0,200),description:(value.description || '').slice(0,2000)};
}

function animationMetadata(value) {
  if(!Number.isFinite(value.duration) || value.duration<=0 || !Array.isArray(value.chapters) || value.chapters.length>256)return null;
  const chapters=[];
  for(const item of value.chapters){
    const caption=animationCaption(item);
    if(!caption || !Number.isFinite(item.at) || item.at<0 || item.at>=value.duration)return null;
    chapters.push({at:item.at,...caption});
  }
  return {duration:value.duration,chapters:chapters.sort((a,b)=>a.at-b.at)};
}

export function animationFrameDocument(token) {
  const worker=String.raw`
let program,ctx,canvas,model,api,width=0,height=0,ratio=1,stopping=false;
const copyPixels=self.createImageBitmap.bind(self);
const prototypeOf=Object.getPrototypeOf;
const builtinExceptions=[[Error.prototype,'Error'],[EvalError.prototype,'EvalError'],[RangeError.prototype,'RangeError'],[ReferenceError.prototype,'ReferenceError'],[SyntaxError.prototype,'SyntaxError'],[TypeError.prototype,'TypeError'],[URIError.prototype,'URIError'],[AggregateError.prototype,'AggregateError']];
const freeze=value=>{if(value&&typeof value==='object'){Object.values(value).forEach(freeze);Object.freeze(value);}return value;};
self.onmessage=async event=>{let failure={kind:'runtime',phase:'worker'};try{
  const message=event.data;
  if(message.stop){stopping=true;Promise.resolve().then(()=>program?.destroy?.()).catch(()=>{}).then(()=>{postMessage({stopped:true});close();});return;}
  if(stopping)return;
  if(message.start){
    failure={kind:'presentation',phase:'initialize'};
    canvas=new OffscreenCanvas(1,1);ctx=canvas.getContext('2d');model=freeze(message.start.model);
    api=Object.freeze({canvas,ctx,blueprint:freeze(message.start.blueprint),inspect:id=>postMessage({inspect:id}),caption:(title,description='')=>postMessage({caption:{title,description}})});
    failure={kind:'program',phase:'initialize'};
    program=new Function('model','api',message.start.code)(model,api);
    if(!program || typeof program.render!=='function' || !Number.isFinite(program.duration) || program.duration<=0)throw Error('Return a render function and positive duration.');
    const chapters=(Array.isArray(program.chapters)?program.chapters:[]).filter(item=>item&&Number.isFinite(item.at)&&item.at>=0&&item.at<program.duration&&typeof item.title==='string').map(item=>({at:item.at,title:item.title,description:typeof item.description==='string'?item.description:''})).sort((a,b)=>a.at-b.at);
    postMessage({ready:true,duration:program.duration,chapters});
  }
  if(message.draw && program){
    const frame=message.draw;
    failure={kind:'presentation',phase:'surface'};
    if(width!==frame.width || height!==frame.height || ratio!==frame.ratio){width=frame.width;height=frame.height;ratio=frame.ratio;canvas.width=Math.max(1,Math.round(width*ratio));canvas.height=Math.max(1,Math.round(height*ratio));}
    ctx.setTransform(ratio,0,0,ratio,0,0);
    const renderStarted=performance.now();
    postMessage({diagnostic:true,frameId:frame.frameId,lastStage:'worker-render-started'});
    failure={kind:'program',phase:'render'};program.render({...frame,ctx});
    const renderMs=Math.round(Math.max(0,performance.now()-renderStarted));
    postMessage({diagnostic:true,frameId:frame.frameId,lastStage:'worker-rendered',renderMs});
    failure={kind:'presentation',phase:'copy'};
    const copyStarted=performance.now();
    const bitmap=await copyPixels(canvas);
    if(stopping){bitmap.close();return;}
    const copyMs=Math.round(Math.max(0,performance.now()-copyStarted));
    postMessage({diagnostic:true,frameId:frame.frameId,lastStage:'worker-copied',renderMs,copyMs});
    postMessage({pixels:bitmap,frameId:frame.frameId,renderMs,copyMs},[bitmap]);
  }
  if(message.pointer && program?.hitTest){failure={kind:'program',phase:'inspect'};const id=program.hitTest(message.pointer.x,message.pointer.y);if(typeof id==='string')postMessage({inspect:id});}
}catch(error){
  if(failure.kind==='program')try{const prototype=prototypeOf(error);for(const [builtin,name] of builtinExceptions)if(prototype===builtin){failure={...failure,exception:name};break;}}catch{}
  if(!stopping)postMessage({error:true,failure});}};`;
  return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; worker-src blob:; connect-src 'none'; style-src 'unsafe-inline'"><style>html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#fafaf7}canvas{display:block;width:100%;height:100%}</style></head><body><canvas aria-label="Custom proposal animation"></canvas><script>
const token=${JSON.stringify(token)},canvas=document.querySelector('canvas'),ctx=canvas.getContext('2d');
const limits=${JSON.stringify(ANIMATION_CAPTURE_LIMITS)};
const failureDetail=${animationFailureDetail.toString()};
let worker,workerUrl,stopping=false,stopTimer,pendingDraw=null,pendingMetrics={},waitingDraw=null,lastDrawId=0,captureSlot=null,ready=false;
const publish=(data,transfer=[])=>parent.postMessage({token,...data},'*',transfer);
const isRecord=value=>!!value && typeof value==='object' && !Array.isArray(value);
const geometry=frame=>{
 if(!Number.isFinite(frame.width)||frame.width<=0||frame.width>limits.cssDimension||!Number.isFinite(frame.height)||frame.height<=0||frame.height>limits.cssDimension||!Number.isFinite(frame.ratio)||frame.ratio<=0||frame.ratio>2)return null;
 const pixelWidth=Math.max(1,Math.round(frame.width*frame.ratio)),pixelHeight=Math.max(1,Math.round(frame.height*frame.ratio));
 return pixelWidth<=limits.pixelDimension&&pixelHeight<=limits.pixelDimension&&pixelWidth*pixelHeight<=limits.pixels?{pixelWidth,pixelHeight}:null;
};
const diagnostic=(frame,lastStage,stageSource,extra={})=>failureDetail({kind:'timeout',phase:'draw',diagnostics:{
 frameId:frame.frameId,time:frame.time,width:frame.width,height:frame.height,ratio:frame.ratio,
 ...geometry(frame),...pendingMetrics,...extra,lastStage,stageSource}}).diagnostics;
const progress=(frame,lastStage,stageSource,extra={})=>publish({diagnostic:true,frameId:frame.frameId,
 diagnostics:diagnostic(frame,lastStage,stageSource,extra)});
const presentWaitingDraw=()=>{
 if(stopping||!waitingDraw||pendingDraw)return;
 const rect=canvas.getBoundingClientRect(),frame={...waitingDraw,width:rect.width,height:rect.height,ratio:Math.min(devicePixelRatio||1,2)};
 if(Number.isFinite(rect.width)&&Number.isFinite(rect.height)&&rect.width>=0&&rect.height>=0&&(rect.width===0||rect.height===0)){
  publish({deferred:true,frameId:frame.frameId});return;
 }
 waitingDraw=null;
 if(!geometry(frame)){if(frame.captureId)publish({captureError:true,captureId:frame.captureId,frameId:frame.frameId,reason:'bounds'});else publish({error:true,failure:{kind:'presentation',phase:'bounds'}});return;}
 pendingDraw=frame;pendingMetrics={};publish({rendering:true,frameId:frame.frameId,
  diagnostics:diagnostic(frame,'iframe-dispatched','iframe')});worker.postMessage({draw:frame});
};
const layoutObserver=typeof ResizeObserver==='function'?new ResizeObserver(presentWaitingDraw):null;
layoutObserver?.observe(canvas);addEventListener('resize',presentWaitingDraw);
const endCapture=(slot,reason,bytes)=>{
 if(captureSlot!==slot)return;
 captureSlot=null;clearTimeout(slot.timer);slot.canvas.width=1;slot.canvas.height=1;
 if(stopping)return;
 if(reason)publish({captureError:true,captureId:slot.frame.captureId,frameId:slot.frame.frameId,reason});
 else publish({captured:true,captureId:slot.frame.captureId,frameId:slot.frame.frameId,frame:slot.frame,mime:'image/png',bytes},[bytes]);
};
const captureDisplayed=frame=>{
 if(captureSlot){publish({captureError:true,captureId:frame.captureId,frameId:frame.frameId,reason:'invalid'});return;}
 const snapshot=document.createElement('canvas');snapshot.width=canvas.width;snapshot.height=canvas.height;
 const slot={frame,canvas:snapshot,timer:null};captureSlot=slot;
 slot.timer=setTimeout(()=>endCapture(slot,'timeout'),limits.encodeTimeout);
 try{
  const snapshotContext=snapshot.getContext('2d');
  snapshotContext.fillStyle='#fafaf7';snapshotContext.fillRect(0,0,snapshot.width,snapshot.height);
  snapshotContext.drawImage(canvas,0,0);
  snapshot.toBlob(async blob=>{
   if(captureSlot!==slot||stopping)return;
   if(!blob||blob.type!=='image/png'){endCapture(slot,'encoding');return;}
   if(blob.size>limits.bytes){endCapture(slot,'bounds');return;}
   try{const bytes=await blob.arrayBuffer();if(captureSlot!==slot||stopping)return;if(bytes.byteLength>limits.bytes){endCapture(slot,'bounds');return;}endCapture(slot,null,bytes);}catch{endCapture(slot,'encoding');}
  },'image/png');
 }catch{endCapture(slot,'encoding');}
};
const finishStop=()=>{clearTimeout(stopTimer);layoutObserver?.disconnect();removeEventListener('resize',presentWaitingDraw);worker?.terminate();worker=null;pendingDraw=null;waitingDraw=null;if(captureSlot){const slot=captureSlot;captureSlot=null;clearTimeout(slot.timer);slot.canvas.width=1;slot.canvas.height=1;}if(workerUrl){URL.revokeObjectURL(workerUrl);workerUrl=null;}publish({stopped:true});};
const workerMessage=event=>{
 const data=event.data;
 if(!isRecord(data))return;
 if(stopping){if(data.pixels instanceof ImageBitmap)data.pixels.close();if(data.stopped===true)finishStop();return;}
 // Authored code cannot attest presentation or provide a capture envelope.
 // Only pixels can reach the displayed canvas, which this iframe owns.
 if(data.pixels instanceof ImageBitmap){
  const bitmap=data.pixels,frame=pendingDraw,size=frame&&geometry(frame);
  if(!frame||data.frameId!==frame.frameId){bitmap.close();return;}
  if(!size||bitmap.width!==size.pixelWidth||bitmap.height!==size.pixelHeight){
   const diagnostics=diagnostic(frame,'iframe-pixels','iframe',
    {receivedPixelWidth:bitmap.width,receivedPixelHeight:bitmap.height});
   bitmap.close();publish({error:true,failure:{kind:'presentation',phase:'copy',diagnostics}});return;
  }
  progress(frame,'iframe-pixels','iframe',{receivedPixelWidth:bitmap.width,receivedPixelHeight:bitmap.height});
  if(frame.captureId){
   const rect=canvas.getBoundingClientRect(),ratio=Math.min(devicePixelRatio||1,2);
   if(rect.width!==frame.width||rect.height!==frame.height||ratio!==frame.ratio){bitmap.close();pendingDraw=null;publish({captureError:true,captureId:frame.captureId,frameId:frame.frameId,reason:'resized'});return;}
  }
  try{
   if(canvas.width!==size.pixelWidth||canvas.height!==size.pixelHeight){canvas.width=size.pixelWidth;canvas.height=size.pixelHeight;}
   ctx.setTransform(1,0,0,1,0,0);ctx.clearRect(0,0,canvas.width,canvas.height);ctx.drawImage(bitmap,0,0);
   bitmap.close();pendingDraw=null;
   // This acknowledgement follows the trusted visible-canvas paint. It does
   // not establish compositor delivery or perceptual quality.
   publish({drawn:true,frameId:frame.frameId,diagnostics:diagnostic(frame,'iframe-painted','iframe')});
   if(frame.captureId)captureDisplayed(frame);
  }catch{bitmap.close();publish({error:true,failure:{kind:'presentation',phase:'paint'}});}
  return;
 }
 // An authored worker can forge these reports. They provide bounded debugging
 // hints only and cannot attest pixels, clear busy state or extend a deadline.
 if(data.diagnostic===true){
  if(!pendingDraw || data.frameId!==pendingDraw.frameId ||
    !['worker-render-started','worker-rendered','worker-copied'].includes(data.lastStage))return;
  const safe=failureDetail({kind:'timeout',phase:'draw',diagnostics:{renderMs:data.renderMs,copyMs:data.copyMs}}).diagnostics || {};
  if(Object.keys(safe).length)pendingMetrics={...pendingMetrics,...safe,workerTimingSource:'untrusted-worker-report'};
  progress(pendingDraw,data.lastStage,'untrusted-worker-report');return;
 }
 if(data.ready===true&&!ready){ready=true;publish({ready:true,duration:data.duration,chapters:data.chapters});return;}
 if(data.error===true){
  const failure=failureDetail(data.failure);delete failure.diagnostics;
  publish({error:true,failure});return;
 }
 if(typeof data.inspect==='string')publish({inspect:data.inspect});
 if(isRecord(data.caption))publish({caption:{title:data.caption.title,description:data.caption.description}});
};
addEventListener('message',event=>{
 if(event.source!==parent || event.data?.token!==token || !isRecord(event.data))return;
 const data=event.data;
 if(data.start && !worker && !stopping){
  workerUrl=URL.createObjectURL(new Blob([${JSON.stringify(worker)}],{type:'text/javascript'}));
  worker=new Worker(workerUrl);worker.onmessage=workerMessage;worker.onerror=()=>{if(stopping)finishStop();else publish({error:true,failure:{kind:'runtime',phase:'worker'}});};
  worker.postMessage({start:data.start});
 }
 if(data.draw && worker && ready && !stopping && !pendingDraw && !waitingDraw){
  const draw=data.draw;
  if(!isRecord(draw)||!Number.isSafeInteger(draw.frameId)||draw.frameId<=lastDrawId||!Number.isFinite(draw.time)||draw.time<0||!Number.isFinite(draw.progress)||draw.progress<0||draw.progress>1||typeof draw.playing!=='boolean'||typeof draw.showTools!=='boolean'||typeof draw.reducedMotion!=='boolean'||draw.selectedId!==null&&typeof draw.selectedId!=='string'||draw.captureId!==undefined&&(typeof draw.captureId!=='string'||!draw.captureId.length||draw.captureId.length>64))return;
  lastDrawId=draw.frameId;waitingDraw={...draw};presentWaitingDraw();
 }
 if(typeof data.cancelCapture==='string' && !stopping){
  if(waitingDraw?.captureId===data.cancelCapture)delete waitingDraw.captureId;
  if(pendingDraw?.captureId===data.cancelCapture)delete pendingDraw.captureId;
  if(captureSlot?.frame.captureId===data.cancelCapture){const slot=captureSlot;captureSlot=null;clearTimeout(slot.timer);slot.canvas.width=1;slot.canvas.height=1;}
 }
 if(data.stop && !stopping){stopping=true;if(worker){worker.postMessage({stop:true});stopTimer=setTimeout(finishStop,150);}else finishStop();}
});
addEventListener('pagehide',()=>{stopping=true;finishStop();});
addEventListener('keydown',event=>{if(!stopping && event.key==='Escape'){event.preventDefault();publish({escape:true});}});
canvas.addEventListener('click',event=>{if(stopping)return;const rect=canvas.getBoundingClientRect();worker?.postMessage({pointer:{x:event.clientX-rect.left,y:event.clientY-rect.top}});});
publish({boot:true});
</script></body></html>`;
}
