import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { mountToolkitView } from '@rkat/toolkit/ui';
import { fixture, control } from '../examples/management-fixture.mjs';

function snapshot(revision = 1) {
  const data = fixture(revision), base = data.connections[0];
  data.tasks = []; data.frontends = []; data.packages = [];
  data.announcement = `Owner snapshot ${revision}`;
  data.connections = ['First connection', 'Second connection'].map((service, index) => ({
    ...structuredClone(base), id: `connection-${index}`, service, account: `account-${index}@example.test`,
    grants: [], pendingTaskIds: [], controls: [control('disconnect', 'shared-owner-offer')],
  }));
  return data;
}
function mount(t, dispatch = async () => {}) {
  const dom = new JSDOM('<!doctype html><button id="outside">Outside</button><main></main>');
  const doc = dom.window.document, host = doc.querySelector('main');
  // jsdom does not model the browser blur caused by disabling its active button.
  // Retain the native property and model only that observed browser consequence.
  const descriptor = Object.getOwnPropertyDescriptor(dom.window.HTMLButtonElement.prototype, 'disabled');
  Object.defineProperty(dom.window.HTMLButtonElement.prototype, 'disabled', {
    ...descriptor, set(value) { if (value && doc.activeElement === this) this.blur(); descriptor.set.call(this, value); },
  });
  const view = mountToolkitView(host, { scope: 'fixture-viewer', dispatch });
  view.update(snapshot());
  t.after(() => { view.dispose(); dom.window.close(); });
  const buttons = () => [...host.querySelectorAll('button')].filter(b => b.textContent === 'Disconnect account');
  const heading = index => [...host.querySelectorAll('h3')][index];
  return { doc, host, view, buttons, heading, outside: doc.getElementById('outside') };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
function captured(calls) {
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0], { scope: 'fixture-viewer', viewRevision: 1, control: snapshot().connections[0].controls[0] });
}
function pending(ui) { assert.ok(ui.buttons().every(b => b.disabled)); }

for (const clicked of [0, 1]) test(`focused dispatch remains at its heading through a snapshot: ${clicked}`, t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  ui.buttons()[clicked].focus(); ui.buttons()[clicked].click();
  assert.equal(ui.doc.activeElement, ui.heading(clicked));
  pending(ui); captured(calls);
  ui.view.update(snapshot(2));
  assert.equal(ui.doc.activeElement, ui.heading(clicked));
  pending(ui); captured(calls);
});

test('programmatic duplicate click preserves the actually focused occurrence', t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  ui.buttons()[1].focus(); ui.buttons()[0].click();
  assert.equal(ui.doc.activeElement, ui.heading(1));
  pending(ui); captured(calls);
});

test('programmatic click never takes external focus', t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  ui.outside.focus(); ui.buttons()[0].click();
  assert.equal(ui.doc.activeElement, ui.outside);
  ui.view.update(snapshot(2));
  assert.equal(ui.doc.activeElement, ui.outside);
  pending(ui); captured(calls);
});

test('dispatch synchronous update sees pending custody and keeps exact old request', t => {
  const calls = []; let ui;
  ui = mount(t, request => { calls.push(request); ui.view.update(snapshot(2)); });
  ui.buttons()[1].focus(); ui.buttons()[0].click();
  assert.equal(ui.doc.activeElement, ui.heading(1));
  assert.equal(ui.host.querySelector('[role=status]').textContent, 'Owner snapshot 2');
  pending(ui); captured(calls);
});

test('dispatch synchronous external focus remains selected', t => {
  const calls = []; let ui;
  ui = mount(t, request => { calls.push(request); ui.outside.focus(); });
  ui.buttons()[0].focus(); ui.buttons()[0].click();
  assert.equal(ui.doc.activeElement, ui.outside); pending(ui); captured(calls);
});

test('dispatch synchronous disposal retains one request without presentation revival', t => {
  const calls = []; let ui;
  ui = mount(t, request => { calls.push(request); ui.view.dispose(); });
  const button = ui.buttons()[0]; button.focus(); button.click(); button.click();
  assert.equal(ui.host.childElementCount, 0); captured(calls);
});

