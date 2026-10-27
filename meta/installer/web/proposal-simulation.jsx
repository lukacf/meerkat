import React, { useEffect, useMemo, useRef, useState } from 'react';
import { createSimulation, simulationSourceKey, trialActivityStatus } from './simulation-state.mjs';
import { createSimulationScene, simulationViewport, smallTeamPreview } from './simulation-scene.mjs';
import { MobSim } from './mob-sim-renderer.mjs';
import { IDENTITY_PAGE_SIZE, populationInspection } from './population-inspection.mjs';
import { proposalClassChanges } from './proposal-changes.mjs';
import { createAnimationConsumer, withAnimationIntent, isStaleAnimationIntent, animationIntentError } from './animation-consumer.mjs';
import { applyVisualizationLayout, runVisualizationCode } from './visualization-code.mjs';
import { composeTopology } from './topology-layout.mjs';
import { FreeformAnimation, animationFailureDetail } from './freeform-animation.mjs';
import { animationReviewOwner,createAnimationReview,createAnimationReviewBudget,createAnimationReviewEpoch,animationReviewPresentation } from './animation-review.mjs';
import { fingerprint } from './state.mjs';
import { hasPublishedAnimation, shouldObserveAnimationTail, checkedAnimationFeedback, animationRequestBody,
  pendingAnimationNotice, shouldReattachAnimationPoll, readAnimationPreferenceRequest, saveAnimationPreference,
  saveAnimationCorrectionPreference, animationCorrectionParentMatches,
  animationObservationUnavailable,
  animationRequestMatches, animationFailure, animationGenerationStatus, animationAdmissionMatches,
  acknowledgeAnimationRefinement } from './animation-publication.mjs';
import { animationDraftSelection, createAnimationDraftStore, animationSessionStorage } from './animation-drafts.mjs';
import { readAnimationRecovery, readAnimationSelection, saveAnimationRecovery, forgetAnimationRecovery, recoveredAnimation, recoveredAnimationSelection } from './animation-recovery.mjs';
import { animationActivityScope, animationActivityKey, createAnimationActivityLog, readAnimationActivity,
  animationActivityPresentation, animationActivityConversation } from './animation-activity.mjs';
import { ConversationTranscript } from '../../packages/console-components/src/conversation/conversation-transcript';
import { Icon as ConsoleIcon, SpriteSheet } from '../../console/src/icon';
import './proposal-simulation.css';
import { NativeBundleSelection } from './native-bundle-selection.jsx';

const CANVAS_REPAIR_NOTICE='The custom animation could not run. Asking the animation agent to repair this scene.';
const VISUAL_REVIEW_SETTLE_MS=300;

// Provisional UI copy is separate from the animation agent's source model.
// Improving this caption must not invalidate a saved custom animation.
function provisionalCaption(phase,interfaceContext) {
  if(phase?.id==='conversation-entry')return interfaceContext?.status==='confirmed'
    ? 'Your confirmed interface requirements describe how people will use this application. This preview illustrates the design; it does not verify a working conversation route.'
    : 'The human interface is not confirmed. The proposal may describe who you would talk to, but a working conversation route still needs to be established.';
  if(phase?.id==='conversation:response')return 'Your proposal describes the intended result. Any unclear responsibility for returning it still needs a decision. This preview has not performed real work or delivered a result.';
  return phase?.description;
}

function releaseAuthoredAnimation(animation) {
  if(!animation)return;
  animation.simulation.destroy();
  setTimeout(()=>animation.layer.remove(),1000);
}

function animationReviewGeometry(animation) {
  const rect=animation.frame.getBoundingClientRect();
  return JSON.stringify([rect.width,rect.height,Math.min(globalThis.devicePixelRatio || 1,2)]);
}

function ControlIcon({type}) {
  return <svg width="14" height="14" viewBox="0 0 20 20" aria-hidden="true" fill="currentColor">{type==='pause'?<path d="M5 3h3v14H5zm7 0h3v14h-3z"/>:type==='play'?<path d="m5 2 13 8-13 8z"/>:type==='step'?<path d="m3 3 10 7-10 7zm12 0h3v14h-3z"/>:<path d="M5 5a7 7 0 1 1-1 9l2-1a5 5 0 1 0 1-6l3 3H2V2z"/>}</svg>;
}

function animationStorage() {
  try { return window.localStorage; } catch { return null; }
}

/** Sequential keyboard stops in the expanded scene, including native disclosures. */
export function expandedAnimationFocusables(root) {
  if (!root) return [];
  const selector='button,input,select,textarea,a[href],area[href],summary,iframe,object,embed,audio[controls],video[controls],[contenteditable],[tabindex]';
  const candidates=[...root.querySelectorAll(selector)];
  const visible=candidates.filter(node=>{
    if(node.matches(':disabled') || node.closest('[hidden],[inert]') ||
        node.localName==='input' && node.type==='hidden')return false;
    const explicit=node.hasAttribute('tabindex');
    const editable=node.hasAttribute('contenteditable') && node.getAttribute('contenteditable').toLowerCase()!=='false';
    const firstSummary=node.localName==='summary' && node.parentElement?.localName==='details' &&
      [...node.parentElement.children].find(child=>child.localName==='summary')===node;
    if(node.tabIndex<0 && (explicit || !firstSummary && !editable))return false;
    if(editable && !explicit && node.parentElement?.closest('[contenteditable]:not([contenteditable="false"])'))return false;
    if(node.localName==='summary' && !firstSummary && !explicit)return false;
    for(let ancestor=node.parentElement;ancestor && root.contains(ancestor);ancestor=ancestor.parentElement){
      if(ancestor.localName==='details' && !ancestor.open){
        const summary=[...ancestor.children].find(child=>child.localName==='summary');
        if(!summary?.contains(node))return false;
      }
    }
    if(!node.getClientRects().length)return false;
    const visibility=root.ownerDocument.defaultView?.getComputedStyle(node).visibility;
    if(visibility==='hidden' || visibility==='collapse')return false;
    return true;
  });
  // A radio group has one sequential stop, rather than one per option.
  return visible.filter(node=>{
    if(node.localName==='input' && node.type==='radio' && node.name){
      const group=visible.filter(other=>other.localName==='input' && other.type==='radio' &&
        other.name===node.name && other.form===node.form);
      if((group.find(other=>other.checked) || group[0])!==node)return false;
    }
    return true;
  });
}

export function trapExpandedAnimationFocus(event, root) {
  if(event.key!=='Tab' || event.defaultPrevented)return false;
  const items=expandedAnimationFocusables(root),first=items[0],last=items.at(-1);
  const active=root?.ownerDocument.activeElement;
  const target=event.shiftKey && active===first ? last : !event.shiftKey && active===last ? first : null;
  if(!target)return false;
  event.preventDefault();target.focus();return true;
}

export function RehearsalActivityBanner({ activity, names }) {
  if (!activity) return null;
  return <div className="ps-runtime-activity" aria-live="polite">
    <strong>{activity.scope === 'native-application-trial' ? 'Native application trial' : 'Native rehearsal'}: {trialActivityStatus(activity.status)}</strong>
    <span>{activity.activeNodeIds.length ? activity.activeNodeIds.map(id=>names.get(id)||id).join(', ') + ' active'
      : activity.last ? `${names.get(activity.last.identity)||activity.last.identity}: ${activity.last.event.replaceAll('_',' ')}` : 'Waiting for native activity'}</span>
    <small>Actual trial activity. The proposal animation remains illustrative; external integrations are unavailable in this trial.</small>
  </div>;
}

function RelationshipGroup({title,rows,disclosure=false,onInspect}) {
  const [query,setQuery]=useState(''),[page,setPage]=useState(0);
  const resultsRef=useRef(null),pageFocusRef=useRef(false);
  const search=query.trim().toLowerCase();
  const matches=rows.filter(({edge,owner})=>`${owner.name} ${owner.id} ${edge.label} ${(edge.toolNames || []).join(' ')}`.toLowerCase().includes(search));
  const pageCount=Math.max(1,Math.ceil(matches.length/IDENTITY_PAGE_SIZE));
  const pageIndex=Math.max(0,Math.min(pageCount-1,page)),offset=pageIndex*IDENTITY_PAGE_SIZE;
  const items=matches.slice(offset,offset+IDENTITY_PAGE_SIZE);
  useEffect(()=>{
    if(resultsRef.current){
      resultsRef.current.scrollTop=0;
      if(pageFocusRef.current)resultsRef.current.focus({preventScroll:true});
    }
    pageFocusRef.current=false;
  },[pageIndex,query]);
  const pageTo=index=>{pageFocusRef.current=true;setPage(index);};
  const content=<>
    {rows.length>12 && <label className="ps-relationship-search"><span>Find by name, identity or access</span>
      <input aria-label={`Find ${title.toLowerCase()}`} value={query} placeholder="Search relationships..."
        onChange={event=>{setQuery(event.target.value);setPage(0);}}/></label>}
    <ul className={`ps-relationships ${rows.length<=12?'is-compact':''}`} ref={resultsRef} tabIndex={-1} aria-label={`${title} results`}>
      {items.map(({edge,owner,inspectable})=><li key={edge.id} data-relationship-id={edge.id}><span>
        {inspectable && typeof onInspect==='function'
          ? <button type="button" className="ps-relationship-owner" aria-label={`Inspect ${owner.name} (${owner.id})`} onClick={event=>onInspect(owner.id,event)}><strong>{owner.name}</strong><code>{owner.id}</code></button>
          : <><strong>{owner.name}</strong><code>{owner.id}</code></>}</span>
        <small>{edge.label}{edge.toolNames?.length>0 && <code>{edge.toolNames.join(', ')}</code>}</small></li>)}
    </ul>
    {rows.length>12 && <small role="status" aria-live="polite" aria-atomic="true">{matches.length
      ? `Showing ${offset+1}-${Math.min(offset+IDENTITY_PAGE_SIZE,matches.length)} of ${matches.length} matching relationship${matches.length===1?'':'s'}.`
      : 'No matching relationships.'}</small>}
    {pageCount>1 && <nav className="ps-relationship-pages" aria-label={`${title} result pages`}>
      <button type="button" disabled={pageIndex===0} onClick={()=>pageTo(pageIndex-1)}>Previous relationships</button>
      <small>Page {pageIndex+1} of {pageCount}</small>
      <button type="button" disabled={pageIndex+1===pageCount} onClick={()=>pageTo(pageIndex+1)}>Next relationships</button>
    </nav>}
  </>;
  return disclosure
    ? <details className="ps-relationship-group"><summary>{title} ({rows.length} {rows.length===1?'relationship':'relationships'})</summary>{content}</details>
    : <div className="ps-relationship-group"><h5>{title} <small>({rows.length})</small></h5>{content}</div>;
}

