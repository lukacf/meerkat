import {test} from 'node:test';
import assert from 'node:assert/strict';
import {hasPublishedAnimation, shouldObserveAnimationTail, checkedAnimationFeedback, animationRequestBody,
  pendingAnimationNotice, shouldReattachAnimationPoll, readAnimationPreference, saveAnimationPreference,
  readAnimationPreferenceRequest, saveAnimationCorrectionPreference, animationCorrectionParentMatches,
  animationAdmissionMatches, acknowledgeAnimationRefinement} from './animation-publication.mjs';
import { animationDraftSelection, createAnimationDraftStore } from './animation-drafts.mjs';

test('acknowledged program renders while its real native tail is observed', () => {
  const receipt = {status: 'published', native_run_status: 'unknown', artifact: {code: 'return program;'}};
  assert.equal(hasPublishedAnimation(receipt), true);
  assert.equal(shouldObserveAnimationTail(receipt), true);
});

test('interrupted host observation retains a program without assuming native completion', () => {
  for (const observation_status of ['interrupted', 'timed_out']) {
    const receipt = {status: 'published', native_run_status: 'unknown', observation_status, artifact: {code: 'return program;'}};
    assert.equal(hasPublishedAnimation(receipt), true);
    assert.equal(shouldObserveAnimationTail(receipt), false);
    assert.equal(receipt.native_run_status, 'unknown');
  }
});

test('a recovered interrupted native run still has its accepted custom program', () => {
  const receipt = {status: 'published', native_run_status: 'interrupted', artifact: {code: 'return program;'}};
  assert.equal(hasPublishedAnimation(receipt), true);
  assert.equal(shouldObserveAnimationTail(receipt), false);
});

test('failed or merely queued generation has no fabricated custom animation', () => {
  for (const status of ['queued', 'running', 'observing', 'failed', 'interrupted']) {
    assert.equal(hasPublishedAnimation({status}), false);
    assert.equal(shouldObserveAnimationTail({status}), false);
  }
  assert.equal(hasPublishedAnimation({status: 'published', artifact: {}}), false);
});

test('an observation timeout remains a polling state without inventing a program or native terminal', () => {
  const receipt = {status: 'observing', observation_status: 'reconnecting', native_run_status: 'unknown'};
  assert.match(pendingAnimationNotice(receipt), /Reconnecting to the same animation request/);
  assert.equal(hasPublishedAnimation(receipt), false);
  assert.equal(shouldObserveAnimationTail(receipt), false);
  for (const status of ['queued', 'running', 'failed', 'interrupted', 'completed'])
    assert.equal(pendingAnimationNotice({...receipt, status}), '');
  assert.equal(pendingAnimationNotice({status: 'observing'}), '');
  assert.equal(receipt.native_run_status, 'unknown');
});

test('a missing ephemeral operation can reattach once; other failures cannot resend generation', () => {
  assert.equal(shouldReattachAnimationPoll(404, 0), true);
  for (const status of [undefined, null, 200, 401, 403, 422, 500, '404'])
    assert.equal(shouldReattachAnimationPoll(status, 0), false);
  for (const attempts of [1, 2, undefined, null, '0'])
    assert.equal(shouldReattachAnimationPoll(404, attempts), false);
});

test('visual feedback reaches the animation request with the exact proposal, graph and view', () => {
  const blueprint = {project: {name: 'Reading companion'}, agent_classes: [{id: 'librarian', instructions: 'Own collection identity'}]};
  const model = {nodes: [{id: 'librarian:main'}], edges: []};
  const before = structuredClone({blueprint, model});
  const request = animationRequestBody(blueprint, model, '  Fewer labels.\nShow the peer handoff, not build status.  ', 'guide-id:recommend');
  assert.equal(request.feedback, 'Fewer labels. Show the peer handoff, not build status.');
  assert.equal(request.stream_id, 'guide-id:recommend');
  assert.equal(request.blueprint, blueprint);
  assert.equal(request.model, model);
  assert.deepEqual({blueprint, model}, before);
  assert.deepEqual(Object.keys(request).sort(), ['blueprint', 'feedback', 'model', 'stream_id']);
});

test('manual feedback rejects empty or oversized text and accepts the boundary without truncation', () => {
  for (const feedback of ['', ' \n\t ', '\u0000', null, {}]) {
    assert.throws(() => checkedAnimationFeedback(feedback, {required: true}), /Describe/);
  }
  assert.equal(checkedAnimationFeedback('x'.repeat(1000), {required: true}), 'x'.repeat(1000));
  assert.throws(() => checkedAnimationFeedback('x'.repeat(1001), {required: true}), /1000/);
  assert.equal(checkedAnimationFeedback(' quieter\u0000 labels\t please '), 'quieter labels please');
  assert.equal(animationRequestBody({}, {}, '', 'preview').feedback, '');
});

