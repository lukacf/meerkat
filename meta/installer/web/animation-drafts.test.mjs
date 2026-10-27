import { test } from 'node:test';
import assert from 'node:assert/strict';
import { animationDraftSelection, createAnimationDraftStore } from './animation-drafts.mjs';

const storage = () => {
  const values = new Map();
  return { values, getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
};
const scope = (setup = 'owned-guide', proposal = 'exact proposal', view = '__overview__') => animationDraftSelection(setup, proposal, view);
const firstHash = 'a'.repeat(64), secondHash = 'b'.repeat(64);

test('raw Unicode and whitespace survive a new tab-store instance only for the exact setup, proposal and view', () => {
  const tab = storage(), owner = scope(), draft = '  Slower 🦊 collaboration.\n\tKeep the two teams distinct.  ';
  const original = createAnimationDraftStore(tab);
  original.attach(owner, firstHash);
  original.update(owner, draft);
  const restored = createAnimationDraftStore(tab);
  restored.attach(owner, firstHash);
  assert.equal(restored.read(owner).text, draft);
  assert.equal(restored.read(owner).persistent, true);
  for (const other of [scope('other-guide'), scope('owned-guide', 'exact proposal', 'another-view'), scope('owned-guide', 'revised proposal')]) {
    restored.attach(other, other.proposalKey === 'revised proposal' ? secondHash : firstHash);
    assert.equal(restored.read(other).text, '');
    restored.update(other, 'Separate editable text');
  }
  assert.equal(restored.read(owner).text, draft);
  assert.ok([...tab.values.keys()].every(key => !key.includes('exact proposal')));
  assert.ok([...tab.values.values()].every(value => !value.includes('exact proposal')));
});

test('a delayed scoped storage read preserves newer edits and new selections do not borrow the old draft', () => {
  const tab = storage(), owner = scope(), prior = createAnimationDraftStore(tab);
  prior.attach(owner, firstHash); prior.update(owner, 'Old saved text');
  const drafts = createAnimationDraftStore(tab);
  drafts.update(owner, 'New text entered before source hashing finished');
  const other = scope('different-guide');
  drafts.attach(other, firstHash);
  assert.equal(drafts.read(other).text, '');
  drafts.attach(owner, firstHash);
  assert.equal(drafts.read(owner).text, 'New text entered before source hashing finished');
  const reopened = createAnimationDraftStore(tab); reopened.attach(owner, firstHash);
  assert.equal(reopened.read(owner).text, drafts.read(owner).text);
  assert.equal(drafts.attach(owner, secondHash), false);
});

test('acknowledgement clears only the captured raw text/edit revision and preserves newer same-text edits and other scopes', () => {
  const tab = storage(), drafts = createAnimationDraftStore(tab), owner = scope(), other = scope('other-guide');
  drafts.attach(owner, firstHash); drafts.attach(other, firstHash);
  drafts.update(owner, '  Same feedback\n'); drafts.update(other, 'Another setup draft');
  const sent = drafts.capture(owner);
  assert.equal(drafts.read(owner).text, sent.text, 'Capturing submission does not consume a draft');
  drafts.update(owner, 'Different edit'); drafts.update(owner, sent.text);
  assert.equal(drafts.clearSubmitted(owner, sent), false);
  assert.equal(drafts.clearSubmitted(other, drafts.capture(owner)), false);
  const next = drafts.capture(owner);
  assert.equal(drafts.clearSubmitted(owner, next), true);
  assert.equal(drafts.read(owner).text, '');
  assert.equal(drafts.read(other).text, 'Another setup draft');
  const reopened = createAnimationDraftStore(tab); reopened.attach(owner, firstHash);
  assert.equal(reopened.read(owner).text, '');
});

test('denied and full storage keep an editable page fallback without pretending reload persistence', () => {
  for (const tab of [null, { getItem() { throw new Error('denied'); }, setItem() {}, removeItem() {} },
    { getItem() { return null; }, setItem() { throw new Error('quota'); }, removeItem() {} }]) {
    const drafts = createAnimationDraftStore(tab), owner = scope();
    drafts.attach(owner, firstHash); drafts.update(owner, '\nDraft in this open page 🦊\t');
    assert.equal(drafts.read(owner).text, '\nDraft in this open page 🦊\t');
    assert.equal(drafts.read(owner).persistent, false);
    drafts.update(owner, 'Still editable');
    assert.equal(drafts.read(owner).text, 'Still editable');
  }
});

test('untrusted records and oversized edits cannot replace current human text or grant request authority', () => {
  const tab = storage(), drafts = createAnimationDraftStore(tab), owner = scope();
  drafts.attach(owner, firstHash); drafts.update(owner, 'Retained');
  assert.equal(drafts.update(owner, 'x'.repeat(1001)), false);
  assert.equal(drafts.read(owner).text, 'Retained');
  assert.equal(animationDraftSelection('', 'proposal', '__overview__'), null);
  const key = [...tab.values.keys()][0];
  for (const value of ['bad JSON', JSON.stringify({ owner: { setup_id: 'another-guide' }, text: 'wrong', revision: 0 }),
    JSON.stringify({ owner: drafts.capture(owner).owner, text: 'wrong', revision: 0, admitted: true })]) {
    tab.values.set(key, value);
    const reopened = createAnimationDraftStore(tab); reopened.attach(owner, firstHash);
    assert.equal(reopened.read(owner).text, '');
  }
});

test('an unsaved manual preview keeps its draft locally without persisting a false setup identity', () => {
  const tab = storage(), drafts = createAnimationDraftStore(tab), owner = scope('unsaved-preview');
  assert.equal(drafts.attach(owner, firstHash, false), true);
  drafts.update(owner, 'Local manual preview text');
  assert.equal(drafts.read(owner).ready, true);
  assert.equal(drafts.read(owner).persistent, false);
  assert.equal(drafts.read(owner).text, 'Local manual preview text');
  assert.equal(tab.values.size, 0);
});
