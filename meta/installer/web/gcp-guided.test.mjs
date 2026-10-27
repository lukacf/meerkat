import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build as bundle } from 'esbuild';
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';
import { gcpEndpointValid, gcpCloudOwned, gcpCloudCurrent, gcpCloudMatchesIntent, gcpCanCreate, gcpMakeIntent,
  gcpStoreIntent, gcpLoadIntent, gcpIntentKey, gcpIntentMatchesPlan, gcpResourceManagementLinks,
  gcpMakeRemovalIntent, gcpLoadRemovalIntent, gcpStoreRemovalIntent, gcpRemovalIntentKey } from './gcp-guided.mjs';
import { remoteCanConnect, remoteReceiptOwned, remoteReceiptBinding, remotePendingBuildHandoff } from './remote-deployment.mjs';

const GUIDE = '11111111-1111-4111-8111-111111111111';
const OTHER = '22222222-2222-4222-8222-222222222222';
const REQUEST = '33333333-3333-4333-8333-333333333333';
const OTHER_REQUEST = '44444444-4444-4444-8444-444444444444';
const state = { id: GUIDE, version: 4, proposal: { target: 'gcp', project: { name: 'My agent system', slug: 'fixture-app' },
  gcp: { project: 'fixture-project', zone: 'europe-west1-b', machine_type: 'e2-medium', disk_gb: 30 } } };
const accepted = { guide_id: GUIDE, guide_version: 4, current_guide_version: 4, build_id: 'fixture-build', status: 'completed',
  current: true, ready_to_package: true, acceptance_current: true, acceptance_trusted: true, acceptance: { status: 'pass' },
  source_digest: 'a'.repeat(64), blueprint_hash: 'b'.repeat(64), package_sha256: 'c'.repeat(64) };
const binding = { guide_id: GUIDE, guide_version: 4, build_id: accepted.build_id, source_digest: accepted.source_digest,
  blueprint_hash: accepted.blueprint_hash, package_sha256: accepted.package_sha256 };
const account = 'first@example.test';
const destination = { project: 'fixture-project', zone: 'europe-west1-b', instance: 'fixture-app',
  machine_type: 'e2-medium', console_port: 18088, disk_gb: 30 };
const check = { id: 'fixture', label: 'Synthetic cloud access', status: 'checked', required: true, detail: 'Offline public observation.' };
function preflight() { return { protocol: 1, binding: structuredClone(binding), destination: structuredClone(destination),
  checked_at: new Date().toISOString(), checks: [structuredClone(check)] }; }
function plan() { return { ...preflight(), account, project: destination.project, ready_for_plan: true, guided_supported: true,
  provisioning_verified: false, plan: { title: 'Prepare my cloud computer', account, destination: structuredClone(destination),
    resources: ['One VM and disk'], access: ['IAP SSH'], mutations: ['Create these resources'], commands: [],
    limitations: ['Application startup remains unverified.'] } }; }
function endpoint(requestId = REQUEST) { return { transport: 'gcp-iap', operation_id: requestId, account,
  project: destination.project, zone: destination.zone, instance: destination.instance, instance_id: '1234567890123456789',
  ownership_token: requestId.replaceAll('-', ''), host: 'compute.1234567890123456789', user: 'mobkit', ssh_port: 22 }; }
function cloud(status = 'vm-ready', requestId = REQUEST) { return { protocol: 1, guide_id: GUIDE, operation_id: requestId, account, status,
  phase: status === 'vm-ready' ? 'vm-ready' : 'checking', detail: 'Owned cloud preparation observed.', binding: structuredClone(binding), destination: structuredClone(destination),
  vm_verified: status === 'vm-ready', application_installed: false, application_status: 'not-installed',
  resume_required: status === 'resume-required', ...(status === 'vm-ready' ? { endpoint: endpoint(requestId) } : {}) }; }
function remote() { return { protocol: 1, guide_id: GUIDE, guide_version: 4, build_id: accepted.build_id,
  deployment_id: '55555555-5555-4555-8555-555555555555', target: 'gcp', cloud_operation_id: REQUEST,
  accepted_source_digest: accepted.source_digest, package_sha256: accepted.package_sha256, transferred_source_digest: 'd'.repeat(64),
  endpoint: endpoint(), destination: '/home/mobkit/src/fixture-app', status: 'configuration-required', source_binding_current: true,
  observation: { available: true }, remote_admission_requested: false, can_install: true, preflight: { ready: true },
  fields: [{ key: 'OPENAI_API_KEY', label: 'Provider key', required: true, secret: true }] }; }
function storage() {
  const values = new Map();
  return { values, fail: false, getItem: key => values.get(key) ?? null,
    setItem(key, value) { if (this.fail) throw new Error('Offline storage write rejected.'); values.set(key, String(value)); } };
}

function removalCloud(status = 'removing', requestId = OTHER_REQUEST, changes = {}) {
  const original = cloud();
  return { ...original, status, phase: 'removing', vm_verified: false,
    resources: { vm: { name: destination.instance, id: endpoint().instance_id },
      disk: { name: destination.instance, id: '1234567890123456790' },
      firewall: { name: destination.instance + '-iap-ssh', id: '1234567890123456791' } },
    removal: { request_id: requestId, acknowledge_data_loss: true,
      resources: Object.fromEntries(['vm', 'disk', 'firewall'].map((kind) => [kind, { status: status === 'removed' ? 'absent' : 'waiting', ...changes[kind] }])) } };
}

test('removal receipts preserve ownership while withdrawing current-source admission and rejecting false absence', () => {
  for (const status of ['removing', 'removal-required', 'removed']) {
    const receipt = removalCloud(status);
    assert.equal(gcpCloudOwned(receipt, GUIDE, REQUEST), true);
    assert.equal(gcpCloudCurrent(receipt, state, accepted), false);
    assert.equal(gcpCloudOwned({ ...receipt, guide_id: OTHER }, GUIDE), false);
  }
  for (const changed of [{ request_id: REQUEST.replace('-4333-', '-3333-') },
    { acknowledge_data_loss: false }, { resources: { vm: { status: 'absent' } } },
    { resources: { vm: { status: 'removed' }, disk: { status: 'absent' }, firewall: { status: 'absent' } } }])
    assert.equal(gcpCloudOwned({ ...removalCloud(), removal: { ...removalCloud().removal, ...changed } }, GUIDE), false);
  assert.equal(gcpCloudOwned(removalCloud('removed', OTHER_REQUEST, { disk: { status: 'checking' } }), GUIDE), false);
  assert.equal(gcpCloudOwned({ ...removalCloud(), endpoint: { ...endpoint(), host: 'foreign.example' } }, GUIDE), false);
});

test('durable removal identity is scoped to the original generation without private values or current-source dependence', () => {
  const store = storage(), intent = gcpMakeRemovalIntent(cloud(), OTHER_REQUEST);
  gcpStoreRemovalIntent(store, intent);
  assert.deepEqual(gcpLoadRemovalIntent(store, GUIDE), { protocol: 1, guide_id: GUIDE, operation_id: REQUEST,
    request_id: OTHER_REQUEST, acknowledge_data_loss: true });
  assert.equal(gcpLoadRemovalIntent(store, OTHER), null);
  assert.throws(() => gcpStoreRemovalIntent(store, { ...intent, private_setting: 'must-not-save' }), /does not identify/);
  store.values.set(gcpRemovalIntentKey(GUIDE), 'broken');
  assert.throws(() => gcpLoadRemovalIntent(store, GUIDE), /unreadable/);
});

test('cloud ownership and current source remain separate from application readiness', () => {
  assert.equal(gcpCloudOwned(cloud(), GUIDE), true);
  assert.equal(gcpCloudCurrent(cloud(), state, accepted), true);
  assert.equal(gcpCloudCurrent(cloud(), state, { ...accepted, source_digest: 'f'.repeat(64) }), false);
  assert.equal(gcpCloudOwned({ ...cloud(), application_installed: true }, GUIDE), false);
  assert.equal(gcpCloudOwned({ ...cloud(), application_status: 'running' }, GUIDE), false);
  for (const changed of [{ guide_id: OTHER }, { operation_id: OTHER_REQUEST }, { vm_verified: false },
    { endpoint: { ...endpoint(), host: 'other.example' } }, { endpoint: { ...endpoint(), account: 'other@example.test' }, account }])
    assert.equal(gcpCloudOwned({ ...cloud(), ...changed }, GUIDE, REQUEST), false);
});

