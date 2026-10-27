import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM, ResourceLoader } from 'jsdom';
import { mountToolkitView } from '@rkat/toolkit/ui';
import { fixture } from '../examples/management-fixture.mjs';
import { catalogFixture } from '../examples/catalog-fixture.mjs';
import { mountCatalogManagement } from '../examples/management-catalog.mjs';

const tick = () => new Promise(resolve => setImmediate(resolve));
const pendingText = 'Opening advanced manual configuration...';
const failedText = 'Could not confirm manual configuration navigation. You can try opening it again.';
function snapshot(revision = 1) {
  const catalog = catalogFixture();
  catalog.entries.push({
    id: 'private-id-only', label: 'Archive [v2]', description: 'Read exact notes.  Two spaces.',
    roles: ['service'], categoryIds: ['communication', '__all__'], support: 'A described profile',
    prerequisites: [], evidence: 'Local package evidence only',
  });
  return { scope: 'catalog-person', revision, observedAt: 'synthetic', announcement: 'Host observation',
    tasks: [], connections: [], frontends: [], packages: [], catalog };
}
function mount(t, initial = snapshot(), extra = {}) {
  const network = [], calls = [];
  class DenyResources extends ResourceLoader {
    fetch(url) { network.push(url); throw new Error('Unexpected resource load'); }
  }
  const dom = new JSDOM('<!doctype html><button id="outside">Outside</button><main></main>', { resources: new DenyResources() });
  const doc = dom.window.document, host = doc.querySelector('main');
  dom.window.fetch = () => { network.push('fetch'); throw new Error('Unexpected fetch'); };
  dom.window.open = () => { network.push('open'); throw new Error('Unexpected navigation'); };
  const options = { scope: initial.scope, async dispatch(request) { calls.push(request); }, ...extra };
  const view = mountToolkitView(host, options);
  view.update(initial);
  t.after(() => { view.dispose(); dom.window.close(); });
  return { dom, doc, host, view, calls, network, options };
}
function catalog(ui) {
  return [...ui.host.querySelectorAll('section')].find(node => node.querySelector('h2')?.textContent === 'Catalog');
}
function search(ui) { return catalog(ui).querySelector('input[type="search"]'); }
function category(ui) { return catalog(ui).querySelector('select'); }
function query(ui, value) {
  const input = search(ui); input.value = value;
  input.dispatchEvent(new ui.dom.window.Event('input', { bubbles: true }));
}
function selectCategory(ui, label) {
  const select = category(ui);
  const option = [...select.options].find(item => item.textContent === (label === 'All categories' ? label : `Category: ${label}`));
  assert.ok(option, `visible category ${label}`);
  select.value = option.value;
  select.dispatchEvent(new ui.dom.window.Event('change', { bubbles: true }));
}
function names(ui) {
  return [...catalog(ui).querySelectorAll('article')].filter(node => !node.hidden).map(node => node.querySelector('h3').textContent);
}
function manual(ui) {
  return [...ui.host.querySelectorAll('button')].find(node => node.textContent === 'Advanced manual configuration');
}

test('catalog omission preserves the existing DOM and supplied owner controls', t => {
  const old = fixture(), ui = mount(t, old);
  assert.equal(catalog(ui), undefined);
  assert.equal(ui.host.querySelector('input,select'), null);
  assert.equal(manual(ui), undefined);
  assert.ok(!ui.host.textContent.includes('Advanced manual configuration'));
  const disconnect = [...ui.host.querySelectorAll('button')].find(node => node.textContent === 'Disconnect account');
  disconnect.click();
  assert.deepEqual(ui.calls, [{ scope: old.scope, viewRevision: old.revision, control: old.connections[0].controls[1] }]);
});

test('catalog displays roles and evidence separately without manufacturing readiness or controls', t => {
  const ui = mount(t), node = catalog(ui);
  assert.deepEqual(names(ui), ['Slack', 'Document library', 'Archive [v2]']);
  assert.match(node.textContent, /Service access, Agent frontend/);
  for (const text of ['Support description', 'Prerequisites', 'Support evidence', 'No support evidence supplied.', 'No prerequisites supplied.']) {
    assert.ok(node.textContent.includes(text));
  }
  assert.equal(node.querySelector('.toolkit-status'), null);
  assert.equal(node.querySelector('button'), null);
  assert.ok(node.textContent.includes('do not establish installation, connectivity or permission'));
  assert.match(node.textContent, /3 of 3 catalog entries/);
  assert.deepEqual(ui.calls, []);
});

