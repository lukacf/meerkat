import assert from 'node:assert/strict';
import test from 'node:test';
import { simulationDesignContext } from './simulation-design-context.mjs';
import { createSimulation, simulationSourceKey } from './simulation-state.mjs';
import { fingerprint } from './state.mjs';
import { animationRequestBody, animationRequestMatches, acknowledgeAnimationRefinement } from './animation-publication.mjs';
import { animationDraftSelection, createAnimationDraftStore } from './animation-drafts.mjs';

const fact = (summary, status = 'confirmed') => ({summary, status,
  source_message_ids: status === 'confirmed' ? ['human-answer'] : []});
const feature = (id, disposition = 'open') => ({id, outcome: `Requested ${id} outcome`,
  supported_subset: '', extension: '', disposition, source_message_ids: ['requested-feature'],
  decision_message_ids: ['deferred', 'cancelled', 'handoff'].includes(disposition) ? ['later-human-decision'] : []});
const brief = () => ({facts: {
  interfaces: fact('Use personal chat; the operator Console is separate.'),
  ownership: fact('Separate project histories, with shared weather expertise.'),
  audience: fact('Two people with distinct equipment and interests.'),
  outcome: fact('Decide when and what to photograph.'),
  workflow: fact('Discuss the observing plan before operating equipment.', 'proposed'),
  authority: fact('Equipment approval policy is still undecided.', 'unresolved'),
}, requirements: ['open', 'supported', 'prerequisite', 'deferred', 'cancelled', 'handoff']
  .map(disposition => feature(disposition, disposition))});
const plan = () => ({project: {name: 'Observing team'},
  agent_classes: [{id: 'owner', name: 'Observing companion', instances: 1,
    description: 'Own the observing conversation', instructions: 'Keep the observing context.',
    members: [], tools: {memory: true, notes: 'none', mcp: []}},
  {id: 'weather', name: 'Weather expertise', instances: 1,
    description: 'Judge weather suitability', instructions: 'Judge cloud, wind and uncertainty.',
    members: [], tools: {memory: true, notes: 'none', mcp: []}}],
  topology: {mode: 'manual', entrypoint: 'owner:main', edges: [{a: 'owner:main', b: 'weather:main'}]},
  connections: [], knowledge: {mode: 'none'}, flows: []});

test('current understanding preserves exact facts, statuses and user dispositions in an independent bounded projection', () => {
  const source = brief(), before = structuredClone(source);
  source.facts.outcome.summary = '  A useful outcome\nwith Unicode 🐘.  ';
  source.facts.workflow.source_message_ids = ['tentative-context'];
  source.facts.ownership.internal = 'Do not copy unrelated source fields';
  source.messages = [{text: 'Do not copy the transcript'}];
  source.operation = {status: 'running'};
  const projected = simulationDesignContext(source);
  assert.equal(projected.facts.outcome.summary, source.facts.outcome.summary);
  assert.deepEqual(projected.facts.workflow, source.facts.workflow);
  assert.deepEqual(projected.requirements, source.requirements);
  assert.deepEqual(Object.keys(projected), ['facts', 'requirements']);
  assert.equal(Object.hasOwn(projected.facts.ownership, 'internal'), false);
  projected.facts.ownership.summary = 'Different context';
  projected.facts.workflow.source_message_ids.push('different-reference');
  projected.requirements[0].source_message_ids.push('different-request');
  projected.requirements[3].decision_message_ids.push('different-decision');
  assert.equal(source.facts.ownership.summary, before.facts.ownership.summary);
  assert.deepEqual(source.facts.workflow.source_message_ids, ['tentative-context']);
  assert.deepEqual(source.requirements, before.requirements);
});