const storedFeedback = () => {
  const entries = new Map();
  return {entries, getItem: key => entries.get(key) ?? null, setItem: (key, value) => entries.set(key, value)};
};

test('animation refinements survive reopening only for the exact setup, view and proposal', () => {
  const storage = storedFeedback(), hash = 'a'.repeat(64);
  assert.equal(saveAnimationPreference(storage, 'reading:overview', hash, ' Shorter labels. '), true);
  assert.equal(readAnimationPreference(storage, 'reading:overview', hash), 'Shorter labels.');
  assert.equal(readAnimationPreference(storage, 'other:overview', hash), '');
  assert.equal(readAnimationPreference(storage, 'reading:workflow', hash), '');
  assert.equal(readAnimationPreference(storage, 'reading:overview', 'b'.repeat(64)), '');
  // The next proposal replaces one bounded view preference rather than accumulating snapshots.
  saveAnimationPreference(storage, 'reading:overview', 'b'.repeat(64), 'Show the new collaboration.');
  assert.equal(storage.entries.size, 1);
  assert.equal(readAnimationPreference(storage, 'reading:overview', hash), '');
});

test('untrusted or unavailable saved refinement cannot fabricate a published animation', () => {
  const storage = storedFeedback(), hash = 'a'.repeat(64), key = 'mobkit-animation-feedback-v1:reading:overview';
  for (const value of ['broken JSON', JSON.stringify({proposal_hash: hash, feedback: 'x'.repeat(1001)}),
    JSON.stringify({proposal_hash: hash, feedback: {code: 'pretend program'}})]) {
    storage.entries.set(key, value);
    assert.equal(readAnimationPreference(storage, 'reading:overview', hash), '');
  }
  const denied = {getItem() {throw new Error('unavailable');}, setItem() {throw new Error('quota');}};
  assert.equal(readAnimationPreference(denied, 'reading:overview', hash), '');
  assert.equal(saveAnimationPreference(denied, 'reading:overview', hash, 'Clear labels.'), false);
  assert.equal(saveAnimationPreference(storage, '', hash, 'Clear labels.'), false);
  assert.equal(hasPublishedAnimation({status: 'queued', feedback: 'Clear labels.'}), false);
});

const correctionRequest = () => ({feedback: 'Keep all labels readable.', review_id: '9'.repeat(64), expected_parent: {
  operation_id: 'e'.repeat(64), code_hash: 'f'.repeat(64), blueprint_hash: 'c'.repeat(64), model_hash: 'd'.repeat(64),
  setup_id: 'reading', stream_id: 'reading:overview'}});

test('one bounded preference retains the exact accepted correction and ordinary parent request for its source only', () => {
  const storage = storedFeedback(), hash = 'a'.repeat(64), correction = correctionRequest();
  saveAnimationPreference(storage, 'reading:overview', hash, 'Clear original labels.');
  assert.equal(saveAnimationCorrectionPreference(storage, 'reading:overview', hash, 'Clear original labels.', correction), true);
  assert.deepEqual(readAnimationPreferenceRequest(storage, 'reading:overview', hash), {feedback: 'Clear original labels.', correction});
  assert.equal(readAnimationPreference(storage, 'reading:overview', hash), 'Clear original labels.');
  assert.equal(storage.entries.size, 1);
  const restored = readAnimationPreferenceRequest(storage, 'reading:overview', hash);
  restored.correction.expected_parent.operation_id = '0'.repeat(64);
  assert.deepEqual(readAnimationPreferenceRequest(storage, 'reading:overview', hash).correction, correction);
  for (const [stream, source] of [['other:overview', hash], ['reading:workflow', hash], ['reading:overview', 'b'.repeat(64)]])
    assert.equal(readAnimationPreferenceRequest(storage, stream, source), null);
  assert.equal(saveAnimationCorrectionPreference(storage, 'reading:overview', hash, '', correction), true);
  assert.equal(readAnimationPreferenceRequest(storage, 'reading:overview', hash).feedback, '');
  saveAnimationPreference(storage, 'reading:overview', hash, 'My next manual refinement.');
  assert.deepEqual(readAnimationPreferenceRequest(storage, 'reading:overview', hash), {feedback: 'My next manual refinement.', correction: null});
});

test('malformed saved correction tuples are refused without normalizing their accepted identity', () => {
  const storage = storedFeedback(), hash = 'a'.repeat(64), key = 'mobkit-animation-feedback-v1:reading:overview';
  for (const change of [value => value.feedback = ' Changed whitespace. ', value => value.review_id = 'bad',
    value => value.expected_parent.operation_id = 'bad', value => value.expected_parent.stream_id = 'foreign:view',
    value => delete value.expected_parent.code_hash, value => value.extra = 'untrusted',
    value => value.expected_parent.extra = 'untrusted']) {
    const correction = correctionRequest(); change(correction);
    assert.equal(saveAnimationCorrectionPreference(storage, 'reading:overview', hash, '', correction), false);
    storage.entries.set(key, JSON.stringify({proposal_hash: hash, feedback: '', correction}));
    assert.equal(readAnimationPreferenceRequest(storage, 'reading:overview', hash), null);
  }
});

