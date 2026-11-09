import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { mountToolkitView } from '@rkat/toolkit/ui';
import { fixture } from '../examples/management-fixture.mjs';

function mount(t, snapshot = fixture(), dispatch = async () => {}) {
  const dom = new JSDOM('<!doctype html><button id="outside">Outside</button><main></main>');
  const doc = dom.window.document;
  const host = doc.querySelector('main');
  const view = mountToolkitView(host, { scope: snapshot.scope, dispatch });
  view.update(snapshot);
  t.after(() => { view.dispose(); dom.window.close(); });
  return { doc, host, view };
}
function buttons(ui, text) {
  return [...ui.host.querySelectorAll('button')].filter(node => node.textContent === text);
}
function context(ui, button) {
  const ids = ['aria-labelledby', 'aria-describedby'].flatMap(name =>
    (button.getAttribute(name) ?? '').split(/\s+/).filter(Boolean));
  for (const id of ids) assert.ok(ui.doc.getElementById(id), 'every accessible reference resolves');
  return ids.map(id => ui.doc.getElementById(id).textContent).join(' ');
}
const tick = () => new Promise(resolve => setImmediate(resolve));

for (const label of ['View setup: Answer support questions in Slack', 'View frontend', 'View connection', 'View package']) {
  test(`navigation heading remains focused through a later snapshot: ${label}`, t => {
    const calls = [], ui = mount(t, fixture(), request => { calls.push(request); });
    buttons(ui, label)[0].click();
    const heading = ui.doc.activeElement.textContent;
    assert.equal(ui.doc.activeElement.tagName, 'H3');
    ui.view.update(fixture(2));
    assert.equal(ui.doc.activeElement.tagName, 'H3');
    assert.equal(ui.doc.activeElement.textContent, heading);
    assert.ok(ui.host.contains(ui.doc.activeElement));
    assert.deepEqual(calls, []);
  });
}

test('removed-control fallback heading remains focused on the following update', t => {
  const calls = [], ui = mount(t, fixture(), request => { calls.push(request); });
  buttons(ui, 'Revoke this access')[1].focus();
  const second = fixture(2); second.connections[0].grants[0].controls = [];
  ui.view.update(second);
  assert.equal(ui.doc.activeElement.tagName, 'H3');
  const third = structuredClone(second); third.revision = 3;
  ui.view.update(third);
  assert.equal(ui.doc.activeElement.tagName, 'H3');
  assert.equal(ui.doc.activeElement.textContent, second.connections[0].grants[0].label);
  assert.deepEqual(calls, []);
});

test('removing the focused heading falls back to the panel and retains that focus later', t => {
  const calls = [], ui = mount(t, fixture(), request => { calls.push(request); });
  buttons(ui, 'View connection')[0].click();
  const second = fixture(2); second.connections = [];
  second.packages.find(item => item.id === 'docs-package').exports[0].instanceIds = [];
  ui.view.update(second);
  assert.equal(ui.doc.activeElement.tagName, 'H2');
  assert.equal(ui.doc.activeElement.textContent, 'Connections and agent frontends');
  const third = structuredClone(second); third.revision = 3;
  ui.view.update(third);
  assert.equal(ui.doc.activeElement.textContent, 'Connections and agent frontends');
  assert.ok(ui.host.contains(ui.doc.activeElement));
  assert.deepEqual(calls, []);
});

test('snapshot refresh never steals external keyboard focus or dispatches', t => {
  const calls = [], ui = mount(t, fixture(), request => { calls.push(request); });
  const outside = ui.doc.getElementById('outside'); outside.focus();
  ui.view.update(fixture(2));
  assert.equal(ui.doc.activeElement, outside);
  assert.deepEqual(calls, []);
});

