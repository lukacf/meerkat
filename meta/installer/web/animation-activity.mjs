import { animationRequestMatches } from './animation-publication.mjs';
import { createGuideFrameLog, ingestGuideTimeline } from './native-guide-chat.mjs';
import { buildConversationViewState, mapFramesToTimelineEntries,
  sortConversationTimelineEntries, parseConversationRichBlocks, hasAssistantMessageIdCarrier } from '../../packages/console-core/src/index.ts';

const digest = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const identifier = value => typeof value === 'string' && value.length > 0 && value.length <= 256 && !/[\u0000-\u001f\u007f]/.test(value);
const cursor = value => typeof value === 'string' && /^console:\d+$/.test(value) && Number.isSafeInteger(Number(value.slice(8)));
const statuses = new Set(['queued', 'running', 'observing', 'published', 'completed', 'failed', 'interrupted', 'superseded']);
const compactions = new Set(['compaction_started', 'compaction_completed', 'compaction_failed']);
const fields = ['id', 'setup_id', 'stream_id', 'request_fingerprint', 'blueprint_hash', 'model_hash', 'native_request_key', 'source_generation'];

function contextPreservationPhase(frames = []) {
  let phase = null;
  for (const frame of frames) {
    const kind = frame.event || frame.kind;
    if (compactions.has(kind)) phase = kind;
  }
  return phase;
}

/** The HTTP receipt identifies the saved request; native admission is separate. */
export function animationActivityScope(receipt, binding, request, current) {
  if (!animationRequestMatches(request, current) || !digest(receipt?.id) || !statuses.has(receipt.status) ||
      !digest(receipt.native_request_key) || !identifier(receipt.stream_id) ||
      !Number.isSafeInteger(receipt.source_generation) || receipt.source_generation < 1 ||
      (receipt.setup_id ?? null) !== (binding?.setup_id ?? null) || receipt.stream_id !== binding?.stream_id ||
      !['request_fingerprint', 'blueprint_hash', 'model_hash'].every(name => digest(receipt[name]) && receipt[name] === binding[name])) return null;
  return { ...Object.fromEntries(fields.map(name => [name, receipt[name] ?? null])), request: { ...request } };
}

export function animationActivityKey(scope) {
  return scope ? JSON.stringify([...fields.map(name => scope[name]), scope.request]) : null;
}

export function animationActivityUrl(scope, after) {
  const query = new URLSearchParams({ limit: '100' });
  for (const name of fields.slice(1)) if (scope[name] !== null) query.set(name, scope[name]);
  if (cursor(after)) query.set('after', after);
  return `/api/visualizations/${encodeURIComponent(scope.id)}/timeline?${query}`;
}

export function animationActivityPageMatches(page, scope) {
  if (!scope || page?.scope !== 'saved-native-request' || !fields.every(name => (page[name] ?? null) === scope[name]) || !statuses.has(page.status) ||
      !['request-bound', 'retained'].includes(page.activity_origin) ||
      !['available', 'unavailable'].includes(page.observation?.status) ||
      typeof page.native?.admission_confirmed !== 'boolean' || !['unknown','completed','failed','interrupted'].includes(page.native.status) ||
      typeof page.publication?.available !== 'boolean' ||
      !Array.isArray(page.frames) || page.frames.length > 200 || typeof page.exhausted !== 'boolean') return false;
  if (page.publication.available && !digest(page.publication.code_hash)) return false;
  if (page.native.admission_confirmed) {
    if (!identifier(page.native.interaction_id) || (page.native.session_id != null && !identifier(page.native.session_id)) ||
        (page.native.identity != null && !identifier(page.native.identity))) return false;
  } else if (page.frames.length || page.native.interaction_id || page.native.session_id || page.native.identity) return false;
  if (page.native.status !== 'unknown' && (!page.native.admission_confirmed || !cursor(page.native.terminal_cursor))) return false;
  const matches = (frame,depth=0) => frame && depth<8 && identifier(frame.id) && cursor(frame.cursor) &&
    frame.interaction_id === page.native.interaction_id &&
    (frame.identity == null || frame.identity === (page.native.identity ?? 'visualizer:main')) &&
    (frame.session_id == null || page.native.session_id == null || frame.session_id === page.native.session_id) &&
    (frame.kind !== 'frame_updated' || matches(frame.payload?.frame,depth+1));
  for (const frame of page.frames) {
    if (!matches(frame)) return false;
  }
  return ['next_cursor', 'latest_cursor'].every(name => page[name] == null || cursor(page[name]));
}

