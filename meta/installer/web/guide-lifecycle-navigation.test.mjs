import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { webcrypto } from 'node:crypto';
import { build } from 'esbuild';
import React, { act } from 'react';
import { GUIDE_KEY } from './guided-state.mjs';
import { inputKey } from './wizard-state.mjs';
import { gcpRemovalIntentKey } from './gcp-guided.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));

// Bundle only when this test module is run after the complete source review.
// App, GuidedSetup, lifecycle controls, private forms and callbacks remain real. These
// replacements remove unrelated Console drawing and generated animation only.
// A file-backed bundle keeps failure stacks bounded; its linked map preserves
// the original source locations and contents for diagnostics.
const bundleDirectory = await mkdtemp(join(tmpdir(), 'mobkit-guide-lifecycle-'));
const appModulePath = join(bundleDirectory, 'guide-lifecycle-app.mjs');
after(() => rm(bundleDirectory, { recursive: true, force: true }));
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('app.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node',
  outfile: appModulePath, sourcemap: 'linked', sourcesContent: true,
  logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'real-app-inert-drawing', setup(builder) {
    // Keep ReactDOM's CommonJS loader outside the ESM bundle too. App and this
    // harness then use the same installed React/ReactDOM module instances.
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /console-workbench\.[tj]sx$/ }, () => ({
      contents: `import React from 'react';
        export function ConsoleWorkbench(props) {
          return React.createElement('div', {'data-console-layout-fixture': true},
            props.launcherHeader, props.launcher, props.launcherFooter,
            props.mainHeader, props.main, props.mainFooter,
            props.activityRailHeader, props.activityRail);
        }`, loader: 'js',
    }));
    builder.onLoad({ filter: /topology-panel\.[tj]sx$/ }, () => ({
      contents: 'export const TopologyPanel = () => null;', loader: 'js',
    }));
    builder.onLoad({ filter: /conversation-pane\.[tj]sx$/ }, () => ({
      contents: 'export const ConversationPane = () => null;', loader: 'js',
    }));
    builder.onLoad({ filter: /conversation-transcript\.[tj]sx$/ }, () => ({
      contents: 'export const ConversationTranscript = () => null;', loader: 'js',
    }));
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: `import React from 'react';
        export function ProposalSimulation(props) {
          return React.createElement('div', {'data-proposal-snapshot': JSON.stringify(props.plan)});
        }`, loader: 'js',
    }));
  } }],
}).catch(async error => {
  await rm(bundleDirectory, { recursive: true, force: true });
  throw error;
});
try {
  for (const output of bundled.outputFiles) await writeFile(output.path, output.contents);
} catch (error) {
  await rm(bundleDirectory, { recursive: true, force: true });
  throw error;
}
const appModuleUrl = pathToFileURL(appModulePath).href;

const A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const unsent = '  A separate unsent idea.\nKeep its exact whitespace.  ';
const feedback = 'Human implementation note for A, still unsent.';
const sample = 'Synthetic trial request for A, with its continuing context.';
const privateValue = 'SYNTHETIC-PRIVATE-DRAFT-A';
const privateUser = 'Synthetic user A';
const urlFor = id => `/api/guide/${id}`;
const feedbackKey = id => `mobkit-implementation-feedback-v1:${id}`;
const clone = value => value == null ? value : structuredClone(value);
const absent = detail => ({ httpError: 404, detail });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject, started: false };
};