function accountSnapshot(revision = 1) {
  const snapshot = fixture(revision), source = snapshot.connections[0];
  snapshot.connections = ['alice@example.test', 'bob@example.test'].map((account, index) => ({
    ...structuredClone(source), id: `connection-${index}`, account, grants: [], pendingTaskIds: [],
    controls: [{ kind: 'disconnect', offer: `offer-${index}`, target: { owner: 'private-owner-canary', reference: `private-reference-${index}`, revision: 'private-revision-canary' } }],
  }));
  snapshot.packages.find(item => item.id === 'docs-package').exports[0].instanceIds = snapshot.connections.map(item => item.id);
  return snapshot;
}

test('same-service connection controls expose only their own displayed account context', t => {
  const snapshot = accountSnapshot(), ui = mount(t, snapshot);
  const controls = buttons(ui, 'Disconnect account');
  assert.equal(controls.length, 2);
  controls.forEach((button, index) => {
    assert.ok(context(ui, button).includes(snapshot.connections[index].account));
    assert.ok(!context(ui, button).includes(snapshot.connections[1 - index].account));
    assert.ok(context(ui, button).includes(snapshot.connections[index].service));
  });
  assert.ok(!ui.host.innerHTML.includes('private-owner-canary'));
  assert.ok(!ui.host.innerHTML.includes('private-reference-'));
  assert.ok(!ui.host.innerHTML.includes('private-revision-canary'));
});

test('service grant controls expose the displayed account inside their own card', t => {
  const snapshot = fixture(), ui = mount(t, snapshot);
  const buttonsByRow = buttons(ui, 'Revoke this access');
  assert.ok(context(ui, buttonsByRow[0]).includes(snapshot.tasks[0].roles[0].account));
  assert.ok(context(ui, buttonsByRow[1]).includes(snapshot.connections[0].grants[0].account));
});

test('frontend controls retain exact endpoint and audience context without another frontend context', t => {
  const snapshot = fixture(), first = snapshot.frontends[0];
  snapshot.tasks = [];
  snapshot.frontends = ['Private Alpha', 'Private Beta'].map((audience, index) => ({
    ...structuredClone(first), id: `frontend-${index}`, label: 'Support frontend',
    endpoint: `workspace-${index} / channel-${index}`, audience,
    controls: [{ kind: 'remove_frontend', offer: `frontend-offer-${index}`, target: { owner: 'private-owner-canary', reference: `private-reference-${index}`, revision: 'r' } }],
  }));
  snapshot.packages.find(item => item.id === 'slack-package').exports[0].instanceIds = snapshot.frontends.map(item => item.id);
  const ui = mount(t, snapshot), controls = buttons(ui, 'Remove frontend');
  assert.equal(controls.length, 2);
  controls.forEach((button, index) => {
    assert.ok(context(ui, button).includes(snapshot.frontends[index].endpoint));
    assert.ok(context(ui, button).includes(snapshot.frontends[index].audience));
    assert.ok(!context(ui, button).includes(snapshot.frontends[1 - index].endpoint));
    assert.ok(!context(ui, button).includes(snapshot.frontends[1 - index].audience));
  });
});

test('pending and uncertain control updates retain account context and exact single dispatch', async t => {
  let reject;
  const pending = new Promise((resolve, failure) => { reject = failure; });
  const calls = [], snapshot = accountSnapshot(), ui = mount(t, snapshot, request => { calls.push(request); return pending; });
  buttons(ui, 'Disconnect account')[0].click();
  assert.ok(context(ui, buttons(ui, 'Disconnect account')[0]).includes(snapshot.connections[0].account));
  assert.match(context(ui, buttons(ui, 'Disconnect account')[0]), /Waiting for owner status/);
  ui.view.update(accountSnapshot(2));
  reject(new Error('private-failure-canary')); await tick();
  const current = buttons(ui, 'Disconnect account')[0];
  assert.ok(context(ui, current).includes(snapshot.connections[0].account));
  assert.match(context(ui, current), /Could not confirm/);
  assert.equal(current.disabled, true);
  assert.ok(!context(ui, current).includes('private-failure-canary'));
  current.click();
  assert.deepEqual(calls, [{ scope: snapshot.scope, viewRevision: 1, control: snapshot.connections[0].controls[0] }]);
});
