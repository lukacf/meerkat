import { test } from 'node:test';
import assert from 'node:assert/strict';
import { loadTestModule as load } from './test-support/bundle.mjs';
import { fileURLToPath } from 'node:url';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const { animationActivityScope, animationActivityKey, animationActivityUrl, animationActivityPageMatches,
  createAnimationActivityLog, readAnimationActivity, animationActivityPresentation, animationActivityConversation,
  animationSummaryFrame } = await load('animation-activity.mjs');
const { AnimationActivityView, AnimationRefinementForm } = await load('proposal-simulation.jsx', {
  loader: { '.css': 'empty' }, nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
});

const request = { streamId: 'guide-one:__overview__', proposalKey: 'exact current proposal', attempt: 2 };
const binding = { setup_id: 'guide-one', stream_id: request.streamId, request_fingerprint: 'a'.repeat(64), blueprint_hash: 'b'.repeat(64), model_hash: 'c'.repeat(64) };
const receipt = { ...binding, id: 'd'.repeat(64), native_request_key: 'e'.repeat(64), source_generation: 7, status: 'running' };
const scope = animationActivityScope(receipt, binding, request, request);
const frame = (number, kind, payload = {}) => ({ id: `unit-frame-${number}`, cursor: `console:${number}`,
  interaction_id: 'actual-bound-interaction', session_id: 'actual-bound-session', identity: 'visualizer:main',
  kind, payload, timestamp_ms: 1000 + number });
const page = (overrides = {}) => ({ ...receipt, scope: 'saved-native-request', activity_origin: 'request-bound',
  publication: { available: false }, native: { admission_confirmed: true, interaction_id: 'actual-bound-interaction',
    session_id: 'actual-bound-session', status: 'unknown', terminal_cursor: null },
  observation: { status: 'available', detail: null }, frames: [], next_cursor: 'console:10', latest_cursor: 'console:10', exhausted: true,
  ...overrides });

test('activity scope refuses foreign setup, view, proposal and attempts without borrowing an old native request', () => {
  const before = structuredClone(receipt);
  assert.ok(scope);
  for (const field of ['setup_id', 'stream_id', 'request_fingerprint', 'blueprint_hash', 'model_hash']) {
    assert.equal(animationActivityScope({ ...receipt, [field]: 'foreign' }, binding, request, request), null);
  }
  for (const changed of [{ ...request, attempt: 3 }, { ...request, proposalKey: 'new proposal' }, { ...request, streamId: 'new-view' }]) {
    assert.equal(animationActivityScope(receipt, binding, request, changed), null);
  }
  const url = new URL(animationActivityUrl(scope, 'console:30'), 'http://localhost');
  assert.equal(url.pathname, `/api/visualizations/${receipt.id}/timeline`);
  for (const key of ['setup_id', 'stream_id', 'request_fingerprint', 'blueprint_hash', 'model_hash', 'native_request_key', 'source_generation']) assert.equal(url.searchParams.get(key), String(scope[key]));
  assert.equal(url.searchParams.get('after'), 'console:30');
  assert.equal(animationActivityKey({ ...scope, source_generation: 8 }) === animationActivityKey(scope), false);
  assert.deepEqual(receipt, before);
});

