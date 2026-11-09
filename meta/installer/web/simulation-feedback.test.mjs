import assert from 'node:assert/strict';
import test from 'node:test';
import {simulationVisualizationFeedback} from './simulation-design-context.mjs';
import {createSimulation, simulationSourceKey} from './simulation-state.mjs';
import {guidePlan, guideDesignBrief, guideVisualizationFeedback} from './evolving-plan-state.mjs';
import {animationRequestBody} from './animation-publication.mjs';
import {fingerprint} from './state.mjs';

const feedback = (text = 'Use open circular groups and give the collaboration room.', source = 'human-visual-preference') =>
  ({text, source_message_id: source});
const plan = () => ({project: {name: 'Observing team', slug: 'observing-team'},
  agent_classes: [{id: 'companion', name: 'Observing companion', instances: 1,
    description: 'Keep the observing conversation', instructions: 'Retain the equipment context.',
    members: [], tools: {memory: true, notes: 'none', mcp: []}},
  {id: 'weather', name: 'Weather expertise', instances: 1,
    description: 'Judge weather suitability', instructions: 'Judge cloud and wind uncertainty.',
    members: [], tools: {memory: true, notes: 'none', mcp: []}}],
  topology: {mode: 'manual', entrypoint: 'companion:main', edges: [{a: 'companion:main', b: 'weather:main'}]},
  connections: [], knowledge: {mode: 'none'}, flows: []});
const brief = () => ({facts: {outcome: {status: 'confirmed', summary: 'Choose an observing opportunity.',
  source_message_ids: ['human-outcome']}}, requirements: []});
const stagedGuide = () => ({id: 'visual-guide', version: 4, status: 'question',
  proposal: plan(), brief: brief(), visualization_feedback: feedback('Saved earlier composition.', 'earlier-human'),
  operation: {status: 'running', version: 4, request_id: 'a'.repeat(64)},
  live_design: {guide_id: 'visual-guide', guide_version: 4, request_id: 'a'.repeat(64),
    blueprint: {...plan(), project: {name: 'Checked observing design', slug: 'observing-team'}}, brief: brief(),
    visualization_feedback: feedback('Use circular expertise clusters and animate the handoff.', 'current-human')}});
const guideModel = state => {
  const visible = guidePlan(state), context = guideDesignBrief(state, visible);
  return createSimulation(visible, '__overview__', context?.facts?.interfaces, context,
    guideVisualizationFeedback(state, visible));
};

test('visualization feedback projection copies exact presentation text and host provenance without aliasing or changing topology', () => {
  const value = plan(), context = brief(), source = feedback('  Use circles.\r\n\tKeep the user beside their contact.  ');
  const original = structuredClone({value, context, source});
  const projected = simulationVisualizationFeedback(source);
  assert.deepEqual(projected, source);
  assert.notEqual(projected, source);
  const legacy = createSimulation(value, '__overview__', undefined, context);
  const model = createSimulation(value, '__overview__', undefined, context, source);
  assert.deepEqual(model.visualization_feedback, source);
  assert.notEqual(model.visualization_feedback, source);
  const {visualization_feedback, ...unchanged} = model;
  assert.deepEqual(unchanged, legacy, 'Composition preferences do not modify configured nodes, links, tools or workflows');
  const body = animationRequestBody(value, model, '', 'visual-guide:__overview__', 'visual-guide');
  assert.deepEqual(body.model.visualization_feedback, source);
  assert.deepEqual(body.blueprint, value);
  assert.equal(Object.hasOwn(body.blueprint, 'visualization_feedback'), false);
  assert.equal(body.feedback, '', 'A guide preference is model input, not an explicit renderer refinement');
  model.visualization_feedback.text = 'Caller mutation';
  projected.source_message_id = 'Changed caller reference';
  assert.deepEqual({value, context, source}, original);
});

test('visualization feedback bounds count Unicode codepoints and reject unsafe text, references and unsupported shapes', () => {
  const boundary = feedback('🐘'.repeat(1000), '🐘'.repeat(256));
  assert.deepEqual(simulationVisualizationFeedback(boundary), boundary);
  assert.deepEqual(simulationVisualizationFeedback(feedback('Line one\r\n\tLine two')), feedback('Line one\r\n\tLine two'));
  const refused = [undefined, null, [], 'Use circles', {}, {text: 'Use circles'},
    {source_message_id: 'human'}, {...feedback(), extra: 'Unbound field'},
    feedback(''), feedback(' \r\n\t '), feedback('🐘'.repeat(1001)), feedback(42),
    feedback('Use circles', ''), feedback('Use circles', '  '), feedback('Use circles', '🐘'.repeat(257)),
    feedback('Use circles', 42), feedback('Use circles', 'line\nbreak'),
    feedback('Use circles', 'line\rbreak'), feedback('Use circles', 'tab\tbreak')];
  for (const point of [0, 1, 7, 11, 31, 127, 0xd800, 0xdfff]) {
    const unsafe = String.fromCodePoint(point);
    refused.push(feedback(`Use ${unsafe}circles`), feedback('Use circles', `human${unsafe}reference`));
  }
  const value = plan(), context = brief(), legacy = createSimulation(value, '__overview__', undefined, context);
  for (const source of refused) {
    assert.equal(simulationVisualizationFeedback(source), null, JSON.stringify(source));
    assert.deepEqual(createSimulation(value, '__overview__', undefined, context, source), legacy,
      'Invalid or absent optional preferences leave the legacy effective input unchanged');
  }
});

