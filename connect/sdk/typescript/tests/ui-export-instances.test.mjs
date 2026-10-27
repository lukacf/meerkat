import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { mountToolkitView } from '@rkat/toolkit/ui';

const ready = () => ({ tone: 'ready', text: 'Host reports ready' });
function connection(id, packageId = 'mail-package', account = id) {
  return { id, packageId, service: 'Mail', account, credentialScope: 'Read mail',
    status: ready(), grants: [], pendingTaskIds: [], controls: [] };
}
function frontend(id, packageId = 'mail-package') {
  return { kind: 'frontend', id, packageId, label: 'Mail assistant', status: ready(),
    endpoint: `${id} / private channel`, audience: `${id} members`, target: 'Assistant',
    conversation: 'Per thread', output: 'Same thread', serviceAccess: 'Host policy',
    lifetime: 'Until removed', controls: [] };
}
function snapshot() {
  return { scope: 'person-a', revision: 1, observedAt: 'host observation', announcement: '',
    tasks: [], connections: [], frontends: [], packages: [{ id: 'mail-package', label: 'Mail package',
      version: '1', status: ready(), exports: [{ kind: 'service', id: 'mail-export', label: 'Mail access',
        status: ready() }], components: [] }] };
}
function mount(t, value) {
  const dom = new JSDOM('<!doctype html><main></main>');
  const doc = dom.window.document, host = doc.querySelector('main'), calls = [];
  const view = mountToolkitView(host, { scope: value.scope, async dispatch(request) { calls.push(request); } });
  t.after(() => { view.dispose(); dom.window.close(); });
  view.update(value);
  return { dom, doc, host, view, calls };
}
function packageCard(ui) {
  return [...ui.host.querySelectorAll('article')].find(row => row.querySelector('h3')?.textContent === 'Mail package 1');
}
function links(ui, label = 'View connection') {
  return [...packageCard(ui).querySelectorAll('button')].filter(button => button.textContent === label);
}
function accessibleName(ui, button) {
  return (button.getAttribute('aria-labelledby') ?? '').split(/\s+/).filter(Boolean)
    .map(id => ui.doc.getElementById(id)?.textContent ?? '').join(' ');
}

test('an export ID never guesses a same-ID instance from another package', t => {
  const value = snapshot();
  value.connections = [connection('mail-export', 'other-package', 'Other account')];
  const ui = mount(t, value);
  assert.equal(links(ui).length, 0, 'legacy export metadata must not link a foreign same-ID connection');
  assert.deepEqual(ui.calls, []);
});

test('one export links two explicitly supplied accounts with distinct visible and accessible context', t => {
  const value = snapshot();
  value.connections = [connection('work-connection', 'mail-package', 'Work account'),
    connection('home-connection', 'mail-package', 'Home account')];
  value.packages[0].exports[0].instanceIds = ['home-connection', 'work-connection'];
  const ui = mount(t, value), buttons = links(ui);
  assert.equal(buttons.length, 2);
  for (const [index, account] of ['Home account', 'Work account'].entries()) {
    assert.ok(packageCard(ui).textContent.includes(account));
    assert.ok(accessibleName(ui, buttons[index]).includes(account));
    buttons[index].click();
    assert.ok(ui.doc.activeElement.closest('article').textContent.includes(account));
  }
  assert.deepEqual(ui.calls, []);
});

test('one frontend export links each endpoint with its own audience context', t => {
  const value = snapshot();
  value.frontends = [frontend('work-entry'), frontend('home-entry')];
  value.packages[0].exports = [{ kind: 'frontend', id: 'mail-entry-export', label: 'Mail entry',
    status: ready(), instanceIds: ['home-entry', 'work-entry'] }];
  const ui = mount(t, value), buttons = links(ui, 'View frontend');
  assert.equal(buttons.length, 2);
  for (const [index, id] of ['home-entry', 'work-entry'].entries()) {
    const instance = value.frontends.find(item => item.id === id);
    assert.ok(accessibleName(ui, buttons[index]).includes(instance.endpoint));
    assert.ok(accessibleName(ui, buttons[index]).includes(instance.audience));
    assert.ok(!accessibleName(ui, buttons[index]).includes(value.frontends.find(item => item.id !== id).audience));
    buttons[index].click();
    assert.ok(ui.doc.activeElement.closest('article').textContent.includes(instance.endpoint));
  }
  assert.deepEqual(ui.calls, []);
});

