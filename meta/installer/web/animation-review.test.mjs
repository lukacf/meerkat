import { setTimeout as realDelay } from 'node:timers/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { animationReviewOwner,animationReviewBindingMatches,animationReviewCorrectionMatches,animationReviewFrame,
  createAnimationReview,createAnimationReviewBudget,createAnimationReviewEpoch,animationReviewPresentation } from './animation-review.mjs';
import { ANIMATION_RENDERER_HASH } from './animation-review-renderer-hash.mjs';

const hash=letter=>letter.repeat(64),rendererHash=hash('f');
const request={streamId:'setup:__overview__',proposalKey:'exact proposal source',attempt:0};
function fixture(codeHash=hash('c')) {
  const artifact={contract:'freeform-animation-v3',blueprint_hash:hash('a'),model_hash:hash('b'),reference_hash:hash('d'),code_hash:codeHash};
  const receipt={id:hash('1'),setup_id:'setup',stream_id:request.streamId,source_generation:1,
    request_fingerprint:hash('2'),native_request_key:hash('3'),blueprint_hash:artifact.blueprint_hash,model_hash:artifact.model_hash,revision_binding:null};
  const candidate={key:request.proposalKey,streamId:request.streamId,request,receipt,artifact,plan:{project:{name:'Guide source'}},model:{nodes:[]}};
  return {candidate,owner:animationReviewOwner(candidate,request,'setup')};
}
function packet(index=1,{playing=false,reduced=false,time=index,...extras}={}) {
  const bytes=new ArrayBuffer(40),prefix=new Uint8Array(bytes),view=new DataView(bytes);
  prefix.set([137,80,78,71,13,10,26,10]);view.setUint32(8,13);prefix.set([73,72,68,82],12);view.setUint32(16,1200);view.setUint32(20,680);
  return {captureId:`capture-${index}`,frameId:index,time,progress:time/40,playing,reducedMotion:reduced,selectedId:null,showTools:false,
    runtimeActivity:{scope:'native-design-rehearsal',activeNodeIds:[]},width:600,height:340,ratio:2,pixelWidth:1200,pixelHeight:680,mime:'image/png',bytes,...extras};
}
function result(owner,verdict='acceptable') {
  const value={id:hash('4'),status:'reviewed',binding:structuredClone(owner.binding),renderer_hash:rendererHash,current:true,report:{verdict,summary:'Readable sampled scene'}};
  if(verdict==='needs_revision')value.correction_advisory={review_id:value.id,feedback:'Keep the reference fine lines readable.',eligible_now:true,
    requires_atomic_expected_parent_admission:true,expected_parent:{operation_id:owner.binding.id,setup_id:owner.binding.setup_id,stream_id:owner.binding.stream_id,
      code_hash:owner.binding.code_hash,blueprint_hash:owner.binding.blueprint_hash,model_hash:owner.binding.model_hash}};
  return value;
}
const settle=()=>new Promise(resolve=>setImmediate(resolve));
function controllerFixture({owner=fixture().owner,playing=false,reduced=false,verdict='acceptable',capability={available:true,renderer_hash:rendererHash},
  final,fetcher:customFetcher,capture:customCapture,budget,duration=40,sampleDelay=100,isCurrent,onTimer}={}) {
  const timers=new Map(),calls=[],statuses=[],corrections=[],captures=[];let next=0,current=true;
  const animation={alive:true,duration,paused:!playing,reducedMotion:reduced,
    capture:customCapture || (async()=>{const value=packet(captures.length+1,{playing,reduced});captures.push(value);return value;}),
    cancelCapture(){animation.cancelledCapture=true;},seek(){throw Error('Review must never seek.');}};
  const fetcher=customFetcher || (async(url,options={})=>{
    calls.push({url,options});return {ok:true,json:async()=>url==='/api/visualization-review-capability'?capability:final || result(owner,verdict)};
  });
  const review=createAnimationReview({owner,animation,isCurrent:()=>current && (isCurrent?.() ?? true),rendererHash,fetcher,budget,
    setTimer:(callback,delay)=>{const id=++next;timers.set(id,{callback,delay});onTimer?.(delay);return id;},clearTimer:id=>timers.delete(id),
    onStatus:value=>statuses.push(value),onCorrection:value=>{corrections.push(value);return true;},sampleDelay});
  const tick=async(delay=100)=>{let entry;for(const deadline=performance.now()+5000;performance.now()<deadline&&!entry;){entry=[...timers].find(([,value])=>value.delay===delay);if(!entry)await realDelay(1);}assert.ok(entry,`One ${delay}ms timer`);
    timers.delete(entry[0]);entry[1].callback();await settle();};
  return {owner,animation,review,timers,calls,statuses,corrections,captures,tick,stale:()=>{current=false;}};
}