test('exact page joins and adopted native interaction/session prevent mixed or stale history', () => {
  const initial = page({ frames: [frame(10, 'text_complete', { text: 'Public unit-test result' })] }), before = structuredClone(initial);
  assert.equal(animationActivityPageMatches(initial, scope), true);
  const log = createAnimationActivityLog(scope);
  assert.equal(log.accept(initial).frames.length, 1);
  for (const field of ['id', 'setup_id', 'stream_id', 'request_fingerprint', 'blueprint_hash', 'model_hash', 'native_request_key', 'source_generation']) {
    assert.equal(log.accept({ ...initial, [field]: 'foreign' }), null);
  }
  assert.equal(log.accept(page({ native: { ...initial.native, interaction_id: 'new-interaction' }, frames: [] })), null);
  assert.equal(log.accept(page({ native: { ...initial.native, session_id: 'different-session' } })), null);
  assert.equal(log.accept(page({ frames: [{ ...frame(11, 'text_complete', { text: 'Wrong output' }), interaction_id: 'other-interaction' }] })), null);
  assert.equal(log.accept(page({ frames: [frame(11, 'frame_updated', {frame:{...frame(10,'text_complete',{text:'Foreign update'}),interaction_id:'foreign'}})] })), null);
  assert.equal(log.accept(page({ frames: Array.from({ length: 201 }, (_, index) => frame(index, 'text_complete', { text: 'too many' })) })), null);
  assert.equal(log.accept(page({ publication: { available: true, code_hash: 'wrong' } })), null);
  assert.equal(log.accept(page({ scope: 'different-activity' })), null);
  assert.equal(log.accept(page({ native: { ...initial.native, status: 'completed', terminal_cursor: null } })), null);
  assert.deepEqual(initial, before);
});

test('queued requests show no fabricated frames, and visibility-filtered cursor progress reaches later public activity', () => {
  const log = createAnimationActivityLog(scope);
  const queued = page({ status: 'queued', native: { admission_confirmed: false, status: 'unknown', interaction_id: null, session_id: null },
    observation: { status: 'unavailable' }, next_cursor: null, latest_cursor: null });
  assert.deepEqual(log.accept(queued).frames, []);
  assert.equal(log.after, undefined);
  assert.equal(animationActivityPresentation(queued).label, 'Waiting for native admission');
  const beforeAdmissionFailure = animationActivityPresentation({ ...queued, status: 'failed' });
  assert.equal(beforeAdmissionFailure.label, 'Request failed before native admission');
  assert.equal(beforeAdmissionFailure.active, false);
  assert.equal(log.accept({ ...queued, frames: [frame(10, 'text_complete', { text: 'Pretend native output' })] }), null);
  assert.equal(log.accept({ ...queued, native: { ...queued.native, identity: 'pretend-author' } }), null);
  const gap = log.accept(page({ frames: [], next_cursor: 'console:100', exhausted: false }));
  assert.equal(gap.more, true);
  assert.equal(log.after, 'console:100');
  const actual = log.accept(page({ frames: [frame(101, 'text_complete', { text: 'Later public output' })], next_cursor: 'console:101' }));
  assert.equal(actual.frames.length, 1);
  assert.equal(log.accept(page({ frames: [frame(101, 'text_complete', { text: 'Later public output' })], next_cursor: 'console:101' })).frames.length, 1);
});

test('Console presentation adopts the admitted author and refuses changed or mixed identities', () => {
  const identity = 'visualizer-request-fresh-author';
  const native = { ...page().native, identity };
  const ownFrame = { ...frame(10, 'text_complete', { text: 'Fresh author publication.' }), identity };
  const wire = page({ native, frames: [ownFrame] });
  const log = createAnimationActivityLog(scope), result = log.accept(wire);
  const { entries, viewState } = animationActivityConversation(scope, result.frames, result.page.native);
  assert.equal(viewState.conversationId, `animation:${receipt.id}`);
  assert.equal(entries.find(entry => entry.id.startsWith('animation-output:')).identity.id, identity);
  const legacy = createAnimationActivityLog(scope).accept(page({ frames: [frame(11, 'text_complete', { text: 'Legacy output.' })] }));
  assert.equal(animationActivityConversation(scope, legacy.frames).entries.find(entry => entry.id.startsWith('animation-output:')).identity.id, 'visualizer:main');
  assert.equal(log.accept(page({ native: { ...native, identity: 'another-author' } })), null);
  assert.equal(log.accept(page({ native: { ...native, identity: undefined } })), null);
  assert.equal(log.accept(page({ native, frames: [frame(11, 'text_complete', { text: 'Legacy author output.' })] })), null);
  assert.equal(log.accept(page({ native, frames: [{ ...ownFrame, kind: 'frame_updated',
    payload: { frame: frame(11, 'text_complete', { text: 'Foreign nested output.' }) } }] })), null);
  assert.equal(animationActivityPageMatches(page({ native: { ...native, identity: '\u0000invalid' } }), scope), false);
  const html = renderToStaticMarkup(React.createElement(AnimationActivityView, { scope, frames: result.frames, page: result.page }));
  assert.match(html, /Fresh author publication/);
});