test('visualization feedback value and originating turn own distinct scene keys while identical topology stays exact', async () => {
  const value = plan(), context = brief(), first = feedback(), sources = [first,
    feedback('Keep the same circular groups but reveal one collaboration at a time.'),
    feedback(first.text, 'later-human-turn')];
  const models = sources.map(source => createSimulation(value, '__overview__', undefined, context, source));
  const keys = models.map(model => simulationSourceKey(value, model));
  const hashes = await Promise.all(models.map(fingerprint));
  assert.equal(new Set(keys).size, 3);
  assert.equal(new Set(hashes).size, 3);
  const legacy = createSimulation(value, '__overview__', undefined, context);
  for (let i = 0; i < models.length; i++) {
    assert.deepEqual(models[i].visualization_feedback, sources[i]);
    const {visualization_feedback, ...unchanged} = models[i];
    assert.deepEqual(unchanged, legacy);
  }
  const reordered = {source_message_id: first.source_message_id, text: first.text};
  assert.equal(simulationSourceKey(value, createSimulation(value, '__overview__', undefined, context, reordered)), keys[0],
    'Object field order alone cannot create another composition revision');
});

test('visualization feedback selection follows the exact checked guide pair and survives its unchanged saved publication', () => {
  const state = stagedGuide(), original = structuredClone(state);
  assert.equal(guideVisualizationFeedback(state), state.live_design.visualization_feedback);
  assert.equal(guideVisualizationFeedback(state, state.proposal), state.visualization_feedback);
  const draftModel = guideModel(state);
  const complete = {...state, version: 5, proposal: state.live_design.blueprint, brief: state.live_design.brief,
    visualization_feedback: state.live_design.visualization_feedback, operation: {...state.operation, status: 'completed'}};
  assert.deepEqual(guideModel(complete), draftModel);
  assert.equal(simulationSourceKey(complete.proposal, guideModel(complete)),
    simulationSourceKey(state.live_design.blueprint, draftModel));
  const unchanged = {...complete, messages: [{id: 'unrelated-message', text: 'A status note'}],
    readiness: {context_ready: true}, visualization_feedback: {...complete.visualization_feedback}};
  assert.equal(simulationSourceKey(guidePlan(unchanged), guideModel(unchanged)),
    simulationSourceKey(complete.proposal, guideModel(complete)));
  const cleared = {...complete, visualization_feedback: null};
  assert.equal(guideVisualizationFeedback(cleared), null);
  assert.equal(Object.hasOwn(guideModel(cleared), 'visualization_feedback'), false);
  assert.deepEqual(state, original);
});

test('visualization feedback selection never borrows a saved preference for an unpaired or explicitly cleared live topology', () => {
  const early = stagedGuide();
  early.live_design.brief = null;
  early.live_design.context_pending = true;
  const original = structuredClone(early);
  assert.equal(guidePlan(early), early.live_design.blueprint);
  assert.equal(guideDesignBrief(early), null);
  assert.equal(guideVisualizationFeedback(early), null,
    'Even a present draft preference is not current until the topology and brief are checked together');
  assert.equal(Object.hasOwn(guideModel(early), 'visualization_feedback'), false);
  assert.equal(guideVisualizationFeedback(early, early.proposal), early.visualization_feedback);
  const paired = stagedGuide();
  delete paired.live_design.visualization_feedback;
  assert.equal(guideVisualizationFeedback(paired), undefined);
  assert.equal(Object.hasOwn(guideModel(paired), 'visualization_feedback'), false);
  paired.live_design.visualization_feedback = null;
  assert.equal(guideVisualizationFeedback(paired), null);
  assert.equal(Object.hasOwn(guideModel(paired), 'visualization_feedback'), false);
  for (const mutate of [state => {state.operation.request_id = 'b'.repeat(64);},
    state => {state.operation.status = 'failed';}, state => {state.version++;},
    state => {state.live_design.guide_id = 'foreign-guide';}, state => {delete state.live_design.brief;}]) {
    const foreign = stagedGuide(); mutate(foreign);
    assert.equal(guidePlan(foreign), foreign.proposal);
    assert.equal(guideVisualizationFeedback(foreign), foreign.visualization_feedback,
      'A refused foreign stage cannot contribute its presentation preference to the saved plan');
    assert.deepEqual(guideModel(foreign).visualization_feedback, foreign.visualization_feedback);
  }
  assert.deepEqual(early, original);
});