for (const explicitEmpty of [false, true]) {
  test(`same-package same-ID legacy metadata never infers a link (empty references: ${explicitEmpty})`, t => {
    const value = snapshot();
    value.connections = [connection('mail-export')];
    if (explicitEmpty) value.packages[0].exports[0].instanceIds = [];
    const ui = mount(t, value);
    assert.equal(links(ui).length, 0);
    assert.ok(ui.host.textContent.includes('Mail access'));
  });
}

const invalidReferences = [
  ['missing instance', value => { value.packages[0].exports[0].instanceIds = ['private-missing-id']; }],
  ['wrong package', value => { value.connections[0].packageId = 'private-other-package'; }],
  ['wrong role', value => { value.connections = []; value.frontends = [frontend('work-connection')]; }],
  ['duplicate target in one export', value => { value.packages[0].exports[0].instanceIds.push('work-connection'); }],
  ['target assigned to two exports', value => { value.packages[0].exports.push({ ...value.packages[0].exports[0], id: 'other-export' }); }],
  ['non-array references', value => { value.packages[0].exports[0].instanceIds = 'work-connection'; }],
  ['non-string reference', value => { value.packages[0].exports[0].instanceIds = [123]; }],
  ['blank reference', value => { value.packages[0].exports[0].instanceIds = [' ']; }],
];
for (const [name, invalidate] of invalidReferences) {
  test(`rejects ${name} atomically and accepts corrected same revision`, t => {
    const value = snapshot();
    value.connections = [connection('work-connection', 'mail-package', 'Work account')];
    value.packages[0].exports[0].instanceIds = ['work-connection'];
    const ui = mount(t, value), button = links(ui)[0];
    button.focus();
    const before = ui.host.innerHTML, next = structuredClone(value); next.revision = 2;
    invalidate(next);
    assert.throws(() => ui.view.update(next), { name: 'TypeError', message: 'Invalid or ambiguous Toolkit projection' });
    assert.equal(ui.host.innerHTML, before);
    assert.equal(ui.doc.activeElement, button);
    button.click();
    assert.ok(ui.doc.activeElement.closest('article').textContent.includes('Work account'));
    const corrected = structuredClone(value); corrected.revision = 2;
    ui.view.update(corrected);
    assert.equal(links(ui).length, 1);
    assert.deepEqual(ui.calls, []);
  });
}

for (const [name, invalidate] of [
  ['missing frontend', value => { value.frontends = []; }],
  ['frontend in another package', value => { value.frontends[0].packageId = 'other-package'; }],
  ['connection in place of frontend', value => { value.frontends = []; value.connections = [connection('entry')]; }],
  ['frontend repeated in one export', value => { value.packages[0].exports[0].instanceIds.push('entry'); }],
  ['frontend shared across exports', value => { value.packages[0].exports.push({ ...value.packages[0].exports[0], id: 'another-frontend-export' }); }],
]) {
  test(`rejects ${name} before rendering or navigating`, t => {
    const value = snapshot(); value.frontends = [frontend('entry')];
    value.packages[0].exports = [{ kind: 'frontend', id: 'entry-export', label: 'Entry', status: ready(), instanceIds: ['entry'] }];
    const ui = mount(t, value), before = ui.host.innerHTML;
    const next = structuredClone(value); next.revision = 2; invalidate(next);
    assert.throws(() => ui.view.update(next), { name: 'TypeError', message: 'Invalid or ambiguous Toolkit projection' });
    assert.equal(ui.host.innerHTML, before);
    links(ui, 'View frontend')[0].click();
    assert.ok(ui.doc.activeElement.closest('article').textContent.includes('entry / private channel'));
    assert.deepEqual(ui.calls, []);
  });
}