export function createAnimationActivityLog(scope) {
  const log = createGuideFrameLog();
  let interactionId = null, sessionId = null, nativeIdentity = null, after;
  return {
    get after() { return after; },
    accept(page) {
      if (!animationActivityPageMatches(page, scope) ||
          (interactionId && (page.native.interaction_id !== interactionId || page.native.session_id !== sessionId ||
            (page.native.identity ?? 'visualizer:main') !== nativeIdentity))) return null;
      if (page.native.admission_confirmed) {
        interactionId = page.native.interaction_id; sessionId = page.native.session_id;
        nativeIdentity = page.native.identity ?? 'visualizer:main';
      }
      const frames = ingestGuideTimeline(log, { ...page, frames: page.frames.map(animationSummaryFrame).filter(Boolean) });
      // Unadmitted or unavailable empty observations cannot skip later real activity.
      const next = page.native.admission_confirmed && page.observation.status === 'available' ? page.next_cursor || after : after;
      const more = page.exhausted === false && !!next && next !== after;
      after = next;
      return { frames: [...frames], page: { ...page, context_preservation: contextPreservationPhase(frames) }, more };
    },
  };
}

/** Presentation copies only: explicit public summaries use the Console's thinking block. */
export function animationSummaryFrame(frame) {
  if (frame.kind === 'frame_updated' && frame.payload?.frame) {
    const inner = animationSummaryFrame(frame.payload.frame);
    return inner ? { ...frame, payload: { ...frame.payload, frame: inner } } : null;
  }
  const summary = frame.payload?.summary;
  if (typeof summary !== 'string' || !summary) return frame.kind?.startsWith('reasoning') ? null : frame;
  const assistant = Object.hasOwn(frame.payload, 'assistant_message_id')
    ? { assistant_message_id: frame.payload.assistant_message_id } : {};
  if (['reasoning_summary_delta', 'reasoning_delta'].includes(frame.kind)) return {
    ...frame, kind: 'reasoning_delta', payload: { delta: summary, ...assistant },
  };
  if (['reasoning_summary', 'reasoning_summary_complete', 'reasoning_complete'].includes(frame.kind)) return {
    ...frame, kind: 'reasoning_complete', payload: { text: summary, ...assistant },
  };
  return frame;
}

/** Both sides of a delayed read must still own this exact operation and attempt. */
export async function readAnimationActivity(load, scope, current, log) {
  if (!current(scope)) return null;
  const page = await load(animationActivityUrl(scope, log.after));
  if (!current(scope)) return null;
  return log.accept(page);
}

export function animationActivityPresentation(page, interrupted = false) {
  const nativeLabels={completed:'Animation author finished',failed:'Animation author failed',interrupted:'Animation author interrupted'};
  const knownTerminal=!!nativeLabels[page?.native?.status];
  if (interrupted || (page?.native?.admission_confirmed && page?.observation?.status === 'unavailable')) return {
    label: knownTerminal?nativeLabels[page.native.status]:'Activity could not be checked', active: !knownTerminal && page?.activity_origin!=='retained',
    detail: knownTerminal?'The saved native terminal state is retained. Public activity could not be checked.'
      :'The saved request remains unchanged. This observation does not establish native failure.',
  };
  if (!page) return { label: 'Checking saved activity', active: true, detail: 'Reading this request does not start another animation.' };
  if (page.status === 'failed' && !page.native.admission_confirmed) return {
    label: 'Request failed before native admission', active: false,
    detail: 'No native admission was recorded. Your saved animation remains available.',
  };
  const retained = page.activity_origin === 'retained';
  const status = page.native.status;
  const terminal = ['completed', 'failed', 'interrupted'].includes(status);
  const preserving = !terminal && (page.context_preservation ?? contextPreservationPhase(page.frames)) === 'compaction_started';
  return { label: retained ? 'Saved animation activity' : !page.native.admission_confirmed ? 'Waiting for native admission'
    : status === 'completed' ? 'Animation author finished' : status === 'failed' ? 'Animation author failed'
      : status === 'interrupted' ? 'Animation author interrupted' : preserving ? 'Preserving context' : 'Animation agent activity',
    active: !retained && !terminal && ['queued', 'running', 'observing', 'published'].includes(page.status),
    detail: retained ? 'Retained native request history. This does not mean new work was started.'
      : !page.native.admission_confirmed ? 'The host accepted this request. No native admission is confirmed yet.'
        : preserving ? 'The native agent is preserving earlier context before continuing. Its public context summary is available below.'
        : page.publication.available && !terminal ? 'An animation is published. Its native completion is still being observed.'
          : terminal && !page.publication.available ? 'No validated animation publication was recorded for this native result.'
            : 'Actual saved native summaries, tools and results for this animation request.' };
}