test('restoring parent ownership requires its exact accepted request and published source', () => {
  const expected = correctionRequest().expected_parent;
  const binding = {setup_id: 'reading', stream_id: 'reading:overview', request_fingerprint: 'b'.repeat(64),
    blueprint_hash: expected.blueprint_hash, model_hash: expected.model_hash};
  const receipt = {...binding, id: expected.operation_id, source_generation: 1, native_request_key: '8'.repeat(64), status: 'completed',
    artifact: {code: 'return program;', code_hash: expected.code_hash}};
  assert.equal(animationCorrectionParentMatches(receipt, binding, expected), true);
  for (const change of [value => value.id = '0'.repeat(64), value => value.artifact.code_hash = '0'.repeat(64),
    value => value.request_fingerprint = '0'.repeat(64), value => value.setup_id = 'foreign',
    value => value.stream_id = 'foreign:view', value => value.blueprint_hash = '0'.repeat(64),
    value => value.model_hash = '0'.repeat(64), value => value.status = 'queued', value => delete value.artifact.code]) {
    const changed = structuredClone(receipt); change(changed);
    assert.equal(animationCorrectionParentMatches(changed, binding, expected), false);
  }
});

test('exact host admission clears a captured draft without claiming native admission; uncertainty and newer edits keep it', () => {
  const values = new Map(), tab = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
  const drafts = createAnimationDraftStore(tab), selection = animationDraftSelection('guide-one', 'exact proposal', '__overview__');
  drafts.attach(selection, 'a'.repeat(64)); drafts.update(selection, '  Exact raw 🦊 text\n\t');
  const submitted = drafts.capture(selection), request = { streamId: 'guide-one:__overview__', proposalKey: 'exact proposal', attempt: 2 };
  const binding = { setup_id: 'guide-one', stream_id: request.streamId, request_fingerprint: 'b'.repeat(64), blueprint_hash: 'c'.repeat(64), model_hash: 'd'.repeat(64) };
  const receipt = { ...binding, id: 'e'.repeat(64), native_request_key: 'f'.repeat(64), source_generation: 1, status: 'queued', native_run_status: 'unknown' };
  for (const changed of [null, { ...receipt, status: 'failed' }, { ...receipt, setup_id: 'other-guide' }, { ...receipt, stream_id: 'different-view' },
    { ...receipt, request_fingerprint: '9'.repeat(64) }, { ...receipt, blueprint_hash: '9'.repeat(64) },
    { ...receipt, model_hash: '9'.repeat(64) }, { ...receipt, native_request_key: null }, { ...receipt, source_generation: null }]) {
    assert.equal(acknowledgeAnimationRefinement(drafts, selection, submitted, changed, binding, request, request), null);
    assert.equal(drafts.read(selection).text, submitted.text);
  }
  for (const current of [{ ...request, attempt: 3 }, { ...request, proposalKey: 'new proposal' }, { ...request, streamId: 'other-view' }]) {
    assert.equal(acknowledgeAnimationRefinement(drafts, selection, submitted, receipt, binding, request, current), null);
  }
  drafts.update(selection, 'Different edit'); drafts.update(selection, submitted.text);
  assert.deepEqual(acknowledgeAnimationRefinement(drafts, selection, submitted, receipt, binding, request, request), { accepted: true, cleared: false });
  assert.equal(drafts.read(selection).text, submitted.text);
  assert.deepEqual(acknowledgeAnimationRefinement(drafts, selection, drafts.capture(selection), receipt, binding, request, request), { accepted: true, cleared: true });
  assert.equal(drafts.read(selection).text, '');
  assert.equal(receipt.native_run_status, 'unknown');
  assert.equal(hasPublishedAnimation(receipt), false);
});

test('the setup binding is sent only when the actual setup exists; unbound receipts cannot clear a scoped draft', () => {
  assert.equal(animationRequestBody({}, {}, 'Clearer labels', 'view', 'owned-setup').setup_id, 'owned-setup');
  assert.equal(Object.hasOwn(animationRequestBody({}, {}, '', 'manual-preview'), 'setup_id'), false);
  const binding = { setup_id: 'owned-setup', stream_id: 'view', request_fingerprint: 'a'.repeat(64), blueprint_hash: 'b'.repeat(64), model_hash: 'c'.repeat(64) };
  const receipt = { ...binding, id: 'd'.repeat(64), native_request_key: 'e'.repeat(64), source_generation: 1, status: 'running' };
  assert.equal(animationAdmissionMatches(receipt, binding), true);
  assert.equal(animationAdmissionMatches({ ...receipt, setup_id: null }, binding), false);
});
