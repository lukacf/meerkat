import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deploymentSetups, readDeploymentSummary, readDeploymentSummaries } from './my-deployments.mjs';
import { loadTestModule } from './test-support/bundle.mjs';

const setup = { id: '11111111-1111-4111-8111-111111111111', name: 'Reading team' };
const local = { guide_id: setup.id, deployment_id: 'local-install', build_id: 'accepted-build',
  destination: '/home/fixture/reading', status: 'stopped' };
const send = value => async url => url.endsWith('/deployment') ? value : null;

test('overview rejects a foreign receipt and never displays its destination', async () => {
  const rows = await readDeploymentSummary(setup, send({ ...local, guide_id: 'other', destination: 'private-canary' }));
  assert.equal(rows.length, 1); assert.equal(rows[0].unknown, true);
  assert.doesNotMatch(JSON.stringify(rows), /private-canary/);
});
test('absence, failed reads and verified stopped receipt remain distinct', async () => {
  assert.deepEqual(await readDeploymentSummary(setup, send(null)), []);
  const failures = await readDeploymentSummary(setup, async () => { throw new Error('unavailable'); });
  assert.equal(failures.length, 3); assert.ok(failures.every(row => row.status === 'Status unknown'));
  const rows = await readDeploymentSummary(setup, send(local));
  assert.match(rows[0].status, /stopped/); assert.match(rows[0].removal, /no generic uninstall/);
  assert.equal((await readDeploymentSummary(setup, send({ ...local, status: 'mystery' })))[0].unknown, true);
});
test('setup enumeration rejects duplicates and malformed identities instead of hiding records', () => {
  assert.deepEqual(deploymentSetups({ setups: [setup] }), [setup]);
  for (const setups of [[setup, setup], [{ ...setup, id: '../someone-else' }], null])
    assert.throws(() => deploymentSetups({ setups }));
});
test('bounded enumeration stops queued work and suppresses late results after view retirement', async () => {
  const controller = new AbortController(), pending = [], published = [];
  let calls = 0;
  const work = readDeploymentSummaries(Array.from({ length: 12 }, (_, i) => ({ ...setup, id: String(i) })),
    () => { calls++; return new Promise(resolve => pending.push(resolve)); }, controller.signal,
    (...args) => published.push(args));
  assert.equal(calls, 9); controller.abort(); pending.forEach(resolve => resolve(null));
  await work; assert.equal(calls, 9); assert.deepEqual(published, []);
});