test('bounded current brief defaults remain compatible with absent optional fields', () => {
  assert.equal(simulationDesignContext(undefined), null);
  assert.deepEqual(simulationDesignContext({}), {facts: {}, requirements: []});
  const projected = simulationDesignContext({facts: {workflow: {summary: 'A proposed workflow', status: 'proposed'}},
    requirements: [{id: 'job', outcome: 'A useful job', disposition: 'open', source_message_ids: ['job-request']}]});
  assert.deepEqual(projected.facts.workflow.source_message_ids, []);
  assert.deepEqual(projected.requirements[0], {...feature('job'), outcome: 'A useful job', source_message_ids: ['job-request']});
});

test('reordering fact fields cannot queue a semantically unchanged scene revision', () => {
  const source = brief(), reordered = {...source, facts: Object.fromEntries(Object.entries(source.facts).reverse())};
  assert.equal(JSON.stringify(simulationDesignContext(source)), JSON.stringify(simulationDesignContext(reordered)));
  assert.equal(simulationSourceKey(plan(), createSimulation(plan(), '__overview__', source.facts.interfaces, source)),
    simulationSourceKey(plan(), createSimulation(plan(), '__overview__', source.facts.interfaces, reordered)));
});

test('invalid shape, bounds and unsupported evidence cannot become animation source context', () => {
  const mutations = [
    value => {value.facts = null;}, value => {value.requirements = {};},
    value => {value.facts.unknown_topic = fact('Not a saved guide topic');},
    value => {value.facts.outcome.summary = '';}, value => {value.facts.outcome.summary = '  ';},
    value => {value.facts.outcome.summary = '🐘'.repeat(1601);},
    value => {value.facts.outcome.summary = 'bad\u0000context';},
    value => {value.facts.outcome.summary = '\ud800';}, value => {value.facts.outcome.summary = '\udfff';},
    value => {value.facts.outcome.status = 'implemented';},
    value => {value.facts.outcome.source_message_ids = [];},
    value => {value.facts.outcome.source_message_ids = ['same', 'same'];},
    value => {value.facts.outcome.source_message_ids = [42];},
    value => {value.facts.outcome.source_message_ids = [' '];},
    value => {value.facts.outcome.source_message_ids = ['bad\nreference'];},
    value => {value.facts.outcome.source_message_ids = ['x'.repeat(257)];},
    value => {value.facts.outcome.source_message_ids = Array.from({length: 41}, (_, i) => `evidence-${i}`);},
    value => {value.requirements = Array.from({length: 33}, (_, i) => feature(`request-${i}`));},
    value => {value.requirements[1].id = value.requirements[0].id;},
    value => {value.requirements[0].id = 'x'.repeat(81);},
    value => {value.requirements[0].outcome = '🐘'.repeat(1201);},
    value => {value.requirements[0].supported_subset = 'x'.repeat(1601);},
    value => {value.requirements[0].extension = '\ud800';},
    value => {value.requirements[0].disposition = 'complete';},
    value => {value.requirements[0].source_message_ids = [];},
    value => {value.requirements[0].source_message_ids = ['same', 'same'];},
    value => {value.requirements[3].decision_message_ids = [];},
    value => {value.requirements[4].decision_message_ids = ['same', 'same'];},
    value => {value.requirements[1].extension = 'An unimplemented extension';},
  ];
  for (const mutate of mutations) {
    const source = brief(); mutate(source);
    assert.equal(simulationDesignContext(source), null, mutate.toString());
    const model = createSimulation(plan(), '__overview__', brief().facts.interfaces, source);
    assert.equal(Object.hasOwn(model, 'design_context'), false);
  }
  const boundary = brief();
  boundary.facts.outcome.summary = '🐘'.repeat(1600);
  boundary.facts.outcome.source_message_ids = Array.from({length: 40}, (_, i) => `${i}-${'🐘'.repeat(253)}`);
  boundary.requirements = Array.from({length: 32}, (_, i) => ({...feature(`request-${i}`),
    outcome: '🐘'.repeat(1200), supported_subset: '🐘'.repeat(1600), extension: '🐘'.repeat(1600)}));
  assert.ok(simulationDesignContext(boundary));
});