test('renderer capability constant pins the actual candidate source bytes',async()=>{
  const source=await readFile(new URL('freeform-animation.mjs',import.meta.url));
  assert.equal(ANIMATION_RENDERER_HASH,createHash('sha256').update(source).digest('hex'));
});

test('review owner freezes exact operation/code/request source and rejects mismatched setup/view/retry',()=>{
  const {candidate,owner}=fixture();candidate.plan.project.name='Changed after capture owner';assert.equal(owner.blueprint.project.name,'Guide source');
  for(const current of [{...request,streamId:'setup:other'},{...request,proposalKey:'other proposal'},{...request,attempt:1}])
    assert.equal(animationReviewOwner(candidate,current,'setup'),null);
  assert.equal(animationReviewOwner(candidate,request,'other-setup'),null);
  const other=structuredClone(owner.binding);other.revision_binding={review_id:hash('9')};assert.equal(animationReviewBindingMatches(other,owner.binding),false);
});

test('frame payload matches the literal backend contract and hashes exact PNG bytes without live caption',async()=>{
  const source=packet(),frame=await animationReviewFrame({...source,caption:{title:'Unrelated latest caption'}});
  assert.deepEqual(Object.keys(frame).sort(),['id','width','height','pixel_ratio','time','selected_id','show_tools','reduced_motion','playing','progress','runtime_activity','png_base64','png_hash'].sort());
  assert.equal(frame.id,'1');assert.equal(typeof frame.id,'string');assert.equal(frame.png_hash,createHash('sha256').update(new Uint8Array(source.bytes)).digest('hex'));
  assert.deepEqual(Buffer.from(frame.png_base64,'base64'),Buffer.from(source.bytes));assert.equal(frame.pixel_ratio,2);
  await assert.rejects(animationReviewFrame(packet(1,{bytes:new ArrayBuffer(1500001)})));
});

test('unavailable or mismatched renderer capability never captures or submits a review',async()=>{
  for(const capability of [{available:false,renderer_hash:rendererHash},{available:true,renderer_hash:hash('e')}]){
    const ui=controllerFixture({capability});await ui.review.start();assert.equal(ui.captures.length,0);assert.equal(ui.calls.length,1);
    assert.equal(ui.statuses.at(-1).status,'unavailable');assert.equal(ui.timers.size,0);
  }
});

test('capture failures expose only bounded codes and safe visible details',async()=>{
  for(const code of ['timeout','resized','encoding','invalid','bounds','private_provider_failure']){
    const ui=controllerFixture({capture:async()=>{throw Object.assign(new Error('PRIVATE_CAPTURE_DETAIL'),{code});}});
    await ui.review.start();
    const failure=ui.statuses.at(-1);
    assert.equal(failure.status,'inconclusive');
    assert.equal(failure.failureCode,code==='private_provider_failure' ? 'capture_failed':`capture_${code}`);
    assert.equal(ui.calls.length,1);assert.equal(ui.animation.alive,true);
    const presentation=animationReviewPresentation(failure);
    assert.match(presentation.detail,/visible frame capture/);
    assert.doesNotMatch(JSON.stringify([failure,presentation]),/PRIVATE_CAPTURE_DETAIL|private_provider_failure/);
  }
});

test('invalid captured frames identify frame preparation without publishing private data',async()=>{
  const ui=controllerFixture({capture:async()=>packet(1,{bytes:null,privateDetail:'PRIVATE_FRAME_DETAIL'})});
  await ui.review.start();const failure=ui.statuses.at(-1);
  assert.equal(failure.failureCode,'frame_invalid');assert.equal(ui.calls.length,1);
  assert.match(animationReviewPresentation(failure).detail,/could not be prepared for review/);
  assert.doesNotMatch(JSON.stringify(failure),/PRIVATE_FRAME_DETAIL/);
});

test('review admission limits produce safe specific wording without extra attempts',async()=>{
  const calls=[];
  const ui=controllerFixture({fetcher:async(url,options)=>{
    calls.push({url,options});return url==='/api/visualization-review-capability'
      ? {ok:true,json:async()=>({available:true,renderer_hash:rendererHash})}
      : {ok:false,status:429,json:async()=>{throw Error('PRIVATE_RESPONSE_DETAIL');}};
  }});
  await ui.review.start();const failure=ui.statuses.at(-1);
  assert.equal(failure.status,'inconclusive');assert.equal(failure.failureCode,'review_limit');
  assert.equal(calls.length,2);assert.equal(ui.corrections.length,0);
  assert.match(animationReviewPresentation(failure).detail,/animation checks have reached their limit/);
  assert.doesNotMatch(JSON.stringify(failure),/PRIVATE_RESPONSE_DETAIL/);
});