export function OwnerRelationships({selected,model,onInspect}) {
  const groups=useMemo(()=>{
    const owners=new Map(model.nodes.map(node=>[node.id,node]));
    const rows=model.edges.filter(edge=>edge.source===selected.id || edge.target===selected.id).map(edge=>{
      const id=edge.source===selected.id?edge.target:edge.source;
      return {edge,owner:owners.get(id) || {id,name:id},inspectable:owners.has(id)};
    });
    return {tools:rows.filter(({edge})=>edge.kind==='tool'),peers:rows.filter(({edge})=>edge.kind==='peer'),
      workflow:rows.filter(({edge})=>!['tool','peer'].includes(edge.kind))};
  },[model,selected.id]);
  return <div className="ps-owner-relationships">
    {!!groups.tools.length && <RelationshipGroup title={selected.kind==='agent'?'Tools and source access':'Configured access'}
      rows={groups.tools} disclosure={selected.kind!=='agent' && groups.tools.length>12} onInspect={onInspect}/>}
    {selected.kind==='agent' && <NativeBundleSelection tools={selected.tools}/>}
    {selected.kind==='agent' && !groups.tools.length && !selected.tools?.rust_bundles?.length && <p>No tools or source access configured in this proposal.</p>}
    {!!groups.peers.length && <RelationshipGroup title="Allowed communication" rows={groups.peers} disclosure onInspect={onInspect}/>}
    {!!groups.workflow.length && <RelationshipGroup title="Workflow connections" rows={groups.workflow} disclosure={groups.workflow.length>12} onInspect={onInspect}/>}
  </div>;
}

export function AnimationGenerationStatus({onRetry,restored=false,correctionRejected=false, ...props}) {
  const status=correctionRejected && props.currentScene && props.generation==='ready'
    ? {message:'Automatic refinement could not be applied. Your current animation is still available.',
      detail:'Your existing animation is preserved. Use Refine animation to describe what should be clearer.',failure:false,canRetry:false}
    : animationGenerationStatus(props);
  if(restored && status.failure && props.authored && props.failure?.kind!=='unavailable')status.detail=props.updating
    ? 'Your previous animation is shown. It does not include the latest proposal changes. The replacement stopped while running.'
    : 'Your previous working animation is shown. The replacement stopped while running.';
  return <div className={`ps-animation-status ${status.failure?'has-error':''}`}>
    <span role="status" aria-live="polite" aria-atomic="true"><span>{status.message}</span>
      {status.detail && <small>{status.detail}</small>}</span>
    {status.canRetry && <button type="button" onClick={onRetry}>{status.retryLabel}</button>}
  </div>;
}

export function AnimationReviewStatus({review}) {
  const presentation=animationReviewPresentation(review);
  if(!presentation)return null;
  return <div className="ps-animation-status"><span role="status" aria-live="polite" aria-atomic="true">
    <span>{presentation.label}</span>{presentation.detail && <small>{presentation.detail}</small>}
  </span></div>;
}

export function ProposalChangeNotice({changes,working=false}) {
  if(!changes.length)return null;
  const remaining=changes.slice(2);
  return <section className="ps-proposal-changes" aria-label="Changes to this proposal">
    <div><h4>{working?'Changes being illustrated':'Changes awaiting illustration'}</h4>
      <p>The visible scene is your previous proposal. These changes are not yet shown.</p></div>
    <div><ul>{changes.slice(0,2).map((change,index)=><li key={index}>{change}</li>)}</ul>
      {!!remaining.length && <details><summary>{remaining.length} more {remaining.length===1?'change':'changes'}</summary>
        <ul>{remaining.map((change,index)=><li key={index}>{change}</li>)}</ul></details>}</div>
  </section>;
}

export function AnimationRefinementForm({streamId, text, error, persistent, ready, sending, waiting = false, inputRef, onText, onSubmit, onCancel}) {
  return <form id={`animation-feedback-${streamId}`} className="ps-animation-feedback" onSubmit={onSubmit}>
    <label htmlFor={`animation-feedback-text-${streamId}`}>What would make this animation clearer?</label>
    <textarea id={`animation-feedback-text-${streamId}`} ref={inputRef} value={text} maxLength={1000} rows={3}
      placeholder="Simplify the labels, emphasize collaboration, or change the pacing."
      aria-describedby={`animation-feedback-help-${streamId}`} aria-invalid={error?true:undefined} onChange={event=>onText(event.target.value)}/>
    <p id={`animation-feedback-help-${streamId}`}>{waiting
      ? 'Write feedback for this proposal while its animation is prepared. Send it when the current preview is ready.'
      : 'Your current animation stays visible while the agent refines it.'}</p>
    {ready && !persistent && <p role="status">This browser could not save your latest draft changes. They remain in this open page.</p>}
    {!ready && <p role="status">Getting this proposal ready for your feedback. You can keep writing.</p>}
    {error && <p className="ps-feedback-error" role="alert">{error}</p>}
    <div><span>{text.length}/1000</span><button type="button" onClick={onCancel}>Cancel</button>
      <button type="submit" disabled={sending || waiting || !ready || !text.trim()}>{sending?'Sending refinement...':waiting?'Waiting for current preview':'Send to animation agent'}</button></div>
  </form>;
}

function useAnimationActivity(operation, currentRequest) {
  const [snapshot,setSnapshot]=useState({key:null,frames:[],page:null,interrupted:false});
  const current=useRef({operation,currentRequest});
  current.current={operation,currentRequest};
  const key=animationActivityKey(operation?.scope);
  useEffect(()=>{
    if(!key)return;
    let stopped=false,timer;
    const scope=operation.scope,log=createAnimationActivityLog(scope),controller=new AbortController();
    setSnapshot({key,frames:[],page:null,interrupted:false});
    const owns=expected=>!stopped && animationActivityKey(current.current.operation?.scope)===animationActivityKey(expected)
      && animationRequestMatches(expected.request,current.current.currentRequest);
    async function poll(){
      let delay=1400;
      try{
        const result=await readAnimationActivity(async url=>{
          const response=await fetch(url,{signal:controller.signal});
          if(!response.ok)throw new Error('Activity observation unavailable');
          return response.json();
        },scope,owns,log);
        if(!owns(scope))return;
        if(!result)throw new Error('Activity source binding changed');
        setSnapshot({key,...result,interrupted:false});
        if(result.more)delay=0;
        else if(['completed','failed','interrupted'].includes(result.page.native.status) && result.page.exhausted && result.page.observation.status==='available')return;
        else if(result.page.observation.status==='unavailable')delay=3000;
      }catch{
        if(!owns(scope))return;
        setSnapshot(previous=>previous.key===key?{...previous,interrupted:true}:{key,frames:[],page:null,interrupted:true});
        delay=3000;
      }
      if(owns(scope))timer=setTimeout(poll,delay);
    }
    poll();
    return()=>{stopped=true;clearTimeout(timer);controller.abort();};
  },[key]);
  return snapshot.key===key && animationRequestMatches(operation?.scope.request,currentRequest)
    ? snapshot:{frames:[],page:null,interrupted:false};
}

export function AnimationActivityView({scope,frames=[],page=null,interrupted=false,expanded,onExpand,scroller,onScroll}) {
  const status=animationActivityPresentation(page,interrupted);
  const {viewState}=animationActivityConversation(scope,frames,page?.native);
  return <details open={expanded ?? status.active} className="ps-agent-activity" data-animation-operation={scope.id}>
    <summary onClick={event=>{event.preventDefault();onExpand?.(!(expanded ?? status.active));}}>
      Visualization agent activity <small>{status.label}</small></summary>
    <p role="status">{status.detail}</p>
    <div className="ps-agent-transcript setup-native-chat" data-native-visualization-chat="true" ref={scroller} onScroll={onScroll}>
      <SpriteSheet/><ConversationTranscript viewState={viewState} Icon={ConsoleIcon} compact showTurnDiff={false}/>
      {!frames.length && <p>{page?.native.admission_confirmed?'No public activity has been recorded for this request yet.':'Waiting for request-bound native activity.'}</p>}
    </div>
  </details>;
}

function AnimationActivity({operation,currentRequest}) {
  const timeline=useAnimationActivity(operation,currentRequest),scroller=useRef(null),following=useRef(true);
  const [expanded,setExpanded]=useState(null);
  const key=animationActivityKey(operation?.scope);
  useEffect(()=>{setExpanded(null);following.current=true;},[key]);
  useEffect(()=>{if(following.current && scroller.current)scroller.current.scrollTop=scroller.current.scrollHeight;},[timeline.frames]);
  if(!operation || !animationRequestMatches(operation.scope.request,currentRequest))return null;
  return <AnimationActivityView scope={operation.scope} {...timeline} expanded={expanded} onExpand={setExpanded} scroller={scroller}
    onScroll={event=>{const node=event.currentTarget;following.current=node.scrollHeight-node.scrollTop-node.clientHeight<50;}}/>;
}