function planFor(id) {
  return { schema_version: 2,
    project: { name: id === A ? 'Observing crew A' : 'Observing crew B',
      slug: id === A ? 'lifecycle-crew-a' : 'lifecycle-crew-b',
      description: 'Prepare an observing plan from synthetic evidence.' },
    provider: 'openai', model: 'gpt-6.1-sol', target: 'mac',
    agent_classes: [{ id: 'planner', name: 'Observing planner',
      description: 'Keep the continuing observing context.', instructions: 'Use the supplied synthetic evidence.',
      instances: 1, members: [], provider: 'openai', model: 'gpt-6.1-sol',
      tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
    topology: { mode: 'manual', entrypoint: 'planner:main', edges: [] },
    connections: [], flows: [], knowledge: { mode: 'none' }, advanced: { port: 4110 } };
}

function guideFor(id) {
  return { id, version: 5, status: 'question', stage: 'purpose', completed_stages: [],
    answers: [], messages: [], proposal: planFor(id), blueprint: null,
    brief: { facts: {}, requirements: [] }, readiness: { ready: true, decision_blockers: [] },
    capabilities: { available: false, model: 'gpt-6.1-sol' },
    question: { id: 'observing-context', stage: 'purpose', title: 'Who should this help?',
      description: 'Describe the continuing observing context.', multiple: false, allow_text: true,
      options: [{ id: 'crew', label: 'Our observing crew', description: 'A shared crew.' },
        { id: 'solo', label: 'Just me', description: 'My own observing context.' }] } };
}

function buildFor(id, change = {}) {
  const digest = (id === A ? 'a' : 'b').repeat(64);
  return { build_id: `checked-build-${id}`, guide_id: id, guide_version: 5,
    current: true, status: 'completed', ready_to_package: true, source_verified: true,
    source_digest: digest, worker_stop_confirmed: true, cleanup_pending: false,
    acceptance_current: true, acceptance_trusted: true,
    acceptance: { issuer: 'mobkit-launch-acceptance-host', source_digest: digest,
      status: 'pass', tests_run: 1, review_round: 1, requirements: [], counterexamples: [],
      summary: 'Synthetic accepted offline source.' },
    summary: id === A ? 'Retained checked source A.' : 'Retained checked source B.',
    files: [], checks: [], progress: [], ...change };
}

function deploymentFor(id, change = {}) {
  return { guide_id: id, guide_version: 5, current: true, build_id: buildFor(id).build_id,
    deployment_id: `owned-installation-${id}`, status: 'stopped', process_live: false,
    source_digest: buildFor(id).source_digest, package_sha256: 'c'.repeat(64),
    destination: `/synthetic/owned-${id === A ? 'a' : 'b'}`, execution_mode: 'interactive',
    detail: id === A ? 'Retained stopped installation A.' : 'Retained stopped installation B.',
    ...change };
}

function trialFor(id, change = {}) {
  return { guide_id: id, guide_version: 5, current: true, rehearsal_id: `owned-trial-${id}`,
    mode: 'design', status: 'waiting_input', keep_open: true, turn_count: 1,
    session_status: 'idle', can_continue: true, text: sample, identities: ['planner:main'],
    turns: [{ identity: 'planner:main', status: 'completed', result: 'Retained synthetic trial answer.' }],
    ...change };
}

function remoteFor(id) {
  return { protocol: 1, target: 'linux', guide_id: id, guide_version: 5,
    deployment_id: '33333333-3333-4333-8333-333333333333', build_id: buildFor(id).build_id,
    accepted_source_digest: buildFor(id).source_digest, package_sha256: 'b'.repeat(64),
    transferred_source_digest: 'c'.repeat(64), controller_sha256: 'd'.repeat(64),
    source_binding_current: true, source_current: true, status: 'running', ready: true,
    remote_admission_requested: true, admitted: true, worker_live: false,
    service_owned: true, service_observed: true, observation: { available: true },
    endpoint: { host: 'synthetic-linux.local', user: 'fixture-user', ssh_port: 22 },
    destination: '/synthetic-owned/src/workshop',
    host: { uid: 1000, home: '/home/fixture-user', destination: '/synthetic-owned/src/workshop' },
    host_key_fingerprint: 'SHA256:' + 'e'.repeat(43),
    service: { service: 'mobkit-launch.22222222-2222-4222-8222-222222222222.service',
      status: 'running', ready: true, scope: 'native-bootstrap-and-console',
      source_digest: 'c'.repeat(64), source_current: true, definition_verified: true,
      console_url: 'http://127.0.0.1:8791/console' } };
}

function remoteBrowserFor(id, change = {}) {
  const receipt = remoteFor(id), consoleUrl = receipt.service.console_url;
  return { protocol: 1, guide_id: id, guide_version: 5, build_id: receipt.build_id,
    deployment_id: receipt.deployment_id, access_id: '11111111-1111-4111-8111-111111111111',
    entry_id: 'native-console', status: 'ready', local_url: consoleUrl, console_url: consoleUrl,
    native_ready: true, cleanup_confirmed: false, observed_at: '2026-10-06T21:40:00Z',
    accepted_source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256,
    transferred_source_digest: receipt.transferred_source_digest, controller_sha256: receipt.controller_sha256,
    service_name: receipt.service.service, service_source_digest: receipt.service.source_digest,
    scope: { authentication: 'local-password', health_scope: 'native-bootstrap-and-console',
      requested_interfaces: 'Native admin UI', custom_interfaces_verified: false }, ...change };
}

function localUseFor(id, build = buildFor(id)) {
  const receipt = deploymentFor(id);
  return { deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: receipt.source_digest, package_sha256: receipt.package_sha256,
    guide_version: receipt.guide_version, current_guide_version: 5, version: 5,
    current: true, status: receipt.status, scope: 'verified-installed-source',
    application_outcomes_verified: false, project: { name: planFor(id).project.name },
    entrypoint: { identity: 'planner:main', name: 'Observing planner' },
    native_runtime_available: false, console_url: null,
    continuation: { build_id: build.build_id, source_digest: build.source_digest,
      available: build.status === 'completed', requires_acknowledgement: false } };
}

function localExperiencesFor(id) {
  const receipt = deploymentFor(id);
  return { deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    package_sha256: receipt.package_sha256, source_digest: receipt.source_digest,
    metadata_sha256: null, scope: 'declared-source', verified: false, entries: [] };
}

function remoteUseFor(id, build = buildFor(id)) {
  const receipt = remoteFor(id);
  const remoteBinding = Object.fromEntries(['deployment_id', 'guide_id', 'guide_version', 'build_id',
    'accepted_source_digest', 'package_sha256', 'transferred_source_digest', 'controller_sha256',
    'endpoint', 'host', 'destination', 'host_key_fingerprint'].map(key => [key, clone(receipt[key])]));
  return { protocol: 1, guide_id: id, version: 5, guide_version: receipt.guide_version,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    accepted_source_digest: receipt.accepted_source_digest, source_digest: receipt.accepted_source_digest,
    package_sha256: receipt.package_sha256, transferred_source_digest: receipt.transferred_source_digest,
    controller_sha256: receipt.controller_sha256, remote_binding: remoteBinding,
    scope: 'frozen-remote-package-source', current: true, status: receipt.status,
    project: { name: planFor(id).project.name }, entrypoint: { identity: 'planner:main', name: 'Observing planner' },
    observation: { scope: 'saved-destination-observation', status: receipt.status, available: true,
      freshness: 'fresh', admitted: true, source_current: true },
    installed_source_verified: true, native_runtime_available: true, application_outcomes_verified: false,
    continuation: { build_id: build.build_id, source_digest: build.source_digest,
      available: build.status === 'completed', requires_acknowledgement: false } };
}

function optionsFor(id, installed = false) {
  const receipt = deploymentFor(id);
  return { build_id: buildFor(id).build_id, guide_version: 5, current_guide_version: 5, current: true,
    source_digest: buildFor(id).source_digest, target: 'mac', host_target: 'mac', supported: true,
    default_destination: `/synthetic/new-${id === A ? 'a' : 'b'}`, console_port: 18840,
    fields: [{ key: 'OPENAI_API_KEY', label: 'Synthetic provider key', required: true,
      secret: true, configured: installed },
      { key: 'APP_USER', label: 'Synthetic application user', required: true,
        secret: false, configured: installed }],
    execution_modes: [{ id: 'interactive', available: true, label: 'While Launch is open',
      description: 'Run under Launch supervision.' }],
    ...(installed ? { scope: 'verified-installed-private-configuration', version: 5,
      deployment_id: receipt.deployment_id, package_sha256: receipt.package_sha256,
      source_verified: true, requires_stopped: true, destination: receipt.destination,
      execution_mode: receipt.execution_mode } : {}) };
}

function memoryStorage(values = new Map()) {
  return { values, getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key),
    key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
}

function fixture({ builds = true, installed = true, active = false, remote = false,
  localUse = false, remoteUse = false } = {}) {
  const guides = new Map([[A, guideFor(A)], [B, guideFor(B)]]);
  if (remoteUse) guides.get(A).proposal.target = 'linux';
  const receipts = {
    implementation: new Map([[A, builds ? buildFor(A, active ? { status: 'running', ready_to_package: false,
      worker_stop_confirmed: false, acceptance_current: false, acceptance_trusted: false } : {}) : null],
      [B, builds ? buildFor(B) : null]]),
    deployment: new Map([[A, installed ? deploymentFor(A, active ? { status: 'running', process_live: true } : {}) : null],
      [B, installed ? deploymentFor(B) : null]]),
    rehearsal: new Map([[A, active ? trialFor(A) : null], [B, null]]),
    remote: new Map([[A, remote ? remoteFor(A) : null], [B, null]]),
    browser: new Map([[A, remote ? remoteBrowserFor(A) : null], [B, null]]),
    cloud: new Map(),
  };
  const inputA = { key: inputKey(guides.get(A)), draft: unsent, selected: [], mode: 'chat' };
  const storage = memoryStorage(new Map([[GUIDE_KEY, A], [inputA.key, JSON.stringify(inputA)]]));
  const session = memoryStorage();
  const queues = new Map(), calls = [], unexpected = [];
  const keyFor = (url, method) => `${method} ${url}`;
  const schedule = (url, method = 'GET', consumers = 1) => {
    const waiting = deferred(), key = keyFor(url, method);
    waiting.startedCount = 0;
    queues.set(key, [...(queues.get(key) || []), ...Array.from({ length: consumers }, () => waiting)]);
    return waiting;
  };
  const read = async (url, options = {}) => {
    const method = options.method || 'GET';
    const body = options.body ? JSON.parse(options.body) : undefined;
    calls.push({ url, method, ...(body === undefined ? {} : { body }) });
    const queue = queues.get(keyFor(url, method));
    if (queue?.length) {
      const waiting = queue.shift(); waiting.started = true; waiting.startedCount += 1;
      return waiting.promise;
    }
    if (method !== 'GET') {
      unexpected.push({ url, method });
      throw Error(`Unexpected offline mutation: ${method} ${url}`);
    }
    if (url === '/api/workbench') return { blueprint: planFor(A),
      architect: { available: false, busy: false, model: 'gpt-6.1-sol', detail: 'Offline architect unavailable.' }, capabilities: {} };
    if (url === '/api/guide/list') return { setups: [...guides.values()].map(guide => ({
      id: guide.id, name: guide.proposal.project.name, status: guide.status })) };
    if (url === '/api/provider-settings') return {
      providers: ['openai', 'anthropic', 'gemini'].map(provider => ({ provider, configured: false, source: null, editable: true })),
      effective_provider: null, runtime_active: false, work_active: false, load_error: null,
      locked_provider: null, provider_binding_error: null, runtime_status: 'stopped' };
    if (url === '/api/architect/status') return { available: false, busy: false, model: 'gpt-6.1-sol' };
    if (url === '/api/capabilities') return { rehearsal: { start_request_replay: false } };
    if (url === '/api/checker-setup') return { status: 'ready', can_prepare: false, detail: 'Offline checker ready.' };
    for (const id of [A, B]) {
      const base = urlFor(id);
      if (url === base) return clone(guides.get(id));
      if (url.startsWith(`${base}/timeline?`)) return { available: true, frames: [], bindings: [], exhausted: true };
      for (const kind of ['implementation', 'deployment', 'rehearsal']) {
        if (url === `${base}/${kind}`) return clone(receipts[kind].get(id)) || absent('No saved synthetic activity.');
      }
      if (url === `${base}/gcp-deployment` || url === `${base}/gcp-deployment?operation_id=${receipts.cloud.get(id)?.operation_id}`)
        return clone(receipts.cloud.get(id)) || null;
      if (url === `${base}/remote-deployment`) return clone(receipts.remote.get(id)) || absent('No saved remote deployment.');
      const ownedRemote = receipts.remote.get(id);
      if (ownedRemote && url === `${base}/remote-deployment/browser-access?deployment_id=${ownedRemote.deployment_id}`)
        return clone(receipts.browser.get(id));
      if (ownedRemote && url === `${base}/remote-deployment/use?deployment_id=${ownedRemote.deployment_id}`)
        return remoteUse ? remoteUseFor(id, receipts.implementation.get(id))
          : absent('Saved remote application guidance is unavailable in this offline browser-cleanup fixture.');
      if (url === `${base}/deployment/options?build_id=${buildFor(id).build_id}`) return remoteUse && id === A
        ? { ...optionsFor(id), target: 'linux', supported: false } : optionsFor(id);
      if (url === `${base}/deployment/settings?deployment_id=${deploymentFor(id).deployment_id}`) return optionsFor(id, true);
      if (url === `${base}/deployment/experiences`)
        return localUse ? localExperiencesFor(id)
          : absent('Everyday application UI details are unavailable in this offline navigation fixture.');
      if (url === `${base}/deployment/use`)
        return localUse ? localUseFor(id, receipts.implementation.get(id))
          : absent('Everyday application UI details are unavailable in this offline navigation fixture.');
      const buildId = receipts.implementation.get(id)?.build_id;
      if (buildId && url === `${base}/implementation/timeline?build_id=${buildId}&limit=200`)
        return { build_id: buildId, frames: [], bindings: [], has_more: false };
      const trialId = receipts.rehearsal.get(id)?.rehearsal_id;
      if (trialId && url === `${base}/rehearsal/timeline?rehearsal_id=${trialId}&limit=200`)
        return { rehearsal_id: trialId, frames: [], bindings: [], next_cursor: null, latest_cursor: null };
    }
    unexpected.push({ url, method });
    throw Error(`Unexpected offline read: ${method} ${url}`);
  };
  return { guides, receipts, storage, session, calls, unexpected, read, schedule,
    posts: () => calls.filter(call => call.method === 'POST'),
    feedback: id => storage.getItem(feedbackKey(id)),
    composer: id => JSON.parse(storage.getItem(inputKey(guides.get(id))) || 'null')?.draft,
  };
}

async function mountedApp(t, state, verify) {
  t.mock.timers.enable({ apis: ['Date', 'setInterval', 'setTimeout'], now: Date.parse('2026-10-06T21:40:00Z') });
  const dom = new JSDOM('<div id="test-root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  Object.defineProperties(dom.window, {
    localStorage: { configurable: true, value: state.storage },
    sessionStorage: { configurable: true, value: state.session },
  });
  const replacements = { window: { value: dom.window }, document: { value: dom.window.document },
    navigator: { value: dom.window.navigator }, HTMLElement: { value: dom.window.HTMLElement },
    requestAnimationFrame: { value: dom.window.requestAnimationFrame.bind(dom.window) },
    cancelAnimationFrame: { value: dom.window.cancelAnimationFrame.bind(dom.window) },
    IS_REACT_ACT_ENVIRONMENT: { value: true }, crypto: { value: webcrypto },
    matchMedia: { value: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) },
    localStorage: { value: state.storage }, sessionStorage: { value: state.session },
    fetch: { value: async (url, options) => {
      const body = await state.read(url, options);
      return { ok: !body?.httpError, status: body?.httpError || 200,
        json: async () => body?.httpError ? { detail: body.detail, ...(body.code ? { code: body.code } : {}) } : body };
    } } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, descriptor] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, ...descriptor });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  let root;
  try {
    const { App } = await import(appModuleUrl);
    const { createRoot } = await import('react-dom/client');
    root = createRoot(dom.window.document.getElementById('test-root'));
    await act(async () => { root.render(React.createElement(App)); await settle(); });
    await act(async () => { await settle(); });
    await verify(dom.window.document, async milliseconds => {
      await act(async () => { t.mock.timers.tick(milliseconds); await settle(); });
    });
    assert.deepEqual(state.unexpected, [], 'Actual composed controls must use only the strict offline response allowlist');
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

function button(root, label) {
  const found = [...root.querySelectorAll('button')].find(node => node.textContent.trim() === label);
  assert.ok(found, `Missing actual control: ${label}`);
  return found;
}
async function click(root, label) {
  const found = button(root, label);
  assert.equal(found.disabled, false, `Actual control should be usable: ${label}`);
  await act(async () => { found.click(); await settle(); });
}
async function inputValue(input, value) {
  assert.ok(input, 'The real controlled field must be present');
  const prototype = input.tagName === 'TEXTAREA' ? input.ownerDocument.defaultView.HTMLTextAreaElement.prototype
    : input.ownerDocument.defaultView.HTMLInputElement.prototype;
  await act(async () => {
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(input, value);
    input.dispatchEvent(new input.ownerDocument.defaultView.Event('input', { bubbles: true }));
    await settle();
  });
}
async function selectControl(document, id) {
  await click(document, 'Your setups');
  const node = [...document.querySelectorAll('.setup-history-menu button')]
    .find(control => control.querySelector('span')?.textContent === planFor(id).project.name);
  assert.ok(node, 'Selection must use the real saved-setup control');
  return node;
}
async function beginSelection(document, state, id = B, retainedControls = []) {
  const waiting = state.schedule(urlFor(id));
  const selection = await selectControl(document, id);
  for (const node of retainedControls) assert.equal(node.disabled, false, 'Retained control was usable before navigation');
  await act(async () => {
    selection.click();
    // This is the real mounted prior render's handler, before React publishes
    // disabled state. No private React callback or test-only guard is invoked.
    for (const node of retainedControls) node.click();
    await settle();
  });
  assert.equal(waiting.started, true, 'The selected guide GET must actually remain pending');
  return waiting;
}
async function finish(waiting, value) {
  await act(async () => { waiting.resolve(clone(value)); await settle(); });
  await act(async () => { await settle(); });
}
async function fail(waiting, detail) {
  await act(async () => { waiting.reject(Error(detail)); await settle(); });
  await act(async () => { await settle(); });
}
const selected = document => document.querySelector('.setup-response-anchor')?.dataset.guideId;
const proposal = document => JSON.parse(document.querySelector('[data-proposal-snapshot]').getAttribute('data-proposal-snapshot'));
const implementation = document => document.querySelector('.implementation-panel');
const deployment = document => document.querySelector('.deployment-panel');
function savedBuildObservationFixture(options = {}) {
  const state = fixture({ builds: false, installed: false, ...options });
  const originalKey = inputKey(state.guides.get(A));
  for (const guide of state.guides.values()) {
    guide.status = 'ready'; guide.question = null;
    guide.readiness = { ready: true, context_ready: true, decision_blockers: [],
      implementation_blockers: ['Build the custom observing dashboard.'] };
  }
  const currentKey = inputKey(state.guides.get(A));
  state.storage.removeItem(originalKey);
  state.storage.setItem(currentKey, JSON.stringify({ key: currentKey, draft: unsent, selected: [], mode: 'chat' }));
  return state;
}
function applicationStatus(document) {
  const status = document.querySelector('.setup-progress-card');
  assert.ok(status, 'The real parent application status must be present independently of the builder');
  return status;
}

function remoteLifecycleFixture(status = 'running', cloud = false) {
  const state = fixture({ installed: false, remote: true });
  state.guides.get(A).proposal.target = cloud ? 'gcp' : 'linux';
  const receipt = { ...remoteFor(A), status, ready: status === 'running', can_stop: status === 'running',
    can_restart: status === 'stopped' };
  if (cloud) {
    receipt.target = 'gcp'; receipt.cloud_operation_id = '44444444-4444-4444-8444-444444444444';
    receipt.endpoint = { transport: 'gcp-iap', operation_id: receipt.cloud_operation_id,
      account: 'fixture@example.test', project: 'fixture-project', zone: 'europe-west1-b', instance: 'fixture-app',
      instance_id: '1234', ownership_token: 'e'.repeat(32), host: 'compute.1234', user: 'mobkit', ssh_port: 22 };
  }
  state.receipts.remote.set(A, receipt);
  return state;
}

for (const [status, cloud, title, label] of [['running', true, 'Your installed cloud application is running', 'Use your remote application'],
  ['stopped', false, 'Your installed remote application is stopped', 'Manage remote application'],
  ['failed', false, 'Your remote installation needs attention', 'Review remote recovery']])
  test(`remote lifecycle narration reaches the parent for checked ${status} ${cloud ? 'cloud' : 'Linux'} installation`, async t => {
    const state = remoteLifecycleFixture(status, cloud), before = clone(state.receipts.remote.get(A));
    await mountedApp(t, state, async (document, advance) => {
      const card = applicationStatus(document);
      assert.equal(card.querySelector('h1,h2').textContent, title);
      assert.deepEqual([...card.querySelectorAll('button')].map(node => node.textContent.trim()),
        [label, 'Ask the guide what is next']);
      assert.equal(card.querySelector('a') === null, true, 'Remote service status cannot create browser access');
      const target = document.querySelector('.remote-deployment'), revealed = [];
      target.scrollIntoView = () => revealed.push(target);
      await click(card, label);
      await advance(20);
      assert.equal(revealed.length, 1, 'The parent action must reveal one existing remote destination control');
      assert.equal(revealed[0] === target, true, 'The original remote section owns the navigation target');
      assert.equal(document.querySelector('.setup-proposal-actions').open, true);
      assert.deepEqual(state.receipts.remote.get(A), before);
      assert.equal(state.posts().length, 0, 'Narration/navigation cannot install, restart or send a job');
      assertComposer(document, state);
    });
  });

test('remote lifecycle narration stays unconfirmed during held reads and recovers after unavailable status', async t => {
  const state = remoteLifecycleFixture(), url = `${urlFor(A)}/remote-deployment`, initial = state.schedule(url);
  await mountedApp(t, state, async (document, advance) => {
    assert.equal(initial.started, true);
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Checking your remote installation status');
    await finish(initial, state.receipts.remote.get(A));
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Your installed remote application is running');
    const outage = state.schedule(url);
    await advance(2100); assert.equal(outage.started, true);
    await finish(outage, { httpError: 503, detail: 'SYNTHETIC REMOTE STATUS DETAIL' });
    const card = applicationStatus(document);
    assert.equal(card.querySelector('h1,h2').textContent, 'Remote installation status is unavailable');
    assert.deepEqual([...card.querySelectorAll('button')].map(node => node.textContent.trim()),
      ['Review remote status', 'Ask the guide what is next']);
    assert.doesNotMatch(card.textContent, /is running|is stopped|passed independent|SYNTHETIC REMOTE STATUS DETAIL/);
    assert.equal(document.querySelector('.remote-deployment').dataset.remoteStatus, 'running', 'Saved evidence is retained, not current runtime truth');
    await click(document.querySelector('.remote-deployment'), 'Recheck remote status');
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Your installed remote application is running');
    assert.equal(state.posts().length, 0); assertComposer(document, state);
  });
});

test('remote lifecycle narration cannot revive a stale A read after A-B-A selection', async t => {
  const state = remoteLifecycleFixture(), url = `${urlFor(A)}/remote-deployment`, oldA = state.schedule(url);
  await mountedApp(t, state, async document => {
    assert.equal(oldA.started, true);
    const openB = await beginSelection(document, state); await finish(openB, state.guides.get(B));
    assert.doesNotMatch(applicationStatus(document).textContent, /installed remote|Manage remote|Use your remote/);
    const freshA = state.schedule(url), openA = await beginSelection(document, state, A);
    await finish(openA, state.guides.get(A)); assert.equal(freshA.started, true);
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Checking your remote installation status');
    await finish(oldA, state.receipts.remote.get(A));
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Checking your remote installation status');
    const stopped = { ...state.receipts.remote.get(A), status: 'stopped', ready: false };
    await finish(freshA, stopped);
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Your installed remote application is stopped');
    assert.equal(state.posts().length, 0); assertComposer(document, state);
  });
});

test('remote lifecycle narration masks a held Stop and its unknown response without reposting', async t => {
  const state = remoteLifecycleFixture(), url = `${urlFor(A)}/remote-deployment`;
  await mountedApp(t, state, async document => {
    const stop = state.schedule(`${url}/action`, 'POST');
    await click(document.querySelector('.remote-deployment'), 'Stop owned application');
    assert.equal(stop.started, true);
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Checking your remote installation status');
    const fresh = state.schedule(url);
    await fail(stop, 'SYNTHETIC UNKNOWN STOP DETAIL'); assert.equal(fresh.started, true);
    assert.doesNotMatch(applicationStatus(document).textContent, /is running|is stopped|passed independent/);
    await finish(fresh, { httpError: 503, detail: 'Synthetic independent status outage' });
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Remote installation status is unavailable');
    const stopped = { ...state.receipts.remote.get(A), status: 'stopped', ready: false, can_stop: false, can_restart: true };
    state.receipts.remote.set(A, stopped);
    await click(document.querySelector('.remote-deployment'), 'Recheck remote status');
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Your installed remote application is stopped');
    assert.equal(state.posts().length, 1);
    assert.deepEqual(state.posts()[0].body, { deployment_id: stopped.deployment_id, action: 'stop' });
    assertComposer(document, state);
  });
});

test('remote lifecycle narration masks a held cloud removal without changing the saved application receipt', async t => {
  const state = remoteLifecycleFixture('running', true), receipt = clone(state.receipts.remote.get(A));
  const { endpoint } = receipt;
  state.receipts.cloud.set(A, { protocol: 1, guide_id: A, operation_id: receipt.cloud_operation_id,
    account: endpoint.account, status: 'vm-ready', phase: 'vm-ready', detail: 'Synthetic checked original cloud computer.',
    binding: { guide_id: A, guide_version: receipt.guide_version, build_id: receipt.build_id,
      source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256, blueprint_hash: 'f'.repeat(64) },
    destination: { project: endpoint.project, zone: endpoint.zone, instance: endpoint.instance,
      machine_type: 'e2-medium', disk_gb: 30, console_port: 8791 },
    endpoint: clone(endpoint), vm_verified: true, resume_required: false,
    application_installed: false, application_status: 'not-installed',
    resources: { vm: { name: endpoint.instance, id: endpoint.instance_id },
      disk: { name: endpoint.instance, id: '1235' }, firewall: { name: `${endpoint.instance}-iap-ssh`, id: '1236' } } });
  await mountedApp(t, state, async document => {
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Your installed cloud application is running');
    const cloud = document.querySelector('.gcp-guided'), removal = state.schedule(`${urlFor(A)}/gcp-deployment/remove`, 'POST');
    await click(cloud, 'Review cloud removal');
    const acknowledgement = cloud.querySelector('.gcp-removal input[type="checkbox"]');
    assert.ok(acknowledgement); await act(async () => { acknowledgement.click(); await settle(); });
    await click(cloud, 'Remove VM, disk and access rule'); assert.equal(removal.started, true);
    const savedIntent = JSON.parse(state.storage.getItem(gcpRemovalIntentKey(A)));
    assert.deepEqual(state.posts()[0].body, { operation_id: receipt.cloud_operation_id,
      request_id: savedIntent.request_id, acknowledge_data_loss: true });
    assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Checking your remote installation status');
    assert.doesNotMatch(applicationStatus(document).textContent, /is running|Use your remote application/);
    assert.equal(document.querySelector('.remote-deployment').dataset.remoteStatus, 'running');
    assert.equal(document.querySelector('.remote-deployment a[href="http://127.0.0.1:8791/console"]') === null, true);
    await fail(removal, 'SYNTHETIC UNKNOWN REMOVAL DETAIL');
    assert.doesNotMatch(applicationStatus(document).textContent, /is running|Use your remote application/);
    assert.deepEqual(state.receipts.remote.get(A), receipt);
    assert.equal(state.storage.getItem(gcpRemovalIntentKey(A)), JSON.stringify(savedIntent));
    assert.equal(state.posts().length, 1, 'Unconfirmed removal cannot repeat the request or operate the application');
    assertComposer(document, state);
  });
});
function assertUnconfirmedSavedBuild(document, phase) {
  const title = phase === 'loading' ? 'Checking your saved build' : 'Build status is unavailable';
  assert.equal(applicationStatus(document).querySelector('h1, h2').textContent, title);
  assert.equal(implementation(document).querySelector('h2').textContent, title);
  assert.doesNotMatch(applicationStatus(document).textContent,
    /Your application needs to be built|No custom application build is needed|passed independent offline checks/);
  assert.ok(![...applicationStatus(document).querySelectorAll('button')].some(node =>
    /^(?:Build application|Try the application|Try the newer application)$/.test(node.textContent.trim())),
  'Unconfirmed observation cannot recommend a build or candidate trial');
}
function assertComposer(document, state, id = A, text = unsent) {
  assert.equal(document.querySelector('#setup-message').value, text);
  assert.equal(state.composer(id), text);
}

function assertInstallationObservation(document, phase) {
  const parent = applicationStatus(document);
  assert.equal(parent.querySelector('h1, h2').textContent,
    phase === 'loading' ? 'Checking your installation status' : 'Installation status is unavailable');
  assert.equal(parent.querySelector('a[href$="/console"]') === null, true, 'Saved health cannot become a currently checked Console link');
  assert.ok(button(parent, 'Review installation status'));
  assert.ok(![...parent.querySelectorAll('button')].some(node => /^(?:Install agent team|Set up and run|Try the application)$/.test(node.textContent.trim())));
}
function checkedRunningInstallation(id = A) {
  return deploymentFor(id, { status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:18840/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } });
}