test('literal case-insensitive query keeps spaces and punctuation and excludes internal IDs', t => {
  const ui = mount(t);
  query(ui, 'ARCHIVE [v2]'); assert.deepEqual(names(ui), ['Archive [v2]']);
  query(ui, '[v.'); assert.deepEqual(names(ui), []);
  query(ui, '  Two'); assert.deepEqual(names(ui), ['Archive [v2]']);
  query(ui, '  Archive'); assert.deepEqual(names(ui), []);
  query(ui, 'private-id-only'); assert.deepEqual(names(ui), []);
  query(ui, 'support evidence'); assert.deepEqual(names(ui), []);
  query(ui, 'No support evidence supplied.'); assert.deepEqual(names(ui), []);
  query(ui, ''); assert.deepEqual(names(ui), ['Slack', 'Document library', 'Archive [v2]']);
  assert.deepEqual(ui.calls, []); assert.deepEqual(ui.network, []);
});

test('query searches displayed evidence, prerequisites, categories and explicit role wording', t => {
  const ui = mount(t);
  query(ui, 'agent frontend'); assert.deepEqual(names(ui), ['Slack']);
  query(ui, 'selected collection'); assert.deepEqual(names(ui), ['Document library']);
  query(ui, 'local package evidence'); assert.deepEqual(names(ui), ['Archive [v2]']);
  query(ui, 'Documents'); assert.deepEqual(names(ui), ['Document library', 'Archive [v2]']);
  query(ui, 'service access'); assert.deepEqual(names(ui), ['Slack', 'Document library', 'Archive [v2]']);
  query(ui, 'service access, agent frontend'); assert.deepEqual(names(ui), []);
});

test('exact category including sentinel-looking ID combines with query in host order', t => {
  const ui = mount(t);
  assert.deepEqual([...category(ui).options].map(item => item.textContent), ['All categories', 'Category: Communication', 'Category: Documents']);
  selectCategory(ui, 'Documents'); assert.deepEqual(names(ui), ['Document library', 'Archive [v2]']);
  query(ui, 'read'); assert.deepEqual(names(ui), ['Document library', 'Archive [v2]']);
  query(ui, 'archive'); assert.deepEqual(names(ui), ['Archive [v2]']);
  assert.match(catalog(ui).textContent, /1 of 3 catalog entries/);
  selectCategory(ui, 'All categories'); query(ui, '');
  assert.deepEqual(names(ui), ['Slack', 'Document library', 'Archive [v2]']);
});

test('a real All categories label is distinguishable without reserving host vocabulary', t => {
  const initial = snapshot(); initial.catalog.categories[1].label = 'All categories';
  const ui = mount(t, initial), select = category(ui);
  assert.deepEqual([...select.options].map(item => item.textContent), ['All categories', 'Category: Communication', 'Category: All categories']);
  select.value = select.options[2].value;
  select.dispatchEvent(new ui.dom.window.Event('change'));
  assert.deepEqual(names(ui), ['Document library', 'Archive [v2]']);
  query(ui, 'Category:'); assert.deepEqual(names(ui), []);
  query(ui, 'All categories'); assert.deepEqual(names(ui), ['Document library', 'Archive [v2]']);
});

test('supplied empty catalog differs from an empty filtered result', t => {
  const ui = mount(t);
  query(ui, 'unmatched'); assert.ok(catalog(ui).textContent.includes('No catalog entries match these filters.'));
  assert.equal([...catalog(ui).querySelectorAll('p')].find(n => n.textContent === 'No catalog entries match these filters.').hidden, false);
  const next = snapshot(2); next.catalog = { categories: [], entries: [] };
  ui.view.update(next);
  assert.match(catalog(ui).textContent, /0 of 0 catalog entries/);
  assert.ok(catalog(ui).textContent.includes('No catalog entries supplied.'));
  assert.equal(manual(ui), undefined);
});

