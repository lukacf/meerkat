import { ANIMATION_RENDERER_HASH } from './animation-review-renderer-hash.mjs';

const digest=value=>typeof value==='string' && /^[a-f0-9]{64}$/.test(value);
const record=value=>!!value && typeof value==='object' && !Array.isArray(value);
const BINDING_FIELDS=['id','setup_id','stream_id','source_generation','request_fingerprint','native_request_key',
  'blueprint_hash','model_hash','reference_hash','code_hash','revision_binding'];
const PARENT_FIELDS=['operation_id','setup_id','stream_id','code_hash','blueprint_hash','model_hash'];
const copy=value=>structuredClone(value);
const CAPTURE_FAILURES=Object.freeze({
  unavailable:'The animation was not ready for a visible frame capture.',
  busy:'Another visible frame capture was still in progress.',
  invalid:'The visible frame capture could not be validated.',
  time:'The visible frame capture requested an invalid animation time.',
  timeout:'The visible frame capture timed out.',
  encoding:'The visible frame capture could not be encoded.',
  bounds:'The visible frame capture exceeded its supported size.',
  destroyed:'The animation stopped before its visible frame capture completed.',
  resized:'The animation resized before its visible frame capture completed.',
  cancelled:'The visible frame capture was cancelled.',
});
function reviewFailureDetail(code) {
  if(typeof code==='string' && code.startsWith('capture_'))return CAPTURE_FAILURES[code.slice(8)] || 'The visible frame capture failed.';
  return code==='frame_invalid' ? 'The captured visible frame could not be prepared for review.'
    : code==='review_limit' ? 'Automatic animation checks have reached their limit.'
    : code==='review_unavailable' ? 'The visual review request could not be completed.':'';
}

function canonical(value) {
  if(Array.isArray(value))return value.map(canonical);
  if(record(value))return Object.fromEntries(Object.keys(value).sort().map(key=>[key,canonical(value[key])]));
  return value;
}

export function animationReviewBindingMatches(actual,expected) {
  if(!record(actual) || !record(expected))return false;
  return BINDING_FIELDS.every(key=>JSON.stringify(canonical(actual[key]))===JSON.stringify(canonical(expected[key])));
}

export function animationReviewOwner(candidate,currentRequest,setupId) {
  const receipt=candidate?.receipt,artifact=candidate?.artifact,request=candidate?.request;
  if(!record(receipt) || artifact?.contract!=='freeform-animation-v3' || !request || !currentRequest ||
      candidate.key!==currentRequest.proposalKey || candidate.streamId!==currentRequest.streamId ||
      request.streamId!==currentRequest.streamId || request.proposalKey!==currentRequest.proposalKey ||
      !Number.isSafeInteger(request.attempt) || request.attempt<0 || request.attempt!==currentRequest.attempt ||
      (receipt.setup_id ?? null)!==(setupId ?? null) || receipt.stream_id!==candidate.streamId ||
      !Number.isSafeInteger(receipt.source_generation) || receipt.source_generation<=0)return null;
  for(const key of ['id','request_fingerprint','native_request_key'])if(!digest(receipt[key]))return null;
  for(const key of ['blueprint_hash','model_hash','reference_hash','code_hash'])if(!digest(artifact[key]))return null;
  if(receipt.blueprint_hash!==artifact.blueprint_hash || receipt.model_hash!==artifact.model_hash)return null;
  const binding={id:receipt.id,setup_id:receipt.setup_id ?? null,stream_id:receipt.stream_id,source_generation:receipt.source_generation,
    request_fingerprint:receipt.request_fingerprint,native_request_key:receipt.native_request_key,
    blueprint_hash:artifact.blueprint_hash,model_hash:artifact.model_hash,reference_hash:artifact.reference_hash,
    code_hash:artifact.code_hash,revision_binding:copy(receipt.revision_binding ?? null)};
  const owner={binding,request:copy(request),sourceKey:JSON.stringify([binding.setup_id,binding.stream_id,request.proposalKey]),
    blueprint:copy(candidate.plan),model:copy(candidate.model)};
  owner.key=JSON.stringify([canonical(binding),owner.request]);
  return owner;
}

export function animationReviewCorrectionMatches(advisory,result,owner) {
  const parent=advisory?.expected_parent,binding=owner?.binding;
  if(!record(parent) || !binding || !record(result) || result.current!==true || result.status!=='reviewed' ||
      result.report?.verdict!=='needs_revision' || !animationReviewBindingMatches(result.binding,binding) ||
      !digest(result.id) || advisory.review_id!==result.id || advisory.eligible_now!==true ||
      advisory.requires_atomic_expected_parent_admission!==true || typeof advisory.feedback!=='string' ||
      !advisory.feedback.trim() || advisory.feedback.length>1000 || Object.keys(parent).length!==PARENT_FIELDS.length)return false;
  const expected={operation_id:binding.id,setup_id:binding.setup_id,stream_id:binding.stream_id,code_hash:binding.code_hash,
    blueprint_hash:binding.blueprint_hash,model_hash:binding.model_hash};
  return PARENT_FIELDS.every(key=>Object.is(parent[key],expected[key]));
}