for (const event of ['focus', 'blur']) test(`${event} callback update cannot retarget revision or leave detached focus`, t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  const button = ui.buttons()[1];
  (event === 'focus' ? ui.heading(1) : button).addEventListener(event, () => ui.view.update(snapshot(2)), { once: true });
  button.focus(); ui.buttons()[0].click();
  assert.equal(ui.doc.activeElement, ui.heading(1));
  assert.equal(ui.host.querySelector('[role=status]').textContent, 'Owner snapshot 2');
  pending(ui); captured(calls);
});

for (const event of ['focus', 'blur']) test(`${event} callback disposal does not cancel or repeat dispatch`, t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  const button = ui.buttons()[0];
  (event === 'focus' ? ui.heading(0) : button).addEventListener(event, () => ui.view.dispose(), { once: true });
  button.focus(); button.click();
  assert.equal(ui.host.childElementCount, 0); captured(calls);
});

for (const event of ['focus', 'blur']) test(`${event} callback moving outside remains authoritative`, t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  const button = ui.buttons()[0];
  (event === 'focus' ? ui.heading(0) : button).addEventListener(event, () => ui.outside.focus(), { once: true });
  button.focus(); button.click();
  assert.equal(ui.doc.activeElement, ui.outside); pending(ui); captured(calls);
});

test('focus reentrant clicks cannot repeat the same offer', t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  const [first, second] = ui.buttons();
  ui.heading(1).addEventListener('focus', () => { first.click(); second.click(); }, { once: true });
  second.focus(); first.click();
  assert.equal(ui.doc.activeElement, ui.heading(1)); pending(ui); captured(calls);
});

test('rejected nested update retains old pending presentation and request', t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  ui.heading(0).addEventListener('focus', () => {
    const invalid = snapshot(2); invalid.scope = 'another-viewer';
    assert.throws(() => ui.view.update(invalid), TypeError);
  }, { once: true });
  ui.buttons()[0].focus(); ui.buttons()[0].click();
  assert.equal(ui.doc.activeElement, ui.heading(0));
  assert.match(ui.host.querySelector('[role=status]').textContent, /Waiting for owner status/);
  pending(ui); captured(calls);
});

test('late uncertainty updates current duplicate offer without moving focus or enabling retry', async t => {
  const calls = []; let reject;
  const result = new Promise((resolve, fail) => { reject = fail; });
  const ui = mount(t, request => { calls.push(request); return result; });
  ui.buttons()[1].focus(); ui.buttons()[0].click();
  ui.view.update(snapshot(2)); ui.outside.focus(); reject(new Error('synthetic owner uncertainty')); await tick();
  assert.equal(ui.doc.activeElement, ui.outside);
  assert.match(ui.host.querySelector('[role=status]').textContent, /Could not confirm/);
  pending(ui); captured(calls);
});

test('late rejection cannot overwrite a newly offered operation', async t => {
  let reject; const calls = [], result = new Promise((resolve, fail) => { reject = fail; });
  const ui = mount(t, request => { calls.push(request); return result; });
  ui.buttons()[0].focus(); ui.buttons()[0].click();
  const next = snapshot(2); next.connections.forEach(row => { row.controls[0].offer = 'fresh-owner-offer'; });
  ui.view.update(next); ui.outside.focus(); reject(new Error('synthetic failure')); await tick();
  assert.equal(ui.doc.activeElement, ui.outside);
  assert.equal(ui.host.querySelector('[role=status]').textContent, 'Owner snapshot 2');
  assert.ok(ui.buttons().every(b => !b.disabled)); captured(calls);
});

test('snapshot and focus navigation alone never dispatch', t => {
  const calls = [], ui = mount(t, request => { calls.push(request); });
  ui.heading(0).focus(); ui.view.update(snapshot(2));
  assert.equal(ui.doc.activeElement, ui.heading(0));
  assert.deepEqual(calls, []);
});