test('exact endpoint fields reject foreign route options, unsafe IDs and private paths', () => {
  assert.equal(gcpEndpointValid(endpoint()), true);
  for (const changed of [{ ProxyCommand: 'unexpected' }, { identity_file: '/private/key' }, { instance_id: '0123' },
    { instance_id: '18446744073709551616' }, { ssh_port: '22' }, { user: 'root' },
    { operation_id: 'AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA' }])
    assert.equal(gcpEndpointValid({ ...endpoint(), ...changed }), false);
});

test('saved request retains only public plan identity and exact accepted account/package binding', () => {
  const store = storage(), intent = gcpMakeIntent(plan(), state, accepted, REQUEST);
  assert.deepEqual(Object.keys(intent).sort(), ['account', 'binding', 'build_id', 'guide_id', 'project', 'protocol', 'request_id', 'version']);
  gcpStoreIntent(store, intent);
  assert.deepEqual(gcpLoadIntent(store, GUIDE), intent);
  assert.equal(gcpIntentMatchesPlan(intent, plan(), state, accepted), true);
  assert.equal(gcpIntentMatchesPlan(intent, { ...plan(), account: 'other@example.test' }, state, accepted), false);
  assert.equal(gcpIntentMatchesPlan(intent, { ...plan(), binding: { ...binding, package_sha256: 'e'.repeat(64) } }, state, accepted), false);
  assert.equal(gcpLoadIntent(store, OTHER), null);
  store.values.set(gcpIntentKey(GUIDE), 'broken');
  assert.throws(() => gcpLoadIntent(store, GUIDE), /unreadable/);
});

test('a saved intent acknowledges only the exact operation, account, project and full source binding', () => {
  const intent = gcpMakeIntent(plan(), state, accepted, REQUEST);
  assert.equal(gcpCloudMatchesIntent(cloud(), intent), true);
  assert.equal(gcpCloudMatchesIntent(cloud('provisioning'), intent), true);
  for (const changed of [
    { operation_id: OTHER_REQUEST, endpoint: endpoint(OTHER_REQUEST) },
    { account: 'other@example.test', endpoint: { ...endpoint(), account: 'other@example.test' } },
    { destination: { ...destination, project: 'other-project' }, endpoint: { ...endpoint(), project: 'other-project' } },
    ...Object.keys(binding).map(key => ({ binding: { ...binding, [key]: key === 'guide_version' ? 5 : key === 'guide_id' ? OTHER : key === 'build_id' ? 'other-build' : 'e'.repeat(64) } })),
  ]) assert.equal(gcpCloudMatchesIntent({ ...cloud(), ...changed }, intent), false);
});

test('cloud remote receipt retains complete endpoint scope and never uses Linux draft handoff', () => {
  const options = { current: true, build_id: accepted.build_id, guide_version: 4, current_guide_version: 4,
    target: 'gcp', host_target: 'mac', supported: false };
  assert.equal(remoteCanConnect(options, accepted, state), true);
  assert.equal(remoteReceiptOwned(remote(), state), true);
  const key = remoteReceiptBinding(remote());
  for (const field of ['account', 'project', 'zone', 'instance', 'instance_id', 'ownership_token', 'operation_id'])
    assert.notEqual(remoteReceiptBinding({ ...remote(), endpoint: { ...endpoint(), [field]: 'different' } }), key, field);
  assert.equal(remoteReceiptOwned({ ...remote(), endpoint: { ...endpoint(), operation_id: OTHER_REQUEST } }, state), false);
  assert.equal(remotePendingBuildHandoff({ ...remote(), source_binding_current: false }, options, accepted, state), null);
  assert.equal(gcpCanCreate({ ...plan(), guided_supported: false }, state, accepted), false);
});

let compiled;
async function components() {
  if (compiled) return compiled;
  const require = createRequire(import.meta.url);
  const result = await bundle({ stdin: { contents: 'export { GcpGuided } from "./gcp-guided.jsx"; export { RemoteDeployment } from "./remote-deployment.jsx"; export { DeploymentPanel } from "./deployment-panel.jsx";',
    resolveDir: fileURLToPath(new URL('.', import.meta.url)), sourcefile: 'offline-gcp-guided-entry.mjs' }, bundle: true,
    write: false, format: 'esm', platform: 'node', logLevel: 'silent', loader: { '.css': 'empty' },
    nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))], plugins: [{ name: 'same-mounted-react', setup(build) {
      build.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    } }] });
  compiled = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
  return compiled;
}

async function mounted(verify, settings = {}) {
  const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const store = settings.store || storage(), calls = [], timers = new Map(), Components = await components();
  let selectedState = state, selectedBuild = accepted, blocked = false, savedCloud = settings.cloud ?? null;
  let savedRemote = Object.hasOwn(settings, 'remoteReceipt') ? settings.remoteReceipt : settings.panel ? null : remote();
  let remoteStatusFailure = null;
  let handler = async (action, body) => action === 'create' ? (savedCloud = cloud('provisioning', body.request_id)) :
    action === 'resume' ? (savedCloud = cloud('provisioning', body.operation_id)) : remote();
  let statusHandler = () => savedCloud, randomCalls = 0, nextTimer = 0;
  const nativeTimeout = globalThis.setTimeout, nativeClear = globalThis.clearTimeout;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: store, IS_REACT_ACT_ENVIRONMENT: true,
    crypto: { randomUUID: () => { randomCalls += 1; return settings.randomId || REQUEST; } },
    setTimeout(callback, delay, ...args) {
      if (delay !== 2000 && delay !== 1500 && delay < 100000) return nativeTimeout(callback, delay, ...args);
      const token = { timer: ++nextTimer }; timers.set(token, () => callback(...args)); return token;
    }, clearTimeout(token) { if (!timers.delete(token)) nativeClear(token); },
    fetch: async (target, init = {}) => {
      const method = init.method || 'GET', body = init.body ? JSON.parse(init.body) : null;
      calls.push({ target, method, body });
      const observed = new URL(target, 'http://localhost'), action = observed.pathname.split('/').at(-1);
      if (observed.pathname.endsWith('/gcp-deployment')) return { ok: true, json: async () => structuredClone(await statusHandler(observed)) };
      if (observed.pathname.includes('/gcp-deployment/')) {
        const value = await handler(action, body, observed); return { ok: true, json: async () => structuredClone(value) };
      }
      if (observed.pathname.endsWith('/deployment')) return { ok: false, status: 404, json: async () => ({ detail: 'No owned local installation.' }) };
      if (observed.pathname.endsWith('/deployment/options')) return { ok: true, json: async () => ({
        build_id: accepted.build_id, guide_version: 4, current_guide_version: 4, current: true, target: 'gcp', host_target: 'mac', supported: false,
        handoff: { mode: 'remote-package', status: 'not-connected', package_url: `/api/guide/${GUIDE}/implementation/package?build_id=${accepted.build_id}`,
          steps: [{ title: 'Manual deployment', description: 'Offline optional instructions.', where: 'This computer', commands: ['offline-manual-command'] }],
          limitations: ['Live startup is unverified.'] } }) };
      if (observed.pathname.endsWith('/remote-deployment')) {
        if (remoteStatusFailure) throw remoteStatusFailure;
        return savedRemote === null ? { ok: false, status: 404, json: async () => ({ detail: 'No owned remote installation.' }) }
          : { ok: true, json: async () => structuredClone(savedRemote) };
      }
      if (observed.pathname.endsWith('/remote-deployment/action') && settings.remoteReceipt) {
        assert.equal(body.deployment_id, savedRemote.deployment_id);
        assert.equal(body.action, 'stop');
        savedRemote = { ...savedRemote, status: 'stopped', can_stop: false };
        return { ok: true, json: async () => structuredClone(savedRemote) };
      }
      if (observed.pathname.endsWith('/options')) return { ok: true, json: async () => ({ ...preflight(), cli: { available: true, status: 'checked', detail: 'Offline CLI inventory.' },
        accounts: [{ id: account, label: 'First account', active: true }], truncated: false }) };
      if (action === 'projects') return { ok: true, json: async () => ({ ...preflight(), account: body.account,
        projects: [{ id: destination.project, label: 'My project' }], truncated: false }) };
      assert.equal(action, 'check-guided', `Unexpected offline request: ${method} ${target}`);
      return { ok: true, json: async () => plan() };
    } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  let root = createRoot(dom.window.document.getElementById('root'));
  async function render() {
    const guided = React.createElement(Components.GcpGuided, { state: selectedState, build: selectedBuild, blocked });
    const remoteView = React.createElement(Components.RemoteDeployment, {
      state: selectedState, build: selectedBuild, plan: selectedState.proposal, options: { current: true, target: 'gcp', host_target: 'mac', supported: false,
        build_id: accepted.build_id, guide_version: 4, current_guide_version: 4 }, renderConfiguration: () => React.createElement('div', null, 'Private fields') });
    const view = settings.panel ? React.createElement(Components.DeploymentPanel, { state: selectedState, build: selectedBuild,
      plan: selectedState.proposal, designBusy: false, pendingManual: false, Icon: () => React.createElement('span') }) :
      settings.combined ? React.createElement(React.Fragment, null, guided, remoteView) : settings.remote ? remoteView : guided;
    await act(async () => root.render(view));
  }
  const document = dom.window.document;
  const button = text => [...document.querySelectorAll('button')].find(item => item.textContent === text || item.querySelector('strong')?.textContent === text);
  async function click(text) { await act(async () => { assert.ok(button(text), `Missing action: ${text}`); button(text).click(); }); }
  async function choosePlan() {
    await click('Check cloud access'); await click('First account'); await click('My project'); await click('Check destination');
  }
  async function change(nextState, nextBuild = accepted) { selectedState = nextState; selectedBuild = nextBuild; await render(); }
  async function remount() { await act(async () => root.unmount()); root = createRoot(document.getElementById('root')); await render(); }
  try {
    await render();
    await verify({ calls, store, document, button, click, choosePlan, change, remount,
      setHandler(value) { handler = value; }, setStatus(value) { statusHandler = typeof value === 'function' ? value : () => value; },
      setRemoteStatusFailure(value) { remoteStatusFailure = value; },
      setRemoteReceipt(value) { savedRemote = value; },
      randomCalls: () => randomCalls, timers, pendingClick(text) { assert.ok(button(text)); act(() => button(text).click()); } });
  } finally {
    await act(async () => root.unmount()); assert.equal(timers.size, 0, 'All owned UI observation timers retire on unmount.'); dom.window.close();
    for (const [key, descriptor] of prior) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; }
  }
}