test('delayed observation cannot publish into a changed attempt, and stale calls perform no read', async () => {
  let current = scope, calls = 0, finish;
  const owns = value => animationActivityKey(current) === animationActivityKey(value);
  const log = createAnimationActivityLog(scope);
  const result = readAnimationActivity(() => { calls++; return new Promise(resolve => { finish = resolve; }); }, scope, owns, log);
  current = { ...scope, request: { ...request, attempt: 3 } };
  finish(page({ frames: [frame(10, 'text_complete', { text: 'Late old result' })] }));
  assert.equal(await result, null);
  assert.equal(log.after, undefined);
  assert.equal(await readAnimationActivity(() => { calls++; }, scope, owns, log), null);
  assert.equal(calls, 1);
});

test('public summaries, tool lifecycle and terminal use the Console view without raw reasoning or input scaffolding', () => {
  const wire = page({ frames: [
    frame(10, 'interaction_started', {}),
    frame(11, 'reasoning_summary_delta', { summary: 'Public summary partial. ' }),
    frame(12, 'reasoning_summary_complete', { summary: 'Public summary final.' }),
    frame(13, 'tool_call_requested', { name: 'inspect_visualization', tool_call_id: 'unit-tool' }),
    frame(14, 'tool_execution_completed', { name: 'inspect_visualization', tool_call_id: 'unit-tool' }),
    frame(15, 'reasoning_delta', { delta: 'RAW_REASONING_MUST_NOT_APPEAR' }),
    frame(16, 'text_complete', { text: 'A scene was published.' }),
    frame(17, 'interaction_complete', {}),
  ], next_cursor: 'console:17', native: { ...page().native, status: 'completed', terminal_cursor: 'console:17' } });
  const before = structuredClone(wire), result = createAnimationActivityLog(scope).accept(wire);
  const { entries, viewState } = animationActivityConversation(scope, result.frames), output = JSON.stringify(viewState);
  assert.match(output, /Public summary final/);
  assert.match(output, /inspect_visualization/);
  assert.match(output, /A scene was published/);
  assert.doesNotMatch(output, /RAW_REASONING_MUST_NOT_APPEAR|Public summary partial/);
  assert.equal(entries.some(entry => entry.identity?.role === 'user'), false);
  assert.equal(viewState.conversationId, `animation:${receipt.id}`);
  assert.equal(animationSummaryFrame(frame(18, 'reasoning_complete', { text: 'Private unlabelled reasoning' })), null);
  assert.deepEqual(wire, before);
});