export function ProposalSimulation({plan,interfaceFact,designBrief,visualizationFeedback,pending,busy,onRequestChange,onExpand,setupId,rehearsalActivity}) {
  const [flowId,setFlowId]=useState('__overview__');
  const model=useMemo(()=>createSimulation(plan,flowId,interfaceFact,designBrief,visualizationFeedback),[plan,flowId,interfaceFact,designBrief,visualizationFeedback]);
  const [generated,setGenerated]=useState(null),[generation,setGeneration]=useState('idle');
  const generationRef=useRef(generation);
  generationRef.current=generation;
  const [candidate,setCandidate]=useState(null);
  const [restoredRequest,setRestoredRequest]=useState(null);
  const [generationError,setGenerationError]=useState(null);
  const [generationNotice,setGenerationNotice]=useState('');
  const [activityOperation,setActivityOperation]=useState(null);
  const [adoptionPending,setAdoptionPending]=useState(false),[adoptionError,setAdoptionError]=useState('');
  const adoptionRef=useRef(null),generationObserverRef=useRef(null);
  const [refinementOpen,setRefinementOpen]=useState(false),[refinementError,setRefinementError]=useState('');
  const [drafts]=useState(()=>createAnimationDraftStore(animationSessionStorage()));
  const [,refreshDrafts]=useState(0),[refinementSending,setRefinementSending]=useState(false);
  const pendingRefinement=useRef(null);
  const refinementInputRef=useRef(null);
  const [authoredFrame,setAuthoredFrame]=useState({time:0,duration:1,chapters:[]}),[authoredCaption,setAuthoredCaption]=useState(null);
  const authoredHostRef=useRef(null),authoredRef=useRef(null),activeAnimationRef=useRef(null),previousAnimationRef=useRef(null);
  const presentationEpochRef=useRef(0),pendingCurrentCandidateRef=useRef(null),pendingRecoveryRef=useRef(null);
  const [retry,setRetry]=useState(0);
  const feedbackRef=useRef(''),executionRepairs=useRef(0),repairRef=useRef(false);
  const expectedParentRef=useRef(null),reviewIdRef=useRef(null),reviewBudgetRef=useRef(createAnimationReviewBudget());
  const reviewAdmissionRef=useRef(null);
  const [visualReview,setVisualReview]=useState(null);
  const [reviewRevision,setReviewRevision]=useState(0);
  const reviewEpochRef=useRef(createAnimationReviewEpoch()),reviewJobRef=useRef(null);
  const preferenceRef=useRef(null);
  const submittedAnimationRef=useRef(null),recheckAnimationRef=useRef(null);
  const publicationResumeRef=useRef(null);
  const [index,setIndex]=useState(0),[playing,setPlaying]=useState(()=>!matchMedia('(prefers-reduced-motion: reduce)').matches);
  const [reduced,setReduced]=useState(()=>matchMedia('(prefers-reduced-motion: reduce)').matches);
  const [selectedId,setSelectedId]=useState(null),[groupId,setGroupId]=useState(null),[search,setSearch]=useState('');
  const [identityPage,setIdentityPage]=useState(null);
  const memberResultsRef=useRef(null),pageFocusRef=useRef(false),exploreRef=useRef(null);
  const [showPeers,setShowPeers]=useState(false),[wide,setWide]=useState(false),[box,setBox]=useState({width:640,height:380});
  const narrowPreview=box.width<480;
  const base=useMemo(()=>plan ? applyVisualizationLayout(createSimulationScene(model,plan),composeTopology(model,{narrow:narrowPreview}),model) : createSimulationScene(model,plan),[model,plan,narrowPreview]);
  const playbackRef=useRef(null);
  playbackRef.current={playing,reduced,showPeers,selectedId};
  reviewEpochRef.current.observe([playing,reduced,showPeers,selectedId,wide]);
  const canvasRef=useRef(null),simRef=useRef(null),sectionRef=useRef(null),keyboardRef=useRef(null),expandRef=useRef(null),inspectorRef=useRef(null),originRef=useRef(null),progressRef=useRef(null);
  const streamId=`${setupId || plan?.project?.slug || 'preview'}:${model.selectedFlowId || '__overview__'}`;
  const key=useMemo(()=>simulationSourceKey(plan,model),[plan,model]);
  const draftSelection=animationDraftSelection(setupId || 'unsaved-preview',key,model.selectedFlowId || '__overview__');
  const refinementDraft=drafts.read(draftSelection),refinementText=refinementDraft.text;
  const request={streamId,proposalKey:key,attempt:retry};
  const currentRequestRef=useRef(request);
  currentRequestRef.current=request;
  const consumerRef=useRef(null),intentOwnerRef=useRef(null);
  if(!consumerRef.current)consumerRef.current=createAnimationConsumer();
  const reviewCurrentRef=useRef({setupId,key,streamId,request});
  reviewCurrentRef.current={setupId,key,streamId,request};
  const authored=generated?.streamId===streamId && generated.artifact.contract==='freeform-animation-v3' ? generated.artifact : null;
  const scene=generated?.key===key && generated.scene ? generated.scene:base;
  const visibleModel=authored?generated.model:model;
  const visibleSourceKey=authored?generated.key:key;
  const proposalChanged=!!authored && generated.key!==key;
  const proposalChanges=useMemo(()=>proposalChanged && !pending
    ? proposalClassChanges(generated.plan,plan):[],[proposalChanged,pending,generated?.plan,plan]);
  const updating=!!authored && (proposalChanged || generated.recovered===true);
  const correctionRejected=restoredRequest?.kind==='correction-rejected' && animationRequestMatches(restoredRequest,request)
    && generated?.key===key && generated.receipt?.id===restoredRequest.operation_id
    && generated.artifact.code_hash===restoredRequest.code_hash;
  const currentAnimation=!!authored && !generated.recovered && (animationRequestMatches(generated.request,request) || correctionRejected);
  const executionDetail=generation==='fallback' && generationError?.kind==='execution'
    && animationRequestMatches(generationError.request,request) && generationError.execution
    ? animationFailureDetail(generationError.execution):null;
  const selected=visibleModel.nodes.find(n=>n.id===selectedId);
  const guideRefinementRef=useRef(null);
  guideRefinementRef.current={busy,pending,updating,request,visibleSourceKey,selected,onRequestChange};
  const group=scene.groups?.find(g=>g.id===groupId);
  const chapter=authoredFrame.chapters.filter(item=>item.at<=authoredFrame.time).at(-1);
  const active=authored ? (authoredCaption || chapter || {title:'Your proposed system',description:'An illustrative animation of the current proposal.'}) : model.phases[index] || model.phases[0];
  const selectionCaption=!!authored && !!authoredCaption && activeAnimationRef.current?.captionSelectionId===selected?.id;
  const caption = active?.kind === 'work'
    ? `${active.activeNodeIds.length > 4 ? `${active.activeNodeIds.length} configured agents` : active.activeNodeIds.map(id=>model.nodes.find(n=>n.id===id)?.name).filter(Boolean).join(', ')} ${active.activeNodeIds.length > 1 ? 'work in parallel' : 'works on this step'}. Select an agent to inspect its responsibilities.`
    : authored ? active?.description : provisionalCaption(active,model.interface_context);

  const agents=visibleModel.nodes.filter(n=>n.kind==='agent');
  const pageOwned=identityPage?.sourceKey===visibleSourceKey && identityPage.groupId===groupId &&
    identityPage.search===search && identityPage.streamId===streamId;
  const identityResults=populationInspection(visibleModel,group?.memberIds,search,pageOwned?identityPage.index:0);
  const rehearsalNames=useMemo(()=>new Map(model.nodes.map(n=>[n.id,n.name])),[model]);
  useEffect(()=>{
    if(!plan)return;
    const owner={request:{...request},lease:null};
    const declare=()=>consumerRef.current.declare({setup_id:setupId || null,stream_id:streamId},{onUnavailable:()=>{
      if(intentOwnerRef.current!==owner || !animationRequestMatches(owner.request,currentRequestRef.current))return;
      reviewJobRef.current?.cancel();
      if(pendingCurrentCandidateRef.current?.intentOwner===owner){pendingCurrentCandidateRef.current=null;setCandidate(null);showPendingRecovery();}
      setGeneration('fallback');setGenerationError(animationFailure(owner.request,'intent'));
      if(pendingRefinement.current && animationRequestMatches(pendingRefinement.current.request,owner.request)){
        pendingRefinement.current=null;setRefinementSending(false);setRefinementError('The animation request could not be confirmed. Your draft is preserved.');
      }
    }});
    intentOwnerRef.current=owner;owner.declare=declare;owner.lease=declare();
    return()=>{if(intentOwnerRef.current===owner)intentOwnerRef.current=null;owner.lease.release();};
  },[key,streamId,retry]);
  useEffect(()=>{
    if(!plan || !draftSelection)return;
    let cancelled=false;
    fingerprint({blueprint:plan,model}).then(hash=>{
      drafts.attach(draftSelection,hash,!!setupId);
      if(!cancelled)refreshDrafts(value=>value+1);
    }).catch(()=>{if(!cancelled)refreshDrafts(value=>value+1);});
    return()=>{cancelled=true;};
  },[key,streamId,setupId]);
  useEffect(()=>{
    if(authoredRef.current)authoredRef.current.runtimeActivity=generated?.key===key && !generated.recovered ? rehearsalActivity || null:null;
    if(simRef.current)simRef.current.runtimeActivity=rehearsalActivity || null;
  },[rehearsalActivity,authored,scene,key,generated]);

  useEffect(()=>{
    const media=matchMedia('(prefers-reduced-motion: reduce)');
    const change=()=>{invalidateVisualReview();setReduced(media.matches);if(media.matches)setPlaying(false);};
    media.addEventListener('change',change);return()=>media.removeEventListener('change',change);
  },[]);
  useEffect(()=>{
    if(!canvasRef.current || authored)return;
    const sim=new MobSim(canvasRef.current,{progress:progressRef.current,onFrame:time=>setIndex(Math.min(model.phases.length-1,Math.floor(time/4.2)))});
    simRef.current=sim;
    sim.setScene(scene);sim.paused=!playing;sim.reducedMotion=reduced;sim.showPeers=showPeers;sim.selectedId=selectedId;
    const observer=new ResizeObserver(([entry])=>setBox({width:entry.contentRect.width,height:entry.contentRect.height}));
    observer.observe(canvasRef.current);
    return()=>{observer.disconnect();sim.destroy();simRef.current=null;};
  },[!!plan,model.phases.length,authored]);
  useEffect(()=>{setGenerated(null);setRestoredRequest(null);return()=>{
    releaseAuthoredAnimation(activeAnimationRef.current);
    releaseAuthoredAnimation(previousAnimationRef.current);
    activeAnimationRef.current=null;previousAnimationRef.current=null;authoredRef.current=null;
    pendingCurrentCandidateRef.current=null;pendingRecoveryRef.current=null;
  };},[streamId]);
  function showPendingRecovery() {
    const saved=pendingRecoveryRef.current,current=currentRequestRef.current;
    if(!saved || saved.streamId!==current.streamId || saved.proposalKey!==current.proposalKey
      || saved.recoveryEpoch!==presentationEpochRef.current || activeAnimationRef.current
      || animationRequestMatches(pendingCurrentCandidateRef.current?.request,current))return false;
    const next={...saved.value,key:simulationSourceKey(saved.value.plan,saved.value.model),streamId:current.streamId,
      request:{...current},recovered:true,recoveryEpoch:saved.recoveryEpoch};
    saved.candidate=next;setCandidate(next);
    return true;
  }
  useEffect(()=>{
    if(!plan || !setupId || activeAnimationRef.current)return;
    const scope={setup_id:setupId,stream_id:streamId},storage=animationSessionStorage();
    const checkpoint=pendingRecoveryRef.current;
    if(checkpoint && checkpoint.streamId===streamId && checkpoint.proposalKey===key
      && checkpoint.recoveryEpoch===presentationEpochRef.current){showPendingRecovery();return;}
    pendingRecoveryRef.current=null;
    const snapshot=readAnimationRecovery(storage,scope);
    const selection=snapshot ? null : readAnimationSelection(storage,scope);
    if(!snapshot && !selection)return;
    let cancelled=false;
    const controller=new AbortController(),epoch=presentationEpochRef.current;
    // Historical verification belongs to this proposal, across repair attempts.
    const isCurrent=()=>!cancelled && epoch===presentationEpochRef.current
      && !activeAnimationRef.current && currentRequestRef.current.streamId===streamId
      && currentRequestRef.current.proposalKey===key;
    (async()=>{
      try {
        const response=await fetch(`/api/visualizations/${(snapshot || selection).operation_id}`,{signal:controller.signal});
        if(!response.ok)return;
        const receipt=await response.json();
        if(!isCurrent())return;
        const recovered=snapshot ? await recoveredAnimation(snapshot,receipt,scope)
          : await recoveredAnimationSelection(selection,receipt,scope,plan,model);
        if(!recovered || !isCurrent())return;
        // Admission is not a first frame. Hold the verified previous inputs
        // while a current candidate starts, without replacing its frame owner.
        pendingRecoveryRef.current={value:recovered,streamId,proposalKey:key,recoveryEpoch:epoch};
        showPendingRecovery();
      } catch { /* Recovery is optional; the current generation remains independent. */ }
    })();
    return()=>{cancelled=true;controller.abort();};
  },[key,streamId,setupId]);
  // A repair changes the current candidate, not the saved scene's source.
  // Keep its original fetch/hash verification alive until this scope changes.
  useEffect(()=>{showPendingRecovery();},[key,streamId,retry,setupId]);
  useEffect(()=>{
    if(!candidate || candidate.streamId!==streamId || candidate.key!==key && !candidate.recovered
      || !animationRequestMatches(candidate.request,currentRequestRef.current) || !authoredHostRef.current)return;
    if(candidate.recovered ? pendingRecoveryRef.current?.candidate!==candidate
      : pendingCurrentCandidateRef.current!==candidate)return;
    const ownsIntent=()=>!candidate.intentOwner || candidate.intentOwner===intentOwnerRef.current &&
      candidate.intentOwner.lease===candidate.intentLease && candidate.intentLease.active;
    if(!candidate.recovered && !ownsIntent())return;
    let cancelled=false,metadata={duration:1,chapters:[]},caption=null,overviewCaption=null,captionSelectionId=null,displayed=null;
    const layer=document.createElement('div');
    layer.className='ps-authored-layer';layer.style.visibility='hidden';
    authoredHostRef.current.append(layer);
    const simulation=new FreeformAnimation(layer,{code:candidate.artifact.code,model:candidate.model,blueprint:candidate.plan,
      onReady:value=>{metadata=value;},
      onEscape:()=>{if(authoredRef.current===simulation)keyboardRef.current?.({key:'Escape'});},
      onFirstFrame:()=>{
        if(cancelled || !animationRequestMatches(candidate.request,currentRequestRef.current))return;
        if(candidate.recovered && (pendingRecoveryRef.current?.candidate!==candidate
          || presentationEpochRef.current!==candidate.recoveryEpoch || activeAnimationRef.current
          || animationRequestMatches(pendingCurrentCandidateRef.current?.request,currentRequestRef.current)))return;
        if(!candidate.recovered && pendingCurrentCandidateRef.current!==candidate)return;
        if(!candidate.recovered && !ownsIntent())return;
        const previous=activeAnimationRef.current;
        const controls=playbackRef.current;
        simulation.paused=!controls.playing;simulation.reducedMotion=controls.reduced;simulation.showPeers=controls.showPeers;simulation.selectedId=controls.selectedId;
        simulation.runtimeActivity=candidate.recovered ? null:rehearsalActivity || null;
        displayed={simulation,layer,candidate,metadata,caption,overviewCaption,captionSelectionId};
        releaseAuthoredAnimation(previousAnimationRef.current);
        previousAnimationRef.current=previous;
        activeAnimationRef.current=displayed;authoredRef.current=simulation;
        layer.style.visibility='visible';
        // A first frame does not establish that later chapters can run. Keep
        // exactly one prior displayed scene paused for a bounded rollback.
        if(previous){previous.layer.style.visibility='hidden';previous.simulation.paused=true;previous.simulation.runtimeActivity=null;}
        setGenerated(candidate);setRestoredRequest(null);setAuthoredFrame({time:simulation.time,...metadata});setAuthoredCaption(caption);
        pendingRecoveryRef.current=null;
        if(!candidate.recovered){
          pendingCurrentCandidateRef.current=null;presentationEpochRef.current+=1;
          saveAnimationRecovery(animationSessionStorage(),{setup_id:setupId,stream_id:streamId},candidate);
          if(candidate.adopted){adoptionRef.current=null;setAdoptionPending(false);setAdoptionError('');}
          setGenerationError(null);setGenerationNotice('');setGeneration('ready');
        }
      },
      onFrame:(time,duration)=>{if(activeAnimationRef.current?.simulation===simulation)setAuthoredFrame(current=>({...current,time,duration}));},
      onCaption:value=>{
        caption=value;captionSelectionId=simulation.selectedId || null;
        if(!captionSelectionId)overviewCaption=value;
        if(displayed)Object.assign(displayed,{caption,overviewCaption,captionSelectionId});
        if(activeAnimationRef.current?.simulation===simulation)setAuthoredCaption(value);
      },
      onInspect:id=>{if(activeAnimationRef.current?.simulation===simulation){invalidateVisualReview();originRef.current=expandRef.current;setSelectedId(id);setGroupId(null);}},
      onError:failure=>{
        const isActive=activeAnimationRef.current?.simulation===simulation;
        if(pendingCurrentCandidateRef.current===candidate)pendingCurrentCandidateRef.current=null;
        if(pendingRecoveryRef.current?.value.receipt.id===candidate.receipt.id)pendingRecoveryRef.current=null;
        // A host publication can remain completed after a client execution
        // failure. Remove only its saved pointer, preserving a valid rollback.
        forgetAnimationRecovery(animationSessionStorage(),{setup_id:setupId,stream_id:streamId},candidate.receipt.id);
        if(previousAnimationRef.current?.simulation===simulation){previousAnimationRef.current=null;layer.remove();}
        if(cancelled && !isActive)return;
        if(isActive){
          const previous=previousAnimationRef.current;
          previousAnimationRef.current=null;
          if(previous && previous.simulation.alive!==false){
            activeAnimationRef.current=previous;authoredRef.current=previous.simulation;
            previous.layer.style.visibility='visible';previous.simulation.paused=true;
            previous.simulation.runtimeActivity=previous.candidate.key===currentRequestRef.current.proposalKey && !previous.candidate.recovered ? rehearsalActivity || null:null;
            setGenerated(previous.candidate);setRestoredRequest({...candidate.request});setAuthoredFrame({time:previous.simulation.time,...previous.metadata});setAuthoredCaption(previous.caption);
            saveAnimationRecovery(animationSessionStorage(),{setup_id:setupId,stream_id:streamId},previous.candidate);
            setPlaying(false);setSelectedId(null);setGroupId(null);setSearch('');
          }else{
            releaseAuthoredAnimation(previous);
            activeAnimationRef.current=null;authoredRef.current=null;setGenerated(null);setRestoredRequest(null);
          }
        }
        layer.remove();
        if(candidate.recovered)return;
        const detail=animationFailureDetail(failure);
        if(candidate.adopted && animationRequestMatches(candidate.request,currentRequestRef.current) && ownsIntent()){
          adoptionRef.current=null;setAdoptionPending(false);
          setAdoptionError('The saved animation could not run here. Your previous preview is preserved.');
          setGeneration('fallback');setGenerationError({...animationFailure(candidate.request,'execution'),execution:detail});return;
        }
        if(!candidate.adopted && animationRequestMatches(candidate.request,currentRequestRef.current) && ownsIntent()){
          feedbackRef.current=`Program ${candidate.artifact.key} failed isolated canvas execution. Failure: ${detail.kind}/${detail.phase}${detail.exception ? ` (${detail.exception})`:''}. Return a complete working animation program.`;
          if(executionRepairs.current++<1){
            repairRef.current=true;setGenerationError(null);setGenerationNotice(CANVAS_REPAIR_NOTICE);
            setGeneration('working');setRetry(value=>value+1);
          }else{
            setGeneration('fallback');setGenerationError({...animationFailure(candidate.request,'execution'),execution:detail});
            showPendingRecovery();
          }
        }else if(isActive){
          if(generationRef.current==='working')setGenerationNotice('The displayed animation stopped. The latest proposal update continues.');
          else{setGeneration('fallback');setGenerationError(animationFailure(currentRequestRef.current,'execution'));}
        }
      }});
    simulation.paused=!playing;simulation.reducedMotion=reduced;simulation.showPeers=showPeers;simulation.selectedId=selectedId;
    return()=>{
      cancelled=true;
      if(pendingCurrentCandidateRef.current===candidate)pendingCurrentCandidateRef.current=null;
      if(pendingRecoveryRef.current?.candidate===candidate)pendingRecoveryRef.current.candidate=null;
      if(activeAnimationRef.current?.simulation!==simulation && previousAnimationRef.current?.simulation!==simulation){simulation.destroy();setTimeout(()=>layer.remove(),1000);}
    };
  },[candidate,streamId,key,retry]);
  useEffect(()=>{const simulation=authoredRef.current;if(simulation){simulation.paused=!playing;simulation.reducedMotion=reduced;simulation.selectedId=selectedId;simulation.showPeers=showPeers;}},[playing,reduced,selectedId,showPeers,authored]);
  useEffect(()=>{if(simRef.current){simRef.current.showPeers=showPeers;simRef.current.setScene(scene);setIndex(0);}},[scene,showPeers,model.phases.length]);
  useEffect(()=>{if(simRef.current){simRef.current.paused=!playing;simRef.current.reducedMotion=reduced;}},[playing,reduced,scene]);
  useEffect(()=>{if(simRef.current)simRef.current.selectedId=selectedId;},[selectedId,scene]);
  useEffect(()=>{reviewJobRef.current?.cancel();setVisualReview(null);},[playing,reduced,showPeers,selectedId,wide]);
  useEffect(()=>{
    const simulation=authoredRef.current,frame=simulation?.frame;
    if(!authored || !frame || generated.recovered)return;
    const geometry=()=>animationReviewGeometry(simulation);
    let previous=geometry();
    const changed=()=>{if(authoredRef.current!==simulation)return;const current=geometry();if(current!==previous){previous=current;invalidateVisualReview();}};
    const observer=new ResizeObserver(changed);
    observer.observe(frame);window.addEventListener('resize',changed);
    return()=>{observer.disconnect();window.removeEventListener('resize',changed);};
  },[authored,generated]);
  useEffect(()=>{
    const simulation=authoredRef.current,owner=animationReviewOwner(generated,request,setupId);
    if(!authored || !owner || !simulation || generated.key!==key || generated.recovered || adoptionPending){setVisualReview(null);return;}
    const presentationEpoch=reviewEpochRef.current.current;
    const presentationGeometry=animationReviewGeometry(simulation);
    const owns=expected=>{
      const current=reviewCurrentRef.current,active=activeAnimationRef.current;
      const sameOwner=reviewEpochRef.current.current===presentationEpoch && (current.setupId ?? null)===expected.binding.setup_id && current.key===generated.key &&
        current.streamId===expected.binding.stream_id && animationRequestMatches(expected.request,current.request) &&
        intentOwnerRef.current?.lease.active && animationRequestMatches(intentOwnerRef.current.request,current.request) &&
        active?.simulation===simulation && active.candidate.receipt?.id===expected.binding.id &&
        active.candidate.artifact.code_hash===expected.binding.code_hash;
      if(!sameOwner)return false;
      if(animationReviewGeometry(simulation)!==presentationGeometry){invalidateVisualReview();return false;}
      return true;
    };
    setVisualReview(null);
    let review;
    // Review the settled visible presentation without changing its playback or
    // regenerating its source. A geometry mismatch can schedule this effect too.
    const timer=setTimeout(()=>{
      if(!owns(owner))return;
      // Saved-source reviews observe this presentation without spending its
      // automatic correction allowance or changing the selected publication.
      review=createAnimationReview({owner,animation:simulation,isCurrent:owns,
        budget:{claim:reviewOwner=>!generated.adopted && reviewBudgetRef.current.claim(reviewOwner)},
        onStatus:value=>{if(owns(owner))setVisualReview({...value,presentationEpoch});},
        onCorrection:advisory=>{
          if(generated.adopted || !owns(owner))return false;
          feedbackRef.current=advisory.feedback;expectedParentRef.current={...advisory.expected_parent};reviewIdRef.current=advisory.review_id;
          reviewAdmissionRef.current={feedback:checkedAnimationFeedback(advisory.feedback),attempt:owner.request.attempt+1,
            parentFeedback:generated.feedback || ''};
          setGenerationError(null);setGenerationNotice('');setGeneration('working');setRetry(value=>value+1);return true;
        }});
      reviewJobRef.current=review;
      review.start();
    },VISUAL_REVIEW_SETTLE_MS);
    return()=>{clearTimeout(timer);review?.cancel();if(reviewJobRef.current===review)reviewJobRef.current=null;};
  },[authored,generated,key,streamId,retry,setupId,reviewRevision,playing,reduced,showPeers,selectedId,wide,adoptionPending]);
  useEffect(()=>{setSelectedId(null);setGroupId(null);setSearch('');feedbackRef.current='';preferenceRef.current=null;
    submittedAnimationRef.current=null;recheckAnimationRef.current=null;
    expectedParentRef.current=null;reviewIdRef.current=null;reviewAdmissionRef.current=null;setVisualReview(null);
    executionRepairs.current=0;repairRef.current=false;setRefinementOpen(false);setRefinementError('');
    pendingRefinement.current=null;setRefinementSending(false);},[key,streamId]);
  useEffect(()=>{
    setAdoptionPending(false);setAdoptionError('');
    return()=>{adoptionRef.current?.controller.abort();adoptionRef.current=null;};
  },[key,streamId,retry]);
  useEffect(()=>{if(refinementOpen)refinementInputRef.current?.focus();},[refinementOpen]);
  useEffect(()=>{if(selectedId || groupId){setPlaying(false);inspectorRef.current?.focus({preventScroll:true});}},[selectedId,groupId]);
  useEffect(()=>{
    if(!pageFocusRef.current)return;
    pageFocusRef.current=false;
    if(memberResultsRef.current){memberResultsRef.current.scrollTop=0;memberResultsRef.current.querySelector('button')?.focus({preventScroll:true});}
  },[identityResults.pageIndex,identityPage]);
  useEffect(()=>{
    setIdentityPage(null);
    if(memberResultsRef.current){
      memberResultsRef.current.scrollTop=0;
      if(document.activeElement===document.body)(memberResultsRef.current.querySelector('button') || inspectorRef.current?.querySelector('input'))?.focus({preventScroll:true});
    }
  },[visibleSourceKey,groupId,search,streamId]);
  useEffect(()=>{
    if(wide && (selectedId || groupId))inspectorRef.current?.scrollIntoView?.({block:'nearest',inline:'nearest'});
  },[wide,selectedId,groupId]);
  useEffect(()=>{
    if(wide && refinementOpen)refinementInputRef.current?.scrollIntoView?.({block:'nearest',inline:'nearest'});
  },[wide,refinementOpen]);
  useEffect(()=>{if(wide)expandRef.current?.focus({preventScroll:true});},[wide]);

  useEffect(()=>{
    if(!plan)return;
    const intentOwner=intentOwnerRef.current;
    if(!intentOwner || !animationRequestMatches(intentOwner.request,request))return;
    let cancelled=false,timer,repaired=false,repairing=repairRef.current,displayedCodeHash=null,reattachments=0,postRechecks=0,submittedFeedback='',submittedBody=null,requestBinding=null;
    let restoringCorrection=null,parentRestored=false;
    const expectedParent=expectedParentRef.current ? {...expectedParentRef.current}:null,reviewId=reviewIdRef.current;
    const reviewAdmission=reviewAdmissionRef.current;
    repairRef.current=false;
    const isCurrent=()=>!cancelled && intentOwnerRef.current===intentOwner && intentOwner.lease.active && animationRequestMatches(request,currentRequestRef.current);
    const submit=async(feedback='',{sameRequest=false}={})=>{
      try {
        const baseBody=animationRequestBody(plan,model,feedback,streamId,setupId);
        const recheck=recheckAnimationRef.current;
        const captured=sameRequest && submittedBody ? structuredClone(submittedBody)
          : recheck && animationRequestMatches(recheck.request,request) ? structuredClone(recheck.body)
          : restoringCorrection ? {...baseBody,...structuredClone(restoringCorrection.correction)}
          : {...baseBody,...(expectedParent && reviewAdmission?.attempt===request.attempt &&
            reviewAdmission.feedback===baseBody.feedback ? {expected_parent:expectedParent,review_id:reviewId}:{})};
        if(!isCurrent())return;
        if(restoringCorrection && !parentRestored){
          const parent=restoringCorrection.correction.expected_parent;
          const parentBody=animationRequestBody(plan,model,restoringCorrection.feedback,streamId,setupId);
          const [requestHash,blueprintHash,modelHash]=await Promise.all([
            fingerprint({blueprint:parentBody.blueprint,model:parentBody.model,...(parentBody.feedback?{feedback:parentBody.feedback}:{})}),
            fingerprint(parentBody.blueprint),fingerprint(parentBody.model)]);
          if(!isCurrent())return;
          const binding={setup_id:setupId || null,stream_id:streamId,request_fingerprint:requestHash,blueprint_hash:blueprintHash,model_hash:modelHash};
          const savedParent=await json(`/api/visualizations/${parent.operation_id}`);
          if(!isCurrent())return;
          if(!animationCorrectionParentMatches(savedParent,binding,parent))
            throw new Error('The saved correction parent no longer matches this proposal.');
          const parentIntent=await intentOwner.lease.binding();
          if(!isCurrent())return;
          const restoredParent=await json('/api/visualizations',{method:'POST',headers:{'Content-Type':'application/json'},
            body:JSON.stringify(withAnimationIntent({...parentBody,replay_parent:parent},parentIntent))});
          if(!isCurrent())return;
          if(!animationCorrectionParentMatches(restoredParent,binding,parent))
            throw new Error('The saved correction parent could not be restored exactly.');
          // A new scheduling revision retains host-owned parent lineage. It
          // changes no source, review reservation or correction request data.
          intentOwner.lease=intentOwner.declare();parentRestored=true;
        }
        // A legacy layout repair changes feedback within this effect. It needs
        // a fresh scheduling revision, while exact-body recovery keeps its ack.
        if(submittedBody && !sameRequest && submittedBody.feedback!==captured.feedback)intentOwner.lease=intentOwner.declare();
        const body=withAnimationIntent(captured,await intentOwner.lease.binding());
        if(!isCurrent())return;
        submittedFeedback=body.feedback;
        submittedBody=structuredClone(body);
        submittedAnimationRef.current={request:{...request},body:structuredClone(body)};
        if(recheckAnimationRef.current===recheck)recheckAnimationRef.current=null;
        const [requestHash,blueprintHash,modelHash]=await Promise.all([
          fingerprint({blueprint:body.blueprint,model:body.model,...(body.feedback?{feedback:body.feedback}:{})}),fingerprint(body.blueprint),fingerprint(body.model)]);
        if(!isCurrent())return;
        const binding={setup_id:setupId || null,stream_id:streamId,request_fingerprint:requestHash,blueprint_hash:blueprintHash,model_hash:modelHash};
        requestBinding=binding;
        let receipt;
        try{receipt=await json('/api/visualizations',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});}
        catch(error){
          if(!isCurrent())return;
          if(error.animationFailureKind!=='intent' && (error.status===undefined || error.status>=500) && postRechecks++===0){
            setGenerationNotice('Checking the same saved animation request.');
            timer=setTimeout(()=>submit(submittedFeedback,{sameRequest:true}),2000);return;
          }
          throw error;
        }
        if(!isCurrent())return;
        if(!animationAdmissionMatches(receipt,binding))throw new Error('The animation admission belongs to another request.');
        if(body.expected_parent && body.review_id && Object.entries(body.expected_parent)
          .every(([name,value])=>receipt.correction_parent?.[name]===value)){
          const preference=preferenceRef.current;
          if(preference?.streamId===streamId && (restoringCorrection || reviewAdmission?.feedback===body.feedback))
            saveAnimationCorrectionPreference(animationStorage(),streamId,preference.proposalHash,
              restoringCorrection?.feedback ?? reviewAdmission.parentFeedback,
              {feedback:body.feedback,expected_parent:body.expected_parent,review_id:body.review_id});
        }
        const submitted=pendingRefinement.current;
        if(submitted && animationRequestMatches(submitted.request,request)){
          const acknowledged=acknowledgeAnimationRefinement(drafts,submitted.selection,submitted.capture,receipt,binding,request,currentRequestRef.current);
          if(acknowledged){
            saveAnimationPreference(animationStorage(),streamId,submitted.capture.owner.proposal_hash,body.feedback);
            pendingRefinement.current=null;setRefinementSending(false);refreshDrafts(value=>value+1);
            if(acknowledged.cleared)setRefinementOpen(false);
          }else{
            pendingRefinement.current=null;setRefinementSending(false);
            setRefinementError('The animation request could not be matched to this refinement. Your draft is preserved.');
          }
        }
        await poll(receipt);
      } catch(error){failed(error);}
    };
    const controller=new AbortController();
    setGeneration('working');setGenerationError(null);setGenerationNotice(repairing?CANVAS_REPAIR_NOTICE:'');
    const json=async(url,opts={})=>{const response=await fetch(url,{...opts,signal:controller.signal});if(!response.ok){
      let data;try{data=await response.json();}catch{}
      const stale=isStaleAnimationIntent(response,data);
      if(stale)intentOwner.lease.invalidate();
      const error=stale?animationIntentError():new Error('Visualization agent unavailable');
      if(!stale && response.status===409 && url==='/api/visualizations' && opts.method==='POST'
        && submittedBody?.expected_parent && submittedBody?.review_id
        && ['This correction does not match its saved visual review.','A newer scene request superseded this visual correction.'].includes(data?.detail))
        error.animationFailureKind='correction-rejected';
      error.status=response.status;throw error;
    }return response.json();};
    const nextPoll=receipt=>{timer=setTimeout(()=>json(`/api/visualizations/${receipt.id}`).then(poll).catch(error=>{
      if(!isCurrent())return;
      if(shouldReattachAnimationPoll(error.status,reattachments)){
        reattachments+=1;setGenerationNotice('Reconnecting to the saved animation request after the setup server restarted.');
        timer=setTimeout(()=>submit(submittedFeedback,{sameRequest:true}),2000);
      }else failed(error);
    }),1800);};
    const poll=async receipt=>{
      if(!isCurrent())return;
      const scope=animationActivityScope(receipt,requestBinding,request,currentRequestRef.current);
      if(!scope)throw new Error('The animation receipt belongs to another proposal or request.');
      setActivityOperation({scope,status:receipt.status});
      setGenerationNotice(pendingAnimationNotice(receipt) || (repairing?CANVAS_REPAIR_NOTICE:''));
      if(animationObservationUnavailable(receipt))throw Object.assign(new Error('The saved animation request cannot currently be observed.'),{animationFailureKind:'unavailable'});
      if(hasPublishedAnimation(receipt)){
        const artifact=receipt.artifact;
        const [blueprintHash,modelHash]=await Promise.all([fingerprint(plan),fingerprint(model)]);
        if(!isCurrent())return;
        if(artifact.blueprint_hash!==blueprintHash || artifact.model_hash!==modelHash)throw new Error('The completed animation belongs to another proposal revision.');
        if(artifact.contract==='freeform-animation-v3'){
          if(isCurrent() && displayedCodeHash!==artifact.code_hash){
            displayedCodeHash=artifact.code_hash;
            const next={key,artifact,model,plan,streamId,request,feedback:submittedFeedback,
              receipt:structuredClone(receipt),intentOwner,intentLease:intentOwner.lease};
            pendingCurrentCandidateRef.current=next;setCandidate(next);
          }
          if(shouldObserveAnimationTail(receipt))nextPoll(receipt);
          return;
        }
        let result;
        try {
          const layout=await runVisualizationCode(artifact.code,model);
          result=applyVisualizationLayout(base,layout,model);
        } catch(error) {
          if(!isCurrent())return;
          feedbackRef.current=`Program ${artifact.key} failed validation: ${String(error.message).slice(0,700)}. Return corrected complete code for the same model.`;
          if(!repaired){
            repaired=true;repairing=true;setGenerationNotice(CANVAS_REPAIR_NOTICE);
            return submit(feedbackRef.current);
          }
          throw Object.assign(new Error('The custom animation could not run.'),{animationFailureKind:'execution'});
        }
        if(isCurrent()){
          setGenerated({key,scene:result,artifact,model,plan,streamId});setGenerationError(null);setGenerationNotice('');setGeneration('ready');
        }
      }else if(receipt.status==='superseded' && receipt.replacement_id){return json(`/api/visualizations/${receipt.replacement_id}`).then(poll);
      }else if(receipt.status==='failed' || receipt.status==='interrupted')
        throw Object.assign(new Error('The visualization could not finish.'),{animationFailureKind:receipt.status});
      else nextPoll(receipt);
    };
    const failed=error=>{if(isCurrent()){
      const displayed=activeAnimationRef.current?.candidate,parent=submittedBody?.expected_parent;
      if(error?.animationFailureKind==='correction-rejected' && displayed && !displayed.recovered
        && displayed.key===key && displayed.streamId===streamId && parent?.operation_id===displayed.receipt?.id
        && parent.code_hash===displayed.artifact.code_hash && parent.blueprint_hash===displayed.artifact.blueprint_hash
        && parent.model_hash===displayed.artifact.model_hash && (parent.setup_id ?? null)===(setupId ?? null)
        && parent.stream_id===streamId){
        // The host explicitly declined this correction before admission. Keep
        // the accepted source/frame, without relabelling or replaying its body.
        setRestoredRequest({...request,kind:'correction-rejected',operation_id:displayed.receipt.id,code_hash:displayed.artifact.code_hash});
        expectedParentRef.current=null;reviewIdRef.current=null;reviewAdmissionRef.current=null;
        submittedAnimationRef.current=null;recheckAnimationRef.current=null;feedbackRef.current='';
        setGeneration('ready');setGenerationError(null);setGenerationNotice('');
        return;
      }
      setGeneration('fallback');setGenerationError(animationFailure(request,error?.animationFailureKind));
      if(pendingRefinement.current && animationRequestMatches(pendingRefinement.current.request,request)){
        pendingRefinement.current=null;setRefinementSending(false);
        setRefinementError('The refinement could not be confirmed. Your draft is preserved.');
      }
    }};
    timer=setTimeout(async()=>{
      try {
        await intentOwner.lease.binding();
        if(!isCurrent())return;
        const proposalHash=await fingerprint({blueprint:plan,model});
        if(!isCurrent())return;
        preferenceRef.current={streamId,proposalHash};
        const preference=readAnimationPreferenceRequest(animationStorage(),streamId,proposalHash);
        const selection=readAnimationSelection(animationSessionStorage(),{setup_id:setupId,stream_id:streamId});
        if(selection && request.attempt===0 && !expectedParent && !repairing && !pendingRefinement.current
          && !recheckAnimationRef.current && !feedbackRef.current && !preference?.feedback && !preference?.correction){
          const [blueprintHash,modelHash]=await Promise.all([fingerprint(plan),fingerprint(model)]);
          if(!isCurrent())return;
          if(selection.blueprint_hash===blueprintHash && selection.model_hash===modelHash){
            const scope={setup_id:setupId,stream_id:streamId};
            const snapshot={version:1,...scope,operation_id:selection.operation_id,blueprint:plan,model};
            const saved=await json(`/api/visualizations/${selection.operation_id}`);
            const checked=await recoveredAnimation(snapshot,saved,scope);
            if(!isCurrent())return;
            if(!checked || checked.artifact.code_hash!==selection.code_hash)
              throw new Error('The selected saved animation could not be verified.');
            const publication={operation_id:selection.operation_id,code_hash:selection.code_hash,
              blueprint_hash:blueprintHash,model_hash:modelHash,...scope};
            const attached=await json('/api/visualizations/published/adopt',{method:'POST',
              headers:{'Content-Type':'application/json'},body:JSON.stringify({publication,
                consumer_intent:await intentOwner.lease.binding(),require_latest:false})});
            const verified=await recoveredAnimation(snapshot,attached,scope);
            if(!isCurrent())return;
            if(!verified || verified.artifact.code_hash!==selection.code_hash)
              throw new Error('The selected saved animation could not be verified.');
            const next={...verified,key,streamId,request:{...request},adopted:true,
              intentOwner,intentLease:intentOwner.lease};
            setAdoptionPending(true);pendingCurrentCandidateRef.current=next;setCandidate(next);return;
          }
        }
        // A fresh viewer should resume checked current source, rather than replay
        // an older empty-feedback request from its generation cache. Explicit
        // choices, corrections, refinements and retries keep their own intent.
        if(setupId && (request.attempt===0 || animationRequestMatches(publicationResumeRef.current,request))
          && !expectedParent && !repairing && !pendingRefinement.current
          && !recheckAnimationRef.current && !feedbackRef.current && !preference?.feedback && !preference?.correction){
          const [blueprint_hash,model_hash]=await Promise.all([fingerprint(plan),fingerprint(model)]);
          if(!isCurrent())return;
          const scope={setup_id:setupId,stream_id:streamId};
          const query=new URLSearchParams({...scope,blueprint_hash,model_hash});
          publicationResumeRef.current={...request};
          let saved,absent=false;
          try{saved=await json(`/api/visualizations/published/latest?${query}`);}
          catch(error){if(error.status!==404)throw error;absent=true;}
          if(!isCurrent())return;
          if(!absent){
            const snapshot={version:1,...scope,operation_id:saved?.id,blueprint:plan,model};
            const checked=await recoveredAnimation(snapshot,saved,scope);
            if(!isCurrent())return;
            if(!checked)throw new Error('The latest saved animation could not be verified.');
            const publication={operation_id:saved.id,code_hash:checked.artifact.code_hash,blueprint_hash,model_hash,...scope};
            const attached=await json('/api/visualizations/published/adopt',{method:'POST',
              headers:{'Content-Type':'application/json'},body:JSON.stringify({publication,
                consumer_intent:await intentOwner.lease.binding()})});
            const verified=await recoveredAnimation(snapshot,attached,scope);
            if(!isCurrent())return;
            if(!verified || verified.artifact.code_hash!==publication.code_hash)
              throw new Error('The latest saved animation could not be verified.');
            const next={...verified,key,streamId,request:{...request},adopted:true,resumed:true,
              intentOwner,intentLease:intentOwner.lease};
            publicationResumeRef.current=null;
            setAdoptionPending(true);pendingCurrentCandidateRef.current=next;setCandidate(next);return;
          }
          publicationResumeRef.current=null;
        }
        if(preference?.correction && !expectedParent && !repairing && !pendingRefinement.current && !recheckAnimationRef.current
          && (!feedbackRef.current || feedbackRef.current===preference.correction.feedback)){
          restoringCorrection=preference;feedbackRef.current=preference.correction.feedback;
        }else if(!feedbackRef.current)feedbackRef.current=preference?.feedback || '';
        await submit(feedbackRef.current);
      } catch(error) { failed(error); }
    },650);
    const observer={stop:()=>{cancelled=true;clearTimeout(timer);controller.abort();}};
    generationObserverRef.current=observer;
    return()=>{observer.stop();if(generationObserverRef.current===observer)generationObserverRef.current=null;};
  },[key,streamId,retry]);

  async function adoptLatestAnimation(){
    if(!setupId || adoptionPending || generation==='working' || busy || pending || refinementOpen || refinementSending
      || pendingRefinement.current || pendingCurrentCandidateRef.current)return;
    const owner=intentOwnerRef.current;
    if(!owner || !animationRequestMatches(owner.request,currentRequestRef.current))return;
    const selection={request:{...request},controller:new AbortController()};
    adoptionRef.current=selection;setAdoptionPending(true);setAdoptionError('');
    setGenerationError(current=>current?.execution ? {...current,execution:null}:current);
    const current=()=>adoptionRef.current===selection && intentOwnerRef.current===owner
      && animationRequestMatches(selection.request,currentRequestRef.current);
    const read=async(url,options={})=>{
      const response=await fetch(url,{...options,signal:selection.controller.signal});
      const value=await response.json();
      if(!response.ok)throw new Error(response.status===404?'No saved animation matches this proposal and view.'
        :'The saved animation could not be selected. Check it again.');
      return value;
    };
    try{
      const [blueprint_hash,model_hash]=await Promise.all([fingerprint(plan),fingerprint(model)]);
      if(!current())return;
      const query=new URLSearchParams({setup_id:setupId,stream_id:streamId,blueprint_hash,model_hash});
      const receipt=await read(`/api/visualizations/published/latest?${query}`);
      if(!current())return;
      const scope={setup_id:setupId,stream_id:streamId};
      const snapshot={version:1,...scope,operation_id:receipt.id,blueprint:plan,model};
      if(!await recoveredAnimation(snapshot,receipt,scope))throw new Error('The saved animation does not match this proposal.');
      if(!current())return;
      // Stop this viewer's old observer before declaring a new source choice.
      // The saved operation and other viewers keep their original ownership.
      generationObserverRef.current?.stop();invalidateVisualReview();
      owner.lease=owner.declare();const lease=owner.lease;
      const consumer_intent=await lease.binding();
      if(!current() || owner.lease!==lease || !lease.active)return;
      const publication={operation_id:receipt.id,code_hash:receipt.artifact.code_hash,
        blueprint_hash,model_hash,...scope};
      const attached=await read('/api/visualizations/published/adopt',{method:'POST',
        headers:{'Content-Type':'application/json'},body:JSON.stringify({publication,consumer_intent})});
      const verified=await recoveredAnimation(snapshot,attached,scope);
      if(!current() || owner.lease!==lease || !lease.active)return;
      if(!verified || attached.artifact.code_hash!==publication.code_hash)throw new Error('The selected animation could not be verified.');
      const next={...verified,key,streamId,request:{...request},adopted:true,intentOwner:owner,intentLease:lease};
      // No original author feedback is synthesized or saved for this choice.
      submittedAnimationRef.current=null;recheckAnimationRef.current=null;
      expectedParentRef.current=null;reviewIdRef.current=null;reviewAdmissionRef.current=null;
      setActivityOperation(null);pendingCurrentCandidateRef.current=next;setCandidate(next);
    }catch(error){
      if(current()){adoptionRef.current=null;setAdoptionPending(false);
        setAdoptionError(error.message || 'The saved animation could not be selected. Check it again.');}
    }
  }

  function invalidateVisualReview(){reviewEpochRef.current.invalidate();reviewJobRef.current?.cancel();setVisualReview(null);setReviewRevision(value=>value+1);}
  function next(){invalidateVisualReview();if(authored){setPlaying(false);setAuthoredCaption(null);authoredRef.current?.seek(authoredFrame.chapters.find(item=>item.at>authoredFrame.time+.05)?.at ?? (authoredFrame.chapters.length?0:(authoredFrame.time+5)%authoredFrame.duration));}else jump((index+1)%model.phases.length);}
  function replay(){invalidateVisualReview();setAuthoredCaption(null);(authored?authoredRef:simRef).current?.seek(.05);setIndex(0);setPlaying(!reduced);}
  function jump(i){setPlaying(false);simRef.current?.seek(i*4.2+.05);setIndex(i);}
  function inspect(id,event){invalidateVisualReview();originRef.current=event.currentTarget;setSelectedId(id);setGroupId(null);}
  function closeInspector(){
    invalidateVisualReview();
    for(const animation of [activeAnimationRef.current,previousAnimationRef.current]){
      if(animation?.captionSelectionId){animation.caption=animation.overviewCaption;animation.captionSelectionId=null;}
    }
    if(activeAnimationRef.current)setAuthoredCaption(activeAnimationRef.current.caption);
    setSelectedId(null);setGroupId(null);
    (originRef.current?.isConnected?originRef.current:exploreRef.current)?.focus({preventScroll:true});
  }
  function identityPageTo(index){
    pageFocusRef.current=true;
    setIdentityPage({sourceKey:visibleSourceKey,groupId,search,streamId,index});
  }
  function retryAnimation(){publicationResumeRef.current=null;recheckAnimationRef.current=null;expectedParentRef.current=null;reviewIdRef.current=null;reviewAdmissionRef.current=null;setVisualReview(null);setGenerationError(null);setGenerationNotice('');setGeneration('working');setRetry(value=>value+1);}
  function checkAnimation(){
    if(animationRequestMatches(publicationResumeRef.current,currentRequestRef.current)){
      publicationResumeRef.current={...request,attempt:retry+1};
      setVisualReview(null);setGenerationError(null);setGenerationNotice('Checking the latest saved animation for this proposal.');
      setGeneration('working');setRetry(value=>value===request.attempt?value+1:value);return;
    }
    const submitted=submittedAnimationRef.current;
    if(!submitted || !animationRequestMatches(submitted.request,currentRequestRef.current))return retryAnimation();
    recheckAnimationRef.current={request:{...request,attempt:retry+1},body:structuredClone(submitted.body)};
    setVisualReview(null);setGenerationError(null);setGenerationNotice('Checking the same saved animation request.');
    setGeneration('working');setRetry(value=>value===submitted.request.attempt?value+1:value);
  }
  function refineAnimation(event){
    event.preventDefault();
    if(!currentAnimation || generation==='working' || refinementSending || !refinementDraft.ready ||
      !animationRequestMatches(request,currentRequestRef.current))return;
    let feedback;
    try{feedback=checkedAnimationFeedback(refinementText,{required:true});}
    catch(error){setRefinementError(error.message);refinementInputRef.current?.focus();return;}
    const capture=drafts.capture(draftSelection);
    if(!capture)return;
    pendingRefinement.current={request:{...request,attempt:retry+1},selection:draftSelection,capture};
    feedbackRef.current=feedback;setRefinementSending(true);setRefinementError('');retryAnimation();
  }
  function refineWithGuide(){
    const current=guideRefinementRef.current;
    if(!selected || busy || pending || updating || current.busy || current.pending || current.updating ||
      !current.onRequestChange || current.visibleSourceKey!==visibleSourceKey || current.selected?.id!==selected.id ||
      !animationRequestMatches(request,current.request))return;
    current.onRequestChange(`Help me refine ${current.selected.name}: its ${current.selected.kind==='agent'?'responsibilities, collaborators and tool access':'configuration and who can use it'}, in the context of this system.`);
  }
  function keyboard(e){
    if(e.key==='Escape'){if(selectedId || groupId)closeInspector();else if(wide){invalidateVisualReview();setWide(false);expandRef.current?.focus();}}
    if(wide)trapExpandedAnimationFocus(e,sectionRef.current);
  }
  keyboardRef.current=keyboard;
  const {sx,sy,ox,oy}=simulationViewport(scene,box.width,box.height);
  const denseIds=new Set(scene.groups?.filter(g=>g.dense).flatMap(g=>g.memberIds) || []);
  const hits=scene.nodes.filter(n=>n.kind!=='text' && !denseIds.has(n.id));
  if(!plan)return <section className="ps-empty" aria-label="Your proposal simulation"><div className="ps-empty-mark" aria-hidden="true"><i/><span/><i/></div><span className="ps-kicker">YOUR MOB TAKES SHAPE HERE</span><h3>From an idea to a living system.</h3><p>Tell your guide what you want to build. Your agents, their connections, and the way they work together will unfold here.</p><small>A tailored simulation, built alongside your proposal.</small></section>;
  return <section ref={sectionRef} onKeyDown={keyboard} role={wide?'dialog':undefined} aria-modal={wide || undefined} className={`proposal-simulation ${wide?'ps-expanded':''} ${playing?'is-playing':'is-paused'}`} aria-label="Animated simulation of your current proposal" data-node-count={agents.length} data-details-open={!!(selectedId || groupId || refinementOpen)} data-phase={active?.id || chapter?.title} data-animation={authored?"custom":"automatic"} data-updating={updating?"true":"false"}>
    <RehearsalActivityBanner activity={rehearsalActivity} names={rehearsalNames} />
    <div className="ps-toolbar"><div><span className="ps-live-dot"/><strong>{pending?'Previewing adjustments':'YOUR MOB IN MOTION'}</strong></div><button ref={expandRef} onClick={()=>{invalidateVisualReview();if(!wide)onExpand?.();setWide(!wide);}} aria-expanded={wide}>{wide?'Close expanded view':'Expand'}</button></div>
    <AnimationGenerationStatus request={request} generation={generation} failure={generationError}
      notice={generationNotice} authored={!!authored} updating={proposalChanged}
      recovered={!!authored && generated.recovered===true}
      restored={!!authored && animationRequestMatches(restoredRequest,request)}
      correctionRejected={correctionRejected}
      currentScene={currentAnimation}
      refining={!!feedbackRef.current} onRetry={['unavailable','intent'].includes(generationError?.kind)?checkAnimation:retryAnimation} />
    <div className="ps-scenario"><div><select aria-label="View to simulate" value={model.selectedFlowId || '__overview__'} onChange={e=>{invalidateVisualReview();setFlowId(e.target.value);}}><option value="__overview__">Team & responsibilities</option>{model.flows.map(f=><option key={f.id} value={f.id}>Workflow: {f.name}</option>)}</select><span>{updating?'Previous preview: ':''}{agents.length} identities · {(authored?generated.plan:plan).agent_classes.length} classes{updating?' · updating...':''}</span></div><button aria-pressed={showPeers} onClick={()=>{invalidateVisualReview();setShowPeers(!showPeers);}}>Tool access</button></div>
    <div className="ps-instrument">
      <div className="ps-instrument-key"><span>PROPOSAL SIMULATION</span>{!authored && <div><span><i className="agent"/>agent</span><span><i className="source"/>connection</span><span><i className="memory"/>memory</span></div>}</div>
      <div className={`ps-stage${smallTeamPreview(model.nodes.filter(node=>node.kind==='agent').length,model.selectedFlowId) ? ' ps-stage--small' : ''}`}>
        <div className="ps-authored-host" ref={authoredHostRef} style={{visibility:authored?'visible':'hidden'}}/>
        {!authored && <><canvas ref={canvasRef} aria-label={`Network overview of ${agents.length} configured identities`} role="img"/>
        <div className="ps-hit-layer">{hits.map(n=><button key={n.id} className="ps-hit" style={{left:ox+n.x*sx,top:oy+n.y*sy}} aria-label={`Inspect ${n.name || n.label}`} title={n.name || n.label} onClick={e=>inspect(n.id,e)}/>)}
</div></>}
      </div>
      {!authored && scene.groups?.some(g=>g.dense) && <div className="ps-populations" aria-label="Agent populations">{scene.groups.filter(g=>g.dense).map(g=><button key={g.id} aria-label={`Inspect ${g.name}, ${g.count} identities`} onClick={e=>{originRef.current=e.currentTarget;setGroupId(g.id);setSelectedId(null);setSearch('');setIdentityPage(null);}}><span>{g.name}</span><strong>{g.count}</strong></button>)}</div>}
      <div className="ps-playback"><div className="ps-controls"><button aria-label={playing?'Pause simulation':'Play simulation'} onClick={()=>{invalidateVisualReview();setPlaying(!playing);}}><ControlIcon type={playing?'pause':'play'}/></button><button aria-label="Next simulation step" onClick={next}><ControlIcon type="step"/></button><button aria-label="Replay simulation" onClick={replay}><ControlIcon type="replay"/></button><span>{authored?`${Math.floor(authoredFrame.time)}s / ${Math.round(authoredFrame.duration)}s`:`${String(index+1).padStart(2,'0')} / ${String(model.phases.length).padStart(2,'0')}`}</span></div><div className="ps-caption" tabIndex={wide?0:undefined} aria-live={playing?'off':'polite'}>{!!selected && !selectionCaption && <span className="ps-caption-context">Playback chapter</span>}<strong>{active?.title}</strong>{typeof caption==='string' && caption.length>220
        ? <div className="ps-caption-description"><p className="ps-caption-preview">{caption}</p><details className="ps-caption-details"><summary>More about this step</summary><p>{caption}</p></details></div>
        : <p>{caption}</p>}</div></div>
      <div className="ps-progress"><i ref={progressRef} style={authored?{width:`${100*authoredFrame.time/authoredFrame.duration}%`}:undefined}/></div>
      {!!visibleModel.profile_only_classes?.length && <p className="ps-status" aria-label="Reusable classes without initial agents">Reusable classes - no initial agents: {visibleModel.profile_only_classes.map(item=>item.name).join(', ')}.</p>}
    </div>
    <ProposalChangeNotice changes={proposalChanges} working={generation==='working'}/>
    <div className="ps-footer">
      <div>{setupId && <button disabled={adoptionPending || generation==='working' || !!busy || !!pending || refinementOpen || refinementSending}
        onClick={adoptLatestAnimation}>{adoptionPending?'Opening saved animation...':'Use latest saved animation'}</button>}<button disabled={adoptionPending} aria-expanded={refinementOpen} aria-controls={`animation-feedback-${streamId}`} onClick={()=>setRefinementOpen(value=>!value)}>Refine animation</button><button ref={exploreRef} onClick={event=>{invalidateVisualReview();originRef.current=event.currentTarget;setGroupId('__all');setSelectedId(null);setSearch('');setIdentityPage(null);setPlaying(false);}}>Explore identities</button></div></div>
    <div className="ps-details" tabIndex={wide?0:undefined} role={wide?'region':undefined} aria-label={wide?'Animation details':undefined}>
    {adoptionError && <p role="status">{adoptionError}</p>}
    {executionDetail && <details className="ps-code"><summary>Animation diagnostics</summary>
      <p>Failure: {executionDetail.kind}/{executionDetail.phase}{executionDetail.exception ? ` (${executionDetail.exception})`:''}.</p>
      {executionDetail.diagnostics && <><p>Frame observations for this failed preview. Worker timing is diagnostic only; it does not verify a displayed frame.</p>
        <pre>{JSON.stringify(executionDetail.diagnostics,null,2)}</pre></>}
    </details>}
    {currentAnimation && generated.adopted && <p role="status">{generated.resumed
      ? 'Resumed the latest saved animation for this proposal. Use Refine animation to change it.'
      : 'Showing the saved animation you selected. Use Refine animation to change it.'}</p>}
    <AnimationActivity operation={activityOperation} currentRequest={request}/>
    <AnimationReviewStatus review={visualReview?.presentationEpoch===reviewEpochRef.current.current ? visualReview:null}/>
    {refinementOpen && <AnimationRefinementForm streamId={streamId} text={refinementText} error={refinementError}
      persistent={refinementDraft.persistent} ready={refinementDraft.ready} sending={refinementSending}
      waiting={generation==='working' || !currentAnimation}
      inputRef={refinementInputRef} onText={text=>{drafts.update(draftSelection,text);refreshDrafts(value=>value+1);setRefinementError('');}}
      onSubmit={refineAnimation} onCancel={()=>{setRefinementOpen(false);setRefinementError('');}}/>}
    {(selected || group || groupId==='__all') && <div className="ps-inspector" ref={inspectorRef} tabIndex={-1} role="region" aria-label={`${selected?.name || group?.name || 'All identities'} details`}>
      <div className="ps-inspector-heading"><div><span>{selected?selected.kind==='agent'?'Agent identity':selected.role:'Population'}</span><h4>{selected?.name || group?.name || 'All identities'}</h4></div><button aria-label="Close inspector" onClick={closeInspector}>×</button></div>
      {selected?<><p>{selected.role}</p>{selected.kind==='agent' && <small>{selected.tools?.memory?'Own conversation memory enabled':'Identity memory is off'}</small>}<details><summary>{selected.kind==='agent'?'Responsibilities & instructions':'How this connection works'}</summary><p>{selected.instructions}</p><code>{selected.id}</code></details><OwnerRelationships key={JSON.stringify([visibleSourceKey,streamId,selected.id])} selected={selected} model={visibleModel} onInspect={inspect}/>{onRequestChange && <><button className="ps-refine" disabled={busy || pending || updating} onClick={refineWithGuide}>Refine with your guide ↗</button>{updating && <p role="status">This is a previous preview. Wait for the current preview before refining this identity.</p>}</>}</>:<><p>{identityResults.populationCount} configured identities. Browse every member or search by name or identity.</p><input aria-label="Find an identity" placeholder="Find an identity..." value={search} onChange={e=>setSearch(e.target.value)}/><div className="ps-member-results" ref={memberResultsRef}>{identityResults.items.map(n=><button key={n.id} onClick={e=>inspect(n.id,e)}><strong>{n.name}</strong><code>{n.id}</code></button>)}</div><small role="status" aria-live="polite" aria-atomic="true">{identityResults.matchCount?`Showing ${identityResults.start}-${identityResults.end} of ${identityResults.matchCount} matching identities.`:'No matching identities.'}</small>{identityResults.pageCount>1 && <nav aria-label="Identity result pages" style={{display:'flex',flexWrap:'wrap',alignItems:'center',justifyContent:'space-between',gap:8,marginTop:10}}><button type="button" disabled={!identityResults.hasPrevious} onClick={()=>identityPageTo(identityResults.pageIndex-1)}>Previous identities</button><small>Page {identityResults.pageIndex+1} of {identityResults.pageCount}</small><button type="button" disabled={!identityResults.hasNext} onClick={()=>identityPageTo(identityResults.pageIndex+1)}>Next identities</button></nav>}</>}
    </div>}
    {generated?.key===key && <details className="ps-code"><summary>Animation source</summary><p>{generated.artifact.summary}</p><pre>{generated.artifact.code}</pre></details>}
    </div>
  </section>;
}