test('create persists exact public identity before one mutation and reports VM separately', async () => {
  await mounted(async ({ choosePlan, click, calls, store, document, setHandler }) => {
    setHandler(async (action, body) => {
      assert.equal(action, 'create'); assert.equal(gcpLoadIntent(store, GUIDE).request_id, body.request_id);
      return cloud('vm-ready', body.request_id);
    });
    await choosePlan(); await click('Create this cloud computer');
    const posts = calls.filter(item => item.target.endsWith('/create'));
    assert.deepEqual(posts, [{ target: `/api/guide/${GUIDE}/gcp-deployment/create`, method: 'POST',
      body: { version: 4, build_id: accepted.build_id, account, project: destination.project, request_id: REQUEST } }]);
    assert.match(document.body.textContent, /This card describes computer preparation/);
    assert.match(document.body.textContent, /Your cloud computer is prepared/);
    assert.ok(!calls.some(item => item.target.endsWith('/install')));
  });
});

test('observed preparation phases update concrete progress without admitting application actions', async () => {
  await mounted(async ({ document, setStatus, click, button, calls }) => {
    const progress = () => document.querySelector('.gcp-provisioning-step');
    assert.equal(progress().textContent.trim(), 'Current step: Checking your cloud destination');
    for (const [phase, label] of [
      ['creating-firewall', 'Creating and checking the SSH access rule'],
      ['creating-vm', 'Creating and checking your cloud computer'],
    ]) {
      setStatus({ ...cloud('provisioning'), phase });
      await click('Recheck original cloud setup');
      assert.equal(progress().dataset.preparationPhase, phase);
      assert.equal(progress().textContent.trim(), `Current step: ${label}`);
      assert.equal(button('Continue to application setup') === undefined, true);
    }
    setStatus(cloud()); await click('Recheck original cloud setup');
    assert.equal(progress().textContent.trim(), 'Completed step: Cloud computer preparation verified');
    assert.ok(button('Continue to application setup'));
    assert.match(document.body.textContent, /This card describes computer preparation/);
    assert.equal(calls.filter(call => call.method === 'POST').length, 0);
  }, { cloud: cloud('provisioning') });
});

test('paused or unavailable preparation shows its last step without claiming active progress', async () => {
  await mounted(async ({ document, setStatus, click, button, calls }) => {
    const progress = () => document.querySelector('.gcp-provisioning-step');
    assert.equal(progress().textContent.trim(), 'Last recorded step: Creating and checking the SSH access rule');
    assert.ok(button('Recheck and continue this setup'));
    setStatus({ ...cloud('resume-required'), phase: 'creating-vm' });
    await click('Recheck original cloud setup');
    assert.equal(progress().textContent.trim(), 'Last recorded step: Creating and checking your cloud computer');
    assert.equal(button('Continue to application setup') === undefined, true);
    setStatus(() => { throw new Error('Offline cloud observation is unavailable.'); });
    await click('Recheck original cloud setup');
    assert.equal(progress().textContent.trim(), 'Last observed step: Creating and checking your cloud computer');
    assert.equal(button('Recheck and continue this setup').disabled, true);
    assert.match(document.querySelector('[role="alert"]').textContent, /Offline cloud observation is unavailable/);
    setStatus({ ...cloud(), guide_id: OTHER });
    await click('Recheck original cloud setup');
    assert.equal(document.querySelector('.gcp-provisioning').dataset.cloudStatus, 'resume-required');
    assert.equal(button('Recheck and continue this setup').disabled, true);
    assert.match(document.querySelector('[role="alert"]').textContent, /does not identify this setup/);
    setStatus(cloud());
    await click('Recheck original cloud setup');
    assert.equal(progress().textContent.trim(), 'Completed step: Cloud computer preparation verified');
    assert.equal(button('Continue to application setup').disabled, false);
    assert.equal(document.querySelector('[role="alert"]') === null, true);
    assert.equal(document.querySelector('.gcp-resource-location').textContent, `${destination.project} / ${destination.zone} / ${destination.instance}`);
    for (const call of calls.filter(call => call.method === 'GET' && new URL(call.target, 'http://localhost').pathname.endsWith('/gcp-deployment')).slice(1))
      assert.equal(new URL(call.target, 'http://localhost').searchParams.get('operation_id'), REQUEST);
    assert.equal(calls.filter(call => call.method === 'POST').length, 0);
  }, { cloud: { ...cloud('blocked'), phase: 'creating-firewall' } });
});