test('native compaction markers keep public context text collapsed across pages and preserve later author activity', () => {
  const log = createAnimationActivityLog(scope);
  const initial = page({ frames: [frame(10, 'text_complete', { text: 'Earlier public answer.' }),
    frame(11, 'compaction_started', { input_tokens: 0, estimated_history_tokens: 827619, message_count: 44 }),
    frame(12, 'text_delta', { delta: 'Partial public context summary.' })], next_cursor: 'console:12', exhausted: false });
  const before = structuredClone(initial), first = log.accept(initial);
  assert.equal(animationActivityPresentation(first.page).label, 'Preserving context');
  let entries = animationActivityConversation(scope, first.frames).entries;
  const context = entries.find(entry => entry.id.startsWith('animation-context:'));
  assert.deepEqual(context.blocks[1], { type: 'thinking', label: 'Context summary',
    text: 'Partial public context summary.', final: true, persisted: true });
  assert.equal(entries.some(entry => entry.id.startsWith('animation-output:') && JSON.stringify(entry).includes('Partial public context')), false);
  const html = renderToStaticMarkup(React.createElement(AnimationActivityView, { scope, frames: first.frames, page: first.page }));
  assert.match(html, /Preserving context/);
  assert.match(html, /<details class="cc-rich-thinking[^>]*cc-rich-thinking--collapsed"[^>]*><summary[^>]*>Context summary<\/summary>/);
  const waiting = log.accept(page({ frames: [], next_cursor: 'console:12' }));
  assert.equal(animationActivityPresentation(waiting.page).label, 'Preserving context');
  const final = log.accept(page({ frames: [
    frame(13, 'reasoning_summary_complete', { summary: 'Regular public reasoning summary.', assistant_message_id: 'public-author-summary' }),
    frame(14, 'tool_call_requested', { name: 'inspect_visualization', tool_call_id: 'inspection' }),
    frame(15, 'tool_execution_completed', { name: 'inspect_visualization', tool_call_id: 'inspection' }),
    frame(16, 'text_complete', { text: 'Exact public context summary.' }),
    frame(17, 'compaction_completed', { summary_tokens: 1912, messages_before: 44, messages_after: 4 }),
    frame(18, 'text_delta', { delta: 'Imperfect author partial.' }),
    frame(19, 'text_complete', { text: 'Ordinary author answer after preservation.' }),
    frame(20, 'interaction_complete', { text: 'Ordinary author answer after preservation.' }),
  ], next_cursor: 'console:20', native: { ...page().native, status: 'completed', terminal_cursor: 'console:20' } }));
  assert.equal(animationActivityPresentation(final.page).label, 'Animation author finished');
  entries = animationActivityConversation(scope, final.frames).entries;
  const completed = entries.find(entry => entry.id.startsWith('animation-context:'));
  assert.equal(completed.blocks[0].text, 'Context preservation completed.');
  assert.equal(completed.blocks[1].text, 'Exact public context summary.');
  const shown = JSON.stringify(entries);
  for (const text of ['Earlier public answer', 'Regular public reasoning summary', 'inspect_visualization',
    'Ordinary author answer after preservation']) assert.ok(shown.includes(text));
  assert.doesNotMatch(shown, /Partial public context summary|Imperfect author partial/);
  assert.equal(entries.filter(entry => entry.id.startsWith('animation-output:')).length, 2);
  assert.equal(entries.filter(entry => JSON.stringify(entry).includes('Ordinary author answer after preservation')).length, 1);
  assert.deepEqual(initial, before);
});

test('compaction failure is a context lifecycle event, and unmarked technical text stays an ordinary answer', () => {
  const log = createAnimationActivityLog(scope);
  const result = log.accept(page({ frames: [frame(10, 'compaction_started'),
    frame(11, 'text_delta', { delta: 'Recorded public summary prefix.' }),
    frame(12, 'compaction_failed'),
    frame(13, 'text_complete', { text: 'Context summary: ordinary unmarked technical author answer.' })], next_cursor: 'console:13' }));
  assert.equal(result.page.native.status, 'unknown');
  assert.equal(animationActivityPresentation(result.page).label, 'Animation agent activity');
  const { entries } = animationActivityConversation(scope, result.frames);
  const context = entries.find(entry => entry.id.startsWith('animation-context:'));
  assert.equal(context.blocks[0].text, 'Context preservation failed.');
  assert.equal(context.blocks[1].text, 'Recorded public summary prefix.');
  assert.match(JSON.stringify(entries.find(entry => entry.id.startsWith('animation-output:'))), /ordinary unmarked technical author answer/);
});