export function animationActivityConversation(scope, frames = [], native = null) {
  const identity = native?.admission_confirmed && identifier(native.identity) ? native.identity : 'visualizer:main';
  const agent = { identity, agent_id: identity, member_id: identity, label: 'Visualization agent', kind: 'host', role: 'Visualization agent' };
  const completed=[],replaced=new Set(),activity=[],contextGroups=[],contextMarkers=new Set();
  let openText=null,lastCompleteText=null,context=null;
  for(const frame of frames){
    if(compactions.has(frame.event)){
      openText=null;
      // Keep native boundaries in the Console adapter so ordinary public
      // output on either side cannot merge into one text segment.
      activity.push(frame);
      contextMarkers.add(frame.id);
      if(frame.event==='compaction_started'){
        context={frame,status:frame.event,parts:[],open:''};
        contextGroups.push(context);
      }else{
        if(context)context.status=frame.event;
        else contextGroups.push({frame,status:frame.event,parts:[],open:''});
        context=null;
      }
      continue;
    }
    // Native assistant identity and canonical history belong to the Console.
    // Keep its exact carrier, including malformed IDs that must not fall back
    // to legacy matching. Only unkeyed predecessor text uses the segment shim.
    if(hasAssistantMessageIdCarrier(frame)){
      openText=null;
      activity.push(frame);
      continue;
    }
    if(context && ['text_delta','text_complete'].includes(frame.event)){
      if(frame.event==='text_delta'){
        const text=frame.data?.delta ?? frame.data?.text;
        if(typeof text==='string')context.open+=text;
      }else if(typeof frame.data?.text==='string'){
        context.parts.push(frame.data.text);
        context.open='';
      }
      continue;
    }
    if(frame.event==='text_delta')openText ||= frame.id;
    else if(frame.event==='text_complete' && typeof frame.data?.text==='string' && frame.data.text.trim()){
      if(openText)replaced.add(openText);
      const time=new Date(frame.timestampMs);
      completed.push({kind:'message',id:`animation-output:${frame.id}`,interactionId:frame.interactionId,
        identity:{id:identity,label:agent.label,role:'assistant'},variant:'rich',
        createdAt:typeof frame.timestampMs==='number' && Number.isFinite(time.getTime())?time.toISOString():undefined,
        blocks:parseConversationRichBlocks(frame.data.text,{displayNormalization:false})});
      lastCompleteText=frame.data.text.trim();
      openText=null;
      continue;
    }else openText=null;
    // Terminal state is shown from native evidence above. An empty payload is
    // not an assistant response and must not become a visible JSON object.
    if(frame.event==='interaction_complete' && frame.data && typeof frame.data==='object' &&
      (!Object.keys(frame.data).length || (typeof frame.data.text==='string' && frame.data.text.trim()===lastCompleteText)))continue;
    activity.push(frame);
  }
  for(const group of contextGroups){
    const time=new Date(group.frame.timestampMs);
    const text=[...group.parts,group.open].filter(Boolean).join('\n\n');
    const phase=group.status==='compaction_completed'?'Context preservation completed.'
      :group.status==='compaction_failed'?'Context preservation failed.':'Preserving earlier context before continuing.';
    completed.push({kind:'message',id:`animation-context:${group.frame.id}`,interactionId:group.frame.interactionId,
      identity:{id:identity,label:agent.label,role:'assistant'},variant:'rich',
      createdAt:typeof group.frame.timestampMs==='number' && Number.isFinite(time.getTime())?time.toISOString():undefined,
      blocks:[{type:'paragraph',text:phase},
        // These Console flags collapse already-recorded public text. Native
        // interaction completion remains separate from this presentation.
        {type:'thinking',label:'Context summary',text,final:true,persisted:true}]});
  }
  const entries = sortConversationTimelineEntries([...mapFramesToTimelineEntries(agent, activity, {
    renderInteractionStartsAsUser: true, renderTextDeltas: true, suppressEmbeddedRunStartedPrompt: true, textMode: 'markdown',
  }).filter(entry => entry.identity?.role !== 'user' && ![...replaced,...contextMarkers].some(id=>entry.id===id || entry.id.startsWith(`${id}:`))).map(entry => ({ ...entry,
    ...(entry.blocks ? { blocks: entry.blocks.map(block => block.type === 'thinking'
      ? { ...block, label: 'Reasoning summary' } : block) } : {}),
  })),...completed]);
  return { entries, viewState: { ...buildConversationViewState({ memberId: identity, agentLabel: agent.label, agent, entries }),
    conversationId: `animation:${scope.id}` } };
}