test('playing review samples four naturally spaced current frames and carries exact source payload',async()=>{
  const ui=controllerFixture({playing:true}),source=structuredClone(ui.owner.blueprint),pending=ui.review.start();await settle();
  assert.equal(ui.captures.length,1);await ui.tick();assert.equal(ui.captures.length,2);await ui.tick();await ui.tick();await pending;
  const post=ui.calls.find(call=>call.options.method==='POST');assert.ok(post);const body=JSON.parse(post.options.body);
  assert.equal(post.url,`/api/visualizations/${ui.owner.binding.id}/review`);assert.equal(body.frames.length,4);
  assert.deepEqual(body.frames.map(frame=>[frame.id,frame.time,frame.playing]),[['1',1,true],['2',2,true],['3',3,true],['4',4,true]]);
  assert.deepEqual(body.binding,ui.owner.binding);assert.deepEqual(body.blueprint,source);assert.equal(body.renderer_hash,rendererHash);
  assert.equal(ui.animation.paused,false);assert.equal(ui.statuses.at(-1).status,'acceptable');assert.equal(ui.timers.size,0);
});

test('default sampling reaches later scenes and preserves normal playback through loop wrap',async()=>{
  let count=0;
  const ui=controllerFixture({playing:true,duration:40,sampleDelay:null,
    capture:async()=>packet(++count,{playing:true,time:(30+(count-1)*12)%40})});
  const pending=ui.review.start();await settle();
  for(let index=0;index<3;index++)await ui.tick(12000);
  await pending;
  const body=JSON.parse(ui.calls.find(call=>call.options.method==='POST').options.body);
  assert.deepEqual(body.frames.map(frame=>frame.time),[30,2,14,26]);
  assert.equal(ui.statuses.at(-1).sampledSeconds,36);
  assert.equal(ui.animation.paused,false);
});

test('very long authored loops retain a bounded sampling window and literal limited coverage',async()=>{
  let count=0;
  const ui=controllerFixture({playing:true,duration:300,sampleDelay:null,
    capture:async()=>packet(++count,{playing:true,time:(count-1)*20,progress:(count-1)*20/300})});
  const pending=ui.review.start();await settle();
  for(let index=0;index<3;index++)await ui.tick(20000);
  await pending;
  const evidence=ui.statuses.at(-1);
  assert.equal(evidence.sampleCount,4);assert.equal(evidence.sampledSeconds,60);assert.equal(evidence.duration,300);
  assert.match(animationReviewPresentation(evidence).detail,/Only the sampled visible scene/);
});

test('hidden or throttled playback cannot submit opening frames as later-scene coverage',async()=>{
  let count=0;
  const ui=controllerFixture({playing:true,duration:40,sampleDelay:null,
    capture:async()=>packet(++count,{playing:true,time:count*.1})});
  const pending=ui.review.start();await settle();await ui.tick(12000);await pending;
  assert.equal(ui.calls.filter(call=>call.options.method==='POST').length,0);
  assert.equal(ui.statuses.at(-1).status,'inconclusive');assert.equal(ui.timers.size,0);
});

test('a changed presentation cannot submit a mixed batch even if its proposal stays current',async()=>{
  for(const changed of [{width:601},{ratio:1},{selectedId:'agent-a'},{showTools:true},{playing:false},{reducedMotion:true}]){
    let count=0;
    const ui=controllerFixture({playing:true,capture:async()=>packet(++count,{playing:true,...(count===2?changed:{})})});
    const pending=ui.review.start();await settle();await ui.tick();await pending;
    assert.equal(ui.calls.filter(call=>call.options.method==='POST').length,0);
    assert.equal(ui.statuses.at(-1).status,'inconclusive');
  }
});

test('presentation reversal and explicit replay retire a review owner permanently',{timeout:5000},async()=>{
  const epoch=createAnimationReviewEpoch();epoch.observe(['playing','unselected']);
  const frozen=epoch.current;let sampleScheduled;
  const ready=new Promise(resolve=>{sampleScheduled=resolve;});
  const ui=controllerFixture({playing:true,isCurrent:()=>epoch.current===frozen,onTimer:delay=>{if(delay===100)sampleScheduled();}});
  const pending=ui.review.start();await ready;
  assert.equal(ui.captures.length,1,'The original current frame has completed before the presentation reverses');
  epoch.observe(['playing','selected']);epoch.observe(['playing','unselected']);
  assert.notEqual(epoch.current,frozen);await ui.tick();await pending;
  assert.equal(ui.calls.filter(call=>call.options.method==='POST').length,0);
  const beforeReplay=epoch.current;epoch.invalidate();assert.notEqual(epoch.current,beforeReplay);
});