for (const action of ['create', 'connect', 'resume', 'remove']) {
  test(`cloud observation recovery preserves the original ${action} failure without repeating it`, async () => {
    const original = action === 'create' ? null : cloud(action === 'resume' ? 'blocked' : 'vm-ready');
    await mounted(async ({ document, click, choosePlan, setHandler, setStatus, calls, store }) => {
      const actionError = `Offline ${action} reply unconfirmed.`;
      const observationError = 'Offline original cloud status unavailable.';
      setHandler((sentAction) => { assert.equal(sentAction, action); throw new Error(actionError); });
      if (action === 'create') {
        await choosePlan(); await click('Create this cloud computer');
      } else if (action === 'remove') {
        await click('Review cloud removal');
        await act(async () => document.querySelector('.gcp-removal input').click());
        await click('Remove VM, disk and access rule');
      } else await click(action === 'connect' ? 'Continue to application setup' : 'Recheck and continue this setup');
      const alerts = () => [...document.querySelectorAll('[role="alert"]')].map(element => element.textContent);
      assert.deepEqual(alerts(), [actionError]);
      const posts = calls.filter(call => call.method === 'POST' && call.target.includes('/gcp-deployment/'));
      assert.equal(posts.length, 1);
      assert.equal(posts[0].target, `/api/guide/${GUIDE}/gcp-deployment/${action}`);
      assert.equal(posts[0].body[action === 'create' ? 'request_id' : 'operation_id'], REQUEST);
      const recheck = action === 'remove' ? 'Recheck removal' : 'Recheck original cloud setup';
      setStatus(() => { throw new Error(observationError); });
      await click(recheck);
      assert.deepEqual(alerts(), [observationError, actionError]);
      setStatus(original);
      await click(recheck);
      assert.deepEqual(alerts(), [actionError]);
      assert.equal(document.querySelector('.deployment-warning') === null, true);
      assert.deepEqual(calls.filter(call => call.method === 'POST' && call.target.includes('/gcp-deployment/')), posts);
      for (const call of calls.filter(call => call.method === 'GET' && new URL(call.target, 'http://localhost').pathname.endsWith('/gcp-deployment')).slice(-2))
        assert.equal(new URL(call.target, 'http://localhost').searchParams.get('operation_id'), REQUEST);
      if (action === 'create') assert.equal(gcpLoadIntent(store, GUIDE).request_id, REQUEST);
      else {
        assert.equal(document.querySelector('.gcp-provisioning').dataset.cloudStatus, original.status);
        assert.equal(document.querySelector('.gcp-resource-location').textContent, `${destination.project} / ${destination.zone} / ${destination.instance}`);
      }
      if (action === 'remove') assert.equal(gcpLoadRemovalIntent(store, GUIDE).operation_id, REQUEST);
    }, { cloud: original });
  });
}

test('storage failure admits no cloud mutation', async () => {
  await mounted(async ({ choosePlan, click, store, calls, document }) => {
    await choosePlan(); store.fail = true; await click('Create this cloud computer');
    assert.equal(calls.filter(item => item.target.endsWith('/create')).length, 0);
    assert.match(document.body.textContent, /Offline storage write rejected/);
  });
});

test('lost create reply recovers exact request after remount without another create', async () => {
  const store = storage();
  await mounted(async ({ choosePlan, click, setHandler, setStatus, remount, calls, document }) => {
    setHandler(async () => { setStatus(cloud()); throw new Error('Offline reply was lost.'); });
    await choosePlan(); await click('Create this cloud computer'); await remount();
    assert.equal(calls.filter(item => item.target.endsWith('/create')).length, 1);
    assert.ok(calls.some(item => item.target.includes(`operation_id=${REQUEST}`)));
    assert.match(document.body.textContent, /Your cloud computer is prepared/);
    assert.equal(gcpLoadIntent(store, GUIDE).request_id, REQUEST);
  }, { store });
});

test('confirmed absent request can deliberately retry only the same exact saved body', async () => {
  await mounted(async ({ choosePlan, click, setHandler, setStatus, calls, randomCalls }) => {
    setHandler(async () => { throw new Error('Offline admission reply missing.'); });
    setStatus(null); await choosePlan(); await click('Create this cloud computer');
    await choosePlan(); await click('Retry the original cloud setup request');
    const posts = calls.filter(item => item.target.endsWith('/create'));
    assert.equal(posts.length, 2); assert.deepEqual(posts[1], posts[0]); assert.equal(randomCalls(), 1);
  });
});

test('saved earlier account/source request cannot be replaced by a fresh key', async () => {
  const store = storage(), intent = gcpMakeIntent(plan(), state, accepted, REQUEST);
  intent.account = 'earlier@example.test'; gcpStoreIntent(store, intent);
  await mounted(async ({ choosePlan, click, calls, document, randomCalls }) => {
    await choosePlan(); await click('Retry the original cloud setup request');
    assert.equal(calls.filter(item => item.target.endsWith('/create')).length, 0);
    assert.equal(randomCalls(), 0); assert.match(document.body.textContent, /earlier source or account/);
  }, { store });
});

test('a wrong-account create acknowledgement cannot enable private cloud setup', async () => {
  await mounted(async ({ choosePlan, click, setHandler, button, document }) => {
    setHandler(async () => ({ ...cloud(), endpoint: { ...endpoint(), account: 'other@example.test' } }));
    await choosePlan(); await click('Create this cloud computer');
    assert.ok(!button('Continue to application setup'), 'An acknowledgement for another account must not grant continuation.');
    assert.match(document.body.textContent, /does not match|uncertain|unavailable/);
  });
});

test('a mismatched create acknowledgement never becomes the saved request receipt', async () => {
  for (const changed of [
    { operation_id: OTHER_REQUEST, endpoint: endpoint(OTHER_REQUEST) },
    { account: 'other@example.test', endpoint: { ...endpoint(), account: 'other@example.test' } },
    ...Object.keys(binding).map(key => ({ binding: { ...binding, [key]: key === 'guide_version' ? 5 : key === 'guide_id' ? OTHER : key === 'build_id' ? 'other-build' : 'e'.repeat(64) } })),
  ]) await mounted(async ({ choosePlan, click, setHandler, button, document, store }) => {
    setHandler(async () => ({ ...cloud(), ...changed }));
    await choosePlan(); await click('Create this cloud computer');
    assert.ok(!button('Continue to application setup'));
    assert.ok(!document.querySelector('[data-cloud-status]'));
    assert.match(document.body.textContent, /does not match|does not identify|uncertain|unavailable/);
    assert.equal(gcpLoadIntent(store, GUIDE).request_id, REQUEST);
  });
});

test('reload rejects another account or source while keeping the exact original action', async () => {
  for (const changed of [
    { account: 'other@example.test', endpoint: { ...endpoint(), account: 'other@example.test' } },
    { binding: { ...binding, source_digest: 'e'.repeat(64) } },
    { operation_id: OTHER_REQUEST, endpoint: endpoint(OTHER_REQUEST) },
  ]) {
    const store = storage(); gcpStoreIntent(store, gcpMakeIntent(plan(), state, accepted, REQUEST));
    await mounted(async ({ button, document, calls, remount }) => {
      assert.ok(!button('Continue to application setup'));
      assert.ok(!document.querySelector('[data-cloud-status]'));
      assert.match(document.body.textContent, /does not identify|does not match|unavailable/);
      await remount();
      assert.ok(!button('Continue to application setup'));
      assert.equal(gcpLoadIntent(store, GUIDE).request_id, REQUEST);
      assert.equal(calls.filter(item => item.method === 'POST').length, 0);
      assert.ok(calls.filter(item => item.target.includes(`operation_id=${REQUEST}`)).length >= 2);
    }, { store, cloud: { ...cloud(), ...changed } });
  }
});

test('late creation reply cannot publish into another guide', async () => {
  await mounted(async ({ choosePlan, pendingClick, setHandler, change, setStatus, document, store, calls }) => {
    let resolve;
    setHandler(() => new Promise(done => { resolve = done; }));
    await choosePlan(); pendingClick('Create this cloud computer');
    assert.equal(typeof resolve, 'function');
    setStatus(null); await change({ ...state, id: OTHER }, { ...accepted, guide_id: OTHER });
    await act(async () => resolve(cloud()));
    assert.ok(!document.querySelector('[data-cloud-status]'));
    assert.equal(gcpLoadIntent(store, GUIDE).request_id, REQUEST); assert.equal(gcpLoadIntent(store, OTHER), null);
    assert.equal(calls.filter(item => item.target.endsWith('/create')).length, 1);
  });
});