for (const consoleChecked of [true, false]) test(`installation panel health copy qualifies saved ${consoleChecked ? 'Console availability' : 'unverified Console health'} after a failed status read and restores checked wording without losing drafts`, async t => {
  const state = savedBuildObservationFixture({ builds: true, installed: true });
  const installed = checkedRunningInstallation();
  if (!consoleChecked) { installed.health = 'unknown'; installed.bootstrap.ready = false; }
  state.receipts.deployment.set(A, installed);
  await mountedApp(t, state, async (document, tick) => {
    const health = () => deployment(document).querySelector('.deployment-health').textContent;
    const current = consoleChecked ? /The process and native MobKit Console are available/ : /The process started, but its live console health has not been verified/;
    assert.match(health(), current); assert.doesNotMatch(health(), /Last checked:/);
    const task = 'Keep my unsent first job across this health read.';
    const report = 'Keep my unsent problem report too.';
    await inputValue(deployment(document).querySelector('.deployment-use-field textarea'), task);
    await inputValue(deployment(document).querySelector('.deployment-use-feedback textarea'), report);
    function assertRetained(liveLinksChecked) {
      assert.equal(deployment(document).querySelector('.deployment-use-field textarea').value, task);
      assert.equal(deployment(document).querySelector('.deployment-use-feedback textarea').value, report);
      assert.equal(!!deployment(document).querySelector('a[href="http://127.0.0.1:18840/console"]'), consoleChecked && liveLinksChecked);
      assertComposer(document, state); assert.equal(state.posts().length, 0);
    }
    const read = state.schedule(`${urlFor(A)}/deployment`);
    await tick(1500); assert.equal(read.started, true);
    await finish(read, { httpError: 503, detail: 'Synthetic unavailable current installation health.' });
    assert.match(health(), /Last checked:/); assert.match(health(), /Current installation status could not be checked/);
    assert.doesNotMatch(health(), current);
    assert.equal(button(deployment(document), 'Stop application').disabled, true); assertRetained(false);
    const fresh = state.schedule(`${urlFor(A)}/deployment`);
    await click(deployment(document), 'Check installation status'); assert.equal(fresh.started, true);
    assert.match(health(), /Last checked:/); assertRetained(false);
    await finish(fresh, installed);
    assert.match(health(), current); assert.doesNotMatch(health(), /Last checked:/);
    assert.equal(button(deployment(document), 'Stop application').disabled, false); assertRetained(true);
  });
});

test('installation panel health copy labels the saved running receipt during a pending Stop and independent receipt read', async t => {
  const state = savedBuildObservationFixture({ builds: true, installed: true });
  const installed = checkedRunningInstallation(); state.receipts.deployment.set(A, installed);
  await mountedApp(t, state, async document => {
    const health = () => deployment(document).querySelector('.deployment-health').textContent;
    const stop = state.schedule(`${urlFor(A)}/deployment/action`, 'POST');
    await click(deployment(document), 'Stop application'); assert.equal(stop.started, true);
    assert.match(health(), /Last checked: the process and native MobKit Console were available/);
    assert.match(health(), /Checking current installation status/);
    assert.doesNotMatch(health(), /The process and native MobKit Console are available/);
    assert.equal(deployment(document).getAttribute('data-deployment-status'), 'running');
    assert.equal(deployment(document).querySelector('a[href="http://127.0.0.1:18840/console"]') === null, true,
      'A retained running receipt cannot publish live links while Stop is unconfirmed');
    const cachedConsole = deployment(document).querySelector('.deployment-interface-scope a');
    assert.equal(cachedConsole?.getAttribute('aria-disabled'), 'true');
    assert.equal(cachedConsole?.tabIndex, -1);
    const body = state.posts()[0].body, fresh = state.schedule(`${urlFor(A)}/deployment`);
    const stopped = deploymentFor(A);
    const actionRequest = { version: 1, action_request_id: body.action_request_id,
      guide_id: A, deployment_id: stopped.deployment_id, action: 'stop', update_id: null, phase: 'settled', outcome: 'returned' };
    const commandRead = state.schedule(`${urlFor(A)}/deployment/action/${stopped.deployment_id}/${body.action_request_id}`);
    await finish(stop, { ...stopped, action_request: actionRequest });
    assert.equal(commandRead.started, true, 'Post-command recovery reads the exact settled Stop identity first.');
    assert.equal(fresh.started, false, 'An ordinary installation receipt does not replace exact command readback.');
    assert.equal(button(deployment(document), 'Stop application').disabled, true);
    assert.match(health(), /Last checked:/);
    await finish(commandRead, { action_request: actionRequest });
    assert.equal(fresh.started, true); assert.match(health(), /Last checked:/);
    await finish(fresh, stopped);
    assert.equal(deployment(document).querySelector('.deployment-health') === null, true);
    assert.equal(button(deployment(document), 'Start installed application').disabled, false);
    assert.equal(state.posts().length, 1, 'Health wording never repeats Stop or starts the installation');
    assertComposer(document, state);
  });
});

test('installation panel health copy uses a successful receipt read despite unreadable command tracking and keeps commands and live links paused', async t => {
  const state = savedBuildObservationFixture({ builds: true, installed: true });
  state.receipts.deployment.set(A, checkedRunningInstallation());
  state.session.setItem(`meta-forge-deployment-support-v1:${A}`, 'broken');
  await mountedApp(t, state, async document => {
    const health = deployment(document).querySelector('.deployment-health').textContent;
    assert.match(health, /The process and native MobKit Console are available/);
    assert.doesNotMatch(health, /Last checked:|Current installation status could not be checked/);
    assert.equal(deployment(document).querySelector('a[href="http://127.0.0.1:18840/console"]') === null, true,
      'Checked health does not establish readable command custody');
    const cachedConsole = deployment(document).querySelector('.deployment-interface-scope a');
    assert.equal(cachedConsole?.getAttribute('aria-disabled'), 'true');
    assert.equal(cachedConsole?.tabIndex, -1);
    assert.match(deployment(document).textContent, /links are paused until installation status can be checked/);
    assert.equal(button(deployment(document), 'Stop application').disabled, true);
    assert.match(deployment(document).textContent, /browser storage/);
    assert.equal(state.posts().length, 0); assertComposer(document, state);
  });
});

test('installation observation routes an accepted native plan to status controls until its installation read is checked', async t => {
  const state = implementationBriefFixture();
  for (const id of [A, B]) {
    state.receipts.implementation.set(id, null);
    state.receipts.deployment.set(id, null);
  }
  const first = state.schedule(`${urlFor(A)}/deployment`);
  await mountedApp(t, state, async document => {
    const teamPackage = document.querySelector('[aria-label="Agent team package"]');
    assert.ok(teamPackage); assert.equal(first.started, true);
    assertInstallationObservation(document, 'loading');
    const labels = () => [...teamPackage.querySelectorAll('button')].map(node => node.textContent.trim());
    const pendingLabels = labels(), pendingGuidance = teamPackage.querySelector('.setup-manual-note').textContent;
    await finish(first, { httpError: 503, detail: 'Synthetic accepted-plan status outage.' });
    assertInstallationObservation(document, 'unknown');
    const unknownLabels = labels();
    // Both old and corrected shortcuts only navigate. Finish the held read
    // before asserting the recommendation so failure cleanup stays bounded.
    await click(teamPackage, unknownLabels.includes('Review installation status') ? 'Review installation status' : 'Install agent team');
    assert.ok(deployment(document));
    assert.equal(state.posts().length, 0, 'Status navigation cannot admit an installation');
    await click(deployment(document), 'Check installation status');
    assert.ok(button(teamPackage, 'Install agent team'));
    assert.ok(!labels().includes('Review installation status'));
    assert.equal(state.posts().length, 0); assertComposer(document, state);
    assert.ok(!pendingLabels.includes('Install agent team'), 'Pending status cannot recommend a new installation');
    assert.ok(pendingLabels.includes('Review installation status'));
    assert.doesNotMatch(pendingGuidance, /Install here with private setup/);
    assert.ok(!unknownLabels.includes('Install agent team'), 'Unavailable status cannot recommend a new installation');
    assert.ok(unknownLabels.includes('Review installation status'));
  });
});

test('installation observation reaches the parent during the first pending read and preserves unsent input', async t => {
  const state = savedBuildObservationFixture({ builds: true, installed: true });
  const first = state.schedule(`${urlFor(A)}/deployment`);
  await mountedApp(t, state, async document => {
    assert.equal(first.started, true); assertInstallationObservation(document, 'loading');
    await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
    await finish(first, checkedRunningInstallation());
    assert.match(applicationStatus(document).querySelector('h1, h2').textContent, /installed.*running/);
    assert.ok(applicationStatus(document).querySelector('a[href="http://127.0.0.1:18840/console"]'));
    assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback); assertComposer(document, state);
    assert.equal(state.posts().length, 0);
  });
});

test('installation observation failure retains checked source and private drafts while same-receipt recovery updates the parent', async t => {
  for (const running of [false, true]) await t.test(running ? 'running receipt' : 'stopped private draft', async subtest => {
    const state = savedBuildObservationFixture({ builds: true, installed: true });
    const installed = running ? checkedRunningInstallation() : deploymentFor(A);
    state.receipts.deployment.set(A, installed);
    await mountedApp(subtest, state, async (document, tick) => {
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      if (!running) await openNewPrivateForm(document);
      const read = state.schedule(`${urlFor(A)}/deployment`); await tick(1500); assert.equal(read.started, true);
      await finish(read, { httpError: 503, detail: 'Synthetic installation observation failure.' });
      assertInstallationObservation(document, 'unknown');
      assert.match(deployment(document).textContent, /Retained (?:healthy|stopped) installation|Retained stopped installation/);
      assert.equal(button(deployment(document), running ? 'Stop application' : 'Start installed application').disabled, true);
      if (!running) assert.equal(document.querySelector('.deployment-form [name="OPENAI_API_KEY"]').value, privateValue);
      await click(deployment(document), 'Check installation status');
      assert.match(applicationStatus(document).querySelector('h1, h2').textContent, running ? /installed.*running/ : /installed.*stopped/);
      assert.deepEqual(state.receipts.deployment.get(A), installed);
      if (!running) assert.equal(document.querySelector('.deployment-form [name="OPENAI_API_KEY"]').value, privateValue);
      assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback); assertComposer(document, state);
      assert.equal(state.posts().length, 0);
    });
  });
});

test('installation observation stays pending after a settled Stop until the independently checked receipt returns', async t => {
  const state = savedBuildObservationFixture({ builds: true, installed: true });
  const installed = checkedRunningInstallation(); state.receipts.deployment.set(A, installed);
  await mountedApp(t, state, async document => {
    const stop = state.schedule(`${urlFor(A)}/deployment/action`, 'POST');
    await click(deployment(document), 'Stop application'); assert.equal(stop.started, true);
    assertInstallationObservation(document, 'loading');
    const body = state.posts()[0].body, fresh = state.schedule(`${urlFor(A)}/deployment`);
    const stopped = deploymentFor(A);
    const actionRequest = { version: 1, action_request_id: body.action_request_id,
      guide_id: A, deployment_id: stopped.deployment_id, action: 'stop', update_id: null, phase: 'settled', outcome: 'returned' };
    const commandRead = state.schedule(`${urlFor(A)}/deployment/action/${stopped.deployment_id}/${body.action_request_id}`);
    await finish(stop, { ...stopped, action_request: actionRequest });
    assert.equal(commandRead.started, true, 'The exact Stop settlement is checked before current installation status.');
    assert.equal(fresh.started, false);
    assertInstallationObservation(document, 'loading');
    assert.equal(button(deployment(document), 'Stop application').disabled, true);
    await finish(commandRead, { action_request: actionRequest });
    assert.equal(fresh.started, true); assertInstallationObservation(document, 'loading');
    assert.equal(deployment(document).getAttribute('data-deployment-status'), 'running');
    assert.equal(button(deployment(document), 'Stop application').disabled, true);
    await finish(fresh, { httpError: 503, detail: 'Synthetic post-command status failure.' });
    assertInstallationObservation(document, 'unknown');
    state.receipts.deployment.set(A, stopped); await click(deployment(document), 'Check installation status');
    assert.match(applicationStatus(document).querySelector('h1, h2').textContent, /installed.*stopped/);
    assert.equal(button(deployment(document), 'Start installed application').disabled, false);
    assert.equal(state.posts().length, 1, 'Status observation never repeats the exact Stop or starts the application'); assertComposer(document, state);
  });
});

test('installation observation does not confuse unreadable command tracking with a failed checked status GET', async t => {
  const state = savedBuildObservationFixture({ builds: true, installed: true });
  state.receipts.deployment.set(A, checkedRunningInstallation());
  state.session.setItem(`meta-forge-deployment-support-v1:${A}`, 'broken');
  await mountedApp(t, state, async document => {
    assert.match(applicationStatus(document).querySelector('h1, h2').textContent, /installed.*running/);
    assert.ok(applicationStatus(document).querySelector('a[href="http://127.0.0.1:18840/console"]'));
    assert.match(deployment(document).textContent, /browser storage/);
    assert.equal(button(deployment(document), 'Stop application').disabled, true);
    assert.equal(state.posts().length, 0); assertComposer(document, state);
  });
});

test('installation observation rejects replaced A reads after A-B-A while the new selected read remains pending', async t => {
  for (const outcome of ['receipt', 'absence', 'failure']) await t.test(outcome, async subtest => {
    const state = savedBuildObservationFixture({ builds: true, installed: true }), oldA = state.schedule(`${urlFor(A)}/deployment`);
    await mountedApp(subtest, state, async document => {
      assert.equal(oldA.started, true); assertInstallationObservation(document, 'loading');
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      const openB = await beginSelection(document, state); await finish(openB, state.guides.get(B));
      const freshA = state.schedule(`${urlFor(A)}/deployment`), openA = await beginSelection(document, state, A);
      await finish(openA, state.guides.get(A)); assert.equal(freshA.started, true); assertInstallationObservation(document, 'loading');
      await finish(oldA, outcome === 'receipt' ? checkedRunningInstallation()
        : outcome === 'absence' ? absent('Stale A absence.') : { httpError: 503, detail: 'STALE A STATUS FAILURE' });
      assertInstallationObservation(document, 'loading'); assert.doesNotMatch(deployment(document).textContent, /STALE A STATUS FAILURE/);
      await finish(freshA, deploymentFor(A));
      assert.match(applicationStatus(document).querySelector('h1, h2').textContent, /installed.*stopped/);
      assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback); assertComposer(document, state);
      assert.equal(state.posts().length, 0);
    });
  });
});

test('installation observation masks retained status on same-guide reload and rejects its prior in-flight read without clearing private drafts', async t => {
  const state = savedBuildObservationFixture({ builds: true, installed: true });
  await mountedApp(t, state, async (document, tick) => {
    await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback); await openNewPrivateForm(document);
    const refused = await beginSelection(document, state); await fail(refused, 'Synthetic B refusal.');
    const oldA = state.schedule(`${urlFor(A)}/deployment`); await tick(1500); assert.equal(oldA.started, true);
    const freshA = state.schedule(`${urlFor(A)}/deployment`), reload = state.schedule(urlFor(A));
    await click(document, 'Reload saved conversation'); await finish(reload, state.guides.get(A));
    assertInstallationObservation(document, 'loading');
    await tick(1500); assert.equal(freshA.started, true);
    await finish(oldA, checkedRunningInstallation()); assertInstallationObservation(document, 'loading');
    assert.equal(document.querySelector('.deployment-form [name="OPENAI_API_KEY"]').value, privateValue);
    await finish(freshA, deploymentFor(A));
    assert.match(applicationStatus(document).querySelector('h1, h2').textContent, /installed.*stopped/);
    assert.equal(document.querySelector('.deployment-form [name="OPENAI_API_KEY"]').value, privateValue);
    assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback); assertComposer(document, state);
    assert.equal(state.posts().length, 0);
  });
});

test('saved-build observation reaches the parent during the first pending read and only a checked result replaces it', async t => {
  for (const result of ['absence', 'saved build']) await t.test(result, async subtest => {
    const state = savedBuildObservationFixture();
    const firstRead = state.schedule(`${urlFor(A)}/implementation`);
    await mountedApp(subtest, state, async document => {
      assert.equal(firstRead.started, true);
      assertUnconfirmedSavedBuild(document, 'loading');
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      assert.equal(button(implementation(document), 'Checking build status...').disabled, true);
      assert.equal(state.posts().length, 0);
      if (result === 'absence') {
        await finish(firstRead, absent('No saved build for this selected guide.'));
        assert.match(applicationStatus(document).textContent, /Your application needs to be built/);
        assert.equal(button(applicationStatus(document), 'Build application').disabled, false);
        await click(applicationStatus(document), 'Build application');
        assert.equal(button(implementation(document), 'Build application').disabled, false);
      } else {
        const saved = buildFor(A);
        state.receipts.implementation.set(A, saved);
        await finish(firstRead, saved);
        assert.match(applicationStatus(document).textContent, /Your application passed independent offline checks/);
        assert.doesNotMatch(applicationStatus(document).textContent, /needs to be built/);
        assert.match(implementation(document).textContent, /Retained checked source A\./);
      }
      assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
      assert.equal(state.feedback(A), feedback); assertComposer(document, state);
      assert.equal(state.posts().length, 0, 'Checking and revealing the builder do not admit a build or consume its note');
    });
  });
});