test('view invalidation after submission suppresses late verdicts and automatic corrections',{timeout:5000},async()=>{
  const owner=fixture().owner,epoch=createAnimationReviewEpoch();epoch.observe([600,340]);
  const frozen=epoch.current;let resolvePost,postReceived;
  const submitted=new Promise(resolve=>{postReceived=resolve;});
  const ui=controllerFixture({owner,isCurrent:()=>epoch.current===frozen,fetcher:async(url)=>({ok:true,json:async()=>{
    if(url==='/api/visualization-review-capability')return {available:true,renderer_hash:rendererHash};
    return new Promise(resolve=>{resolvePost=resolve;postReceived();});
  }})});
  const pending=ui.review.start();await submitted;assert.equal(typeof resolvePost,'function');
  epoch.invalidate();resolvePost(result(owner,'needs_revision'));await pending;
  assert.equal(ui.corrections.length,0);assert.equal(ui.statuses.at(-1).status,'submitting');
  assert.equal(ui.timers.size,0);
});

test('paused and reduced-motion scenes produce one static sample without control changes',async()=>{
  for(const config of [{playing:false},{playing:true,reduced:true}]){
    const ui=controllerFixture(config);await ui.review.start();assert.equal(ui.captures.length,1);
    assert.equal(JSON.parse(ui.calls.find(call=>call.options.method==='POST').options.body).frames.length,1);
    assert.equal(ui.animation.paused,!config.playing);assert.equal(ui.animation.reducedMotion,!!config.reduced);assert.equal(ui.timers.size,0);
  }
});

test('a frozen playing scene is inconclusive without submitting duplicate-time motion evidence',async()=>{
  const ui=controllerFixture({playing:true,capture:async()=>packet(1,{playing:true,time:0})}),pending=ui.review.start();await settle();await ui.tick();await pending;
  assert.equal(ui.statuses.at(-1).status,'inconclusive');assert.equal(ui.calls.length,1);assert.equal(ui.corrections.length,0);
});

test('cancel on source change aborts in-flight capture and cannot submit a late sample',async()=>{
  let release;
  const ui=controllerFixture({capture:()=>new Promise(resolve=>release=resolve)}),pending=ui.review.start();await settle();
  ui.stale();ui.review.cancel();assert.equal(ui.animation.cancelledCapture,true);release(packet());await pending;
  assert.equal(ui.calls.length,1);assert.equal(ui.corrections.length,0);assert.equal(ui.timers.size,0);
});

test('stale poll result cannot apply correction or update a newly selected source',async()=>{
  const owner=fixture().owner;let release,calls=0;
  const ui=controllerFixture({owner,fetcher:async url=>{
    calls++;if(url==='/api/visualization-review-capability')return {ok:true,json:async()=>({available:true,renderer_hash:rendererHash})};
    if(calls===2)return {ok:true,json:async()=>({...result(owner),status:'reviewing',report:null})};
    return new Promise(resolve=>release=()=>resolve({ok:true,json:async()=>result(owner,'needs_revision')}));
  }}),pending=ui.review.start();await settle();await ui.tick(1000);ui.stale();ui.review.cancel();release();await pending;
  assert.equal(ui.corrections.length,0);assert.equal(ui.statuses.at(-1).status,'reviewing');
});

test('automatic correction requires exact saved advisory and occurs once across replacement code for the source/view',async()=>{
  const budget=createAnimationReviewBudget();
  const first=controllerFixture({verdict:'needs_revision',budget});await first.review.start();assert.equal(first.corrections.length,1);
  const second=controllerFixture({owner:fixture(hash('e')).owner,verdict:'needs_revision',budget});await second.review.start();
  assert.equal(second.corrections.length,0);assert.equal(second.statuses.at(-1).status,'needs_revision');
  const changed=structuredClone(first.owner);changed.sourceKey='new source/view';assert.equal(budget.claim(changed),true);
  const response=result(first.owner,'needs_revision');response.correction_advisory.expected_parent.code_hash=hash('a');
  assert.equal(animationReviewCorrectionMatches(response.correction_advisory,response,first.owner),false);
});

test('inconclusive, mismatched binding, noncurrent result and unavailable helper do not retry or correct',async()=>{
  const owner=fixture().owner;
  const cases=[{...result(owner),status:'inconclusive',helper_state:'result_unknown'},
    {...result(owner,'needs_revision'),current:false},{...result(owner),binding:{...owner.binding,code_hash:hash('e')}}];
  for(const final of cases){const ui=controllerFixture({owner,final});await ui.review.start();assert.equal(ui.statuses.at(-1).status,'inconclusive');
    assert.equal(ui.calls.length,2);assert.equal(ui.corrections.length,0);assert.equal(ui.animation.alive,true);assert.equal(ui.timers.size,0);}
  assert.match(animationReviewPresentation({status:'acceptable',summary:'Readable'}).detail,/Only the sampled visible scene/);
});