test('native compaction boundaries keep ordinary public text segments separate', () => {
  const result = createAnimationActivityLog(scope).accept(page({ frames: [
    frame(10, 'text_delta', { delta: 'Earlier ordinary public segment.' }),
    frame(11, 'compaction_started'),
    frame(12, 'text_delta', { delta: 'Public context summary.' }),
    frame(13, 'compaction_completed'),
    frame(14, 'text_delta', { delta: 'Imperfect later ordinary segment.' }),
    frame(15, 'text_complete', { text: 'Exact later ordinary answer.' }),
  ], next_cursor: 'console:15' }));
  const { entries } = animationActivityConversation(scope, result.frames);
  assert.equal(entries.filter(entry => JSON.stringify(entry).includes('Earlier ordinary public segment')).length, 1);
  assert.equal(entries.filter(entry => JSON.stringify(entry).includes('Exact later ordinary answer')).length, 1);
  assert.equal(entries.some(entry => JSON.stringify(entry).includes('Imperfect later ordinary segment')), false);
  assert.equal(entries.some(entry => JSON.stringify(entry).includes('Earlier ordinary public segment') &&
    JSON.stringify(entry).includes('Exact later ordinary answer')), false);
  assert.equal(entries.some(entry => entry.blocks?.some(block => block.type === 'code')), false);
});

test('production activity distinguishes publication, terminal, retained history and unavailable observation', () => {
  const active = page({ status: 'published', publication: { available: true, code_hash: 'f'.repeat(64), authority: 'publish_visualization' } });
  const render = value => renderToStaticMarkup(React.createElement(AnimationActivityView, { scope, page: value }));
  assert.match(render(active), /^<details open=""/);
  assert.match(render(active), /An animation is published. Its native completion is still being observed/);
  const completed = { ...active, native: { ...active.native, status: 'completed' } };
  assert.doesNotMatch(render(completed), /^<details open=/);
  assert.match(render(completed), /Animation author finished/);
  const retained = { ...completed, activity_origin: 'retained' };
  assert.match(render(retained), /Retained native request history. This does not mean new work was started/);
  assert.doesNotMatch(render(retained), /^<details open=/);
  assert.match(render({...completed,observation:{status:'unavailable'}}), /saved native terminal state is retained/);
  assert.doesNotMatch(render({...completed,observation:{status:'unavailable'}}), /^<details open=/);
  assert.match(render({ ...active, observation: { status: 'unavailable', detail: 'Untrusted backend detail' } }), /does not establish native failure/);
  assert.doesNotMatch(render({ ...active, observation: { status: 'unavailable', detail: 'Untrusted backend detail' } }), /Untrusted backend detail/);
  let expanded, prevented = false;
  const element = AnimationActivityView({ scope, page: completed, onExpand: value => { expanded = value; } });
  element.props.children[0].props.onClick({ preventDefault() { prevented = true; } });
  assert.equal(prevented, true); assert.equal(expanded, true);
});

test('a complete public output replaces only its open text segment, preserving earlier result/tool history without empty terminal JSON', () => {
  const log = createAnimationActivityLog(scope);
  const first = log.accept(page({ frames: [frame(10, 'text_delta', {delta:'Earlier result.'}),
    frame(11,'text_complete',{text:'Earlier result.'}),
    frame(12,'tool_call_requested',{name:'publish_visualization',tool_call_id:'publication'}),
    frame(13,'tool_execution_completed',{name:'publish_visualization',tool_call_id:'publication'}),
    frame(14,'text_delta',{delta:'Imperfect partial ending.'})], next_cursor:'console:14', exhausted:false }));
  assert.match(JSON.stringify(animationActivityConversation(scope,first.frames)),/Imperfect partial ending/);
  const final = log.accept(page({ frames: [frame(15,'text_complete',{text:'Exact final public result.'}),frame(16,'interaction_complete',{text:'Exact final public result.'})],
    next_cursor:'console:16',native:{...page().native,status:'completed',terminal_cursor:'console:16'} }));
  const {entries,viewState}=animationActivityConversation(scope,final.frames), shown=JSON.stringify(viewState);
  assert.match(shown,/Earlier result/);
  assert.match(shown,/publish_visualization/);
  assert.match(shown,/Exact final public result/);
  assert.doesNotMatch(shown,/Imperfect partial ending/);
  assert.equal(entries.filter(entry=>entry.id.startsWith('animation-output:')).length,2);
  assert.equal(entries.filter(entry=>JSON.stringify(entry).includes('Exact final public result')).length,1);
  assert.equal(entries.some(entry=>entry.blocks?.some(block=>block.type==='code' && block.body==='{}')),false);
});