test('saved-build observation publishes a first 503 with null receipt and recovers through the existing status refresh', async t => {
  for (const result of ['absence', 'saved build']) await t.test(result, async subtest => {
    const state = savedBuildObservationFixture();
    const firstRead = state.schedule(`${urlFor(A)}/implementation`);
    await mountedApp(subtest, state, async document => {
      assertUnconfirmedSavedBuild(document, 'loading');
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      await finish(firstRead, { httpError: 503, detail: 'Synthetic first build status failure.' });
      assertUnconfirmedSavedBuild(document, 'unknown');
      const recoveryAction = [...applicationStatus(document).querySelectorAll('button')]
        .find(node => !/^Ask the guide/.test(node.textContent.trim()));
      assert.ok(recoveryAction && !recoveryAction.disabled, 'The status card must reveal the existing builder recovery');
      await act(async () => { recoveryAction.click(); await settle(); });
      const recovery = state.schedule(`${urlFor(A)}/implementation`);
      await click(implementation(document), 'Refresh build status');
      assert.equal(recovery.started, true); assertUnconfirmedSavedBuild(document, 'loading');
      if (result === 'absence') {
        await finish(recovery, absent('Checked absence after failed read.'));
        assert.match(applicationStatus(document).textContent, /Your application needs to be built/);
      } else {
        const saved = buildFor(A);
        state.receipts.implementation.set(A, saved);
        await finish(recovery, saved);
        assert.match(applicationStatus(document).textContent, /Your application passed independent offline checks/);
      }
      assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
      assert.equal(state.feedback(A), feedback); assertComposer(document, state);
      assert.equal(state.posts().length, 0, 'A failed read and explicit status recovery must not send the note or admit work');
    });
  });
});

test('saved-build observation retains same-receipt findings and stopped or running installed actions after a later 503', async t => {
  for (const running of [false, true]) await t.test(running ? 'running installation' : 'stopped installation', async subtest => {
    const state = savedBuildObservationFixture({ builds: true, installed: true });
    const installed = deploymentFor(A, running ? { status: 'running', process_live: true, health: 'ready',
      console_url: 'http://127.0.0.1:18840/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } } : {});
    state.receipts.deployment.set(A, installed);
    const saved = clone(state.receipts.implementation.get(A));
    await mountedApp(subtest, state, async (document, tick) => {
      const installedAction = running ? 'Use your application' : 'Manage application';
      assert.equal(button(applicationStatus(document), installedAction).disabled, false);
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      const failedRead = state.schedule(`${urlFor(A)}/implementation`);
      await tick(6000); assert.equal(failedRead.started, true);
      await finish(failedRead, { httpError: 503, detail: 'Synthetic later build status failure.' });
      assert.equal(implementation(document).querySelector('h2').textContent, 'Build status is unavailable');
      assert.match(implementation(document).textContent, /Retained checked source A\./);
      const parent = applicationStatus(document);
      assert.match(parent.querySelector('h1, h2').textContent, running ? /installed.*running/ : /installed.*stopped/);
      assert.match(parent.textContent, /build status.*unavailable|could not check.*build status|builder receipt could not be checked/i);
      assert.equal(button(parent, installedAction).disabled, false);
      if (running) assert.equal(parent.querySelector('a[href="http://127.0.0.1:18840/console"]').textContent.trim(), 'Administration Console');
      assert.ok(![...parent.querySelectorAll('button')].some(node => /^Try (?:the|the newer) application$/.test(node.textContent.trim())),
        'Retained candidate checks do not recommend a new trial after refresh failed');
      assert.deepEqual(state.receipts.implementation.get(A), saved);
      assert.deepEqual(state.receipts.deployment.get(A), installed);
      assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
      assert.equal(state.feedback(A), feedback); assertComposer(document, state);
      assert.equal(state.posts().length, 0);
    });
  });
});

test('saved-build observation ignores prior A receipt, absence or failure after A-B-A selection while the new A read is pending', async t => {
  for (const result of ['receipt', 'absence', 'failure']) await t.test(result, async subtest => {
    const state = savedBuildObservationFixture();
    const firstA = state.schedule(`${urlFor(A)}/implementation`);
    await mountedApp(subtest, state, async document => {
      assert.equal(firstA.started, true); assertUnconfirmedSavedBuild(document, 'loading');
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      const openB = await beginSelection(document, state);
      await finish(openB, state.guides.get(B));
      assert.equal(selected(document), B);
      assert.match(applicationStatus(document).textContent, /Your application needs to be built/);
      const noteB = 'A separate unsent builder note for B.';
      await inputValue(document.getElementById(`implementation-feedback-${B}`), noteB);
      // The visible menu and newly selected panel both read this canonical URL.
      const nextA = state.schedule(`${urlFor(A)}/implementation`, 'GET', 2);
      const openA = await beginSelection(document, state, A);
      await finish(openA, state.guides.get(A));
      assert.equal(selected(document), A); assert.equal(nextA.started, true);
      assert.equal(nextA.startedCount, 2, 'Both menu and selected implementation reads remain on the same pending barrier.');
      assertUnconfirmedSavedBuild(document, 'loading');
      const stale = result === 'receipt' ? buildFor(A, { summary: 'STALE PRIOR A OBSERVATION' })
        : result === 'absence' ? absent('Old A read saw no build.')
          : { httpError: 503, detail: 'STALE PRIOR A OBSERVATION FAILURE' };
      await finish(firstA, stale);
      assertUnconfirmedSavedBuild(document, 'loading');
      assert.doesNotMatch(implementation(document).textContent, /STALE PRIOR A OBSERVATION/);
      assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
      assert.equal(state.feedback(B), noteB);
      const current = buildFor(A, { summary: 'CURRENT RETURNED A OBSERVATION' });
      state.receipts.implementation.set(A, current);
      await finish(nextA, current);
      assert.match(applicationStatus(document).textContent, /Your application passed independent offline checks/);
      assert.match(implementation(document).textContent, /CURRENT RETURNED A OBSERVATION/);
      assertComposer(document, state);
      assert.equal(state.posts().length, 0);
    });
  });
});

test('saved-build observation masks retained known status on same-guide token reload and ignores the prior read outcome', async t => {
  for (const result of ['receipt', 'failure']) await t.test(result, async subtest => {
    const state = savedBuildObservationFixture({ builds: true });
    await mountedApp(subtest, state, async (document, tick) => {
      assert.match(applicationStatus(document).textContent, /Your application passed independent offline checks/);
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      const refused = await beginSelection(document, state);
      await fail(refused, 'Synthetic B refusal makes explicit same-guide reload available.');
      const oldA = state.schedule(`${urlFor(A)}/implementation`);
      await tick(6000); assert.equal(oldA.started, true);
      const freshA = state.schedule(`${urlFor(A)}/implementation`);
      const reload = state.schedule(urlFor(A));
      await click(document, 'Reload saved conversation');
      assert.equal(reload.started, true);
      await finish(reload, state.guides.get(A));
      assert.equal(selected(document), A);
      assertUnconfirmedSavedBuild(document, 'loading');
      await tick(6000); assert.equal(freshA.started, true);
      await finish(oldA, result === 'receipt' ? buildFor(A, { summary: 'STALE SAME-GUIDE OBSERVATION' })
        : { httpError: 503, detail: 'STALE SAME-GUIDE OBSERVATION FAILURE' });
      assertUnconfirmedSavedBuild(document, 'loading');
      assert.doesNotMatch(implementation(document).textContent, /STALE SAME-GUIDE OBSERVATION/);
      assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
      const current = buildFor(A, { summary: 'CURRENT SAME-GUIDE OBSERVATION' });
      state.receipts.implementation.set(A, current);
      await finish(freshA, current);
      assert.match(applicationStatus(document).textContent, /Your application passed independent offline checks/);
      assert.match(implementation(document).textContent, /CURRENT SAME-GUIDE OBSERVATION/);
      assert.equal(state.feedback(A), feedback); assertComposer(document, state);
      assert.equal(state.posts().length, 0, 'Reload and late reads are observations, not builder admissions');
    });
  });
});
async function prepareDrafts(document) {
  await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
  await inputValue(document.getElementById(`trial-${A}`), sample);
}
async function openNewPrivateForm(document) {
  await click(deployment(document), 'Install into another folder');
  const form = document.querySelector('.deployment-form');
  assert.ok(form, 'The real installation form must open before testing its submit ownership');
  await inputValue(form.querySelector('[name="OPENAI_API_KEY"]'), privateValue);
  await inputValue(form.querySelector('[name="APP_USER"]'), privateUser);
  assert.equal(button(form, 'Install and start').disabled, false);
  return form;
}

const installedTask = 'Compare the synthetic forecast evidence for our observing plan.';
const installedChange = 'Explain which synthetic forecast source was missing from the answer.';
const installedOwner = (document, remote) => remote
  ? document.querySelector('.remote-installed-use') : deployment(document);
const improvementUrl = remote => remote
  ? `${urlFor(A)}/remote-deployment/feedback` : `${urlFor(A)}/deployment/feedback`;

async function prepareInstalledFeedback(document, remote) {
  const owner = installedOwner(document, remote);
  assert.ok(owner, 'The real installed-use owner must be mounted');
  const disclosure = owner.querySelector('.deployment-use-feedback-disclosure');
  assert.ok(disclosure, 'The actual installed feedback form must be available');
  await act(async () => { disclosure.querySelector('summary').click(); await settle(); });
  await inputValue(owner.querySelector('.deployment-use-field textarea'), installedTask);
  await act(async () => {
    owner.querySelector('.deployment-use-feedback input[value="partly"]').click(); await settle();
  });
  await inputValue(owner.querySelector('.deployment-use-feedback textarea'), installedChange);
  assert.equal(button(owner, remote ? 'Ask builder to improve it' : 'Improve this behavior').disabled, false);
}

function improvementTracking(state, remote) {
  const found = [...state.session.values.entries()].filter(([key]) =>
    key.includes('mobkit-installed-use-improvement-v1:') &&
    (remote ? key.startsWith('mobkit-remote-installed-use-v1:') : key.startsWith('mobkit-installed-use-improvement-v1:')));
  assert.equal(found.length, 1, 'One exact captured installed-source tracking namespace must exist');
  const [key, raw] = found[0];
  return { key, raw, saved: JSON.parse(raw) };
}

function admittedImprovement(remote, change = {}) {
  return buildFor(A, { build_id: remote ? 'remote-feedback-admission-a' : 'local-feedback-admission-a',
    status: 'running', ready_to_package: false, acceptance_current: false, acceptance_trusted: false,
    worker_stop_confirmed: false,
    request: remote ? 'Exact host-authored remote improvement request A.' : 'Exact host-authored local improvement request A.',
    summary: remote ? 'ADMITTED REMOTE INSTALLED IMPROVEMENT A' : 'ADMITTED LOCAL INSTALLED IMPROVEMENT A', ...change });
}

function assertInstalledDrafts(document, state, remote) {
  const owner = installedOwner(document, remote);
  assert.equal(owner.querySelector('.deployment-use-field textarea').value, installedTask);
  assert.equal(owner.querySelector('.deployment-use-feedback textarea').value, installedChange);
  assert.equal(owner.querySelector('.deployment-use-feedback input[value="partly"]').checked, true);
  assertComposer(document, state);
}

test('actual saved-setup selection synchronously holds captured Build, Try, runtime and private staging controls', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const controls = [button(implementation(document), 'Continue implementation'),
      button(document, 'Try agent design'), button(deployment(document), 'Start installed application'),
      button(deployment(document), 'Change private settings'), button(deployment(document), 'Install into another folder')];
    const readsBefore = state.calls.length;
    const waiting = await beginSelection(document, state, B, controls);
    assert.equal(selected(document), A, 'A remains displayed while B is not yet loaded');
    assert.equal(state.posts().length, 0, 'Captured A callbacks must not admit outgoing work after selection begins');
    assert.equal(state.calls.slice(readsBefore).some(call => call.url.includes('/deployment/settings?')), false,
      'Captured private staging must not read A settings after selection begins');
    assert.equal(document.querySelector('.deployment-form') === null, true);
    for (const control of controls) assert.equal(control.disabled, true, 'Rendered A lifecycle controls visibly pause while B loads');
    assertComposer(document, state);
    await finish(waiting, state.guides.get(B));
    assert.equal(selected(document), B);
    assert.equal(proposal(document).project.name, planFor(B).project.name);
    assert.equal(state.feedback(A), feedback);
    assert.equal(state.composer(A), unsent);
    assert.equal(state.posts().length, 0);
  });
});

test('a pre-opened real private form cannot submit through a captured handler and survives selected-guide refusal', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    const form = await openNewPrivateForm(document);
    const waiting = await beginSelection(document, state, B, [button(form, 'Install and start')]);
    assert.equal(state.posts().length, 0, 'Selection holds an actual private installation submit, not just its opener');
    assert.equal(form.querySelector('[name="OPENAI_API_KEY"]').value, privateValue);
    assert.equal(form.querySelector('[name="APP_USER"]').value, privateUser);
    assert.equal(button(form, 'Install and start').disabled, true);
    await fail(waiting, 'Synthetic B selection refused.');
    assert.equal(selected(document), A);
    const retained = document.querySelector('.deployment-form');
    assert.ok(retained, 'Refused B selection keeps A private staging in this page');
    assert.equal(retained.querySelector('[name="OPENAI_API_KEY"]').value, privateValue);
    assert.equal(retained.querySelector('[name="APP_USER"]').value, privateUser);
    assert.equal(button(retained, 'Install and start').disabled, false, 'A fresh explicit submit can be captured after refusal');
    assert.equal(state.posts().length, 0, 'Recovery never automatically sends the retained credentials');
    assert.equal([...state.storage.values.values()].some(value => value.includes(privateValue)), false,
      'Private staging does not enter persistent browser storage');
    assertComposer(document, state);
  });
});

test('an admitted A builder response settling before B loads cannot publish its receipt or consume human feedback', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const admission = state.schedule(`${urlFor(A)}/implementation`, 'POST');
    await click(implementation(document), 'Continue implementation');
    assert.equal(admission.started, true);
    assert.deepEqual(state.posts()[0].body, { version: 5, request: feedback, previous_build_id: buildFor(A).build_id });
    const waiting = await beginSelection(document, state);
    await finish(admission, buildFor(A, { build_id: 'new-admitted-build-a', status: 'running',
      ready_to_package: false, summary: 'STALE A BUILDER PUBLICATION' }));
    assert.equal(selected(document), A);
    assert.doesNotMatch(implementation(document).textContent, /STALE A BUILDER PUBLICATION/);
    assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
    assert.equal(state.feedback(A), feedback);
    assertComposer(document, state);
    await finish(waiting, state.guides.get(B));
    assert.equal(selected(document), B);
    assert.doesNotMatch(implementation(document).textContent, /STALE A BUILDER PUBLICATION/);
    assert.equal(state.posts().length, 1, 'The already admitted A operation stays intact and is never resent or canceled');
  });
});

test('an admitted A trial response settling before B loads cannot consume the sample or show an open trial', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const admission = state.schedule(`${urlFor(A)}/rehearsal`, 'POST');
    await click(document, 'Try agent design');
    assert.equal(admission.started, true);
    assert.deepEqual(state.posts()[0].body, { version: 5, text: sample, mode: 'design', keep_open: true });
    const savedDraft = [...state.session.values.entries()];
    const waiting = await beginSelection(document, state);
    await finish(admission, trialFor(A, { turns: [{ identity: 'planner:main', status: 'completed',
      result: 'STALE A TRIAL PUBLICATION' }] }));
    assert.equal(selected(document), A);
    assert.equal(document.getElementById(`trial-${A}`).value, sample);
    assert.deepEqual([...state.session.values.entries()], savedDraft, 'A stale acknowledgment cannot clear the exact saved sample');
    assert.equal([...document.querySelectorAll('button')].some(node => node.textContent.trim() === 'End trial'), false);
    assert.doesNotMatch(document.querySelector('.rehearsal-panel').textContent, /STALE A TRIAL PUBLICATION/);
    assertComposer(document, state);
    await finish(waiting, state.guides.get(B));
    assert.equal(selected(document), B);
    assert.equal(state.posts().length, 1, 'The admitted A trial is retained without an automatic end or resend');
  });
});

test('a private settings read started on A cannot open its form after B selection begins', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    const options = state.schedule(`${urlFor(A)}/deployment/settings?deployment_id=${deploymentFor(A).deployment_id}`);
    await click(deployment(document), 'Change private settings');
    assert.equal(options.started, true);
    const waiting = await beginSelection(document, state);
    const stale = optionsFor(A, true);
    stale.fields[0].label = 'STALE PRIVATE SETTINGS FROM A';
    await finish(options, stale);
    assert.equal(selected(document), A);
    assert.equal(document.querySelector('.deployment-form') === null, true, 'A late settings response must not mount its private form');
    assert.doesNotMatch(deployment(document).textContent, /STALE PRIVATE SETTINGS FROM A/);
    await finish(waiting, state.guides.get(B));
    assert.equal(selected(document), B);
    assert.equal(document.querySelector('.deployment-form') === null, true);
    assert.equal(state.posts().length, 0);
    assert.equal(state.composer(A), unsent);
  });
});

