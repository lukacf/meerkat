import {test} from 'node:test';
import assert from 'node:assert/strict';
import {proposalClassChanges} from './proposal-changes.mjs';

const role = (id, name, instances = 1, extra = {}) => ({id, name, instances,
  description: `Responsibility for ${name}.`, instructions: `Keep the context for ${name}.`, ...extra});
const plan = (...agent_classes) => ({agent_classes});
function freeze(value) {
  if (value && typeof value === 'object') {
    Object.values(value).forEach(freeze);
    Object.freeze(value);
  }
  return value;
}

test('class additions and removals describe proposed agents and zero-identity profiles separately', () => {
  const previous = plan(role('archive', 'Archive specialists', 3), role('helper', 'Future helper', 0));
  const current = plan(role('weather', 'Weather specialist'), role('future', 'Future astronomer', 0));
  assert.deepEqual(proposalClassChanges(previous, current), [
    'Weather specialist added - 1 starting agent proposed.',
    'Future astronomer expertise profile added - no starting agents.',
    'Archive specialists removed - 3 starting agents no longer proposed.',
    'Future helper expertise profile removed from the proposal.',
  ]);
});

test('stable class identity distinguishes a rename, population change, role change and instruction change', () => {
  const previous = plan(role('catalogue', 'Catalogue team', 3));
  const current = plan(role('catalogue', 'Collection cataloguers', 8, {
    description: 'Reconcile restricted collection evidence.',
    instructions: 'Keep the original private evidence and its revisions.',
  }));
  const changes = proposalClassChanges(previous, current);
  assert.deepEqual(changes, [
    'Catalogue team renamed to Collection cataloguers in the proposal.',
    'Collection cataloguers: 3 to 8 starting agents proposed.',
    'Collection cataloguers: role updated.',
    'Collection cataloguers: instructions updated.',
  ]);
  assert.ok(changes.every(text => !text.includes('private evidence')));
});

test('provider, model, tools and other proposal changes do not masquerade as expertise changes', () => {
  const previous = plan(role('weather', 'Weather specialist', 1, {
    provider: 'openai', model: 'gpt-6.1-sol', tools: {workspace: true},
  }));
  const current = structuredClone(previous);
  Object.assign(current.agent_classes[0], {
    provider: 'anthropic', model: 'claude-sonnet-4-6', tools: {workspace: false, knowledge: true},
  });
  current.topology = {mode: 'custom'};
  current.freeformDesign = {shape: 'Custom composition remains unrestricted'};
  assert.deepEqual(proposalClassChanges(previous, current), []);
});

test('zero and large initial populations remain proposal counts, without creating worker or audience claims', () => {
  const previous = plan(role('contact', 'Personal contacts', 0), role('repo', 'Repository owners', 80));
  const current = plan(role('contact', 'Personal contacts', 500), role('repo', 'Repository owners', 80));
  current.audience = {people: 900};
  current.temporaryWorkerCapacity = 700;
  assert.deepEqual(proposalClassChanges(previous, current), [
    'Personal contacts: 0 to 500 starting agents proposed.',
  ]);
  assert.deepEqual(proposalClassChanges(current, previous), [
    'Personal contacts: 500 to 0 starting agents proposed.',
  ]);
});

test('class ordering is immaterial and frozen source snapshots are never changed', () => {
  const previous = freeze(plan(role('weather', 'Weather specialist'), role('astronomy', 'Astronomer')));
  const current = freeze(plan(role('astronomy', 'Astronomer'), role('weather', 'Weather specialist')));
  const before = structuredClone({previous, current});
  assert.deepEqual(proposalClassChanges(previous, current), []);
  assert.deepEqual({previous, current}, before);
  assert.deepEqual(proposalClassChanges(previous, freeze(plan(role('weather', 'Weather specialist', 2)))), [
    'Weather specialist: 1 to 2 starting agents proposed.',
    'Astronomer removed - 1 starting agent no longer proposed.',
  ]);
  assert.deepEqual({previous, current}, before);
});

test('malformed snapshots, missing or duplicate identities, and invalid counts omit concrete notices', () => {
  const valid = plan(role('weather', 'Weather specialist'));
  const invalid = [null, undefined, {}, {agent_classes: 'weather'},
    plan(null), plan([]), plan(role('', 'Weather specialist')), plan(role(' ', 'Weather specialist')),
    plan(role('weather', '')), plan(role('weather', 'Weather'), role('weather', 'Different role')),
    ...[-1, 0.5, true, '2', NaN, Infinity, 513].map(instances => plan(role('weather', 'Weather', instances)))];
  for (const malformed of invalid) {
    assert.deepEqual(proposalClassChanges(malformed, valid), []);
    assert.deepEqual(proposalClassChanges(valid, malformed), []);
  }
});