test('context-only revisions change scene source while preserving every configured relationship and workflow fact', async () => {
  const source = brief(), value = plan(), before = structuredClone(source);
  const base = createSimulation(value, '__overview__', source.facts.interfaces);
  const first = createSimulation(value, '__overview__', source.facts.interfaces, source);
  assert.deepEqual(Object.fromEntries(Object.entries(first).filter(([key]) => key !== 'design_context')), base);
  assert.deepEqual(source, before);
  for (const topic of ['ownership', 'audience', 'workflow', 'outcome', 'authority', 'inputs']) {
    const next = structuredClone(source);
    next.facts[topic] = fact(`Changed ${topic} context`, topic === 'authority' ? 'unresolved' : 'confirmed');
    const model = createSimulation(value, '__overview__', source.facts.interfaces, next);
    assert.notEqual(simulationSourceKey(value, model), simulationSourceKey(value, first));
    assert.notEqual(await fingerprint(model), await fingerprint(first));
    assert.deepEqual(Object.fromEntries(Object.entries(model).filter(([key]) => key !== 'design_context')), base);
    assert.equal(model.edges.some(edge => ['input', 'output'].includes(edge.kind)), false);
  }
  for (const change of [
    next => {next.facts.ownership.source_message_ids = ['newer-user-evidence'];},
    next => {next.facts.workflow.status = 'unresolved';},
    next => {next.requirements[0].outcome = 'A revised result';},
    next => {next.requirements[0].disposition = 'deferred'; next.requirements[0].decision_message_ids = ['later-deferral'];},
  ]) {
    const next = structuredClone(source); change(next);
    assert.notEqual(await fingerprint(createSimulation(value, '__overview__', source.facts.interfaces, next)), await fingerprint(first));
  }
});

test('a context-only stale admission cannot clear the current scene refinement draft', async () => {
  const value = plan(), source = brief(), next = structuredClone(source);
  next.facts.ownership = fact('Independent family equipment contexts share weather judgment.');
  const modelA = createSimulation(value, '__overview__', source.facts.interfaces, source);
  const modelB = createSimulation(value, '__overview__', source.facts.interfaces, next);
  const request = model => ({streamId: 'guide:__overview__', proposalKey: simulationSourceKey(value, model), attempt: 0});
  const requestA = request(modelA), requestB = request(modelB);
  const body = model => animationRequestBody(value, model, '', requestA.streamId, 'guide');
  const binding = async model => {const submitted = body(model); return {setup_id: submitted.setup_id,
    stream_id: submitted.stream_id, blueprint_hash: await fingerprint(submitted.blueprint),
    model_hash: await fingerprint(submitted.model), request_fingerprint: await fingerprint({blueprint: value, model})};};
  const bindingA = await binding(modelA), bindingB = await binding(modelB);
  const receipt = selected => ({...selected, id: 'a'.repeat(64), native_request_key: 'b'.repeat(64), source_generation: 1, status: 'queued'});
  assert.equal(bindingA.blueprint_hash, bindingB.blueprint_hash);
  assert.notEqual(bindingA.request_fingerprint, bindingB.request_fingerprint);
  assert.equal(animationRequestMatches(requestA, requestB), false);
  const drafts = createAnimationDraftStore(null), selection = animationDraftSelection('guide', requestB.proposalKey, '__overview__');
  drafts.attach(selection, await fingerprint({blueprint: value, model: modelB}), false);
  drafts.update(selection, 'Keep the new ownership context visible.');
  const submitted = drafts.capture(selection);
  assert.equal(acknowledgeAnimationRefinement(drafts, selection, submitted, receipt(bindingA), bindingB, requestB, requestB), null);
  assert.equal(acknowledgeAnimationRefinement(drafts, selection, submitted, receipt(bindingA), bindingA, requestA, requestB), null);
  assert.equal(drafts.read(selection).text, submitted.text);
  assert.deepEqual(acknowledgeAnimationRefinement(drafts, selection, submitted, receipt(bindingB), bindingB, requestB, requestB), {accepted: true, cleared: true});
});