test('an earlier absent status reply cannot clear a subsequently acknowledged cloud request', async () => {
  await mounted(async ({ choosePlan, pendingClick, click, setHandler, setStatus, document, button, calls }) => {
    let resolve;
    await choosePlan();
    setStatus(() => new Promise(done => { resolve = done; }));
    pendingClick('Recheck original cloud setup');
    for (let turn = 0; turn < 16 && typeof resolve !== 'function'; turn += 1)
      await act(async () => new Promise(setImmediate));
    assert.equal(typeof resolve, 'function', 'The earlier GET must actually reach its unresolved JSON observation.');
    setHandler(async () => cloud()); await click('Create this cloud computer');
    assert.ok(button('Continue to application setup'));
    await act(async () => resolve(null));
    assert.ok(button('Continue to application setup'));
    assert.equal(document.querySelector('[data-cloud-status]')?.dataset.cloudStatus, 'vm-ready');
    assert.equal(calls.filter(item => item.target.endsWith('/create')).length, 1);
  });
});

test('a delayed original acknowledgement remains historical after the accepted source changes', async () => {
  await mounted(async ({ choosePlan, pendingClick, setHandler, change, document, button, store, calls }) => {
    let resolve;
    setHandler(() => new Promise(done => { resolve = done; }));
    await choosePlan(); pendingClick('Create this cloud computer');
    await change(state, { ...accepted, source_digest: 'e'.repeat(64) });
    await act(async () => resolve(cloud()));
    assert.equal(document.querySelector('[data-cloud-status]')?.dataset.cloudStatus, 'vm-ready');
    assert.match(document.body.textContent, /earlier accepted setup/);
    assert.ok(!button('Continue to application setup'));
    assert.equal(gcpLoadIntent(store, GUIDE).binding.source_digest, accepted.source_digest);
    assert.equal(calls.filter(item => item.target.endsWith('/create')).length, 1);
  });
});

test('current VM can connect only an exact cloud endpoint and owned application receipt', async () => {
  await mounted(async ({ click, calls, setHandler, document }) => {
    setHandler(async action => { assert.equal(action, 'connect'); return { ...remote(), endpoint: { ...endpoint(), ownership_token: 'f'.repeat(32) } }; });
    await click('Continue to application setup');
    assert.deepEqual(calls.find(item => item.target.endsWith('/connect')).body,
      { operation_id: REQUEST, version: 4, build_id: accepted.build_id });
    assert.match(document.body.textContent, /does not match the verified cloud computer/);
    assert.equal(document.querySelector('.gcp-application-handoff') === null, true);
  }, { cloud: cloud() });
});

test('a successful cloud handoff reveals an exact private-setup link and focuses the actual remote section', async () => {
  await mounted(async ({ click, calls, document }) => {
    await click('Continue to application setup');
    const next = document.querySelector('.gcp-application-handoff a'); assert.ok(next);
    assert.equal(next.textContent, 'Continue to private application setup');
    assert.equal(next.getAttribute('href'), `#remote-cloud-${GUIDE}`);
    const destinationView = document.getElementById(`remote-cloud-${GUIDE}`); assert.ok(destinationView);
    assert.equal(destinationView.tabIndex, -1);
    assert.match(next.parentElement.textContent, /Application access checks have started/);
    await act(async () => next.click()); assert.equal(document.activeElement === destinationView, true);
    assert.equal(calls.filter(item => item.target.endsWith('/connect')).length, 1);
    assert.equal(calls.filter(item => item.target.endsWith('/install')).length, 0);
    assert.equal(document.querySelector('.remote-private-form') === null, true);
  }, { cloud: cloud(), combined: true });
});

test('cloud handoff acknowledgements require the exact accepted source and current private binding', async () => {
  for (const changed of [{ guide_version: 5 }, { build_id: 'other-build' },
    { accepted_source_digest: 'e'.repeat(64) }, { package_sha256: 'e'.repeat(64) }, { source_binding_current: false }])
    await mounted(async ({ click, setHandler, document, calls }) => {
      setHandler(async action => { assert.equal(action, 'connect'); return { ...remote(), ...changed }; });
      await click('Continue to application setup');
      assert.equal(document.querySelector('.gcp-application-handoff') === null, true);
      assert.match(document.body.textContent, /does not match the verified cloud computer/);
      assert.equal(calls.filter(item => item.target.endsWith('/connect')).length, 1);
      assert.equal(calls.filter(item => item.target.endsWith('/install')).length, 0);
    }, { cloud: cloud() });
});

test('the actual parent keeps cloud creation primary and package instructions only in collapsed advanced mode', async () => {
  await mounted(async ({ choosePlan, document, button, calls }) => {
    await choosePlan();
    const manual = document.querySelector('.remote-manual-deployment'); assert.ok(manual);
    assert.equal(manual.open, false); assert.match(manual.querySelector('summary').textContent, /Manual Google Cloud.*advanced/);
    const downloads = [...document.querySelectorAll('button')].filter(node => node.textContent.includes('Download accepted GCP package'));
    assert.equal(downloads.length, 1); assert.equal(downloads[0].closest('details') === manual, true);
    assert.ok(button('Create this cloud computer')); assert.equal(button('Create this cloud computer').closest('details') === null, true);
    assert.ok([...document.querySelectorAll('.gcp-guided button')].every(node => !node.textContent.includes('Download accepted GCP package')));
    assert.equal(calls.filter(item => item.method === 'POST' && item.target.includes('/gcp-deployment/')).length, 0);
  }, { panel: true });
});

test('a stale VM stays visible while current install and resume actions are withheld', async () => {
  await mounted(async ({ change, document, button, calls }) => {
    await change(state, { ...accepted, source_digest: 'e'.repeat(64) });
    assert.match(document.body.textContent, /earlier accepted setup/);
    assert.ok(!button('Continue to application setup'));
    assert.ok(!calls.some(item => item.method === 'POST'));
  }, { cloud: cloud() });
});

test('cloud remote installation reuses private configuration without exposing a manual SSH form', async () => {
  await mounted(async ({ document, click }) => {
    assert.ok(document.querySelector('[aria-label="Guided cloud application installation"]'));
    assert.ok(!document.querySelector('[name="host"]'));
    await click('Continue to private setup');
    assert.match(document.body.textContent, /Private settings for this cloud installation/);
    assert.match(document.body.textContent, /Private fields/);
  }, { remote: true });
});

test('cloud management links use fixed project pages and reject malformed destination values', () => {
  const links = gcpResourceManagementLinks({ ...destination, self_link: 'https://untrusted.example/remove' });
  for (const [kind, pathname] of [['vm', '/compute/instances'], ['disk', '/compute/disks']]) {
    const url = new URL(links[kind]);
    assert.equal(url.origin, 'https://console.cloud.google.com');
    assert.equal(url.pathname, pathname);
    assert.deepEqual([...url.searchParams], [['project', destination.project]]);
    assert.equal(url.hash, '');
  }
  for (const invalid of [null, { ...destination, project: 'foreign-project&action=delete' },
    { ...destination, project: [destination.project] }, { ...destination, zone: '../zone' },
    { ...destination, instance: 'https://untrusted.example' }, { ...destination, instance: null }])
    assert.equal(gcpResourceManagementLinks(invalid), null);
});

test('cloud resource guidance retains its saved destination during stale or unavailable preparation', async () => {
  await mounted(async ({ document, change, setStatus, click, calls }) => {
    const links = () => [...document.querySelectorAll('.gcp-resource-guidance a')];
    const originalLinks = links().map((link) => link.href);
    assert.equal(originalLinks.length, 2);
    await change({ ...state, version: 5, proposal: { ...state.proposal,
      gcp: { ...state.proposal.gcp, project: 'new-proposal-project' } } });
    assert.deepEqual(links().map((link) => link.href), originalLinks);
    assert.match(document.querySelector('.gcp-resource-guidance').textContent, /fixture-project/);
    setStatus(() => { throw new Error('Offline observation unavailable.'); });
    await click('Recheck original cloud setup');
    assert.deepEqual(links().map((link) => link.href), originalLinks);
    assert.match(document.body.textContent, /does not prove creation failed or resources are absent/);
    assert.ok(links().every((link) => link.target === '_blank' && link.rel.includes('noopener')));
    assert.equal(calls.filter((call) => call.method === 'POST').length, 0);
  }, { cloud: cloud() });
});