/** One automatic correction per current source/view, including replacement code. */
export function createAnimationReviewBudget() {
  let current=null,used=false;
  return {claim(owner){if(owner.sourceKey!==current){current=owner.sourceKey;used=false;}if(used)return false;used=true;return true;}};
}

/** A changed presentation cannot regain ownership by returning to its old values. */
export function createAnimationReviewEpoch() {
  let epoch=0,view=null;
  return {get current(){return epoch;},invalidate(){return ++epoch;},observe(next){
    const key=JSON.stringify(next);
    if(view!==null && view!==key)epoch++;
    view=key;return epoch;
  }};
}

function samePresentation(a,b) {
  return ['width','height','ratio','selectedId','showTools','playing','reducedMotion'].every(key=>Object.is(a[key],b[key]));
}

export async function animationReviewFrame(packet) {
  if(!record(packet) || !(packet.bytes instanceof ArrayBuffer) || packet.bytes.byteLength<33 || packet.bytes.byteLength>1500000 ||
      !Number.isSafeInteger(packet.frameId) || packet.frameId<=0 || !Number.isFinite(packet.width) || packet.width<1 || packet.width>4096 ||
      !Number.isFinite(packet.height) || packet.height<1 || packet.height>4096 || !Number.isFinite(packet.ratio) || packet.ratio<.25 || packet.ratio>2 ||
      Math.round(packet.width*packet.ratio)*Math.round(packet.height*packet.ratio)>8000000 ||
      !Number.isFinite(packet.time) || packet.time<0 || !Number.isFinite(packet.progress) || packet.progress<0 || packet.progress>1 ||
      typeof packet.playing!=='boolean' || typeof packet.reducedMotion!=='boolean' || typeof packet.showTools!=='boolean' ||
      packet.selectedId!==null && typeof packet.selectedId!=='string' || packet.mime!=='image/png')throw new Error('Invalid rendered-scene evidence.');
  const hash=new Uint8Array(await crypto.subtle.digest('SHA-256',packet.bytes));
  const bytes=new Uint8Array(packet.bytes);let binary='';
  for(let offset=0;offset<bytes.length;offset+=8192)binary+=String.fromCharCode(...bytes.subarray(offset,offset+8192));
  return {id:String(packet.frameId),width:packet.width,height:packet.height,pixel_ratio:packet.ratio,time:packet.time,
    selected_id:packet.selectedId,show_tools:packet.showTools,reduced_motion:packet.reducedMotion,playing:packet.playing,
    progress:packet.progress,runtime_activity:copy(packet.runtimeActivity ?? null),png_base64:btoa(binary),
    png_hash:[...hash].map(value=>value.toString(16).padStart(2,'0')).join('')};
}

export function animationReviewPresentation(review) {
  const labels={sampling:'Checking the visible animation...',submitting:'Checking the visible animation...',reviewing:'Checking the visible animation...',
    acceptable:'Sampled animation looks clear',needs_revision:'The animation needs a visual revision',correcting:'Refining the animation after its visual check...',
    inconclusive:'Animation quality not confirmed',unavailable:'Automatic animation check unavailable'};
  if(!review || !Object.hasOwn(labels,review.status))return null;
  const coverage=review.sampleCount>1 && Number.isFinite(review.sampledSeconds) && Number.isFinite(review.duration)
    ? `${review.sampleCount} frames across ${Math.round(review.sampledSeconds)}s of a ${Math.round(review.duration)}s loop. `:'';
  const failure=reviewFailureDetail(review.failureCode);
  return {label:labels[review.status],detail:review.status==='inconclusive' || review.status==='unavailable'
    ? `${failure ? failure+' ':''}Your animation remains available. If something looks unclear, use Refine animation to describe it.`
    : review.status==='acceptable' || review.status==='needs_revision'
      ? `${review.summary ? review.summary.slice(0,280)+' ':''}${coverage}Only the sampled visible scene was checked.${review.status==='needs_revision' ? ' Use Refine animation to explain what should be clearer.':''}`:''};
}