test('query and exact category survive updates and temporary catalog omission', t => {
  const ui = mount(t);
  selectCategory(ui, 'Documents'); query(ui, 'read');
  search(ui).focus(); search(ui).setSelectionRange(1, 3, 'backward');
  ui.view.update(snapshot(2));
  assert.equal(ui.doc.activeElement, search(ui));
  assert.equal(search(ui).value, 'read');
  assert.deepEqual([search(ui).selectionStart, search(ui).selectionEnd, search(ui).selectionDirection], [1, 3, 'backward']);
  assert.equal(category(ui).selectedOptions[0].textContent, 'Category: Documents');
  const omitted = snapshot(3); delete omitted.catalog;
  ui.view.update(omitted);
  assert.equal(catalog(ui), undefined);
  assert.equal(ui.doc.activeElement.textContent, 'Connections and agent frontends');
  ui.view.update(snapshot(4));
  assert.equal(search(ui).value, 'read');
  assert.equal(category(ui).selectedOptions[0].textContent, 'Category: Documents');
  assert.deepEqual(names(ui), ['Document library', 'Archive [v2]']);
});

test('category focus is retained and a missing category resets visibly without clearing query', t => {
  const ui = mount(t);
  selectCategory(ui, 'Documents'); query(ui, 'read'); category(ui).focus();
  const next = snapshot(2);
  next.catalog.categories = next.catalog.categories.filter(item => item.id !== '__all__');
  next.catalog.entries.forEach(item => { item.categoryIds = item.categoryIds.filter(id => id !== '__all__'); });
  ui.view.update(next);
  assert.equal(ui.doc.activeElement, category(ui));
  assert.equal(category(ui).selectedOptions[0].textContent, 'All categories');
  assert.equal(search(ui).value, 'read');
  const reset = [...catalog(ui).querySelectorAll('p')].find(item => item.textContent.startsWith('The selected category'));
  assert.equal(reset.hidden, false);
});

test('caller mutation and rejected or stale updates cannot replace local filters or current DOM', t => {
  const initial = snapshot(), ui = mount(t, initial);
  initial.catalog.entries[0].label = 'Changed elsewhere';
  selectCategory(ui, 'Documents'); query(ui, 'archive'); search(ui).focus();
  const html = ui.host.innerHTML, input = search(ui);
  const rejected = snapshot(2); rejected.catalog.categories.push({ id: '__all__', label: 'Repeated' });
  assert.throws(() => ui.view.update(rejected), /Invalid or ambiguous Toolkit projection/);
  assert.equal(ui.host.innerHTML, html); assert.equal(ui.doc.activeElement, input);
  assert.equal(ui.view.update(snapshot(1)), false);
  assert.equal(search(ui).value, 'archive'); assert.deepEqual(names(ui), ['Archive [v2]']);
  assert.equal(ui.view.update(snapshot(2)), true);
  assert.equal(search(ui).value, 'archive'); assert.deepEqual(names(ui), ['Archive [v2]']);
});

test('filtered or removed focused entry falls back to catalog, with external focus untouched', t => {
  const ui = mount(t);
  catalog(ui).querySelector('h3').focus(); query(ui, 'archive');
  assert.equal(ui.doc.activeElement.textContent, 'Catalog');
  query(ui, ''); catalog(ui).querySelector('h3').focus();
  const next = snapshot(2); next.catalog.entries.shift(); ui.view.update(next);
  assert.equal(ui.doc.activeElement.textContent, 'Catalog');
  const outside = ui.doc.getElementById('outside'); outside.focus();
  query(ui, 'read'); ui.view.update(snapshot(3));
  assert.equal(ui.doc.activeElement, outside);
  assert.deepEqual(ui.calls, []);
});

test('focus reentrancy adopts the newer catalog and caret without stale restoration', t => {
  const ui = mount(t); query(ui, 'read'); search(ui).focus(); search(ui).setSelectionRange(0, 1);
  let reentered = false;
  ui.host.addEventListener('focusin', event => {
    if (!reentered && event.target.matches('input[type="search"]')) {
      reentered = true; ui.view.update(snapshot(3)); search(ui).setSelectionRange(2, 4);
    }
  });
  ui.view.update(snapshot(2));
  assert.equal(reentered, true);
  assert.equal(ui.doc.activeElement, search(ui));
  assert.deepEqual([search(ui).selectionStart, search(ui).selectionEnd], [2, 4]);
  assert.equal(ui.view.update(snapshot(3)), false);
});