function runningRemote() {
  return { ...remote(), status: 'running', can_stop: true, can_restart: false, admitted: false,
    source_current: false, detail: 'Offline observed application service is running.' };
}

test('preparation remains stage scoped and billing guidance survives an owned application stop', async () => {
  await mounted(async ({ document, click, calls }) => {
    const preparation = document.querySelector('.gcp-provisioning');
    assert.doesNotMatch(preparation.textContent, /application has not been installed/);
    assert.match(preparation.textContent, /Application setup and current application status are shown below/);
    const application = document.querySelector('.remote-deployment');
    assert.equal(application.dataset.remoteStatus, 'running');
    assert.equal(application.querySelectorAll('.gcp-resource-guidance a').length, 2);
    await click('Stop owned application');
    assert.equal(application.dataset.remoteStatus, 'stopped');
    assert.match(application.querySelector('.gcp-resource-guidance').textContent, /Cloud charges can continue/);
    assert.match(application.querySelector('.gcp-resource-guidance').textContent, /Back up records.*Guided removal is unavailable here/);
    assert.deepEqual(calls.filter((call) => call.method === 'POST'), [{
      target: `/api/guide/${GUIDE}/remote-deployment/action`, method: 'POST',
      body: { deployment_id: runningRemote().deployment_id, action: 'stop' },
    }]);
  }, { combined: true, cloud: { ...cloud(), detail: 'The owned cloud VM and access rule were observed. The application has not been installed or tested.' },
    remoteReceipt: runningRemote() });
});

test('the actual parent retains saved cloud observation and stop when the proposal changes to Mac', async () => {
  await mounted(async ({ document, change, button, click, calls }) => {
    assert.ok(document.querySelector('.deployment-panel'));
    const savedLink = document.querySelector('.gcp-provisioning .gcp-resource-guidance a').href;
    const staleState = { ...state, version: 5, proposal: { ...state.proposal, target: 'mac',
      project: { name: 'My Mac alternative', slug: 'mac-alternative' } } };
    await change(staleState);
    assert.ok(document.querySelector('.gcp-provisioning'));
    assert.equal(document.querySelector('.gcp-provisioning .gcp-resource-guidance a').href, savedLink);
    assert.equal(document.querySelector('.remote-deployment').dataset.remoteStatus, 'running');
    assert.ok(button('Stop owned application'));
    for (const label of ['Create this cloud computer', 'Continue to application setup', 'Recheck and continue this setup',
      'Connect and check destination', 'Continue to private setup', 'Restart accepted application'])
      assert.equal(button(label) === undefined, true, label);
    assert.equal(document.querySelector('.remote-private-form') === null, true);
    assert.equal(document.querySelector('.remote-destination-form') === null, true);
    await click('Stop owned application');
    assert.equal(document.querySelector('.remote-deployment').dataset.remoteStatus, 'stopped');
    assert.deepEqual(calls.filter((call) => call.method === 'POST'), [{
      target: `/api/guide/${GUIDE}/remote-deployment/action`, method: 'POST',
      body: { deployment_id: runningRemote().deployment_id, action: 'stop' },
    }]);
  }, { panel: true, cloud: cloud(), remoteReceipt: runningRemote() });
});

test('the actual parent retains saved cloud stop while Agent team is selected', async () => {
  await mounted(async ({ document, button, click, calls }) => {
    const radio = document.querySelector('input[name="deployment-source"][value="native-team"]');
    assert.ok(radio);
    await act(async () => radio.click());
    assert.equal(radio.checked, true);
    assert.ok(document.querySelector('.gcp-provisioning'));
    assert.ok(document.querySelector('.remote-deployment .gcp-resource-guidance'));
    assert.ok(button('Stop owned application'));
    for (const label of ['Check cloud access', 'Create this cloud computer', 'Continue to application setup',
      'Recheck and continue this setup', 'Connect and check destination', 'Continue to private setup', 'Restart accepted application'])
      assert.equal(button(label) === undefined, true, label);
    assert.equal(document.querySelector('.remote-private-form') === null, true);
    assert.equal(document.querySelector('.remote-destination-form') === null, true);
    await click('Stop owned application');
    assert.equal(document.querySelector('.remote-deployment').dataset.remoteStatus, 'stopped');
    assert.deepEqual(calls.filter((call) => call.method === 'POST'), [{
      target: `/api/guide/${GUIDE}/remote-deployment/action`, method: 'POST',
      body: { deployment_id: runningRemote().deployment_id, action: 'stop' },
    }]);
  }, { panel: true, cloud: cloud(), remoteReceipt: runningRemote() });
});

test('Agent team selection withdraws previously available cloud private setup and restart', async () => {
  for (const [receipt, availableAction] of [
    [remote(), 'Continue to private setup'],
    [{ ...remote(), status: 'stopped', source_current: true, can_restart: true }, 'Restart accepted application'],
  ]) {
    await mounted(async ({ document, button, calls }) => {
      assert.ok(button(availableAction), `${availableAction} must be available before changing the installation source.`);
      assert.ok(button('Continue to application setup'));
      await act(async () => document.querySelector('input[name="deployment-source"][value="native-team"]').click());
      assert.ok(document.querySelector('.gcp-provisioning'));
      assert.ok(document.querySelector('.remote-deployment .gcp-resource-guidance'));
      assert.equal(button(availableAction) === undefined, true);
      assert.equal(button('Continue to application setup') === undefined, true);
      assert.equal(document.querySelector('.remote-private-form') === null, true);
      assert.equal(document.querySelector('.remote-destination-form') === null, true);
      assert.equal(calls.filter((call) => call.method === 'POST').length, 0);
    }, { panel: true, cloud: cloud(), remoteReceipt: receipt });
  }
});

test('the actual parent preserves cloud and remote custody through a Mac pivot and unavailable reads', async () => {
  await mounted(async ({ document, change, click, button, calls, setStatus, setRemoteStatusFailure }) => {
    const preparation = document.querySelector('.gcp-provisioning');
    const application = document.querySelector('.remote-deployment');
    const originalLinks = [...document.querySelectorAll('.gcp-resource-guidance a')].map((link) => link.href);
    const readsBeforePivot = calls.filter((call) => call.method === 'GET' &&
      /\/(?:gcp-deployment|remote-deployment)$/.test(new URL(call.target, 'http://localhost').pathname)).length;
    await change({ ...state, version: 5, proposal: { ...state.proposal, target: 'mac',
      project: { name: 'Local alternative', slug: 'local-alternative' } } });
    assert.equal(document.querySelector('input[name="deployment-source"]') === null, true, 'The local installation block is closed for the stale build.');
    assert.equal(document.querySelector('.gcp-provisioning') === preparation, true);
    assert.equal(document.querySelector('.remote-deployment') === application, true);
    assert.equal(calls.filter((call) => call.method === 'GET' &&
      /\/(?:gcp-deployment|remote-deployment)$/.test(new URL(call.target, 'http://localhost').pathname)).length, readsBeforePivot,
    'The same guide retains its cloud and application observers rather than remounting them.');
    setStatus(() => { throw new Error('Offline cloud status unavailable.'); });
    setRemoteStatusFailure(new Error('Offline remote status unavailable.'));
    await click('Recheck original cloud setup');
    await click('Recheck remote status');
    assert.equal(document.querySelector('.gcp-provisioning') === preparation, true);
    assert.equal(document.querySelector('.remote-deployment') === application, true);
    assert.equal(application.dataset.remoteStatus, 'running');
    assert.match(application.textContent, /Remote observation is unavailable/);
    assert.match(document.body.textContent, /Cloud observation is unavailable/);
    assert.match(application.querySelector('.deployment-location').textContent, /compute\.1234567890123456789.*\/home\/mobkit\/src\/fixture-app/);
    assert.deepEqual([...document.querySelectorAll('.gcp-resource-guidance a')].map((link) => link.href), originalLinks);
    assert.ok(button('Stop owned application'));
    for (const label of ['Create this cloud computer', 'Continue to application setup', 'Continue to private setup', 'Restart accepted application'])
      assert.equal(button(label) === undefined, true, label);
    await click('Stop owned application');
    assert.equal(application.dataset.remoteStatus, 'stopped');
    assert.deepEqual(calls.filter((call) => call.method === 'POST'), [{
      target: `/api/guide/${GUIDE}/remote-deployment/action`, method: 'POST',
      body: { deployment_id: runningRemote().deployment_id, action: 'stop' },
    }]);
  }, { panel: true, cloud: cloud(), remoteReceipt: runningRemote() });
});

