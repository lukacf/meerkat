import test from 'node:test';
import assert from 'node:assert/strict';
import { IDENTITY_PAGE_SIZE, populationInspection } from './population-inspection.mjs';

const model = count => ({nodes: [
  {id: 'human:unresolved', name: 'Human input', kind: 'input'},
  ...Array.from({length: count}, (_, index) => ({id: `context:${index + 1}`, name: `Context ${index + 1}`, kind: 'agent'})),
  {id: 'resource:notes', name: 'Shared notes', kind: 'notes'},
]});

test('every identity in a 611-member population is reachable exactly once, including the final partial page', () => {
  const value = model(611), before = structuredClone(value), seen = [];
  for (let page = 0; page < 11; page++) {
    const result = populationInspection(value, undefined, '', page);
    assert.equal(result.populationCount, 611);
    assert.equal(result.matchCount, 611);
    assert.equal(result.pageIndex, page);
    assert.equal(result.pageCount, 11);
    assert.equal(result.start, page * IDENTITY_PAGE_SIZE + 1);
    assert.equal(result.end, Math.min((page + 1) * IDENTITY_PAGE_SIZE, 611));
    assert.equal(result.hasPrevious, page > 0);
    assert.equal(result.hasNext, page < 10);
    seen.push(...result.items.map(item => item.id));
  }
  assert.deepEqual(seen, value.nodes.filter(node => node.kind === 'agent').map(node => node.id));
  assert.equal(new Set(seen).size, 611);
  assert.deepEqual(value, before);
});

test('population subsets and search counts use exact current members, excluding other kinds and absent IDs', () => {
  const value = model(611), members = value.nodes.slice(201, 612).map(node => node.id);
  const result = populationInspection(value, [...members, 'resource:notes', 'foreign:missing'], 'CONTEXT:6', 0);
  const expected = value.nodes.filter(node => node.kind === 'agent' && members.includes(node.id) && node.id.startsWith('context:6'));
  assert.equal(result.populationCount, 411);
  assert.equal(result.matchCount, expected.length);
  assert.deepEqual(result.items, expected);
  const final = populationInspection(value, members, 'context:611');
  assert.deepEqual(final.items.map(item => item.id), ['context:611']);
  assert.equal(final.start, 1);
  assert.equal(final.end, 1);
  assert.equal(populationInspection(value, [], '').populationCount, 0);
});

test('search and smaller source populations clamp stale page indexes without dropping matches', () => {
  const searched = populationInspection(model(611), undefined, 'Context 61', 10);
  assert.equal(searched.pageIndex, 0);
  assert.equal(searched.matchCount, 3);
  assert.deepEqual(searched.items.map(item => item.id), ['context:61', 'context:610', 'context:611']);
  const replacement = populationInspection(model(65), undefined, '', 10);
  assert.equal(replacement.pageIndex, 1);
  assert.deepEqual(replacement.items.map(item => item.id), ['context:61', 'context:62', 'context:63', 'context:64', 'context:65']);
  for (const page of [-1, 1.5, NaN, Infinity, '10']) assert.equal(populationInspection(model(611), undefined, '', page).pageIndex, 0);
  assert.equal(populationInspection(model(611), undefined, '', 100).pageIndex, 10);
});

test('no matches and an empty model produce an explicit empty result rather than a misleading range', () => {
  for (const value of [model(611), null]) {
    const result = populationInspection(value, undefined, 'No such identity', 5);
    assert.equal(result.matchCount, 0);
    assert.equal(result.pageIndex, 0);
    assert.equal(result.pageCount, 1);
    assert.equal(result.start, 0);
    assert.equal(result.end, 0);
    assert.deepEqual(result.items, []);
    assert.equal(result.hasPrevious, false);
    assert.equal(result.hasNext, false);
  }
});