test('a failed B selection releases fresh A runtime actions but not earlier promises, and retries B explicitly', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    const oldStart = state.schedule(`${urlFor(A)}/deployment/action`, 'POST');
    await click(deployment(document), 'Start installed application');
    assert.equal(oldStart.started, true);
    const oldBody = state.posts()[0].body, installed = deploymentFor(A);
    const canonicalRequestId = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
    assert.match(oldBody.action_request_id, canonicalRequestId);
    assert.deepEqual(oldBody, { deployment_id: installed.deployment_id, action: 'restart',
      action_request_id: oldBody.action_request_id });
    const actionMetadata = (body, phase, outcome = null) => ({ version: 1,
      action_request_id: body.action_request_id, guide_id: A, deployment_id: installed.deployment_id,
      action: 'restart', update_id: null, phase, outcome });
    const observeUrl = `${urlFor(A)}/deployment/action/${installed.deployment_id}/${oldBody.action_request_id}`;
    const waiting = await beginSelection(document, state);
    await fail(oldStart, 'STALE A RESTART FAILURE');
    assert.doesNotMatch(deployment(document).textContent, /STALE A RESTART FAILURE/);
    assert.equal(state.posts().length, 1);
    const returnedRead = state.schedule(observeUrl);
    await fail(waiting, 'Synthetic B selection refused.');
    assert.equal(returnedRead.started, true, 'Returning to A observes its exact retained command without replay.');
    await finish(returnedRead, { action_request: actionMetadata(oldBody, 'pending') });
    assert.equal(selected(document), A);
    const failureText = [...document.querySelectorAll('.setup-error')].map(node => node.textContent).join('\n');
    assert.match(failureText, /Synthetic B selection refused/);
    assert.match(failureText, /Observing crew A/);
    assert.equal(button(document, 'Retry selected setup').disabled, false);
    assert.equal(button(deployment(document), 'Start installed application').disabled, true,
      'A failed B selection does not resolve the earlier A command admission.');
    await act(async () => { button(deployment(document), 'Start installed application').click(); await settle(); });
    assert.equal(state.posts().length, 1, 'The uncertain earlier Start cannot dispatch a second POST.');
    for (const phase of ['pending', 'unknown']) {
      const observation = state.schedule(observeUrl);
      const before = state.calls.length;
      await click(deployment(document), 'Check installation status');
      assert.equal(observation.started, true, 'Recovery reads the exact retained command identity.');
      await finish(observation, { action_request: actionMetadata(oldBody, phase) });
      assert.deepEqual(state.calls.slice(before).filter(call => call.url === observeUrl || call.url === `${urlFor(A)}/deployment`)
        .map(call => [call.method, call.url]), [['GET', observeUrl], ['GET', `${urlFor(A)}/deployment`]],
      'Exact command readback and the ordinary stopped receipt remain observation only.');
      assert.equal(button(deployment(document), 'Start installed application').disabled, true,
        `An unchanged stopped receipt cannot resolve a ${phase} command.`);
      await act(async () => { button(deployment(document), 'Start installed application').click(); await settle(); });
      assert.equal(state.posts().length, 1);
      assert.doesNotMatch(deployment(document).textContent, /STALE A RESTART FAILURE/);
    }
    const settlement = state.schedule(observeUrl);
    await click(deployment(document), 'Check installation status');
    assert.equal(settlement.started, true);
    await finish(settlement, { action_request: actionMetadata(oldBody, 'settled', 'error') });
    assert.equal(button(deployment(document), 'Start installed application').disabled, false,
      'Exact terminal command evidence and a fresh stopped receipt permit a new explicit action.');
    assert.equal(state.posts().length, 1, 'Settlement observation never replays the original Start.');
    const freshStart = state.schedule(`${urlFor(A)}/deployment/action`, 'POST');
    await click(deployment(document), 'Start installed application');
    assert.equal(freshStart.started, true, 'A distinct explicit Start is admitted only after the old command settled.');
    const freshBody = state.posts()[1].body;
    assert.match(freshBody.action_request_id, canonicalRequestId);
    assert.notEqual(freshBody.action_request_id, oldBody.action_request_id,
      'A genuinely new human command has its own identity rather than replaying the lost command.');
    assert.deepEqual(state.posts().map(call => call.body), [
      { deployment_id: installed.deployment_id, action: 'restart', action_request_id: oldBody.action_request_id },
      { deployment_id: installed.deployment_id, action: 'restart', action_request_id: freshBody.action_request_id }]);
    const started = deploymentFor(A, { status: 'running', process_live: true, detail: 'FRESH A RESTART ACKNOWLEDGED' });
    state.receipts.deployment.set(A, started);
    const freshCommandRead = state.schedule(`${urlFor(A)}/deployment/action/${installed.deployment_id}/${freshBody.action_request_id}`);
    await finish(freshStart, { ...started, action_request: actionMetadata(freshBody, 'settled', 'returned') });
    assert.equal(freshCommandRead.started, true, 'The new explicit Start keeps its own exact completion check.');
    assert.equal(button(deployment(document), 'Start installed application').disabled, true,
      'The earlier command settlement cannot unlock a newer command awaiting its readback.');
    await finish(freshCommandRead, { action_request: actionMetadata(freshBody, 'settled', 'returned') });
    assert.match(deployment(document).textContent, /FRESH A RESTART ACKNOWLEDGED/);
    const retry = state.schedule(urlFor(B));
    await click(document, 'Retry selected setup');
    assert.equal(retry.started, true, 'Only the explicit retry reattempts the original B selection');
    assert.equal(state.posts().length, 2, 'Retrying selection does not dispatch any application work');
    assert.equal(button(deployment(document), 'Stop application').disabled, true);
    assertComposer(document, state);
    await finish(retry, state.guides.get(B));
    assert.equal(selected(document), B);
    assert.doesNotMatch(deployment(document).textContent, /FRESH A RESTART ACKNOWLEDGED/);
    assert.equal(state.composer(A), unsent);
  });
});

test('a retired installation response cannot release a newer same-guide operation or its exact command custody', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    const oldStart = state.schedule(`${urlFor(A)}/deployment/action`, 'POST');
    await click(deployment(document), 'Start installed application');
    assert.equal(oldStart.started, true);
    const oldBody = state.posts()[0].body, installed = deploymentFor(A);
    const metadata = (body, outcome) => ({ version: 1,
      action_request_id: body.action_request_id, guide_id: A, deployment_id: installed.deployment_id,
      action: 'restart', update_id: null, phase: 'settled', outcome });
    const waiting = await beginSelection(document, state);
    const oldObservation = state.schedule(`${urlFor(A)}/deployment/action/${installed.deployment_id}/${oldBody.action_request_id}`);
    await fail(waiting, 'Synthetic B selection refused while A response remains held.');
    assert.equal(oldObservation.started, true);
    assert.equal(selected(document), A);
    assert.equal(button(deployment(document), 'Start installed application').disabled, true,
      'Retiring local busy ownership cannot release an unresolved saved command.');
    await finish(oldObservation, { action_request: metadata(oldBody, 'error') });
    assert.equal(button(deployment(document), 'Start installed application').disabled, false,
      'A checked exact settlement and fresh stopped receipt permit a new explicit action.');
    assert.equal(state.posts().length, 1, 'Observation never resends the retained Start.');

    const freshStart = state.schedule(`${urlFor(A)}/deployment/action`, 'POST');
    await click(deployment(document), 'Start installed application');
    assert.equal(freshStart.started, true);
    const freshBody = state.posts()[1].body;
    assert.notEqual(freshBody.action_request_id, oldBody.action_request_id);
    const commandKey = `meta-forge-deployment-action-v1:${A}`;
    const freshIntent = state.session.getItem(commandKey);
    assert.equal(JSON.parse(freshIntent).action_request_id, freshBody.action_request_id);
    const sourceRadios = () => [...deployment(document).querySelectorAll('input[name="deployment-source"]')];
    assert.equal(sourceRadios().length, 2);
    assert.ok(sourceRadios().every(node => node.disabled), 'The actual newer operation owns local busy controls.');

    await fail(oldStart, 'STALE RETIRED A TRANSPORT FAILURE');
    assert.ok(sourceRadios().every(node => node.disabled),
      'The old finally cannot release the newer same-guide operation.');
    assert.equal(state.session.getItem(commandKey), freshIntent,
      'A retired response cannot consume or replace the newer exact command identity.');
    assert.doesNotMatch(deployment(document).textContent, /STALE RETIRED A TRANSPORT FAILURE/);
    await act(async () => { button(deployment(document), 'Start installed application').click(); await settle(); });
    assert.equal(state.posts().length, 2, 'The pending newer operation cannot admit a duplicate command.');
    assertComposer(document, state);

    const running = checkedRunningInstallation(); state.receipts.deployment.set(A, running);
    const freshCommandRead = state.schedule(`${urlFor(A)}/deployment/action/${installed.deployment_id}/${freshBody.action_request_id}`);
    const freshReceiptRead = state.schedule(`${urlFor(A)}/deployment`);
    await finish(freshStart, { ...running, action_request: metadata(freshBody, 'returned') });
    assert.ok(sourceRadios().every(node => !node.disabled), 'Only the newer operation retires its own local busy state.');
    assert.equal(freshCommandRead.started, true);
    assert.equal(freshReceiptRead.started, false);
    assert.equal(button(deployment(document), 'Start installed application').disabled, true,
      'Retiring local busy ownership still requires the newer exact command and installation readbacks.');
    await finish(freshCommandRead, { action_request: metadata(freshBody, 'returned') });
    assert.equal(freshReceiptRead.started, true);
    assert.equal(button(deployment(document), 'Start installed application').disabled, true,
      'Exact command settlement alone cannot establish the current running installation.');
    await finish(freshReceiptRead, running);
    assert.equal(button(deployment(document), 'Stop application').disabled, false);
    assert.equal(state.session.getItem(commandKey), null);
    assert.equal(state.posts().length, 2);
    assertComposer(document, state);
  });
});

test('same-owner Reload saved conversation invalidates a prior builder response while keeping its note', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const oldBuild = state.schedule(`${urlFor(A)}/implementation`, 'POST');
    await click(implementation(document), 'Continue implementation');
    const refused = await beginSelection(document, state);
    await fail(refused, 'Synthetic B selection refused.');
    const reload = state.schedule(urlFor(A));
    await click(document, 'Reload saved conversation');
    assert.equal(reload.started, true, 'Reload uses the retained saved owner A');
    await finish(oldBuild, buildFor(A, { build_id: 'old-build-before-reload', status: 'running',
      ready_to_package: false, summary: 'STALE SAME-OWNER RELOAD PUBLICATION' }));
    assert.equal(selected(document), A);
    assert.doesNotMatch(implementation(document).textContent, /STALE SAME-OWNER RELOAD PUBLICATION/);
    assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
    await finish(reload, state.guides.get(A));
    assert.equal(selected(document), A);
    assert.equal(button(implementation(document), 'Continue implementation').disabled, false);
    assert.equal(state.feedback(A), feedback);
    assertComposer(document, state);
    assert.equal(state.posts().length, 1, 'Reload is observation only and cannot repeat or cancel the old admission');
  });
});

test('A-B-A navigation never makes an old A builder promise current again and retains each owner draft', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const oldBuild = state.schedule(`${urlFor(A)}/implementation`, 'POST');
    await click(implementation(document), 'Continue implementation');
    const openB = await beginSelection(document, state);
    await finish(openB, state.guides.get(B));
    assert.equal(selected(document), B);
    const noteB = 'Separate unsent implementation note for B.';
    const composerB = '  Unsent idea belonging only to B.  ';
    await inputValue(document.getElementById(`implementation-feedback-${B}`), noteB);
    await inputValue(document.querySelector('#setup-message'), composerB);
    const openA = await beginSelection(document, state, A);
    await finish(oldBuild, buildFor(A, { build_id: 'old-original-a-build', status: 'running',
      ready_to_package: false, summary: 'STALE A-B-A PUBLICATION' }));
    assert.equal(selected(document), B);
    assert.doesNotMatch(implementation(document).textContent, /STALE A-B-A PUBLICATION/);
    assert.equal(document.getElementById(`implementation-feedback-${B}`).value, noteB);
    assertComposer(document, state, B, composerB);
    await finish(openA, state.guides.get(A));
    assert.equal(selected(document), A);
    assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
    assert.equal(document.getElementById(`trial-${A}`).value, sample);
    assert.doesNotMatch(implementation(document).textContent, /STALE A-B-A PUBLICATION/);
    assert.equal(state.feedback(B), noteB);
    assert.equal(state.composer(B), composerB);
    assertComposer(document, state);
    assert.equal(state.posts().length, 1);
  });
});

test('selecting another setup holds real Stop and End trial actions without stopping admitted A work', async t => {
  for (const keepOpen of [true, false]) {
    await t.test(keepOpen ? 'open continuing trial End' : 'non-open active rehearsal Stop', async subtest => {
      const state = fixture({ active: true });
      if (!keepOpen) state.receipts.rehearsal.set(A, trialFor(A, {
        keep_open: false, status: 'running', session_status: 'running', can_continue: false,
        turns: [{ identity: 'planner:main', status: 'running' }],
      }));
      await mountedApp(subtest, state, async (document, tick) => {
        const trialReadsBefore = state.calls.filter(call => call.method === 'GET' && call.url === `${urlFor(A)}/rehearsal`).length;
        await tick(1);
        assert.ok(state.calls.filter(call => call.method === 'GET' && call.url === `${urlFor(A)}/rehearsal`).length > trialReadsBefore,
          'The real scheduled status poll must adopt the saved active trial before its controls are captured');
        const trialControl = keepOpen ? 'End trial' : 'Stop rehearsal';
        const labels = ['Stop build', trialControl, 'Stop application'];
        const controls = labels.map(label => button(document, label));
        const retainedBuild = clone(state.receipts.implementation.get(A));
        const retainedTrial = clone(state.receipts.rehearsal.get(A));
        const retainedRuntime = clone(state.receipts.deployment.get(A));
        const waiting = await beginSelection(document, state, B, controls);
        assert.equal(state.posts().length, 0, 'Selecting B must not admit new cleanup or cancel already admitted A work');
        for (const control of controls) assert.equal(control.disabled, true);
        assert.deepEqual(state.receipts.implementation.get(A), retainedBuild);
        assert.deepEqual(state.receipts.rehearsal.get(A), retainedTrial);
        assert.deepEqual(state.receipts.deployment.get(A), retainedRuntime);
        await fail(waiting, 'Synthetic B selection refused.');
        assert.equal(selected(document), A);
        for (const label of labels)
          assert.equal(button(document, label).disabled, false, 'Freshly captured A owned cleanup becomes available again');
        assert.equal(state.posts().length, 0, 'Recovery alone never stops any existing operation');
        assertComposer(document, state);
      });
    });
  }
});

test('passive A lifecycle reads started before B selection cannot publish while the selected B read is pending', async t => {
  const state = fixture();
  await mountedApp(t, state, async (document, tick) => {
    await prepareDrafts(document);
    const builderRead = state.schedule(`${urlFor(A)}/implementation`);
    const trialRead = state.schedule(`${urlFor(A)}/rehearsal`);
    const runtimeRead = state.schedule(`${urlFor(A)}/deployment`);
    await tick(6000);
    for (const read of [builderRead, trialRead, runtimeRead])
      assert.equal(read.started, true, 'Real panel polling must begin before the next selection');
    const waiting = await beginSelection(document, state);
    await finish(builderRead, buildFor(A, { build_id: 'passive-late-build-a', status: 'running',
      ready_to_package: false, summary: 'STALE PASSIVE A BUILD' }));
    await finish(trialRead, trialFor(A, { turns: [{ identity: 'planner:main', status: 'completed',
      result: 'STALE PASSIVE A TRIAL' }] }));
    await finish(runtimeRead, deploymentFor(A, { status: 'running', process_live: true,
      detail: 'STALE PASSIVE A RUNTIME' }));
    assert.equal(selected(document), A, 'The selected B read is still unresolved');
    assert.doesNotMatch(implementation(document).textContent, /STALE PASSIVE A BUILD/);
    assert.doesNotMatch(document.querySelector('.rehearsal-panel').textContent, /STALE PASSIVE A TRIAL/);
    assert.doesNotMatch(deployment(document).textContent, /STALE PASSIVE A RUNTIME/);
    assert.equal(deployment(document).dataset.deploymentStatus, 'stopped');
    assert.equal(button(implementation(document), 'Continue implementation').disabled, true);
    assert.equal(button(document, 'Try agent design').disabled, true);
    assert.equal(button(deployment(document), 'Start installed application').disabled, true);
    assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
    assert.equal(document.getElementById(`trial-${A}`).value, sample);
    assertComposer(document, state);
    await finish(waiting, state.guides.get(B));
    assert.equal(selected(document), B);
    assert.equal(proposal(document).project.name, planFor(B).project.name);
    assert.equal(state.posts().length, 0, 'Observing saved jobs and navigating never dispatch or cancel them');
  });
});