const require = createRequire(import.meta.url), React = require('react'), { act } = React;
const { JSDOM } = createRequire(resolve(fileURLToPath(new URL('.', import.meta.url)), '../../console/package.json'))('jsdom');
const { MyDeployments } = await loadTestModule('my-deployments.jsx', { loader: { '.css': 'empty' } });
async function mounted(read, verify) {
  const dom = new JSDOM('<button id="opener">My deployments</button><div id="root"></div>', { url: 'http://offline.test/' });
  dom.window.HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  dom.window.HTMLDialogElement.prototype.close = function () { this.open = false; };
  const calls = [], opened = [];
  const values = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (url, options = {}) => {
      calls.push({ url, method: options.method || 'GET' });
      const value = await read(url, options);
      return { ok: true, status: 200, json: async () => value };
    } };
  const previous = new Map(Object.keys(values).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(values)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = require('react-dom/client'), root = createRoot(document.getElementById('root'));
  const settle = () => new Promise(resolve => setTimeout(resolve, 10));
  const button = text => [...document.querySelectorAll('button')].find(node => node.textContent === text);
  const click = async text => { assert.ok(button(text), text); await act(async () => { button(text).click(); await settle(); }); };
  function Harness() {
    const [open, setOpen] = React.useState(true);
    return open ? React.createElement(MyDeployments, { Icon: () => null, onClose: () => setOpen(false),
      onOpenSetup: id => opened.push(id) }) : null;
  }
  try {
    document.getElementById('opener').focus();
    await act(async () => { root.render(React.createElement(Harness)); await settle(); });
    await verify({ document, calls, opened, click, button, settle });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test('mounted overview requests all setups, exposes unknown status, and performs only reads', async () => {
  await mounted(async url => {
    if (url.startsWith('/api/guide/list')) return { setups: [setup] };
    if (url.endsWith('/remote-deployment')) throw new Error('offline');
    return url.endsWith('/deployment') ? local : null;
  }, async ({ document, calls, click }) => {
    assert.match(document.body.textContent, /Reading team/);
    assert.match(document.body.textContent, /Status unknown/);
    assert.doesNotMatch(document.body.textContent, /No recorded deployments/);
    assert.ok(calls.some(call => call.url === '/api/guide/list?include_all=true'));
    assert.ok(calls.every(call => call.method === 'GET'));
    await click('Close'); assert.equal(document.querySelector('dialog'), null);
    assert.equal(document.activeElement.id, 'opener');
  });
});
test('closing during setup read discards the response and does not mount action controls', async () => {
  let release;
  await mounted(async url => {
    if (url.startsWith('/api/guide/list')) return { setups: [setup] };
    if (url === `/api/guide/${setup.id}`) return new Promise(resolve => { release = resolve; });
    return url.endsWith('/deployment') ? local : null;
  }, async ({ document, calls, click, settle }) => {
    await click('Open and manage'); await click('Close');
    await act(async () => { release({ id: setup.id, version: 1 }); await settle(); });
    assert.equal(document.querySelector('dialog'), null); assert.ok(calls.every(call => call.method === 'GET'));
  });
});
test('management detail reuses lifecycle controls without offering a fresh installation', async () => {
  await mounted(async url => {
    if (url.startsWith('/api/guide/list')) return { setups: [setup] };
    if (url === `/api/guide/${setup.id}`) return { id: setup.id, version: 1, status: 'ready', blueprint: {
      project: { name: setup.name }, target: 'linux', agent_classes: [], topology: { mode: 'hub' } } };
    return url.endsWith('/deployment') ? local : null;
  }, async ({ document, calls, click, button, settle }) => {
    await click('Open and manage'); await act(settle);
    assert.ok(document.querySelector('[aria-label="Installation and operation"]'));
    assert.ok(button('Start installed application'));
    assert.doesNotMatch(document.body.textContent, /What to install|Install into another folder|Install on this computer/);
    assert.ok(calls.every(call => call.method === 'GET'));
    await click('All my deployments'); assert.match(document.body.textContent, /Reading team/);
  });
});

test('management detail retains Stop and exact cloud removal confirmation', async () => {
  const operation = '22222222-2222-4222-8222-222222222222';
  const cloud = { protocol: 1, guide_id: setup.id, operation_id: operation, account: 'fixture@example.test',
    status: 'blocked', phase: 'checking', detail: 'Fixture cloud receipt', vm_verified: false,
    application_installed: false, application_status: 'not-installed', resume_required: false,
    binding: { guide_id: setup.id, guide_version: 1, build_id: 'accepted-build',
      source_digest: 'a'.repeat(64), blueprint_hash: 'b'.repeat(64), package_sha256: 'c'.repeat(64) },
    destination: { project: 'fixture-project', zone: 'europe-west1-b', instance: 'fixture-reading',
      machine_type: 'e2-medium', disk_gb: 30, console_port: 18088 } };
  await mounted(async url => {
    if (url.startsWith('/api/guide/list')) return { setups: [setup] };
    if (url === `/api/guide/${setup.id}`) return { id: setup.id, version: 1, status: 'ready',
      blueprint: { project: { name: setup.name }, target: 'gcp', agent_classes: [], topology: { mode: 'hub' } } };
    if (url.includes('/gcp-deployment')) return cloud;
    return url.endsWith('/deployment') ? { ...local, status: 'running' } : null;
  }, async ({ document, calls, click, button, settle }) => {
    await click('Open and manage'); await act(settle);
    assert.ok(button('Stop application'));
    await click('Remove cloud computer...');
    const confirmation = button('Remove VM, disk and access rule');
    assert.ok(confirmation); assert.equal(confirmation.disabled, true);
    const removal = document.querySelector('[aria-label="Remove original cloud resources"]');
    assert.match(removal.textContent, /fixture-reading/);
    assert.match(removal.textContent, /permanently deletes/);
    assert.equal(removal.querySelectorAll('[data-removal-resource]').length, 3);
    await act(async () => { removal.querySelector('input[type="checkbox"]').click(); await settle(); });
    assert.equal(confirmation.disabled, false);
    await click('Keep cloud resources');
    assert.ok(calls.every(call => call.method === 'GET'));
  });
});