test('service and frontend instance IDs are separate role identities', t => {
  const value = snapshot();
  value.connections = [connection('shared-id')]; value.frontends = [frontend('shared-id')];
  value.packages[0].exports = [
    { kind: 'service', id: 'shared-export', label: 'Service export', status: ready(), instanceIds: ['shared-id'] },
    { kind: 'frontend', id: 'shared-export', label: 'Frontend export', status: ready(), instanceIds: ['shared-id'] },
  ];
  const ui = mount(t, value);
  links(ui)[0].click();
  assert.match(ui.doc.activeElement.textContent, /Mail/);
  assert.ok(!ui.doc.activeElement.closest('article').textContent.includes('private channel'));
  links(ui, 'View frontend')[0].click();
  assert.ok(ui.doc.activeElement.closest('article').textContent.includes('shared-id / private channel'));
});

test('reference order changes preserve the exact focused instance link', t => {
  const value = snapshot();
  value.connections = [connection('work'), connection('home')];
  value.packages[0].exports[0].instanceIds = ['work', 'home'];
  const ui = mount(t, value); links(ui)[0].focus();
  const next = structuredClone(value); next.revision = 2;
  next.packages[0].exports[0].instanceIds.reverse();
  ui.view.update(next);
  assert.equal(ui.doc.activeElement, links(ui)[1]);
  assert.ok(accessibleName(ui, ui.doc.activeElement).includes('Connected account: work'));
});

test('moving an instance to a different export does not transfer focused link identity', t => {
  const value = snapshot(); value.connections = [connection('work')];
  value.packages[0].exports[0].instanceIds = ['work'];
  const ui = mount(t, value), staleButton = links(ui)[0]; staleButton.focus();
  const next = structuredClone(value); next.revision = 2;
  next.packages[0].exports[0].id = 'new-export';
  ui.view.update(next);
  assert.equal(ui.doc.activeElement, packageCard(ui).querySelector('h3'));
  staleButton.click();
  assert.equal(ui.doc.activeElement, packageCard(ui).querySelector('h3'));
  links(ui)[0].click();
  assert.equal(ui.doc.activeElement.textContent, 'Mail');
  assert.deepEqual(ui.calls, []);
});

test('removing an instance with its reference removes navigation without a guessed replacement', t => {
  const value = snapshot(); value.connections = [connection('work')];
  value.packages[0].exports[0].instanceIds = ['work'];
  const ui = mount(t, value), staleButton = links(ui)[0]; staleButton.focus();
  const next = structuredClone(value); next.revision = 2;
  next.connections = [connection('mail-export')]; next.packages[0].exports[0].instanceIds = [];
  ui.view.update(next);
  assert.equal(links(ui).length, 0);
  assert.equal(ui.doc.activeElement, packageCard(ui).querySelector('h3'));
  staleButton.click();
  assert.equal(ui.doc.activeElement, packageCard(ui).querySelector('h3'));
});

test('navigation context is display text, never HTML or an external address', t => {
  const value = snapshot();
  const unsafe = '<img src="https://private.example.test/track" onerror="bad()">';
  value.connections = [connection('work', 'mail-package', unsafe)];
  value.frontends = [{ ...frontend('entry'), endpoint: 'javascript:bad()', audience: unsafe }];
  value.packages[0].exports[0].instanceIds = ['work'];
  value.packages[0].exports.push({ kind: 'frontend', id: 'entry-export', label: 'Entry', status: ready(), instanceIds: ['entry'] });
  const ui = mount(t, value);
  assert.equal(ui.host.querySelector('img, a, [onclick], [onerror]'), null);
  assert.ok(accessibleName(ui, links(ui)[0]).includes(unsafe));
  assert.ok(accessibleName(ui, links(ui, 'View frontend')[0]).includes('javascript:bad()'));
  assert.deepEqual(ui.calls, []);
});