test('a pending A Build neither blocks B Build nor releases B request ownership when A later settles', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const oldA = state.schedule(`${urlFor(A)}/implementation`, 'POST');
    await click(implementation(document), 'Continue implementation');
    assert.equal(oldA.started, true);
    const openB = await beginSelection(document, state);
    await finish(openB, state.guides.get(B));
    assert.equal(selected(document), B);
    const noteB = 'Human implementation request belonging only to B.';
    const composerB = '  An unrelated unsent B thought.  ';
    await inputValue(document.getElementById(`implementation-feedback-${B}`), noteB);
    await inputValue(document.querySelector('#setup-message'), composerB);
    const ownB = state.schedule(`${urlFor(B)}/implementation`, 'POST');
    await click(implementation(document), 'Continue implementation');
    assert.equal(ownB.started, true, 'B can admit its own Build while the earlier A response remains unresolved');
    assert.deepEqual(state.posts().map(call => ({ url: call.url, body: call.body })), [
      { url: `${urlFor(A)}/implementation`, body: { version: 5, request: feedback, previous_build_id: buildFor(A).build_id } },
      { url: `${urlFor(B)}/implementation`, body: { version: 5, request: noteB, previous_build_id: buildFor(B).build_id } }]);
    await finish(oldA, buildFor(A, { build_id: 'late-build-a-while-b-pending', status: 'running',
      ready_to_package: false, summary: 'STALE A BUILD WHILE B IS PENDING' }));
    assert.equal(selected(document), B);
    assert.equal(button(implementation(document), 'Starting builder...').disabled, true,
      'A finally cannot release the exact in-flight B submission or allow a duplicate B admission');
    assert.doesNotMatch(implementation(document).textContent, /STALE A BUILD WHILE B IS PENDING/);
    assert.equal(document.getElementById(`implementation-feedback-${B}`).value, noteB);
    assert.equal(state.feedback(B), noteB);
    assert.equal(state.feedback(A), feedback);
    assertComposer(document, state, B, composerB);
    const acceptedB = buildFor(B, { build_id: 'new-admitted-build-b', status: 'running', ready_to_package: false,
      worker_stop_confirmed: false, acceptance_current: false, acceptance_trusted: false,
      summary: 'CURRENT B BUILD ADMISSION' });
    state.receipts.implementation.set(B, acceptedB);
    await finish(ownB, acceptedB);
    assert.match(implementation(document).textContent, /CURRENT B BUILD ADMISSION/);
    assert.doesNotMatch(implementation(document).textContent, /STALE A BUILD WHILE B IS PENDING/);
    assert.equal(document.getElementById(`implementation-feedback-${B}`).value, '');
    assert.equal(state.feedback(B), null, 'Only B own exact response consumes its submitted feedback');
    assert.equal(state.feedback(A), feedback);
    assert.equal(state.posts().length, 2);
    assertComposer(document, state, B, composerB);
  });
});

test('a pending A Try neither blocks B Try nor releases B sample ownership when A later settles', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const savedA = [...state.session.values.entries()];
    const oldA = state.schedule(`${urlFor(A)}/rehearsal`, 'POST');
    await click(document, 'Try agent design');
    assert.equal(oldA.started, true);
    const openB = await beginSelection(document, state);
    await finish(openB, state.guides.get(B));
    const sampleB = 'A distinct synthetic request for observing crew B.';
    const composerB = '  B has another unsent idea.  ';
    await inputValue(document.getElementById(`trial-${B}`), sampleB);
    await inputValue(document.querySelector('#setup-message'), composerB);
    const savedB = [...state.session.values.entries()].find(([key, raw]) =>
      key.startsWith('mobkit-trial-draft-v1:') && JSON.parse(raw)?.owner?.guide_id === B &&
      JSON.parse(raw)?.draft?.sample === sampleB);
    assert.ok(savedB, 'B sample must be retained by the actual rehearsal draft store before admission');
    const ownB = state.schedule(`${urlFor(B)}/rehearsal`, 'POST');
    await click(document, 'Try agent design');
    assert.equal(ownB.started, true, 'B can admit its own Try while the earlier A response remains unresolved');
    assert.deepEqual(state.posts().map(call => ({ url: call.url, body: call.body })), [
      { url: `${urlFor(A)}/rehearsal`, body: { version: 5, text: sample, mode: 'design', keep_open: true } },
      { url: `${urlFor(B)}/rehearsal`, body: { version: 5, text: sampleB, mode: 'design', keep_open: true } }]);
    await finish(oldA, trialFor(A, { rehearsal_id: 'late-trial-a-while-b-pending',
      turns: [{ identity: 'planner:main', status: 'completed', result: 'STALE A TRIAL WHILE B IS PENDING' }] }));
    assert.equal(selected(document), B);
    assert.equal(button(document, 'Starting rehearsal...').disabled, true,
      'A finally cannot release B in-flight Try or admit a duplicate B request');
    assert.equal(document.getElementById(`trial-${B}`).value, sampleB);
    assert.equal(state.session.getItem(savedB[0]), savedB[1], 'A finally cannot consume B saved sample');
    assert.doesNotMatch(document.querySelector('.rehearsal-panel').textContent, /STALE A TRIAL WHILE B IS PENDING/);
    for (const [key, value] of savedA)
      assert.equal(state.session.getItem(key), value, 'A stale response leaves its exact saved sample intact');
    assertComposer(document, state, B, composerB);
    const acceptedB = trialFor(B, { rehearsal_id: 'new-admitted-trial-b', text: sampleB,
      turns: [{ identity: 'planner:main', status: 'completed', result: 'CURRENT B TRIAL ANSWER' }] });
    state.receipts.rehearsal.set(B, acceptedB);
    await finish(ownB, acceptedB);
    assert.equal(document.getElementById(`trial-${B}`) === null, true, 'B own exact admission opens its retained follow-up area');
    assert.ok(document.getElementById('trial-follow-up-new-admitted-trial-b'));
    assert.equal(state.session.getItem(savedB[0]), null, 'Only B own exact admission consumes its submitted sample');
    assert.equal(document.getElementById('trial-follow-up-late-trial-a-while-b-pending') === null, true);
    for (const [key, value] of savedA) assert.equal(state.session.getItem(key), value);
    assert.equal(state.posts().length, 2, 'Neither navigation nor stale settlement resends or ends either trial');
    assertComposer(document, state, B, composerB);
  });
});

test('real saved remote browser Close is held during selection and its earlier response cannot publish before B loads', async t => {
  const state = fixture({ remote: true });
  await mountedApp(t, state, async document => {
    await prepareDrafts(document);
    const remotePanel = () => document.querySelector('.remote-browser-access');
    assert.equal(remotePanel().dataset.browserStatus, 'ready');
    const close = button(remotePanel(), 'Close browser connection');
    const waiting = await beginSelection(document, state, B, [close]);
    assert.equal(state.posts().length, 0, 'A captured owned connection cleanup is not newly admitted after selection begins');
    assert.equal(close.disabled, true);
    await fail(waiting, 'Synthetic B selection refused.');
    assert.equal(selected(document), A);
    const closeResponse = state.schedule(`${urlFor(A)}/remote-deployment/browser-access/close`, 'POST');
    await click(remotePanel(), 'Close browser connection');
    assert.equal(closeResponse.started, true, 'Fresh deliberate A cleanup is available after selected-guide refusal');
    assert.deepEqual(state.posts()[0].body, { deployment_id: remoteFor(A).deployment_id,
      access_id: remoteBrowserFor(A).access_id });
    const retry = state.schedule(urlFor(B));
    await click(document, 'Retry selected setup');
    assert.equal(retry.started, true);
    const before = state.calls.length;
    await finish(closeResponse, remoteBrowserFor(A, { status: 'closed', local_url: null,
      native_ready: false, cleanup_confirmed: true }));
    assert.equal(selected(document), A, 'B selection remains unresolved when the earlier A cleanup response arrives');
    assert.equal(remotePanel().dataset.browserStatus, 'ready', 'The stale A response does not publish a closed observation');
    assert.equal(remotePanel().getAttribute('aria-disabled'), 'true');
    assert.equal(state.calls.slice(before).some(call => call.url.includes('/remote-deployment/browser-access/open')), false);
    assert.equal(state.posts().length, 1, 'No automatic reconnect, resend or application stop accompanies this stale response');
    assert.equal(document.getElementById(`implementation-feedback-${A}`).value, feedback);
    assert.equal(document.getElementById(`trial-${A}`).value, sample);
    assertComposer(document, state);
    await finish(retry, state.guides.get(B));
    assert.equal(selected(document), B);
    assert.equal(remotePanel() === null, true, 'B does not inherit A owned browser connection');
    assert.equal(state.posts().length, 1);
    assert.equal(state.feedback(A), feedback);
    assert.equal(state.composer(A), unsent);
  });
});

for (const remote of [false, true]) {
  test(`an admitted A ${remote ? 'remote' : 'local'} installed improvement survives full unmount and returns through fresh status without replay`, async t => {
    const state = fixture({ localUse: !remote, remote, remoteUse: remote });
    await mountedApp(t, state, async document => {
      await prepareInstalledFeedback(document, remote);
      const originalOwner = installedOwner(document, remote);
      const originalView = remote ? originalOwner : originalOwner.querySelector('.deployment-use');
      const admission = state.schedule(improvementUrl(remote), 'POST');
      await click(originalOwner, remote ? 'Ask builder to improve it' : 'Improve this behavior');
      assert.equal(admission.started, true);
      const pending = improvementTracking(state, remote);
      assert.equal(pending.saved.hint.buildId, null);
      assert.equal(pending.saved.hint.hostRequest, null);
      assert.equal(pending.saved.hint.guideId, A);
      assert.equal(pending.saved.hint.task, installedTask);
      assert.equal(pending.saved.hint.request, installedChange);
      const body = state.posts()[0].body;
      assert.equal(body.version, 5);
      assert.equal(body.continuation_build_id, buildFor(A).build_id);
      assert.equal(body.continuation_source_digest, buildFor(A).source_digest);
      assert.equal(body.task, installedTask);
      assert.equal(body.outcome, 'partly');
      assert.equal(body.request, installedChange);
      if (remote) assert.deepEqual(body.remote_binding, remoteUseFor(A).remote_binding);
      else assert.equal(body.deployment_id, deploymentFor(A).deployment_id);
      const openB = await beginSelection(document, state);
      await finish(openB, state.guides.get(B));
      assert.equal(selected(document), B);
      assert.equal(originalView.isConnected, false, 'A installed-use component is fully unmounted before its admission response settles');
      const composerB = '  B draft stays separate from A installed feedback.  ';
      await inputValue(document.querySelector('#setup-message'), composerB);
      const accepted = admittedImprovement(remote);
      await finish(admission, accepted);
      const retained = improvementTracking(state, remote);
      assert.equal(retained.key, pending.key, 'Confirmation retains the original local or remote source namespace');
      assert.deepEqual(retained.saved.hint, { ...pending.saved.hint, buildId: accepted.build_id, hostRequest: accepted.request });
      assert.equal(selected(document), B);
      assert.equal(document.querySelector('.installed-improvement-handoff') === null, true);
      assert.doesNotMatch(implementation(document).textContent, /ADMITTED (?:LOCAL|REMOTE) INSTALLED IMPROVEMENT A/);
      assertComposer(document, state, B, composerB);
      state.receipts.implementation.set(A, accepted);
      const readsBefore = state.calls.length;
      const returnA = await beginSelection(document, state, A);
      await finish(returnA, state.guides.get(A));
      assert.equal(selected(document), A);
      assert.equal(installedOwner(document, remote).querySelector('.installed-improvement-handoff')?.dataset.buildId,
        accepted.build_id, 'Remounted A reconciles its exact saved admission with a fresh host observation');
      assert.ok(state.calls.slice(readsBefore).some(call => call.method === 'GET' && call.url === `${urlFor(A)}/implementation`));
      assert.ok(state.calls.slice(readsBefore).some(call => call.method === 'GET' && call.url === (remote
        ? `${urlFor(A)}/remote-deployment/use?deployment_id=${remoteFor(A).deployment_id}` : `${urlFor(A)}/deployment/use`)));
      assertInstalledDrafts(document, state, remote);
      assert.equal(state.posts().length, 1, 'Fresh observation restores the admission without resending feedback or applying an update');
      assert.equal(state.composer(B), composerB);
    });
  });
}

test('local installed feedback remounted before confirmation recovers its exact stored admission through Check Build without another reload', async t => {
  const state = fixture({ localUse: true });
  await mountedApp(t, state, async document => {
    await prepareInstalledFeedback(document, false);
    const originalOwner = installedOwner(document, false);
    const originalView = originalOwner.querySelector('.deployment-use');
    const admission = state.schedule(improvementUrl(false), 'POST');
    await click(originalOwner, 'Improve this behavior');
    const pending = improvementTracking(state, false);
    const openB = await beginSelection(document, state);
    await finish(openB, state.guides.get(B));
    assert.equal(originalView.isConnected, false);
    const returnA = await beginSelection(document, state, A);
    await finish(returnA, state.guides.get(A));
    assert.equal(selected(document), A);
    assert.equal(installedOwner(document, false).querySelector('.installed-improvement-handoff') === null, true);
    assert.match(installedOwner(document, false).textContent, /Admission of your improvement request is unconfirmed/);
    const accepted = admittedImprovement(false);
    state.receipts.implementation.set(A, accepted);
    await finish(admission, accepted);
    assert.deepEqual(improvementTracking(state, false).saved.hint,
      { ...pending.saved.hint, buildId: accepted.build_id, hostRequest: accepted.request });
    assert.equal(installedOwner(document, false).querySelector('.installed-improvement-handoff') === null, true,
      'The retired response confirms storage without publishing into the newly mounted A view');
    const readsBefore = state.calls.length;
    await click(installedOwner(document, false), 'Check Build status');
    assert.equal(installedOwner(document, false).querySelector('.installed-improvement-handoff')?.dataset.buildId, accepted.build_id);
    assert.ok(state.calls.slice(readsBefore).some(call => call.method === 'GET' && call.url === `${urlFor(A)}/implementation`));
    assert.equal(state.calls.slice(readsBefore).some(call => call.url === urlFor(A)), false,
      'The existing Check Build action needs no extra guide reload');
    assertInstalledDrafts(document, state, false);
    assert.equal(state.posts().length, 1, 'Recovery performs no feedback replay or application action');
  });
});

for (const replacement of ['newer', 'corrupt']) {
  test(`an unmounted local feedback response preserves ${replacement} retained same-owner tracking`, async t => {
    const state = fixture({ localUse: true });
    await mountedApp(t, state, async document => {
      await prepareInstalledFeedback(document, false);
      const originalOwner = installedOwner(document, false);
      const originalView = originalOwner.querySelector('.deployment-use');
      const admission = state.schedule(improvementUrl(false), 'POST');
      await click(originalOwner, 'Improve this behavior');
      const pending = improvementTracking(state, false);
      const openB = await beginSelection(document, state);
      await finish(openB, state.guides.get(B));
      assert.equal(originalView.isConnected, false);
      const changed = replacement === 'corrupt' ? '{retained-unreadable-tracking' : JSON.stringify({
        ...pending.saved, hint: { ...pending.saved.hint, task: 'A distinct later synthetic report.',
          request: 'Keep this later report rather than the earlier response.',
          buildId: 'newer-retained-improvement-a', hostRequest: 'Exact later host-authored request A.' } });
      state.session.setItem(pending.key, changed);
      await finish(admission, admittedImprovement(false));
      assert.equal(state.session.getItem(pending.key), changed,
        'Fresh captured-namespace read prevents an older response from overwriting newer or damaged tracking');
      assert.equal(selected(document), B);
      assert.equal(document.querySelector('.installed-improvement-handoff') === null, true);
      const returnA = await beginSelection(document, state, A);
      await finish(returnA, state.guides.get(A));
      assert.equal(selected(document), A);
      assert.equal(state.session.getItem(pending.key), changed);
      assert.equal(installedOwner(document, false).querySelector('.installed-improvement-handoff') === null, true);
      assert.equal(button(installedOwner(document, false), 'Improve this behavior').disabled, true,
        'Retained uncertainty does not release an accidental duplicate improvement request');
      assertInstalledDrafts(document, state, false);
      assert.equal(state.posts().length, 1);
    });
  });
}

test('a mounted installed improvement remains confirmed with an honest open-page warning when only tracking storage is unavailable', async t => {
  const state = fixture({ localUse: true });
  const save = state.session.setItem;
  let trackingWrites = 0;
  state.session.setItem = (key, value) => {
    if (key.startsWith('mobkit-installed-use-improvement-v1:')) {
      trackingWrites++;
      throw Error('Synthetic improvement tracking quota refusal.');
    }
    save(key, value);
  };
  await mountedApp(t, state, async document => {
    await prepareInstalledFeedback(document, false);
    const admission = state.schedule(improvementUrl(false), 'POST');
    await click(installedOwner(document, false), 'Improve this behavior');
    assert.equal(admission.started, true, 'The supported inline-only tracking path admits the deliberate report');
    assert.equal(trackingWrites, 1, 'Only the initial exact tracking save encountered the fixture quota refusal');
    assert.equal([...state.session.values.keys()].some(key => key.startsWith('mobkit-installed-use-improvement-v1:')), false);
    const accepted = admittedImprovement(false);
    state.receipts.implementation.set(A, accepted);
    await finish(admission, accepted);
    assert.equal(installedOwner(document, false).querySelector('.installed-improvement-handoff')?.dataset.buildId, accepted.build_id);
    assert.match(installedOwner(document, false).textContent,
      /Improvement tracking is available in this open page only.*Browser storage is unavailable.*reload cannot recover this request/);
    assert.equal(trackingWrites, 1, 'Inline confirmation does not make another failing storage write or claim persistent recovery');
    assert.equal([...state.session.values.keys()].some(key => key.startsWith('mobkit-installed-use-improvement-v1:')), false);
    assertInstalledDrafts(document, state, false);
    assert.equal(state.posts().length, 1, 'The successful exact admission is not discarded or replayed');
  });
});