/** Observe natural playback only. No seek, pause, expansion, or guide mutation. */
export function createAnimationReview({owner,animation,isCurrent,fetcher=fetch,rendererHash=ANIMATION_RENDERER_HASH,
  onStatus=()=>{},onCorrection=()=>false,budget=createAnimationReviewBudget(),setTimer=setTimeout,clearTimer=clearTimeout,
  now=()=>Date.now(),sampleDelay,pollDelay=1000,pollTimeout=180000,timeout=270000}) {
  let cancelled=false,started=false,capturing=false,waiting=null,phase='review';
  const controller=new AbortController();
  const owned=()=>!cancelled && animation.alive!==false && isCurrent(owner);
  const status=(value,extra={})=>{if(owned())onStatus({status:value,ownerKey:owner.key,...extra});};
  const wait=delay=>new Promise(resolve=>{
    const entry={resolve,timer:null};waiting=entry;
    entry.timer=setTimer(()=>{if(waiting===entry)waiting=null;resolve(owned());},delay);
  });
  const json=async(url,options={})=>{
    const response=await fetcher(url,{...options,signal:controller.signal});
    if(!response.ok)throw Object.assign(new Error('Visual review unavailable.'),
      {code:response.status===429 ? 'review_limit':'review_unavailable'});
    return response.json();
  };
  const cancel=()=>{
    if(cancelled)return;cancelled=true;controller.abort();
    if(waiting){const entry=waiting;waiting=null;clearTimer(entry.timer);entry.resolve(false);}
    if(capturing)animation.cancelCapture?.();
  };
  const start=async()=>{
    if(started || !owned())return;started=true;
    const deadlineTimer=setTimer(()=>{status('inconclusive');cancel();},timeout);
    try{
      const capability=await json('/api/visualization-review-capability');
      if(!owned())return;
      if(capability?.available!==true || capability.renderer_hash!==rendererHash){status('unavailable');return;}
      status('sampling');let frames=[],packets=[];
      const duration=animation.duration;
      if(!Number.isFinite(duration) || duration<=0){status('inconclusive');return;}
      // Four observations spread across natural playback. Long custom loops
      // retain a bounded observation window; their unseen remainder is not checked.
      const delay=sampleDelay ?? Math.min(20000,Math.max(100,duration*300));
      let observedSpan=0;
      for(let index=0;index<4;index++){
        if(index && !await wait(delay))return;
        if(!owned())return;
        phase='capture';capturing=true;const packet=await animation.capture();capturing=false;
        if(!owned())return;
        phase='frame';const frame=await animationReviewFrame(packet);if(!owned())return;
        if(packets.length && !samePresentation(packets[0],packet)){status('inconclusive');return;}
        if(!packet.playing || packet.reducedMotion){frames=[frame];packets=[packet];break;}
        if(packets.some(previous=>Math.abs(previous.time-packet.time)<.02)){status('inconclusive');return;}
        if(packets.length){
          const previous=packets.at(-1);
          const advanced=(packet.time-previous.time+duration)%duration;
          // A hidden or throttled tab must not turn three wall-clock waits
          // into a claim that later portions of the animation were observed.
          if(advanced+.05<delay/1000*.8){status('inconclusive');return;}
          observedSpan+=advanced;
        }
        frames.push(frame);packets.push(packet);
      }
      if(!owned())return;
      const evidence={sampleCount:frames.length,sampledSeconds:observedSpan,duration};
      status('submitting',evidence);
      phase='review';
      let result=await json(`/api/visualizations/${owner.binding.id}/review`,{method:'POST',headers:{'Content-Type':'application/json'},
        body:JSON.stringify({binding:owner.binding,blueprint:owner.blueprint,model:owner.model,frames,renderer_hash:rendererHash})});
      const deadline=now()+pollTimeout;
      while(owned()){
        if(!digest(result?.id) || !animationReviewBindingMatches(result.binding,owner.binding) || result.renderer_hash!==rendererHash){status('inconclusive');return;}
        if(result.current!==true){status('inconclusive');return;}
        if(result.status==='inconclusive'){status('inconclusive');return;}
        if(result.status==='reviewed'){
          const verdict=result.report?.verdict;
          if(!['acceptable','needs_revision','inconclusive'].includes(verdict)){status('inconclusive');return;}
          status(verdict,{...evidence,summary:typeof result.report.summary==='string'?result.report.summary.slice(0,280):''});
          const advisory=result.correction_advisory;
          if(verdict==='needs_revision' && animationReviewCorrectionMatches(advisory,result,owner) && owned() && budget.claim(owner)){
            if(onCorrection(copy(advisory),owner)===true)status('correcting',evidence);
          }
          return;
        }
        if(result.status!=='reviewing' || now()>=deadline){status('inconclusive');return;}
        status('reviewing',evidence);if(!await wait(pollDelay))return;
        result=await json(`/api/visualizations/${owner.binding.id}/reviews/${result.id}`);
      }
    }catch(error){
      const failureCode=phase==='capture' ? `capture_${Object.hasOwn(CAPTURE_FAILURES,error?.code) ? error.code:'failed'}`
        : phase==='frame' ? 'frame_invalid':error?.code==='review_limit' ? 'review_limit':'review_unavailable';
      capturing=false;if(owned())status('inconclusive',{failureCode});
    }
    finally{clearTimer(deadlineTimer);}
  };
  return {start,cancel};
}