test('a local parent with no saved cloud receipt, request or remote installation shows no phantom controls', async () => {
  await mounted(async ({ document, change, calls, store }) => {
    const style = document.createElement('style');
    style.textContent = await readFile(new URL('./deployment-panel.css', import.meta.url), 'utf8');
    document.head.appendChild(style);
    assert.notEqual(document.defaultView.getComputedStyle(document.querySelector('.deployment-panel')).display, 'none');
    assert.equal(gcpLoadIntent(store, GUIDE), null);
    await change({ ...state, version: 5, proposal: { ...state.proposal, target: 'mac' } }, null);
    const panel = document.querySelector('.deployment-panel');
    assert.ok(panel);
    assert.equal(panel.textContent.trim(), '');
    assert.equal(panel.childElementCount, 0);
    assert.equal(document.defaultView.getComputedStyle(panel).display, 'none');
    assert.equal(document.querySelector('.gcp-guided') === null, true);
    assert.equal(document.querySelector('.remote-deployment') === null, true);
    assert.equal(calls.filter((call) => call.method === 'POST').length, 0);
  }, { panel: true, cloud: null, remoteReceipt: null });
});

test('review removal keeps original names after a source pivot and Keep admits no deletion', async () => {
  await mounted(async ({ document, change, click, button, calls, store }) => {
    await change({ ...state, version: 5, proposal: { ...state.proposal, target: 'mac', project: { name: 'Alternative', slug: 'mac-alternative' } } });
    await click('Review cloud removal');
    const review = document.querySelector('.gcp-removal');
    assert.match(review.textContent, /VM: fixture-app.*Boot disk: fixture-app.*SSH access rule: fixture-app-iap-ssh/);
    assert.doesNotMatch(review.textContent, /mac-alternative/);
    assert.equal(button('Remove VM, disk and access rule').disabled, true);
    assert.match(review.textContent, /permanently deletes.*Forge does not make a backup/);
    assert.equal(document.querySelector('.gcp-provisioning-step') === null, true);
    assert.equal(document.querySelector('.gcp-provisioning .gcp-resource-guidance') === null, true);
    assert.doesNotMatch(document.querySelector('.gcp-provisioning').textContent, /earlier accepted setup|computer preparation/);
    assert.equal(review.querySelector('details').open, false);
    await act(async () => review.querySelector('input[type=checkbox]').click());
    assert.equal(button('Remove VM, disk and access rule').disabled, false);
    await click('Keep cloud resources');
    assert.equal(document.querySelector('.gcp-removal') === null, true);
    assert.equal(gcpLoadRemovalIntent(store, GUIDE), null);
    assert.equal(calls.filter((call) => call.method === 'POST').length, 0);
    await click('Review cloud removal');
    assert.equal(document.querySelector('.gcp-removal input').checked, false);
    await click('Keep cloud resources'); await change(state);
    const nativeChoice = document.querySelector('input[name="deployment-source"][value="native-team"]');
    assert.ok(nativeChoice); await act(async () => nativeChoice.click()); await click('Review cloud removal');
    assert.ok(button('Keep cloud resources'));
  }, { panel: true, cloud: { ...cloud(), resources: removalCloud().resources }, remoteReceipt: runningRemote() });
});

test('missing original IDs stay explicitly unverified during deliberate removal review', async () => {
  await mounted(async ({ document, click }) => {
    await click('Review cloud removal');
    assert.equal(document.querySelectorAll('[data-removal-resource]').length, 3);
    assert.equal([...document.querySelectorAll('[data-removal-resource]')].filter((row) => row.textContent.includes('identity not yet verified')).length, 3);
  }, { cloud: cloud('resume-required') });
});

test('storage failure sends no removal and leaves the original application stop available', async () => {
  await mounted(async ({ document, click, store, calls, button }) => {
    await click('Review cloud removal');
    await act(async () => document.querySelector('.gcp-removal input').click());
    store.fail = true;
    await click('Remove VM, disk and access rule');
    assert.equal(calls.filter((call) => call.method === 'POST').length, 0);
    assert.ok(button('Stop owned application'));
    assert.match(document.body.textContent, /storage write rejected/);
  }, { panel: true, cloud: cloud(), remoteReceipt: runningRemote(), randomId: OTHER_REQUEST });
});

test('a durable removal request immediately withdraws sibling controls before any server response', async () => {
  await mounted(async ({ document, click, button, pendingClick, setHandler, calls, store, setStatus, change }) => {
    let resolve;
    setHandler((action, body) => {
      assert.equal(action, 'remove');
      assert.equal(gcpLoadRemovalIntent(store, GUIDE).request_id, body.request_id);
      return new Promise((done) => { resolve = done; });
    });
    await click('Review cloud removal');
    await act(async () => document.querySelector('.gcp-removal input').click());
    pendingClick('Remove VM, disk and access rule');
    assert.equal(typeof resolve, 'function');
    assert.equal(button('Stop owned application') === undefined, true);
    assert.equal(button('Continue to application setup') === undefined, true);
    assert.equal(document.querySelector('.remote-installed-use') === null, true);
    assert.match(document.querySelector('.remote-deployment').textContent, /Application access, installation and restart are withdrawn/);
    await change({ ...state, version: 5, proposal: { ...state.proposal, target: 'mac' } });
    assert.equal(button('Stop owned application') === undefined, true);
    setStatus(removalCloud());
    await act(async () => resolve(removalCloud()));
    assert.equal(document.querySelector('[data-cloud-status]').dataset.cloudStatus, 'removing');
    assert.deepEqual(calls.filter((call) => call.method === 'POST'), [{ target: `/api/guide/${GUIDE}/gcp-deployment/remove`, method: 'POST',
      body: { operation_id: REQUEST, request_id: OTHER_REQUEST, acknowledge_data_loss: true } }]);
  }, { panel: true, cloud: cloud(), remoteReceipt: runningRemote(), randomId: OTHER_REQUEST });
});

test('a lost removal reply and failed reads preserve identity through reload and retry the same request', async () => {
  await mounted(async ({ document, click, button, setHandler, setStatus, remount, calls, store, randomCalls }) => {
    setHandler(() => { throw new Error('Offline removal response lost.'); });
    await click('Review cloud removal');
    await act(async () => document.querySelector('.gcp-removal input').click());
    setStatus(() => { throw new Error('Offline resource status unavailable.'); });
    await click('Remove VM, disk and access rule');
    assert.equal(button('Stop owned application') === undefined, true);
    assert.match(document.body.textContent, /cloud charges may continue/);
    await remount();
    assert.equal(gcpLoadRemovalIntent(store, GUIDE).request_id, OTHER_REQUEST);
    assert.equal(calls.filter((call) => call.method === 'POST').length, 1);
    assert.equal(button('Create this cloud computer') === undefined, true);
    setStatus(cloud()); await click('Recheck removal');
    assert.equal(button('Continue to application setup') === undefined, true);
    setHandler(() => removalCloud('removal-required', OTHER_REQUEST, { vm: { status: 'absent' }, disk: { status: 'absent' }, firewall: { status: 'checking' } }));
    await click('Continue removal');
    assert.equal(document.querySelectorAll('[data-removal-resource]').length, 3);
    assert.equal([...document.querySelectorAll('[data-removal-resource]')].filter((row) => row.textContent.includes('Confirmed absent')).length, 2);
    const posts = calls.filter((call) => call.method === 'POST');
    assert.deepEqual(posts[1].body, posts[0].body);
    assert.equal(randomCalls(), 1);
  }, { panel: true, cloud: cloud(), remoteReceipt: runningRemote(), randomId: OTHER_REQUEST });
});