test('a nested update reads the retained caret before its search focus callback runs', t => {
  const ui = mount(t); query(ui, 'service'); search(ui).focus(); search(ui).setSelectionRange(1, 4, 'backward');
  let entered = false;
  ui.host.addEventListener('focusin', event => {
    if (entered || event.target !== search(ui)) return;
    entered = true;
    assert.deepEqual([search(ui).selectionStart, search(ui).selectionEnd, search(ui).selectionDirection], [1, 4, 'backward']);
    ui.view.update(snapshot(3));
  });
  ui.view.update(snapshot(2));
  assert.equal(entered, true); assert.equal(ui.view.update(snapshot(3)), false);
  assert.equal(ui.doc.activeElement, search(ui));
  assert.deepEqual([search(ui).selectionStart, search(ui).selectionEnd, search(ui).selectionDirection], [1, 4, 'backward']);
  assert.deepEqual(ui.calls, []);
});

for (const mode of ['update', 'outside', 'dispose']) {
  test(`filtering a focused entry honors its host blur callback: ${mode}`, t => {
    const ui = mount(t), heading = catalog(ui).querySelector('h3');
    const errors = [];
    ui.dom.window.addEventListener('error', event => { errors.push(event.error); event.preventDefault(); });
    heading.focus();
    let entered = 0;
    heading.addEventListener('blur', () => {
      entered++;
      if (mode === 'update') ui.view.update(snapshot(2));
      else if (mode === 'outside') ui.doc.getElementById('outside').focus();
      else ui.view.dispose();
    }, { once: true });
    query(ui, 'archive');
    assert.equal(entered, 1); assert.deepEqual(errors, []); assert.deepEqual(ui.calls, []);
    if (mode === 'dispose') {
      assert.equal(ui.host.childElementCount, 0); assert.equal(ui.doc.activeElement, ui.doc.body);
    } else if (mode === 'outside') {
      assert.equal(ui.doc.activeElement, ui.doc.getElementById('outside'));
      ui.view.update(snapshot(3)); assert.equal(ui.doc.activeElement, ui.doc.getElementById('outside'));
    } else {
      assert.equal(ui.doc.activeElement, catalog(ui).querySelector('h2'));
      assert.equal(ui.doc.activeElement.isConnected, true); assert.equal(search(ui).value, 'archive');
      assert.deepEqual(names(ui), ['Archive [v2]']); assert.equal(ui.view.update(snapshot(2)), false);
      ui.view.update(snapshot(3)); assert.equal(ui.doc.activeElement, catalog(ui).querySelector('h2'));
    }
  });
}

test('hostile catalog strings stay inert and browsing never requests resources or dispatches', t => {
  const value = '<img src="https://example.invalid/pixel" onerror="alert(1)">';
  const initial = snapshot(); initial.catalog.entries[0].description = value;
  initial.catalog.entries[0].evidence = 'https://example.invalid/evidence';
  const ui = mount(t, initial);
  assert.ok(catalog(ui).textContent.includes(value));
  assert.equal(catalog(ui).querySelector('img,a,iframe,script'), null);
  query(ui, '<img'); selectCategory(ui, 'Communication');
  assert.deepEqual(names(ui), ['Slack']); assert.deepEqual(ui.network, []); assert.deepEqual(ui.calls, []);
});

test('manual route remains available without catalog and captures the mount callback', async t => {
  const initial = snapshot(); delete initial.catalog;
  const seen = [], ui = mount(t, initial, { onManualConfiguration: () => { seen.push('captured'); } });
  ui.options.onManualConfiguration = () => { seen.push('replacement'); };
  assert.equal(catalog(ui), undefined);
  assert.ok(ui.host.textContent.includes('Manual configuration'));
  manual(ui).click(); await tick();
  assert.deepEqual(seen, ['captured']);
  assert.ok(ui.host.textContent.includes('Manual configuration navigation requested.'));
  assert.deepEqual(ui.calls, []);
});