test('public summary retains its exact native assistant carrier without exposing other payload fields', () => {
  const input = frame(10, 'reasoning_summary_complete', { summary: 'Public summary.',
    assistant_message_id: ' native occurrence ', text: 'PRIVATE_RAW_REASONING' });
  const before = structuredClone(input), projected = animationSummaryFrame(input);
  assert.deepEqual(projected.payload, { text: 'Public summary.', assistant_message_id: ' native occurrence ' });
  assert.deepEqual(input, before);
  for (const value of ['', 23, null]) {
    const projected = animationSummaryFrame(frame(10, 'reasoning_summary_complete', {
      summary: 'Public summary.', assistant_message_id: value }));
    assert.equal(projected.payload.assistant_message_id, value, 'Malformed explicit identity cannot silently become legacy text');
  }
});

test('native assistant source replaces provisional animation text once and preserves equal independent replies', () => {
  const canonicalText = '  Exact canonical answer\r\nKeep its source bytes.  \n';
  const history = (number, identity, text) => ({ ...frame(number, 'text_complete', {
    assistant_message_id: identity, text,
    message: { role: 'block_assistant', assistant_message_id: identity,
      blocks: [{ block_type: 'text', data: { text } }] },
  }), source: { kind: 'session_history' } });
  const result = createAnimationActivityLog(scope).accept(page({ frames: [
    frame(10, 'text_delta', { assistant_message_id: 'native-one', delta: 'Discard this draft.' }),
    frame(11, 'text_complete', { assistant_message_id: 'native-one', text: 'Discard this provisional result.' }),
    history(12, 'native-one', canonicalText),
    frame(13, 'interaction_complete', { assistant_message_id: 'native-one', text: 'Do not render this result reference.' }),
    history(14, 'native-two', canonicalText),
  ], next_cursor: 'console:14' }));
  const { entries, viewState } = animationActivityConversation(scope, result.frames);
  assert.deepEqual(entries.map(entry => entry.id), ['unit-frame-12', 'unit-frame-14']);
  assert.deepEqual(entries.map(entry => entry.copyText), [canonicalText, canonicalText]);
  assert.doesNotMatch(JSON.stringify(viewState), /Discard this|Do not render this/);
  assert.equal(entries.some(entry => entry.id.startsWith('animation-output:')), false,
    'The native Console owns canonical assistant messages');
});

test('production refinement form stays editable while awaiting host admission and passes raw text only on explicit controls', () => {
  const raw = '  🦊 Raw text\n\t', events = [], props = { streamId: 'owned-view', text: raw, ready: true, persistent: false, sending: true,
    onText: value => events.push(['edit', value]), onSubmit: () => events.push(['submit']), onCancel: () => events.push(['cancel']) };
  const html = renderToStaticMarkup(React.createElement(AnimationRefinementForm, props));
  assert.match(html, /latest draft changes. They remain in this open page/);
  assert.match(html, /type="submit" disabled=""/);
  assert.doesNotMatch(html, /<textarea[^>]*disabled/);
  assert.deepEqual(events, []);
  const element = AnimationRefinementForm(props);
  element.props.children[1].props.onChange({ target: { value: 'Newer same-page edit\n' } });
  assert.deepEqual(events, [['edit', 'Newer same-page edit\n']]);
  assert.equal(element.props.children[1].props.value, raw);
  const preparing = renderToStaticMarkup(React.createElement(AnimationRefinementForm, { ...props, ready: false }));
  assert.match(preparing, /Getting this proposal ready for your feedback/);
  assert.doesNotMatch(preparing, /could not save/);
});