test('completed removal keeps its original receipt without installation, restart or first-use controls', async () => {
  await mounted(async ({ document, button, setStatus, click, calls }) => {
    assert.match(document.querySelector('.gcp-provisioning').textContent, /Original cloud resources confirmed absent/);
    assert.equal([...document.querySelectorAll('[data-removal-resource]')].filter((row) => row.textContent.includes('Confirmed absent')).length, 3);
    assert.equal(document.querySelector('.remote-private-form') === null, true);
    assert.equal(document.querySelector('.remote-installed-use') === null, true);
    for (const label of ['Create this cloud computer', 'Review cloud removal', 'Continue removal', 'Continue to application setup',
      'Continue to private setup', 'Restart accepted application', 'Stop owned application']) assert.equal(button(label) === undefined, true, label);
    setStatus(() => { throw new Error('Offline recheck failed.'); }); await click('Recheck removal');
    assert.equal(document.querySelector('[data-cloud-status]').dataset.cloudStatus, 'removed');
    assert.match(document.body.textContent, /Last observed: Confirmed absent/);
    assert.equal(calls.filter((call) => call.method === 'POST').length, 0);
  }, { panel: true, cloud: removalCloud('removed'), remoteReceipt: { ...runningRemote(), cloud_status: 'removed' } });
});

test('a resumed backend removal uses its existing request even after the browser lost its local intent', async () => {
  await mounted(async ({ click, store, setHandler, randomCalls, change, calls }) => {
    await change({ ...state, version: 5, proposal: { ...state.proposal, target: 'mac' } });
    setHandler((action, body) => {
      assert.equal(action, 'remove'); assert.equal(gcpLoadRemovalIntent(store, GUIDE).request_id, OTHER_REQUEST);
      assert.deepEqual(body, { operation_id: REQUEST, request_id: OTHER_REQUEST, acknowledge_data_loss: true });
      return removalCloud('removed');
    });
    await click('Continue removal');
    assert.equal(randomCalls(), 0);
    assert.equal(calls.filter((call) => call.method === 'POST').length, 1);
  }, { cloud: removalCloud('removal-required') });
});

test('foreign removal acknowledgements cannot replace the original resource generation or claim completion', async () => {
  for (const changed of [
    { account: 'other@example.test', endpoint: { ...endpoint(), account: 'other@example.test' } },
    { binding: { ...binding, source_digest: 'f'.repeat(64) } },
    { resources: { ...removalCloud().resources, disk: { name: destination.instance, id: '9999' } } },
    { removal: { ...removalCloud('removed').removal, request_id: REQUEST } },
  ]) await mounted(async ({ document, click, setHandler, setStatus, calls, store }) => {
    await click('Review cloud removal');
    await act(async () => document.querySelector('.gcp-removal input').click());
    setHandler(() => ({ ...removalCloud('removed'), ...changed }));
    setStatus({ ...cloud(), resources: removalCloud().resources });
    await click('Remove VM, disk and access rule');
    assert.equal(document.querySelector('[data-cloud-status]').dataset.cloudStatus, 'vm-ready');
    assert.doesNotMatch(document.querySelector('.gcp-provisioning').textContent, /Original cloud resources confirmed absent/);
    assert.equal(gcpLoadRemovalIntent(store, GUIDE).request_id, OTHER_REQUEST);
    assert.equal(calls.filter((call) => call.method === 'POST').length, 1);
  }, { cloud: { ...cloud(), resources: removalCloud().resources }, randomId: OTHER_REQUEST });
});

test('a pending preparation GET cannot roll back a confirmed removal acknowledgement', async () => {
  await mounted(async ({ document, click, pendingClick, setStatus, setHandler }) => {
    let resolve;
    setStatus(() => new Promise((done) => { resolve = done; }));
    pendingClick('Recheck original cloud setup');
    for (let turn = 0; turn < 16 && typeof resolve !== 'function'; turn += 1) await act(async () => new Promise(setImmediate));
    assert.equal(typeof resolve, 'function');
    await click('Review cloud removal');
    await act(async () => document.querySelector('.gcp-removal input').click());
    setHandler(() => removalCloud('removed'));
    await click('Remove VM, disk and access rule');
    await act(async () => resolve(cloud()));
    assert.equal(document.querySelector('[data-cloud-status]').dataset.cloudStatus, 'removed');
  }, { cloud: cloud(), randomId: OTHER_REQUEST });
});

test('a removal signal does not revoke a separate saved cloud generation in the same guide', async () => {
  await mounted(async ({ button, document, calls }) => {
    assert.ok(button('Stop owned application'));
    assert.doesNotMatch(document.querySelector('.remote-deployment').textContent, /Application access, installation and restart are withdrawn/);
    assert.equal(calls.filter((call) => call.method === 'POST').length, 0);
  }, { panel: true, cloud: removalCloud('removed'), remoteReceipt: {
    ...runningRemote(), cloud_operation_id: OTHER_REQUEST, endpoint: endpoint(OTHER_REQUEST),
  } });
});


test('an existing cloud computer joins the current unchanged package across later chat revisions', () => {
  const next = { ...state, version: 6 }, currentBuild = { ...accepted, current_guide_version: 6 };
  const created = { ...cloud(), binding: { ...binding, guide_version: 5 } };
  assert.equal(gcpCloudCurrent(created, next, currentBuild), true);
  assert.equal(gcpCloudCurrent(created, next, accepted), false);
  for (const changed of [{ binding: { ...created.binding, guide_version: 7 } },
    { binding: { ...created.binding, source_digest: 'e'.repeat(64) } },
    { binding: { ...created.binding, package_sha256: 'e'.repeat(64) } },
    { binding: { ...created.binding, blueprint_hash: 'e'.repeat(64) } },
    { destination: { ...destination, project: 'another-project' } }])
    assert.equal(gcpCloudCurrent({ ...created, ...changed }, next, currentBuild), false);
  assert.equal(gcpCloudCurrent(created, next, currentBuild, true), false);
  assert.equal(created.binding.guide_version, 5);
});

test('cloud handoff after harmless chat uses the original VM and the current connection snapshot', async () => {
  const created = { ...cloud(), binding: { ...binding, guide_version: 5 } };
  await mounted(async ({ change, click, calls, setHandler, document }) => {
    await change({ ...state, version: 6 }, { ...accepted, current_guide_version: 6 });
    setHandler(async action => { assert.equal(action, 'connect');
      return { ...remote(), guide_version: 6, current_guide_version: 6 }; });
    await click('Continue to application setup');
    assert.ok(document.querySelector('.gcp-application-handoff'));
    const posts = calls.filter(call => call.method === 'POST'); assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { operation_id: REQUEST, version: 6, build_id: accepted.build_id });
    assert.equal(created.binding.guide_version, 5);
  }, { cloud: created });
});

test('a delayed cloud connection acknowledgement cannot regain authority after a later chat', async () => {
  await mounted(async ({ pendingClick, change, setHandler, document, calls }) => {
    let resolve;
    setHandler(() => new Promise(done => { resolve = done; }));
    pendingClick('Continue to application setup');
    await change({ ...state, version: 5 }, { ...accepted, current_guide_version: 5 });
    await act(async () => resolve(remote()));
    assert.equal(document.querySelector('.gcp-application-handoff') === null, true);
    assert.equal(calls.filter(call => call.method === 'POST').length, 1);
  }, { cloud: cloud() });
});