const savedHistoryUrl = '/api/guide/list';
const savedHistoryEmpty = 'Your conversations are saved here as you go.';
// List-read feedback is a direct menu child; saved rows have their own build status.
const savedHistoryMenu = document => document.querySelector('.setup-history-menu');
const savedHistoryCalls = state => state.calls.filter(call => call.method === 'GET' && call.url === savedHistoryUrl);
const savedHistoryNames = document => [...savedHistoryMenu(document).querySelectorAll('button > span')]
  .map(node => node.textContent);
function savedHistoryResult(a = planFor(A).project.name, b = planFor(B).project.name) {
  return { setups: [{ id: A, name: a, status: 'ready' }, { id: B, name: b, status: 'review' }] };
}

test('saved history reads expose pending and retryable failure without losing the current setup or unsent composer', async t => {
  const state = fixture();
  const initial = state.schedule(savedHistoryUrl);
  await mountedApp(t, state, async document => {
    assert.equal(initial.started, true, 'The initial guide load really started its saved-history read');
    assert.equal(selected(document), A);
    assertComposer(document, state);
    await click(document, 'Your setups');
    assert.equal(savedHistoryCalls(state).length, 1, 'Opening the menu reuses the already pending history read');
    assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'true');
    assert.match(savedHistoryMenu(document).querySelector(':scope > [role="status"]').textContent, /Reading saved setups/);
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), false,
      'An unobserved list is not a confirmed empty history');

    await fail(initial, 'SYNTHETIC PRIVATE HISTORY FAILURE DETAIL');
    assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'false');
    assert.equal(savedHistoryMenu(document).querySelector(':scope > [role="alert"]').textContent,
      'Saved setups could not be read. Your current setup is unchanged.');
    assert.doesNotMatch(savedHistoryMenu(document).textContent, /SYNTHETIC PRIVATE HISTORY FAILURE DETAIL/);
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), false);
    assert.equal(selected(document), A);
    assert.equal(proposal(document).project.name, planFor(A).project.name);
    assertComposer(document, state);

    const retry = state.schedule(savedHistoryUrl);
    const capturedRetry = button(savedHistoryMenu(document), 'Retry saved setups');
    await act(async () => {
      capturedRetry.click();
      capturedRetry.click();
      await settle();
    });
    assert.equal(retry.started, true, 'The deliberate retry really started another saved-history read');
    assert.equal(savedHistoryCalls(state).length, 2,
      'The same captured retry clicked twice before pending state renders admits only one GET');
    assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'true');
    const pendingCount = savedHistoryCalls(state).length;
    await click(document, 'Your setups');
    assert.ok(savedHistoryMenu(document) === null, 'Expected this saved-history node to be absent');
    assert.equal(savedHistoryCalls(state).length, pendingCount, 'Closing the menu does not read or retry');
    await click(document, 'Your setups');
    assert.equal(savedHistoryCalls(state).length, pendingCount, 'Reopening during retry does not admit a duplicate read');
    await finish(retry, savedHistoryResult());
    assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'false');
    assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="alert"]') === null, 'Expected this saved-history node to be absent');
    assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="status"]') === null, 'Expected this saved-history node to be absent');
    assert.deepEqual(savedHistoryNames(document), [planFor(A).project.name, planFor(B).project.name]);
    assert.equal(selected(document), A);
    assertComposer(document, state);
    assert.equal(state.posts().length, 0, 'History recovery never sends the unsent conversation or starts a job');
  });
});

test('saved history keeps last-good entries across a refused refresh and replaces them only after a successful retry', async t => {
  const state = fixture();
  await mountedApp(t, state, async document => {
    const lastGood = [planFor(A).project.name, planFor(B).project.name];
    const refresh = state.schedule(savedHistoryUrl);
    await click(document, 'Your setups');
    assert.equal(refresh.started, true, 'Opening the menu starts the current refresh');
    assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'true');
    assert.match(savedHistoryMenu(document).querySelector(':scope > [role="status"]').textContent, /Updating saved setups/);
    assert.deepEqual(savedHistoryNames(document), lastGood, 'Last-good entries remain usable while refresh is pending');
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), false);
    const retainedSelected = savedHistoryMenu(document).querySelector('button[aria-current="true"]');
    assert.equal(retainedSelected.querySelector('span').textContent, planFor(A).project.name);

    await fail(refresh, 'SYNTHETIC REFRESH TRANSPORT DETAIL');
    assert.equal(savedHistoryMenu(document).querySelector(':scope > [role="alert"]').textContent,
      'Saved setups could not be refreshed. The list below is from the last successful check.');
    assert.doesNotMatch(savedHistoryMenu(document).textContent, /SYNTHETIC REFRESH TRANSPORT DETAIL/);
    assert.deepEqual(savedHistoryNames(document), lastGood);
    assert.equal(selected(document), A);
    assertComposer(document, state);

    const retry = state.schedule(savedHistoryUrl);
    await click(savedHistoryMenu(document), 'Retry saved setups');
    assert.equal(retry.started, true);
    assert.deepEqual(savedHistoryNames(document), lastGood, 'A retry does not discard last-good entries before success');
    await finish(retry, savedHistoryResult('Fresh observing crew A', 'Fresh observing crew B'));
    assert.deepEqual(savedHistoryNames(document), ['Fresh observing crew A', 'Fresh observing crew B']);
    assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="alert"]') === null, 'Expected this saved-history node to be absent');
    assert.equal(savedHistoryMenu(document).querySelector('button[aria-current="true"] span').textContent,
      'Fresh observing crew A');
    assert.match(savedHistoryMenu(document).textContent, /Accepted plan/);
    assert.match(savedHistoryMenu(document).textContent, /Design ready to review/);
    assert.equal(selected(document), A);
    assert.equal(proposal(document).project.name, planFor(A).project.name,
      'History metadata cannot replace the selected proposal');
    assertComposer(document, state);
    assert.equal(state.posts().length, 0);
  });
});

test('only a successful genuinely empty saved-history read displays the ordinary empty message', async t => {
  const state = fixture();
  const initial = state.schedule(savedHistoryUrl);
  await mountedApp(t, state, async document => {
    await click(document, 'Your setups');
    assert.equal(initial.started, true);
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), false);
    await finish(initial, { setups: [] });
    assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'false');
    assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="alert"]') === null, 'Expected this saved-history node to be absent');
    assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="status"]') === null, 'Expected this saved-history node to be absent');
    assert.deepEqual(savedHistoryNames(document), []);
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), true);

    await click(document, 'Your setups');
    const refresh = state.schedule(savedHistoryUrl);
    await click(document, 'Your setups');
    assert.equal(refresh.started, true);
    assert.match(savedHistoryMenu(document).querySelector(':scope > [role="status"]').textContent, /Reading saved setups/);
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), false,
      'The previous empty observation does not hide the next pending read');
    await fail(refresh, 'Synthetic empty-history refresh refused.');
    assert.match(savedHistoryMenu(document).querySelector(':scope > [role="alert"]').textContent, /could not be read/);
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), false,
      'A failed read cannot be presented as an empty history');
    const retry = state.schedule(savedHistoryUrl);
    await click(savedHistoryMenu(document), 'Retry saved setups');
    await finish(retry, { setups: [] });
    assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="alert"]') === null, 'Expected this saved-history node to be absent');
    assert.equal(savedHistoryMenu(document).textContent.includes(savedHistoryEmpty), true);
    assert.equal(selected(document), A);
    assertComposer(document, state);
    assert.equal(state.posts().length, 0);
  });
});

for (const settlement of ['success', 'failure']) {
  test(`an older saved-history ${settlement} cannot publish or release a newer guide-load list read`, async t => {
    const state = fixture();
    const bDraft = '  A distinct unsent idea for saved setup B.\nKeep it separate.  ';
    const bInput = { key: inputKey(state.guides.get(B)), draft: bDraft, selected: [], mode: 'chat' };
    state.storage.setItem(bInput.key, JSON.stringify(bInput));
    await mountedApp(t, state, async document => {
      const older = state.schedule(savedHistoryUrl);
      await click(document, 'Your setups');
      assert.equal(older.started, true);
      assert.deepEqual(savedHistoryNames(document), [planFor(A).project.name, planFor(B).project.name]);
      const openB = [...savedHistoryMenu(document).querySelectorAll('button')]
        .find(node => node.querySelector('span')?.textContent === planFor(B).project.name);
      assert.ok(openB, 'The newer owner is selected through its retained real saved-history entry');
      const selectedRead = state.schedule(urlFor(B));
      const newer = state.schedule(savedHistoryUrl);
      await act(async () => { openB.click(); await settle(); });
      assert.equal(selectedRead.started, true);
      await finish(selectedRead, state.guides.get(B));
      assert.equal(newer.started, true, 'Adopting the selected guide starts its newer list observation');
      assert.equal(selected(document), B);
      assertComposer(document, state, B, bDraft);
      const newerCount = savedHistoryCalls(state).length;
      await click(document, 'Your setups');
      assert.equal(savedHistoryCalls(state).length, newerCount, 'Menu-open reuses the newer guide-load read');
      assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'true');

      if (settlement === 'success') await finish(older, savedHistoryResult('STALE HISTORY A', 'STALE HISTORY B'));
      else await fail(older, 'STALE HISTORY FAILURE');
      assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'true',
        'An earlier result or refusal cannot retire the newer pending history state');
      assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="alert"]') === null, 'Expected this saved-history node to be absent');
      assert.match(savedHistoryMenu(document).querySelector(':scope > [role="status"]').textContent, /Updating saved setups/);
      assert.deepEqual(savedHistoryNames(document), [planFor(A).project.name, planFor(B).project.name]);
      assert.doesNotMatch(savedHistoryMenu(document).textContent, /STALE HISTORY/);
      await click(document, 'Your setups');
      await click(document, 'Your setups');
      assert.equal(savedHistoryCalls(state).length, newerCount,
        'The old finally cannot release the newer request latch and admit a replacement menu read');

      await finish(newer, savedHistoryResult('Current saved crew A', 'Current saved crew B'));
      assert.equal(savedHistoryMenu(document).getAttribute('aria-busy'), 'false');
      assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="alert"]') === null, 'Expected this saved-history node to be absent');
      assert.ok(savedHistoryMenu(document).querySelector(':scope > [role="status"]') === null, 'Expected this saved-history node to be absent');
      assert.deepEqual(savedHistoryNames(document), ['Current saved crew A', 'Current saved crew B']);
      assert.equal(savedHistoryMenu(document).querySelector('button[aria-current="true"] span').textContent,
        'Current saved crew B');
      assert.equal(selected(document), B);
      assert.equal(proposal(document).project.name, planFor(B).project.name);
      assert.equal(state.storage.getItem(GUIDE_KEY), B);
      assertComposer(document, state, B, bDraft);
      assert.equal(state.composer(A), unsent, 'History observation never consumes the prior owner composer');
      assert.equal(state.posts().length, 0, 'The list and guide observations never mutate a setup or admit a job');
    });
  });
}

function implementationBriefFixture() {
  const state = fixture();
  for (const id of [A, B]) {
    const current = state.guides.get(id);
    current.status = 'ready';
    current.stage = 'review';
    current.blueprint = current.proposal;
    current.proposal = null;
    current.question = null;
    current.readiness = { ready: true, context_ready: true, decision_blockers: [], implementation_blockers: [] };
    current.brief = { facts: { audience: { summary: `Synthetic observing crew ${id === A ? 'A' : 'B'}.`, status: 'confirmed' } },
      requirements: [{ outcome: 'Prepare an observing brief.', disposition: 'handoff',
        supported_subset: 'A continuing planner and synthetic evidence.', extension: 'Verify the observing evidence separately.' }] };
    current.messages = [{ id: `synthetic-guidance-${id}`, role: 'assistant', text: `Current implementation guidance ${id === A ? 'A' : 'B'}.` }];
    const input = { key: inputKey(current), draft: id === A ? unsent : '  Separate saved idea for B.  ', selected: [], mode: 'chat' };
    state.storage.setItem(input.key, JSON.stringify(input));
  }
  const originalRead = state.read;
  state.read = async (url, options = {}) => {
    if (url === '/api/guide/list' && (options.method || 'GET') === 'GET') {
      state.calls.push({ url, method: 'GET' });
      return { setups: [...state.guides.values()].map(guide => ({ id: guide.id,
        name: (guide.proposal || guide.blueprint).project.name, status: guide.status })) };
    }
    for (const id of [A, B]) {
      const guide = state.guides.get(id);
      if ((options.method || 'GET') === 'GET' &&
          url === `${urlFor(id)}/deployment/options?source_kind=native-team&version=${guide.version}`) {
        state.calls.push({ url, method: 'GET' });
        return { source_kind: 'native-team', build_id: null,
          source_id: `${id === A ? '1' : '2'}${guide.version.toString(16).padStart(63, '0')}`,
          source_digest: `${id === A ? 'c' : 'd'}${guide.version.toString(16).padStart(63, '0')}`, guide_version: guide.version,
          current_guide_version: guide.version, current: true, target: 'mac', host_target: 'mac', supported: true,
          default_destination: `/synthetic/new-native-${id === A ? 'a' : 'b'}`, console_port: guide.blueprint.advanced.port,
          fields: [{ key: 'OPENAI_API_KEY', label: 'OpenAI API key', required: true, secret: true, configured: false }],
          configured_deployment_id: null, configured_destination: null,
          can_use_host_provider_credentials: false, host_provider_keys: [], knowledge_mode: 'none',
          handoff: { mode: 'local', status: 'not-connected',
            package_url: `${urlFor(id)}/deployment/package?source_kind=native-team&version=${guide.version}`,
            steps: [], operations: [], limitations: [] },
          console_scope: { authentication: 'loopback-only', health_scope: 'native-bootstrap-and-console',
            requested_interfaces: '', custom_interfaces_verified: false },
          default_execution_mode: 'interactive', execution_modes: [
            { id: 'interactive', label: 'While Launch is open', available: true, description: 'Launch owns this synthetic process.' },
            { id: 'background', label: 'Run in the background', available: false, reason: 'Offline fixture has no service manager.' } ] };
      }
    }
    const result = await originalRead(url, options);
    for (const id of [A, B]) {
      if (url === `${urlFor(id)}/deployment/options?build_id=${buildFor(id).build_id}`)
        return { ...result, guide_version: state.guides.get(id).version, current_guide_version: state.guides.get(id).version };
    }
    return result;
  };
  return state;
}

function installedProgressNavigationFixture(installed = true) {
  const state = implementationBriefFixture();
  const custom = buildFor(A, { build_id: 'checked-custom-build-b', source_digest: 'b'.repeat(64),
    acceptance: { ...buildFor(A).acceptance, source_digest: 'b'.repeat(64) } });
  const native = deploymentFor(A, { source_kind: 'native-team', source_id: '1'.repeat(64),
    source_digest: 'd'.repeat(64), build_id: null });
  state.receipts.implementation.set(A, custom);
  state.receipts.deployment.set(A, installed ? native : null);
  const originalRead = state.read;
  state.read = async (url, options = {}) => {
    if ((options.method || 'GET') === 'GET') {
      if (url === `${urlFor(A)}/deployment/options?build_id=${custom.build_id}`) {
        state.calls.push({ url, method: 'GET' });
        return { ...optionsFor(A), build_id: custom.build_id, source_digest: custom.source_digest };
      }
      if (url === `${urlFor(A)}/implementation/timeline?build_id=${custom.build_id}&limit=200`) {
        state.calls.push({ url, method: 'GET' });
        return { build_id: custom.build_id, frames: [], bindings: [], has_more: false };
      }
      if (url === `${urlFor(A)}/deployment/settings?deployment_id=${native.deployment_id}`) {
        state.calls.push({ url, method: 'GET' });
        return { ...optionsFor(A, true), source_kind: native.source_kind, source_id: native.source_id,
          source_digest: native.source_digest, build_id: null };
      }
    }
    return originalRead(url, options);
  };
  return state;
}

test('installed progress management preserves the native A source and exact private draft beside checked custom B', async t => {
  const state = installedProgressNavigationFixture();
  await mountedApp(t, state, async document => {
    assert.equal(deployment(document).querySelector('[name="deployment-source"]:checked').value, 'native-team');
    await click(deployment(document), 'Change private settings');
    const form = document.querySelector('.deployment-form');
    assert.ok(form, 'The actual installed native team settings form must open');
    const exactSecret = '  SYNTHETIC-PRIVATE-NATIVE-A\tKeep whitespace.  ';
    const exactUser = '  Synthetic native user A  ';
    await inputValue(form.querySelector('[name="OPENAI_API_KEY"]'), exactSecret);
    await inputValue(form.querySelector('[name="APP_USER"]'), exactUser);
    const draft = [...form.querySelectorAll('input')].map(input => ({ name: input.name, value: input.value, checked: input.checked }));
    const settingsKey = form.dataset.configurationKey;
    const storage = [...state.storage.values.entries()], session = [...state.session.values.entries()];
    await click(applicationStatus(document), 'Manage agent team');
    assert.equal(deployment(document).querySelector('[name="deployment-source"]:checked').value, 'native-team',
      'Managing an existing installation must not choose a separate checked custom application');
    assert.equal(document.querySelector('.deployment-form') === form, true, 'Management navigation preserves the actual mounted private form');
    assert.equal(form.dataset.configurationKey, settingsKey);
    assert.deepEqual([...form.querySelectorAll('input')].map(input => ({ name: input.name, value: input.value, checked: input.checked })), draft);
    assert.deepEqual([...state.storage.values.entries()], storage);
    assert.deepEqual([...state.session.values.entries()], session);
    assert.equal(state.posts().length, 0, 'Navigation neither saves credentials nor starts, installs or updates anything');
    assert.equal(state.calls.some(call => call.body && JSON.stringify(call.body).includes('SYNTHETIC-PRIVATE-NATIVE-A')), false);
  });
});