test('missing manual route is honest and invalid callback refuses before DOM creation', t => {
  const ui = mount(t);
  assert.ok(catalog(ui).textContent.includes('Advanced manual configuration is not provided by this host.'));
  const empty = ui.doc.createElement('div');
  assert.throws(() => mountToolkitView(empty, { scope: 'x', dispatch: async () => {}, onManualConfiguration: 'https://example.invalid' }),
    { name: 'TypeError', message: 'Manual configuration navigation must be a function' });
  assert.equal(empty.childNodes.length, 0);
});

test('manual navigation is single-flight across snapshots, omission and repeated clicks', async t => {
  let resolve, calls = 0;
  const pending = new Promise(done => { resolve = done; });
  const ui = mount(t, snapshot(), { onManualConfiguration: () => { calls++; return pending; } });
  const original = manual(ui); original.focus(); original.click(); original.click();
  assert.equal(calls, 1); assert.equal(original.disabled, false); assert.equal(original.getAttribute('aria-disabled'), 'true');
  assert.equal(ui.doc.activeElement, original); assert.ok(ui.host.textContent.includes(pendingText));
  ui.view.update(snapshot(2)); manual(ui).click();
  assert.equal(ui.doc.activeElement, manual(ui)); assert.equal(calls, 1);
  const omitted = snapshot(3); delete omitted.catalog; ui.view.update(omitted); manual(ui).click();
  assert.equal(calls, 1); assert.equal(ui.doc.activeElement, manual(ui));
  resolve(); await tick();
  assert.equal(manual(ui).getAttribute('aria-disabled'), 'false');
  assert.ok(ui.host.textContent.includes('Manual configuration navigation requested.'));
  assert.deepEqual(ui.calls, []);
});

for (const failure of ['throw', 'reject']) {
  test(`manual ${failure} is bounded feedback with explicit retry and no domain-state claim`, async t => {
    let count = 0;
    const ui = mount(t, snapshot(), { onManualConfiguration() {
      count++;
      const error = new Error('synthetic-private-diagnostic');
      if (failure === 'throw') throw error;
      return Promise.reject(error);
    } });
    query(ui, 'archive'); manual(ui).click(); await tick();
    assert.ok(ui.host.textContent.includes(failedText)); assert.ok(!ui.host.textContent.includes('synthetic-private-diagnostic'));
    assert.equal(count, 1); assert.equal(search(ui).value, 'archive'); assert.deepEqual(names(ui), ['Archive [v2]']);
    manual(ui).click(); await tick(); assert.equal(count, 2); assert.deepEqual(ui.calls, []);
  });
}

test('synchronous navigation update and reentrant click reserve one invocation', async t => {
  let calls = 0, resolve, ui;
  const pending = new Promise(done => { resolve = done; });
  ui = mount(t, snapshot(), { onManualConfiguration() {
    calls++; ui.view.update(snapshot(2)); manual(ui).click(); return pending;
  } });
  manual(ui).click(); assert.equal(calls, 1);
  assert.equal(manual(ui).getAttribute('aria-disabled'), 'true');
  resolve(); await tick();
  assert.ok(ui.host.textContent.includes('Manual configuration navigation requested.'));
  assert.equal(ui.view.update(snapshot(2)), false); assert.deepEqual(ui.calls, []);
});

test('detached filter and manual listeners are inert; disposal does not cancel navigation', async t => {
  let calls = 0, settled = false, resolve;
  const pending = new Promise(done => { resolve = done; }).then(() => { settled = true; });
  const ui = mount(t, snapshot(), { onManualConfiguration() { calls++; return pending; } });
  const oldSearch = search(ui), oldManual = manual(ui);
  ui.view.update(snapshot(2));
  oldSearch.value = 'archive'; oldSearch.dispatchEvent(new ui.dom.window.Event('input'));
  oldManual.click(); assert.equal(calls, 0); assert.equal(search(ui).value, '');
  manual(ui).click(); const current = manual(ui); ui.view.dispose(); current.click();
  assert.equal(calls, 1); resolve(); await tick();
  assert.equal(settled, true); assert.equal(ui.host.childNodes.length, 0); assert.deepEqual(ui.calls, []);
});