test('new-install progress explicitly selects checked custom B without admitting installation', async t => {
  const state = installedProgressNavigationFixture(false);
  await mountedApp(t, state, async document => {
    const native = deployment(document).querySelector('[name="deployment-source"][value="native-team"]');
    await act(async () => { native.click(); await settle(); });
    assert.equal(deployment(document).querySelector('[name="deployment-source"]:checked').value, 'native-team');
    await click(applicationStatus(document), 'Set up and run');
    assert.equal(deployment(document).querySelector('[name="deployment-source"]:checked').value, 'application',
      'A new custom application install action must still choose its explicit checked source');
    assert.equal(document.querySelector('.deployment-form') === null, true, 'Source navigation does not open or submit private setup automatically');
    assert.equal(state.posts().length, 0);
  });
});

async function mountedImplementationBrief(t, state, verify) {
  const originalURL = Object.getOwnPropertyDescriptor(globalThis, 'URL');
  const NativeURL = globalThis.URL, blobs = new Map(), downloads = [];
  class BriefURL extends NativeURL {}
  BriefURL.createObjectURL = blob => {
    const url = `blob:implementation-brief-${blobs.size + 1}`;
    blobs.set(url, blob);
    return url;
  };
  BriefURL.revokeObjectURL = () => {};
  Object.defineProperty(globalThis, 'URL', { configurable: true, writable: true, value: BriefURL });
  try {
    await mountedApp(t, state, async (document, tick) => {
      const prototype = document.defaultView.HTMLAnchorElement.prototype;
      const originalClick = Object.getOwnPropertyDescriptor(prototype, 'click');
      Object.defineProperty(prototype, 'click', { configurable: true, writable: true, value() {
        assert.ok(blobs.has(this.href), 'Only the actual local brief Blob is downloaded');
        downloads.push({ filename: this.download, blob: blobs.get(this.href) });
      } });
      try { await verify(document, tick, downloads, blobs); }
      finally {
        if (originalClick) Object.defineProperty(prototype, 'click', originalClick);
        else delete prototype.click;
      }
    });
  } finally { Object.defineProperty(globalThis, 'URL', originalURL); }
}

async function implementationBriefControl(document) {
  const panel = document.querySelector('.setup-manual-export');
  assert.ok(panel, 'The real accepted-plan manual export disclosure is present');
  if (!panel.open) await act(async () => { panel.querySelector('summary').click(); await settle(); });
  return button(panel, 'Download implementation brief');
}

async function assertImplementationBrief(download, guide) {
  const plan = guide.proposal || guide.blueprint;
  assert.equal(download.filename, `${plan.project.slug}-implementation-handoff.md`);
  assert.equal(download.blob.type, 'text/markdown;charset=utf-8');
  const text = await download.blob.text();
  assert.match(text, new RegExp(`^# ${plan.project.name}: implementation handoff`));
  assert.ok(text.includes(guide.messages.at(-1).text), 'Export includes current saved implementation guidance');
  assert.ok(text.includes(guide.brief.facts.audience.summary), 'Export includes the current agreed understanding');
  const json = text.match(/```json\n([\s\S]*?)\n```/);
  assert.ok(json, 'Export contains the native proposal as readable JSON');
  assert.deepEqual(JSON.parse(json[1]), plan);
  assert.match(text, /This is a design handoff, not a complete deployment package/);
  return text;
}

test('implementation brief holds a captured pending-selection click and recovers deliberately after refusal', async t => {
  const state = implementationBriefFixture();
  await mountedImplementationBrief(t, state, async (document, tick, downloads, blobs) => {
    const retained = await implementationBriefControl(document);
    assert.equal(retained.disabled, false);
    const waiting = await beginSelection(document, state, B, [retained]);
    assert.equal(selected(document), A);
    assert.equal(retained.disabled, true, 'The old displayed brief is visibly held during selected-guide loading');
    assert.equal(blobs.size, 0, 'A captured pre-redraw click cannot create an outgoing brief Blob');
    assert.deepEqual(downloads, []);
    assertComposer(document, state);
    assert.equal(state.storage.getItem(GUIDE_KEY), A);
    assert.equal(state.posts().length, 0);
    await fail(waiting, 'Synthetic brief destination refused.');
    assert.equal(selected(document), A);
    assert.match(document.body.textContent, /is still selected/);
    assert.equal(downloads.length, 0, 'Selection refusal cannot automatically export anything');
    const current = await implementationBriefControl(document);
    assert.equal(current.disabled, false, 'A fresh explicit export is available after returning to retained A');
    const before = clone(state.calls);
    await click(document.querySelector('.setup-manual-export'), 'Download implementation brief');
    assert.equal(downloads.length, 1);
    await assertImplementationBrief(downloads[0], state.guides.get(A));
    assert.deepEqual(state.calls, before, 'Local brief export makes no API request');
    assertComposer(document, state);
    assert.equal(state.storage.getItem(GUIDE_KEY), A);
    assert.equal(state.posts().length, 0);
  });
});

test('implementation brief follows actual A-B-A selection and exports only each deliberate current understanding', async t => {
  const state = implementationBriefFixture(), bDraft = state.composer(B);
  await mountedImplementationBrief(t, state, async (document, tick, downloads, blobs) => {
    const retainedA = await implementationBriefControl(document);
    const bRead = await beginSelection(document, state, B, [retainedA]);
    assert.equal(blobs.size, 0);
    await finish(bRead, state.guides.get(B));
    assert.equal(selected(document), B);
    assertComposer(document, state, B, bDraft);
    await implementationBriefControl(document);
    const bCalls = clone(state.calls);
    await click(document.querySelector('.setup-manual-export'), 'Download implementation brief');
    assert.equal(downloads.length, 1);
    const bText = await assertImplementationBrief(downloads[0], state.guides.get(B));
    assert.doesNotMatch(bText, /Current implementation guidance A\./);
    assert.deepEqual(state.calls, bCalls);

    const retainedB = await implementationBriefControl(document);
    const aRead = await beginSelection(document, state, A, [retainedB]);
    assert.equal(selected(document), B);
    assert.equal(retainedB.disabled, true);
    assert.equal(downloads.length, 1, 'The captured previous-owner callback cannot export while returning to A');
    assert.equal(blobs.size, 1);
    await finish(aRead, state.guides.get(A));
    assert.equal(selected(document), A);
    assert.equal(downloads.length, 1, 'A-B-A adoption never replays an earlier export');
    const current = await implementationBriefControl(document);
    assert.equal(current.disabled, false);
    const aCalls = clone(state.calls);
    await click(document.querySelector('.setup-manual-export'), 'Download implementation brief');
    assert.equal(downloads.length, 2);
    const aText = await assertImplementationBrief(downloads[1], state.guides.get(A));
    assert.doesNotMatch(aText, /Current implementation guidance B\./);
    assert.deepEqual(state.calls, aCalls);
    assertComposer(document, state);
    assert.equal(state.composer(B), bDraft);
    assert.equal(state.storage.getItem(GUIDE_KEY), A);
    assert.equal(state.posts().length, 0);
  });
});

test('implementation brief waits for same-owner reload and exports the fresh saved revision without consuming its draft', async t => {
  const state = implementationBriefFixture();
  await mountedImplementationBrief(t, state, async (document, tick, downloads, blobs) => {
    const refused = await beginSelection(document, state);
    await fail(refused, 'Synthetic selected setup refused before explicit reload.');
    assert.equal(selected(document), A);
    assertComposer(document, state);
    const retained = await implementationBriefControl(document);
    const read = state.schedule(urlFor(A));
    await act(async () => {
      button(document, 'Reload saved conversation').click();
      retained.click();
      await settle();
    });
    assert.equal(read.started, true);
    assert.equal(retained.disabled, true);
    assert.equal(blobs.size, 0, 'The same-id captured handler cannot export its older saved revision during reload');
    assert.deepEqual(downloads, []);
    assertComposer(document, state);
    const next = clone(state.guides.get(A));
    next.version += 1;
    next.blueprint.project.description = 'A revised synthetic observing plan with its actual continuing context.';
    next.brief.facts.audience.summary = 'Revised observing crew A with a new confirmed scope.';
    next.messages.push({ id: 'revised-guidance-a', role: 'assistant', text: 'Fresh implementation guidance A after reload.' });
    state.guides.set(A, next);
    state.receipts.implementation.set(A, buildFor(A, { guide_version: next.version }));
    state.receipts.deployment.set(A, deploymentFor(A, { guide_version: next.version }));
    await finish(read, next);
    assert.equal(selected(document), A);
    assert.equal(downloads.length, 0);
    assertComposer(document, state);
    await implementationBriefControl(document);
    const before = clone(state.calls);
    await click(document.querySelector('.setup-manual-export'), 'Download implementation brief');
    assert.equal(downloads.length, 1);
    const text = await assertImplementationBrief(downloads[0], next);
    assert.doesNotMatch(text, /Current implementation guidance A\./);
    assert.deepEqual(state.calls, before);
    assertComposer(document, state);
    assert.equal(state.storage.getItem(GUIDE_KEY), A);
    assert.equal(state.posts().length, 0);
  });
});

// Initial Build can prepare source before it publishes latest.json. These are
// controlled response interleavings, not a reconstruction of production timing.
const missingBuildSelection = () => ({ httpError: 409, code: 'build_selection_unavailable',
  detail: 'Saved application work exists without a selected build.' });
const initialRunningBuild = () => buildFor(A, { build_id: 'initial-build-after-preparation',
  status: 'running', phase: 'implementation', ready_to_package: false, source_verified: false,
  acceptance_current: false, acceptance_trusted: false, worker_stop_confirmed: false,
  summary: 'ACTUAL SELECTED SYNTHETIC BUILD' });
function assertPendingAdmission(document) {
  assert.equal(implementation(document).querySelector('h2').textContent, 'Build request in progress');
  assert.equal(applicationStatus(document).querySelector('h1,h2').textContent, 'Build request in progress');
  assert.equal(button(implementation(document), 'Starting builder...').disabled, true);
  assert.equal(document.querySelector('.implementation-selection-recovery') === null, true);
  assert.doesNotMatch(implementation(document).textContent,
    /Build status is unavailable|saved work.*selected build|Checking saved work|Synthetic status failure/);
}

test('pending initial Build retains its own request presentation across missing status and fences a late pre-admission read', async t => {
  for (const lateStatus of ['absence', 'missing selection']) await t.test(lateStatus, async subtest => {
    const state = savedBuildObservationFixture(), endpoint = `${urlFor(A)}/implementation`;
    await mountedApp(subtest, state, async (document, tick) => {
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      const admission = state.schedule(endpoint, 'POST');
      await click(implementation(document), 'Build application');
      assert.equal(admission.started, true); assertPendingAdmission(document);
      for (const [delay, result] of [[15000, missingBuildSelection()],
        [3000, absent('No selected build yet.')],
        [15000, { httpError: 503, detail: 'Synthetic status failure during preparation.' }]]) {
        const observation = state.schedule(endpoint);
        await tick(delay); assert.equal(observation.started, true);
        await finish(observation, result); assertPendingAdmission(document);
      }
      const priorRead = state.schedule(endpoint);
      await tick(3000); assert.equal(priorRead.started, true);
      const accepted = initialRunningBuild(); state.receipts.implementation.set(A, accepted);
      await finish(admission, accepted);
      assert.match(implementation(document).textContent, /ACTUAL SELECTED SYNTHETIC BUILD/);
      assert.doesNotMatch(applicationStatus(document).textContent, /Build request in progress/);
      await finish(priorRead, lateStatus === 'absence' ? absent('Old absence.') : missingBuildSelection());
      assert.match(implementation(document).textContent, /ACTUAL SELECTED SYNTHETIC BUILD/);
      assert.equal(document.querySelector('.implementation-selection-recovery') === null, true);
      assert.doesNotMatch(applicationStatus(document).textContent, /unavailable|needs to be built/);
      assert.equal(state.calls.some(call => call.url.endsWith('/saved-selection')), false);
      assert.deepEqual(state.posts().map(call => ({ url: call.url, body: call.body })), [{
        url: endpoint, body: { version: 5, request: feedback, previous_build_id: null },
      }]);
      assert.equal(state.feedback(A), null, 'Only the exact accepted request consumes its note');
    });
  });
});

test('pending initial Build yields to a current selected receipt while its POST remains outstanding', async t => {
  const state = savedBuildObservationFixture(), endpoint = `${urlFor(A)}/implementation`;
  await mountedApp(t, state, async (document, tick) => {
    const admission = state.schedule(endpoint, 'POST');
    await click(implementation(document), 'Build application'); assertPendingAdmission(document);
    const accepted = initialRunningBuild(); state.receipts.implementation.set(A, accepted);
    await tick(15000);
    assert.match(implementation(document).textContent, /ACTUAL SELECTED SYNTHETIC BUILD/);
    assert.doesNotMatch(applicationStatus(document).textContent, /Build request in progress/);
    assert.equal(document.querySelector('.implementation-selection-recovery') === null, true);
    assert.equal(state.posts().length, 1);
    await finish(admission, accepted);
    assert.equal(state.posts().length, 1);
  });
});

test('genuine missing selection without an owned pending Build still offers existing recovery', async t => {
  const state = savedBuildObservationFixture(), endpoint = `${urlFor(A)}/implementation`;
  state.receipts.implementation.set(A, missingBuildSelection());
  const recovery = state.schedule(`${endpoint}/saved-selection`);
  await mountedApp(t, state, async document => {
    assert.equal(recovery.started, true);
    await finish(recovery, { guide_id: A, version: 5, candidates: [], unavailable_count: 1 });
    assert.ok(document.querySelector('.implementation-selection-recovery'));
    assert.match(implementation(document).textContent, /Your saved build needs attention/);
    assert.doesNotMatch(applicationStatus(document).textContent, /Build request in progress/);
    assert.equal(state.posts().length, 0);
  });
});

test('definitive initial Build refusal retires pending presentation and restores missing-selection recovery', async t => {
  const state = savedBuildObservationFixture(), endpoint = `${urlFor(A)}/implementation`;
  await mountedApp(t, state, async document => {
    await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
    const admission = state.schedule(endpoint, 'POST');
    await click(implementation(document), 'Build application'); assertPendingAdmission(document);
    state.receipts.implementation.set(A, missingBuildSelection());
    const recovery = state.schedule(`${endpoint}/saved-selection`);
    await finish(admission, { httpError: 422, detail: 'Synthetic definite preparation refusal.' });
    assert.equal(recovery.started, true);
    await finish(recovery, { guide_id: A, version: 5, candidates: [], unavailable_count: 1 });
    assert.ok(document.querySelector('.implementation-selection-recovery'));
    assert.match(implementation(document).textContent, /Synthetic definite preparation refusal/);
    assert.doesNotMatch(applicationStatus(document).textContent, /Build request in progress/);
    assert.equal(state.feedback(A), feedback);
    assert.equal(state.posts().length, 1, 'No retry or recovery write is automatic');
  });
});

test('pending initial Build presentation cannot follow navigation away and back or a new guide version', async t => {
  for (const changedVersion of [false, true]) await t.test(changedVersion ? 'new version' : 'new selection', async subtest => {
    const state = savedBuildObservationFixture(), endpoint = `${urlFor(A)}/implementation`;
    await mountedApp(subtest, state, async document => {
      await inputValue(document.getElementById(`implementation-feedback-${A}`), feedback);
      const admission = state.schedule(endpoint, 'POST');
      await click(implementation(document), 'Build application'); assertPendingAdmission(document);
      const openB = await beginSelection(document, state);
      await finish(openB, state.guides.get(B));
      assert.doesNotMatch(applicationStatus(document).textContent, /Build request in progress/);
      if (changedVersion) state.guides.get(A).version += 1;
      const openA = await beginSelection(document, state, A);
      await finish(openA, state.guides.get(A));
      assert.doesNotMatch(applicationStatus(document).textContent, /Build request in progress/);
      assert.doesNotMatch(implementation(document).querySelector('h2').textContent, /Build request in progress/);
      await finish(admission, initialRunningBuild());
      assert.doesNotMatch(implementation(document).textContent, /ACTUAL SELECTED SYNTHETIC BUILD/);
      assert.equal(state.feedback(A), feedback);
      assert.equal(state.posts().length, 1);
    });
  });
});