test('synchronous navigation disposal leaves no recreated view or late error payload', async t => {
  let calls = 0, ui;
  ui = mount(t, snapshot(), { onManualConfiguration() {
    calls++; ui.view.dispose(); return Promise.reject(new Error('private-late-diagnostic'));
  } });
  manual(ui).click(); await tick();
  assert.equal(calls, 1); assert.equal(ui.host.childNodes.length, 0); assert.deepEqual(ui.calls, []);
});

test('manual completion updates current feedback only and leaves host status and external focus intact', async t => {
  let reject;
  const ui = mount(t, snapshot(), { onManualConfiguration: () => new Promise((_, fail) => { reject = fail; }) });
  manual(ui).click();
  const next = snapshot(2); next.announcement = 'A later owner observation'; ui.view.update(next);
  const outside = ui.doc.getElementById('outside'); outside.focus(); reject(new Error('private-late-diagnostic')); await tick();
  assert.equal(ui.doc.activeElement, outside);
  assert.equal(ui.host.querySelector('[role="status"]').textContent, 'A later owner observation');
  assert.ok(ui.host.textContent.includes(failedText)); assert.deepEqual(ui.calls, []);
});

test('public example performs only supplied host-panel navigation', async t => {
  const ui = mount(t); ui.view.dispose();
  const panel = ui.doc.createElement('section'), heading = ui.doc.createElement('h2');
  panel.hidden = true; heading.textContent = 'Host manual configuration'; panel.append(heading); ui.doc.body.append(panel);
  const initial = snapshot();
  const view = mountCatalogManagement({ container: ui.host, manualPanel: panel, manualHeading: heading, snapshot: initial,
    async dispatch(request) { ui.calls.push(request); } });
  t.after(() => view.dispose()); manual(ui).click(); await tick();
  assert.equal(panel.hidden, false); assert.equal(ui.doc.activeElement, heading);
  assert.deepEqual(ui.calls, []); assert.deepEqual(ui.network, []);
});

const invalidCatalogs = [
  ['null catalog', s => { s.catalog = null; }],
  ['duplicate entry ID', s => { s.catalog.entries.push(structuredClone(s.catalog.entries[0])); }],
  ['duplicate category ID', s => { s.catalog.categories.push({ id: '__all__', label: 'Duplicate ID' }); }],
  ['duplicate category label', s => { s.catalog.categories.push({ id: 'other', label: 'Documents' }); }],
  ['blank entry ID', s => { s.catalog.entries[0].id = ' '; }],
  ['blank category ID', s => { s.catalog.categories[0].id = ''; }],
  ['unknown category reference', s => { s.catalog.entries[0].categoryIds = ['missing']; }],
  ['repeated category reference', s => { s.catalog.entries[0].categoryIds = ['communication', 'communication']; }],
  ['empty roles', s => { s.catalog.entries[0].roles = []; }],
  ['unknown role', s => { s.catalog.entries[0].roles = ['connected']; }],
  ['repeated role', s => { s.catalog.entries[0].roles = ['service', 'service']; }],
  ['invalid description', s => { s.catalog.entries[0].description = false; }],
  ['blank support', s => { s.catalog.entries[0].support = '\t'; }],
  ['null evidence', s => { s.catalog.entries[0].evidence = null; }],
  ['blank prerequisite', s => { s.catalog.entries[0].prerequisites = ['']; }],
  ['sparse entries', s => { s.catalog.entries.length += 1; }],
  ['sparse categories', s => { s.catalog.categories.length += 1; }],
];
for (const [name, corrupt] of invalidCatalogs) {
  test(`invalid catalog is refused atomically: ${name}`, t => {
    const ui = mount(t); query(ui, 'archive'); search(ui).focus();
    const html = ui.host.innerHTML, active = ui.doc.activeElement, next = snapshot(2); corrupt(next);
    assert.throws(() => ui.view.update(next), { name: 'TypeError', message: 'Invalid or ambiguous Toolkit projection' });
    assert.equal(ui.host.innerHTML, html); assert.equal(ui.doc.activeElement, active);
    assert.deepEqual(names(ui), ['Archive [v2]']); assert.deepEqual(ui.calls, []);
    assert.equal(ui.view.update(snapshot(2)), true);
  });
}
