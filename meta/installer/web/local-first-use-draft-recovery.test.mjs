import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act, useState } from 'react';
import { createInstalledUseDraftStore, installedUseScopeKey } from './deployment-use.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
const directory = await mkdtemp(join(tmpdir(), 'mobkit-local-first-use-draft-recovery-'));
const modulePath = join(directory, 'deployment-panel.mjs');
after(() => rm(directory, { recursive: true, force: true }));

// Run only after the complete source batch is frozen and reviewed. The actual
// panel and its children use shared React/ReactDOM and linked source maps, so a
// failure remains bounded instead of printing a data-URL application bundle.
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('deployment-panel.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', outfile: modulePath,
  sourcemap: 'linked', sourcesContent: true, logLevel: 'silent', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  plugins: [{ name: 'shared-real-react', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
}).catch(async error => {
  await rm(directory, { recursive: true, force: true });
  throw error;
});
try {
  for (const output of bundled.outputFiles) await writeFile(output.path, output.contents);
} catch (error) {
  await rm(directory, { recursive: true, force: true });
  throw error;
}

const A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const version = 7;
const clone = value => structuredClone(value);
const suffix = id => id === A ? 'a' : 'b';
const deploymentUrl = id => `/api/guide/${id}/deployment`;
const entryId = id => `workspace-${suffix(id)}`;
const entryLabel = id => `Observing workspace ${suffix(id).toUpperCase()}`;
const entryUrl = id => `http://127.0.0.1:${id === A ? 18881 : 18891}/workspace-${suffix(id)}`;
const taskFor = id => `  Compare café notes for ${suffix(id).toUpperCase()} with 觀測 🌌.\nKeep my exact context and spacing.  `;
const reportFor = id => `  The ${suffix(id).toUpperCase()} page was temporarily unavailable.\nKeep this report for later.  `;
const draftFor = id => ({ task: taskFor(id), outcome: 'partly', improvement: reportFor(id) });
const ideaFor = id => ({ label: `Review observing notes ${suffix(id).toUpperCase()}`,
  request: `Review my café notes for ${suffix(id).toUpperCase()}.\nExplain one useful next step.`,
  description: 'Start with a useful summary of my own notes.' });

function planFor(id) {
  return { schema_version: 2, target: 'mac', provider: 'openai', model: 'gpt-6.1-sol',
    project: { name: `Observing notes ${suffix(id).toUpperCase()}`, slug: `draft-recovery-${suffix(id)}`,
      description: 'Summarize synthetic observing notes.' },
    agent_classes: [{ id: 'lead', name: 'Observing lead', description: 'Keep observing context.',
      instructions: 'Use the supplied synthetic evidence.', instances: 1, members: [],
      provider: 'openai', model: 'gpt-6.1-sol',
      tools: { workspace: false, notes: 'none', memory: true, mcp: [] } }],
    topology: { mode: 'manual', entrypoint: 'lead:main', edges: [] },
    connections: [], flows: [], knowledge: { mode: 'none' }, advanced: { port: 4110 } };
}
function guideFor(id) {
  return { id, version, status: 'ready', blueprint: planFor(id), proposal: null,
    question: null, operation: null, readiness: { ready: true, decision_blockers: [] } };
}
function buildFor(id) {
  const digest = suffix(id).repeat(64);
  return { build_id: `accepted-build-${suffix(id)}`, guide_id: id, guide_version: version,
    current: true, status: 'completed', ready_to_package: true, source_verified: true,
    source_digest: digest, worker_stop_confirmed: true, cleanup_pending: false,
    acceptance_current: true, acceptance_trusted: true,
    acceptance: { issuer: 'mobkit-launch-acceptance-host', source_digest: digest,
      status: 'pass', tests_run: 1, review_round: 1, requirements: [], counterexamples: [],
      summary: 'Synthetic accepted offline source.' }, files: [], checks: [], progress: [] };
}
function receiptFor(id) {
  return { guide_id: id, guide_version: version, current: true, source_kind: 'application',
    deployment_id: `owned-install-${suffix(id)}`, build_id: buildFor(id).build_id,
    source_digest: buildFor(id).source_digest, package_sha256: (id === A ? 'c' : 'd').repeat(64),
    status: 'running', source_verified: true, process_live: true, health: 'ready',
    console_url: `http://127.0.0.1:${id === A ? 18880 : 18890}/console`,
    bootstrap: { complete: true, ready: true, counts: { broken: 0 }, identities: { 'lead:main': {} } },
    destination: `/synthetic/owned-${suffix(id)}`, execution_mode: 'interactive',
    detail: `Retained healthy synthetic installation ${suffix(id).toUpperCase()}.` };
}
function optionsFor(id) {
  const receipt = receiptFor(id);
  return { build_id: receipt.build_id, source_digest: receipt.source_digest,
    guide_version: version, current_guide_version: version, current: true,
    source_kind: 'application', target: 'mac', host_target: 'mac', supported: true,
    default_destination: `/synthetic/new-${suffix(id)}`, console_port: id === A ? 18880 : 18890,
    fields: [{ key: 'OPENAI_API_KEY', label: 'Provider key', required: true, secret: true, configured: true }],
    execution_modes: [{ id: 'interactive', available: true, label: 'While Launch is open',
      description: 'Synthetic interactive option.' }] };
}
function metadataFor(id) {
  const receipt = receiptFor(id), build = buildFor(id);
  return { scope: 'verified-installed-source', application_outcomes_verified: false,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: receipt.source_digest, package_sha256: receipt.package_sha256,
    guide_version: version, current_guide_version: version, version, current: true,
    status: 'running', project: { name: planFor(id).project.name },
    entrypoint: { identity: 'lead:main', name: 'Observing lead' },
    native_runtime_available: true, console_url: receipt.console_url,
    continuation: { build_id: build.build_id, source_digest: build.source_digest,
      available: true, requires_acknowledgement: false },
    first_use: { scope: 'authored-first-use-suggestions', metadata_sha256: 'e'.repeat(64),
      entry_id: entryId(id), tasks: [ideaFor(id)], detail: null } };
}
function experiencesFor(id, available = true) {
  const receipt = receiptFor(id);
  return { deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    package_sha256: receipt.package_sha256, source_digest: receipt.source_digest,
    metadata_sha256: 'f'.repeat(64), scope: 'declared-source', verified: false,
    entries: [{ id: entryId(id), label: entryLabel(id), path: `/workspace-${suffix(id)}`,
      port: id === A ? 18881 : 18891, url: available ? entryUrl(id) : null,
      reason: available ? null : 'The application UI observation was temporarily unavailable.',
      status: 'declared', verified: false }] };
}

function offlineFixture({ availableA = true, postSuccess = null, settledAction = null,
  postAction = null, sourceKind = 'application', applicationUi = true } = {}) {
  const responses = new Map(), waiting = new Map(), calls = [], unexpected = [];
  for (const id of [A, B]) {
    const base = deploymentUrl(id);
    const receipt = receiptFor(id), metadata = metadataFor(id);
    if (sourceKind === 'native-team') {
      Object.assign(receipt, { source_kind: sourceKind, source_id: '1'.repeat(64), build_id: null });
      Object.assign(metadata, { source_kind: sourceKind, source_id: receipt.source_id, build_id: null });
      const options = { ...optionsFor(id), source_kind: sourceKind, source_id: receipt.source_id, build_id: null };
      responses.set(`${base}/options?source_kind=native-team&version=${version}`, options);
    } else responses.set(`${base}/options?build_id=${buildFor(id).build_id}`, optionsFor(id));
    responses.set(base, receipt);
    responses.set(`${base}/experiences`, applicationUi ? experiencesFor(id, id === A ? availableA : true)
      : { ...experiencesFor(id), entries: [] });
    responses.set(`${base}/use`, metadata);
    responses.set(`/api/guide/${id}/gcp-deployment`, null);
    responses.set(`/api/guide/${id}/remote-deployment`, { httpError: 404, detail: 'No saved remote deployment.' });
  }
  if (settledAction) responses.set(`${deploymentUrl(settledAction.guide_id)}/action/${settledAction.deployment_id}/${settledAction.action_request_id}`,
    { action_request: { ...settledAction, phase: 'settled', outcome: 'returned' } });
  function exactUrl(id, tail) {
    const url = deploymentUrl(id) + tail;
    assert.ok(responses.has(url), 'Only an existing exact read may be replaced or deferred');
    return url;
  }
  return { calls, unexpected, sourceKind,
    runtimeStatus(id, status) {
      assert.ok(['running', 'stopped'].includes(status), 'Only the selected inert runtime status changes');
      const receipt = clone(responses.get(deploymentUrl(id))), available = status === 'running';
      Object.assign(receipt, { status, process_live: available, health: available ? 'ready' : 'stopped',
        console_url: available ? receiptFor(id).console_url : null });
      responses.set(deploymentUrl(id), receipt);
      const metadata = clone(responses.get(deploymentUrl(id) + '/use'));
      Object.assign(metadata, { status, native_runtime_available: available, console_url: receipt.console_url });
      responses.set(deploymentUrl(id) + '/use', metadata);
      const experiences = clone(responses.get(deploymentUrl(id) + '/experiences'));
      for (const entry of experiences.entries) Object.assign(entry, { url: available ? entryUrl(id) : null,
        reason: available ? null : 'Start this stopped application before opening its UI.' });
      responses.set(deploymentUrl(id) + '/experiences', experiences);
      return clone(receipt);
    },
    actionObservation(command, changes = {}) {
      const url = `${deploymentUrl(command.guide_id)}/action/${command.deployment_id}/${command.action_request_id}`;
      assert.ok(responses.has(url), 'An observation belongs to the original explicitly admitted command');
      responses.set(url, { action_request: { ...command, phase: 'pending', outcome: null, ...changes } });
    },
    count: (id, tail = '') => calls.filter(call => call.url === deploymentUrl(id) + tail).length,
    reply(id, tail, value) { responses.set(exactUrl(id, tail), clone(value)); },
    hold(id, tail) {
      const url = exactUrl(id, tail);
      let resolve, reject;
      const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
      const read = { promise, resolve, reject, started: false };
      waiting.set(url, [...(waiting.get(url) || []), read]);
      return read;
    },
    async fetch(url, options = {}) {
      const call = { url: String(url), method: options.method || 'GET' };
      calls.push(call);
      if (call.method === 'POST' && call.url === deploymentUrl(A) + '/action' && postAction) {
        call.body = JSON.parse(options.body);
        assert.equal(call.body.action, 'stop', 'This fixture admits only the one requested inert Stop');
        assert.equal(call.body.deployment_id, receiptFor(A).deployment_id);
        assert.match(call.body.action_request_id, /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/);
        const command = { version: 1, guide_id: A, deployment_id: call.body.deployment_id,
          action_request_id: call.body.action_request_id, action: 'stop', update_id: null };
        responses.set(`${deploymentUrl(A)}/action/${command.deployment_id}/${command.action_request_id}`,
          { action_request: { ...command, phase: 'pending', outcome: null } });
        const value = await postAction(call.body, command);
        return { ok: true, status: 200, json: async () => clone(value) };
      }
      if (call.method === 'POST' && call.url === deploymentUrl(A) + '/use/success' && postSuccess) {
        call.body = JSON.parse(options.body);
        const value = await postSuccess(call.body);
        return { ok: true, status: 200, json: async () => clone(value) };
      }
      if (call.method !== 'GET' || options.body != null || !responses.has(call.url)) {
        unexpected.push(call);
        throw Error(`Unexpected offline request: ${call.method} ${call.url}`);
      }
      const queued = waiting.get(call.url)?.shift();
      if (queued) queued.started = true;
      const value = queued ? await queued.promise : clone(responses.get(call.url));
      return { ok: !value?.httpError, status: value?.httpError || 200,
        json: async () => value?.httpError ? { detail: value.detail } : clone(value) };
    },
  };
}

async function mountedPanel(t, fixture, verify, { deniedStorage = false, storageAdapter = null, storageSetup = null,
  storageGetterDenied = false, globalStorage = null, permitsSuccess = false, permitsStop = false } = {}) {
  t.mock.timers.enable({ apis: ['setInterval', 'setTimeout'] });
  const dom = new JSDOM('<div id="test-root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const actualStorage = dom.window.sessionStorage;
  storageSetup?.(actualStorage);
  const storage = storageAdapter ? storageAdapter(actualStorage) : deniedStorage ? {
    getItem() { throw Error('Synthetic browser storage denied.'); },
    setItem() { throw Error('Synthetic browser storage denied.'); },
    removeItem() { throw Error('Synthetic browser storage denied.'); },
  } : dom.window.sessionStorage;
  if (deniedStorage || storageAdapter) Object.defineProperty(dom.window, 'sessionStorage', { configurable: true, value: storage });
  if (storageGetterDenied) Object.defineProperty(dom.window, 'sessionStorage', { configurable: true,
    get() { throw Error('PRIVATE_WINDOW_STORAGE_GETTER'); } });
  const copied = [];
  Object.defineProperty(dom.window.navigator, 'clipboard', { configurable: true, value: {
    async writeText(value) { copied.push(value); throw Error('Synthetic clipboard denied.'); },
  } });
  const replacements = { window: dom.window, document: dom.window.document,
    navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    sessionStorage: globalStorage || storage, localStorage: dom.window.localStorage,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: fixture.fetch.bind(fixture) };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  let root;
  try {
    const { DeploymentPanel } = await import(pathToFileURL(modulePath).href);
    const { createRoot } = await import('react-dom/client');
    const admissions = [], receipts = [];
    function Parent() {
      const [id, select] = useState(A);
      return React.createElement('main', { 'data-selected-guide': id },
        React.createElement('nav', null,
          React.createElement('button', { type: 'button', onClick: () => select(A) }, 'Select setup A'),
          React.createElement('button', { type: 'button', onClick: () => select(B) }, 'Select setup B')),
        React.createElement(DeploymentPanel, { state: guideFor(id), plan: planFor(id),
          build: fixture.sourceKind === 'native-team' ? null : buildFor(id), initialSourceKind: fixture.sourceKind,
          designBusy: false, pendingManual: false, Icon: () => null,
          onRepairReceipt: value => admissions.push(value), onReceipt: value => receipts.push(value) }));
    }
    async function mount() {
      root = createRoot(dom.window.document.getElementById('test-root'));
      await act(async () => { root.render(React.createElement(Parent)); await settle(); });
      await act(async () => { await settle(); });
    }
    await mount();
    await verify({ document: dom.window.document, window: dom.window, copied, receipts,
      remount: async () => {
        await act(async () => root.unmount()); root = null;
        await mount();
      },
      tick: async milliseconds => {
        await act(async () => { t.mock.timers.tick(milliseconds); await settle(); });
        await act(async () => { await settle(); });
      } });
    assert.deepEqual(fixture.unexpected, [], 'Only exact local and inert GCP/remote reads or the explicitly permitted fixture POST are allowed');
    assert.ok(fixture.calls.length > 0);
    assert.ok(fixture.calls.every(call => call.method === 'GET' || call.method === 'POST' &&
      (permitsSuccess && call.url === deploymentUrl(A) + '/use/success' ||
       permitsStop && call.url === deploymentUrl(A) + '/action' && call.body.action === 'stop')),
      'Only the explicitly allowed first success or inert Stop submission may mutate the offline fixture');
    assert.deepEqual(admissions, [], 'Draft recovery does not admit an application build');
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

const useView = document => document.querySelector('.deployment-use');
const taskField = document => useView(document)?.querySelector('.deployment-use-field textarea');
const reportField = document => useView(document)?.querySelector('.deployment-use-feedback textarea');
const taskLink = document => [...(useView(document)?.querySelectorAll('a') || [])]
  .find(link => link.textContent.trim() === 'Continue with this task');
function button(document, label) {
  const node = [...document.querySelectorAll('button')].find(item => item.textContent.trim() === label);
  assert.ok(node, `Missing actual control: ${label}`);
  return node;
}
async function click(node, twice = false) {
  await act(async () => { node.click(); if (twice) node.click(); await settle(); });
  await act(async () => { await settle(); });
}
async function finish(read, value) {
  assert.equal(read.started, true, 'A deferred response must belong to an actual admitted GET');
  await act(async () => { read.resolve(clone(value)); await settle(); });
  await act(async () => { await settle(); });
}
async function fail(read) {
  assert.equal(read.started, true);
  await act(async () => { read.reject(Error('Synthetic first-use observation unavailable.')); await settle(); });
  await act(async () => { await settle(); });
}
async function editDrafts(document, window, draft) {
  const disclosure = useView(document)?.querySelector('.deployment-use-feedback-disclosure');
  assert.ok(disclosure, 'The actual problem-report disclosure remains available');
  if (!disclosure.open) await click(disclosure.querySelector('summary'));
  assert.equal(disclosure.open, true);
  const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
  await act(async () => {
    for (const [field, value] of [[taskField(document), draft.task], [reportField(document), draft.improvement]]) {
      assert.ok(field, 'The actual source-scoped editor remains mounted');
      assert.equal(field.disabled, false);
      setter.call(field, value); field.dispatchEvent(new window.Event('input', { bubbles: true }));
    }
    const choice = useView(document)?.querySelector(`.deployment-use-outcomes input[value="${draft.outcome}"]`);
    assert.ok(choice); assert.equal(choice.disabled, false); choice.click(); await settle();
  });
  assertDrafts(document, draft);
}
async function authoredDrafts(document, window, id) {
  const idea = useView(document)?.querySelector('.deployment-use-ideas button');
  assert.ok(idea, 'The checked authored starting point is reachable through its real button');
  await click(idea);
  assert.equal(taskField(document)?.value, ideaFor(id).request);
  await editDrafts(document, window, draftFor(id));
}
function assertDrafts(document, draft) {
  assert.equal(taskField(document)?.value, draft.task);
  assert.equal(reportField(document)?.value, draft.improvement);
  assert.equal(useView(document)?.querySelector(`input[value="${draft.outcome}"]`)?.checked, true);
}
function assertStored(window, id, draft = draftFor(id)) {
  const store = createInstalledUseDraftStore(window.sessionStorage);
  const scope = installedUseScopeKey(receiptFor(id), guideFor(id));
  assert.deepEqual(store.read(scope), draft);
  assert.deepEqual(store.selectedIdea(scope), { catalogue_sha256: 'e'.repeat(64), index: 0 });
}
function assertUnchecked(document, draft) {
  const view = useView(document);
  assert.ok(view);
  assert.equal(view.querySelector('h3')?.textContent, 'Use your application');
  assert.match(view.textContent, /first-use details could not be checked/);
  assertDrafts(document, draft);
  assert.equal(view.querySelector('a') === null, true, 'Unchecked metadata cannot publish application, native or task links');
  assert.equal(view.querySelector('.deployment-use-ideas') === null, true);
  assert.equal(view.querySelector('.deployment-use-walkthrough') === null, true);
  assert.equal(view.querySelector('.deployment-use-success') === null, true);
  assert.equal(view.querySelector('.deployment-use-feedback')?.tagName, 'FORM', 'The report editor keeps its original guarded container');
  assert.equal(view.querySelector('button[type="submit"]') === null, true);
  assert.equal([...view.querySelectorAll('button')].some(button =>
    ['It worked', 'Improve this behavior'].includes(button.textContent.trim())), false);
  assert.doesNotMatch(view.textContent, /UNCHECKED NAME|UNCHECKED IDEA/);
}
function assertLinks(document, id, task = taskFor(id), handoffAllowed = true) {
  const view = useView(document), link = view?.querySelector('.deployment-use-interfaces ul a');
  assert.equal(view?.querySelector('h3')?.textContent, `Use ${planFor(id).project.name}`);
  assert.equal(link?.getAttribute('href'), handoffAllowed ? entryUrl(id) : null);
  assert.equal(link?.textContent.trim(), `Open ${entryLabel(id)}`);
  const handoff = taskLink(document);
  if (!handoffAllowed) {
    assert.equal(link?.getAttribute('aria-disabled'), 'true');
    assert.equal(link?.tabIndex, -1);
    assert.equal(handoff === undefined, true, 'Fresh entrypoint observation cannot release uncertain command custody');
    return;
  }
  assert.ok(handoff, 'Only the exact checked authored entry receives the human task');
  assert.equal(handoff.getAttribute('aria-label'), `Continue with this task in ${entryLabel(id)}`);
  assert.equal(handoff.href.split('#')[0], entryUrl(id));
  assert.deepEqual(JSON.parse(decodeURIComponent(new URL(handoff.href).hash.slice('#mobkit-task='.length))),
    { version: 1, task });
}
async function retryLinks(document, fixture, id = A) {
  const experiences = fixture.hold(id, '/experiences'), use = fixture.hold(id, '/use');
  const before = { experiences: fixture.count(id, '/experiences'), use: fixture.count(id, '/use') };
  const retry = button(document, 'Retry UI links');
  assert.equal(retry.disabled, false);
  await click(retry, true);
  assert.equal(fixture.count(id, '/experiences'), before.experiences + 1);
  assert.equal(fixture.count(id, '/use'), before.use + 1);
  assert.equal(button(document, 'Retry UI links').disabled, true);
  return { experiences, use };
}

for (const focusedEditor of ['task', 'report']) {
  test(`local first-job editor continuity preserves focused ${focusedEditor} node, selection and open report through pending, failed and recovered details`, async t => {
    const fixture = offlineFixture({ availableA: false });
    await mountedPanel(t, fixture, async ({ document, window }) => {
      await authoredDrafts(document, window, A);
      const original = { task: taskField(document), report: reportField(document),
        form: useView(document).querySelector('.deployment-use-feedback'),
        disclosure: useView(document).querySelector('.deployment-use-feedback-disclosure'),
        help: useView(document).querySelector('.deployment-use-job-help'),
        outcome: useView(document).querySelector('input[value="partly"]') };
      assert.equal(original.help.open, false, 'Optional authored job help starts closed beside the editable draft');
      original.help.open = true; original.help.open = false;
      const focused = original[focusedEditor];
      focused.focus(); focused.setSelectionRange(9, 22, 'backward');
      function assertEditors(phase) {
        assert.equal(taskField(document) === original.task, true, `${phase}: the task editor node is retained`);
        assert.equal(reportField(document) === original.report, true, `${phase}: the report editor node is retained`);
        assert.equal(useView(document).querySelector('.deployment-use-feedback') === original.form, true, `${phase}: the report form is retained`);
        assert.equal(useView(document).querySelector('.deployment-use-feedback-disclosure') === original.disclosure, true, `${phase}: the native disclosure is retained`);
        assert.equal(useView(document).querySelector('.deployment-use-job-help') === original.help, true, `${phase}: the job-help disclosure is retained`);
        assert.equal(original.help.open, false, `${phase}: the person's closed job help stays closed`);
        assert.equal(useView(document).querySelector('input[value="partly"]') === original.outcome, true, `${phase}: the selected outcome node is retained`);
        assert.equal(original.disclosure.open, true, `${phase}: the person's open report remains open`);
        assert.equal(document.activeElement === focused, true, `${phase}: focus remains in the same editor`);
        assert.deepEqual([focused.selectionStart, focused.selectionEnd, focused.selectionDirection], [9, 22, 'backward'], `${phase}: the caret selection is retained`);
        assertDrafts(document, draftFor(A)); assertStored(window, A);
      }
      async function assertNoSubmission(phase) {
        const event = new window.Event('submit', { bubbles: true, cancelable: true });
        await act(async () => { original.form.dispatchEvent(event); await settle(); });
        assert.equal(event.defaultPrevented, true, `${phase}: the stable form cannot submit while details are unconfirmed`);
        assert.equal(fixture.calls.some(call => call.method === 'POST'), false, `${phase}: no improvement or installation is admitted`);
        assertEditors(phase);
      }
      assertEditors('available');
      const pending = await retryLinks(document, fixture);
      assertEditors('pending'); await assertNoSubmission('pending');
      await finish(pending.experiences, experiencesFor(A));
      await fail(pending.use);
      assertEditors('failed'); assertUnchecked(document, draftFor(A));
      await assertNoSubmission('failed');
      const recovered = fixture.hold(A, '/use');
      await click(button(document, 'Check first-use details'));
      assertEditors('recovery pending'); await assertNoSubmission('recovery pending');
      await finish(recovered, metadataFor(A));
      assertEditors('recovered'); assertLinks(document, A);
      assert.equal(button(document, 'Improve this behavior').disabled, false, 'Checked current details restore the existing deliberate report action');
      assert.equal(fixture.calls.some(call => call.method === 'POST'), false);
    });
  });
}

test('local first-use draft recovery keeps denied-storage drafts editable and manually copyable through a failed use read', async t => {
  const fixture = offlineFixture({ availableA: false });
  await mountedPanel(t, fixture, async ({ document, window, copied }) => {
    await authoredDrafts(document, window, A);
    const retry = await retryLinks(document, fixture);
    await finish(retry.experiences, experiencesFor(A));
    await fail(retry.use);
    assertUnchecked(document, draftFor(A));
    assert.match(useView(document).textContent, /Browser draft storage is unavailable/);
    assert.match(useView(document).textContent, /draft remains in this open page.*before reloading/);
    const edited = { task: `  Keep the café draft visible.\nUse 觀測 context only after checking the app.  `,
      outcome: 'blocked', improvement: `  I could not check the page.\nPreserve this exact report and spacing.  ` };
    await editDrafts(document, window, edited);
    await click(button(document, 'Copy task'));
    assert.deepEqual(copied, [edited.task], 'Clipboard refusal still receives the exact current human task');
    await click(button(document, 'Select task to copy'));
    const field = taskField(document);
    assert.equal(document.activeElement === field, true);
    assert.equal(field.selectionStart, 0); assert.equal(field.selectionEnd, edited.task.length);
    assert.match(useView(document).textContent, /Select it and copy manually/);
    assertUnchecked(document, edited);
    const recovered = fixture.hold(A, '/use'), before = fixture.count(A, '/use');
    await click(button(document, 'Check first-use details'), true);
    assert.equal(fixture.count(A, '/use'), before + 1, 'Captured duplicate recovery clicks admit one read');
    assertDrafts(document, edited);
    await finish(recovered, metadataFor(A));
    assertDrafts(document, edited); assertLinks(document, A, edited.task, false);
    assert.equal(button(document, 'It worked').disabled, true);
    assert.equal(button(document, 'Improve this behavior').disabled, true);
    assert.equal(button(document, 'Stop application').disabled, true);
    assert.match(useView(document).textContent, /Browser draft storage is unavailable/);
  }, { deniedStorage: true });
});

test('local first-use draft recovery restores saved drafts after remount without adopting unavailable or foreign source metadata', async t => {
  const fixture = offlineFixture();
  await mountedPanel(t, fixture, async ({ document, window, remount, tick }) => {
    await authoredDrafts(document, window, A); assertStored(window, A); assertLinks(document, A);
    fixture.reply(A, '/use', { httpError: 503, detail: 'Synthetic first-use observation unavailable.' });
    await remount();
    assertUnchecked(document, draftFor(A)); assertStored(window, A);
    assert.match(useView(document).textContent, /drafts stay in this browser tab across reloads/);
    const foreign = metadataFor(A);
    foreign.source_digest = buildFor(B).source_digest;
    foreign.project.name = 'UNCHECKED NAME';
    foreign.first_use.tasks = [{ label: 'UNCHECKED IDEA', request: 'Replace the saved human task.' }];
    fixture.reply(A, '/use', foreign);
    const before = fixture.count(A, '/use');
    await click(button(document, 'Check first-use details'));
    assert.equal(fixture.count(A, '/use'), before + 1);
    assertUnchecked(document, draftFor(A)); assertStored(window, A);
    await tick(1500);
    assert.equal(fixture.count(A, '/use'), before + 1, 'Healthy parent polling cannot authorize or automatically retry a foreign first-use read');
    fixture.reply(A, '/use', metadataFor(A));
    await click(button(document, 'Check first-use details'));
    assert.equal(fixture.count(A, '/use'), before + 2);
    assertDrafts(document, draftFor(A)); assertStored(window, A); assertLinks(document, A);
    assert.equal(useView(document).querySelector('.deployment-use-ideas button strong')?.textContent, ideaFor(A).label);
  });
});

test('local first-use draft recovery rejects late A use publication after a real parent switch and preserves B exact source drafts', async t => {
  const fixture = offlineFixture({ availableA: false });
  await mountedPanel(t, fixture, async ({ document, window }) => {
    await authoredDrafts(document, window, A); assertStored(window, A);
    await click(button(document, 'Select setup B'));
    await authoredDrafts(document, window, B); assertStored(window, B); assertLinks(document, B);
    await click(button(document, 'Select setup A'));
    assertDrafts(document, draftFor(A)); assertStored(window, A);
    const waiting = await retryLinks(document, fixture);
    await click(button(document, 'Select setup B'));
    assert.equal(document.querySelector('main').dataset.selectedGuide, B);
    assertDrafts(document, draftFor(B)); assertStored(window, B); assertLinks(document, B);
    const before = { experiences: fixture.count(B, '/experiences'), use: fixture.count(B, '/use') };
    await finish(waiting.experiences, experiencesFor(A));
    await finish(waiting.use, metadataFor(A));
    assert.equal(fixture.count(B, '/experiences'), before.experiences);
    assert.equal(fixture.count(B, '/use'), before.use);
    assertDrafts(document, draftFor(B)); assertStored(window, B); assertLinks(document, B);
    assertStored(window, A);
  });
});

for (const condition of ['denied', 'unreadable']) {
  test(`parent receipt storage recovery ${condition} command tracking preserves fresh receipt and safe drafts while all admissions stay paused`, async t => {
    const fixture = offlineFixture();
    await mountedPanel(t, fixture, async ({ document, window, copied, receipts, tick }) => {
      assert.ok(fixture.count(A) > 0, 'Unavailable command tracking must not suppress the ordinary receipt GET');
      assert.equal(document.querySelector('.deployment-panel')?.dataset.deploymentStatus, 'running');
      assert.equal(receipts.filter(Boolean).at(-1)?.deployment_id, receiptFor(A).deployment_id);
      await authoredDrafts(document, window, A); assertDrafts(document, draftFor(A));
      assert.equal(button(document, 'Stop application').disabled, true);
      assert.equal(button(document, 'Improve this behavior').disabled, true);
      assert.equal(button(document, 'It worked').disabled, true, 'Success admission also requires readable command custody');
      assert.equal(taskLink(document) === undefined, true, 'An uncertain command cannot publish an application task handoff');
      await click(button(document, 'Stop application')); await click(button(document, 'Improve this behavior'));
      await click(button(document, 'It worked')); await click(button(document, 'Copy task'));
      assert.deepEqual(copied, [draftFor(A).task]);
      await tick(1600);
      assertDrafts(document, draftFor(A)); assert.equal(button(document, 'It worked').disabled, true);
      assert.ok(fixture.count(A) >= 2, 'Passive retry still observes the receipt without re-admitting anything');
      assert.match(document.body.textContent, /saved installation command.*(could not|unreadable|unavailable)/i);
      assert.doesNotMatch(document.body.textContent, /PRIVATE_BROWSER_STORAGE|PRIVATE_STORAGE_SOURCE/);
      assert.ok(fixture.calls.every(call => call.method === 'GET'));
    }, condition === 'denied' ? { storageAdapter: () => ({
      getItem() { throw Error('PRIVATE_BROWSER_STORAGE'); }, setItem() { throw Error('PRIVATE_BROWSER_STORAGE'); },
      removeItem() { throw Error('PRIVATE_BROWSER_STORAGE'); },
    }) } : { storageSetup: storage => storage.setItem('meta-forge-deployment-action-v1:' + A, 'PRIVATE_STORAGE_SOURCE') });
  });
}

test('parent receipt storage recovery a newly unreadable command fences the late entered success and keeps its exact retry', async t => {
  let denyActionRead = false, replySuccess, originalBody;
  const pending = new Promise(resolve => { replySuccess = resolve; });
  const fixture = offlineFixture({ postSuccess: body => { originalBody = body; return pending; } });
  await mountedPanel(t, fixture, async ({ document, window, tick }) => {
    await authoredDrafts(document, window, A);
    assert.equal(button(document, 'It worked').disabled, false);
    await click(button(document, 'It worked'));
    assert.ok(originalBody?.request_id);
    const retryKey = 'mobkit-installed-use-success-v1:' + installedUseScopeKey(receiptFor(A), guideFor(A));
    const retained = window.sessionStorage.getItem(retryKey);
    denyActionRead = true; await tick(1600);
    await act(async () => {
      replySuccess({ ...originalBody, scope: 'human-reported-installed-success', guide_id: A, outcome: 'worked',
        application_outcomes_verified: false, recorded_at: '2026-10-08T10:00:00Z' });
      await settle();
    });
    assert.equal(window.sessionStorage.getItem(retryKey), retained, 'Unknown command custody cannot clear the original success body');
    const later = { task: '  My later task remains mine.  ', outcome: 'blocked', improvement: ' Keep this later report. ' };
    await editDrafts(document, window, later); assertDrafts(document, later);
    assert.equal(useView(document).querySelector('[aria-label="Task awaiting success confirmation"]')?.textContent, originalBody.task);
    assert.doesNotMatch(useView(document).querySelector('.deployment-use-success').textContent, /Your success report is saved/);
    assert.equal(button(document, 'Retry success confirmation').disabled, true);
    assert.equal(taskLink(document) === undefined, true);
    assert.equal(fixture.calls.filter(call => call.method === 'POST').length, 1);
  }, { permitsSuccess: true, storageAdapter: storage => ({
    getItem(key) { if (denyActionRead && key.startsWith('meta-forge-deployment-action-v1:')) throw Error('PRIVATE_BROWSER_STORAGE'); return storage.getItem(key); },
    setItem(key, value) { storage.setItem(key, value); }, removeItem(key) { storage.removeItem(key); },
  }) });
});

test('parent receipt storage recovery failed exact command retirement preserves its identity and fresh receipt without admitting work', async t => {
  const intent = { version: 1, guide_id: A, deployment_id: receiptFor(A).deployment_id,
    action_request_id: '77777777-7777-4777-8777-777777777777', action: 'restart', update_id: null };
  const fixture = offlineFixture({ settledAction: intent });
  const storageKey = 'meta-forge-deployment-action-v1:' + A;
  const encoded = JSON.stringify(intent);
  await mountedPanel(t, fixture, async ({ document, window, receipts, copied, tick }) => {
    assert.equal(fixture.count(A, `/action/${intent.deployment_id}/${intent.action_request_id}`), 1);
    assert.equal(fixture.count(A), 1, 'Receipt observation follows the exact settled readback');
    assert.equal(receipts.filter(Boolean).at(-1)?.deployment_id, intent.deployment_id);
    assert.equal(document.querySelector('.deployment-panel')?.dataset.deploymentStatus, 'running');
    assert.equal(window.sessionStorage.getItem(storageKey), encoded);
    await authoredDrafts(document, window, A);
    assert.equal(button(document, 'Stop application').disabled, true);
    assert.equal(button(document, 'It worked').disabled, true);
    assert.equal(button(document, 'Improve this behavior').disabled, true);
    assert.equal(taskLink(document) === undefined, true);
    await click(button(document, 'Copy task'));
    assert.deepEqual(copied, [draftFor(A).task]);
    await tick(1600);
    assert.equal(window.sessionStorage.getItem(storageKey), encoded);
    assertDrafts(document, draftFor(A));
    assert.equal(button(document, 'It worked').disabled, true);
    assert.ok(fixture.count(A) >= 2);
    assert.ok(fixture.calls.every(call => call.method === 'GET'));
    assert.doesNotMatch(document.body.textContent, /PRIVATE_BROWSER_STORAGE/);
  }, { storageSetup: storage => storage.setItem(storageKey, encoded), storageAdapter: storage => ({
    getItem: key => storage.getItem(key), setItem: (key, value) => storage.setItem(key, value),
    removeItem(key) { if (key === storageKey) throw Error('PRIVATE_BROWSER_STORAGE'); storage.removeItem(key); },
  }) });
});

test('parent receipt storage recovery denied Window getter never borrows a global decoy to release command custody', async t => {
  const fixture = offlineFixture(), decoyCalls = [];
  await mountedPanel(t, fixture, async ({ document, window, receipts, copied, tick }) => {
    assert.ok(fixture.count(A) > 0);
    assert.equal(receipts.filter(Boolean).at(-1)?.deployment_id, receiptFor(A).deployment_id);
    await authoredDrafts(document, window, A);
    assert.equal(button(document, 'Stop application').disabled, true);
    assert.equal(button(document, 'It worked').disabled, true);
    assert.equal(button(document, 'Improve this behavior').disabled, true);
    assert.equal(taskLink(document) === undefined, true);
    await click(button(document, 'Copy task'));
    assert.deepEqual(copied, [draftFor(A).task]);
    await tick(1600);
    assertDrafts(document, draftFor(A));
    assert.deepEqual(decoyCalls, [], 'Another global storage owner cannot prove this Window has no saved command');
    assert.ok(fixture.calls.every(call => call.method === 'GET'));
    assert.doesNotMatch(document.body.textContent, /PRIVATE_WINDOW_STORAGE_GETTER/);
  }, { storageGetterDenied: true, globalStorage: {
    getItem(key) { decoyCalls.push(['getItem', key]); return null; },
    setItem(key, value) { decoyCalls.push(['setItem', key, value]); },
    removeItem(key) { decoyCalls.push(['removeItem', key]); },
  } });
});


function cachedEntryLinks(document) {
  return [...document.querySelectorAll('.deployment-use-interfaces a, .deployment-use-admin a, ' +
    '.deployment-use-console-handoff a, .deployment-use > a, .deployment-interface-scope a')].filter(link => !link.closest('.deployment-use-entry-handoff'));
}
function assertCachedEntriesPaused(document, originalLabels) {
  const links = cachedEntryLinks(document);
  assert.deepEqual(links.map(link => link.textContent.trim()), originalLabels,
    'Checked entry labels remain visible while their current runtime is unconfirmed');
  for (const link of links) {
    assert.equal(link.getAttribute('href'), null, 'Every cached application or Console URL is withdrawn');
    assert.equal(link.getAttribute('aria-disabled'), 'true');
    assert.equal(link.tabIndex, -1, 'A paused live entry is removed from sequential keyboard navigation');
  }
  assert.equal(document.querySelector(`a[href="${entryUrl(A)}"]`) === null, true);
  assert.equal(document.querySelector(`a[href="${receiptFor(A).console_url}"]`) === null, true);
  assert.equal(taskLink(document) === undefined, true, 'Unknown command custody cannot publish a task handoff');
}
function assertCachedEntriesLive(document, expectedUrls) {
  const links = cachedEntryLinks(document);
  assert.deepEqual(links.map(link => link.getAttribute('href')), expectedUrls);
  for (const link of links) {
    assert.notEqual(link.getAttribute('aria-disabled'), 'true');
    assert.notEqual(link.tabIndex, -1, 'A checked live entry returns to sequential keyboard navigation');
  }
}

for (const variant of ['application UI', 'application Console', 'native team']) {
  test(`cached live entries ${variant} pause through held Stop and exact status recovery without losing drafts or replaying commands`, async t => {
    let rejectStop, enteredCommand;
    const pendingStop = new Promise((resolve, reject) => { rejectStop = reject; });
    const nativeTeam = variant === 'native team';
    const fixture = offlineFixture({ sourceKind: nativeTeam ? 'native-team' : 'application',
      applicationUi: variant === 'application UI', postAction(body, command) {
        enteredCommand = clone(command);
        return pendingStop;
      } });
    await mountedPanel(t, fixture, async ({ document, window, copied, receipts, tick }) => {
      if (nativeTeam) {
        const field = taskField(document);
        assert.ok(field); assert.equal(field.disabled, false);
        const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
        await act(async () => { setter.call(field, taskFor(A));
          field.dispatchEvent(new window.Event('input', { bubbles: true })); await settle(); });
      } else await authoredDrafts(document, window, A);
      const originalEditors = { task: taskField(document), report: nativeTeam ? null : reportField(document) };
      const originalLinks = cachedEntryLinks(document), originalLabels = originalLinks.map(link => link.textContent.trim());
      const originalUrls = variant === 'application UI'
        ? [entryUrl(A), receiptFor(A).console_url, receiptFor(A).console_url]
        : [receiptFor(A).console_url, receiptFor(A).console_url];
      assert.equal(originalLinks.length, variant === 'application UI' ? 3 : 2,
        'The actual first-use link and runtime-details link are both covered, including custom UI administration');
      assertCachedEntriesLive(document, originalUrls);
      assert.ok(originalUrls.includes(receiptFor(A).console_url));
      if (variant === 'application UI') assert.ok(originalUrls.includes(entryUrl(A)));
      const originalReads = { use: fixture.count(A, '/use'), experiences: fixture.count(A, '/experiences') };
      const stopLabel = nativeTeam ? 'Stop agent team' : 'Stop application';
      assert.equal(button(document, stopLabel).disabled, false);
      await click(button(document, stopLabel));
      assert.ok(enteredCommand);
      assert.equal(fixture.calls.filter(call => call.method === 'POST').length, 1);
      const storedKey = 'meta-forge-deployment-action-v1:' + A;
      assert.deepEqual(JSON.parse(window.sessionStorage.getItem(storedKey)), enteredCommand);
      assertCachedEntriesPaused(document, originalLabels);
      assert.match(useView(document).textContent, /installation status.*(check|unconfirmed)|check installation status/i);
      assert.equal(taskField(document) === originalEditors.task, true);
      assert.equal(taskField(document).value, taskFor(A));
      assert.equal(taskField(document).disabled, false);
      if (!nativeTeam) { assertDrafts(document, draftFor(A)); assertStored(window, A);
        assert.equal(reportField(document) === originalEditors.report, true); }
      assert.equal(button(document, 'Copy task').disabled, false);
      await click(button(document, 'Copy task'));
      assert.deepEqual(copied, [taskFor(A)]);
      await click(button(document, stopLabel));
      assert.equal(fixture.calls.filter(call => call.method === 'POST').length, 1,
        'A repeated click cannot replay the original still-held Stop');

      // The entered request loses its response, then its independent receipt
      // read fails. Cached metadata must never reopen a live-looking URL.
      const unavailableReceipt = fixture.hold(A, '');
      await act(async () => { rejectStop(Error('Synthetic Stop response was unavailable.')); await settle(); });
      assert.equal(unavailableReceipt.started, true);
      assertCachedEntriesPaused(document, originalLabels);
      await fail(unavailableReceipt);
      assertCachedEntriesPaused(document, originalLabels);
      assert.equal(fixture.count(A, '/use'), originalReads.use);
      assert.equal(fixture.count(A, '/experiences'), originalReads.experiences);
      assert.equal(taskField(document) === originalEditors.task, true);
      assert.equal(taskField(document).value, taskFor(A));
      if (!nativeTeam) assertDrafts(document, draftFor(A));
      assert.deepEqual(JSON.parse(window.sessionStorage.getItem(storedKey)), enteredCommand);

      // A superficially settled response for another request cannot retire the
      // original command or release its retained live entrypoints.
      fixture.actionObservation(enteredCommand, { phase: 'settled', outcome: 'returned',
        action_request_id: '00000000-0000-4000-8000-000000000000' });
      await click(button(document, 'Check installation status'));
      assertCachedEntriesPaused(document, originalLabels);
      assert.deepEqual(JSON.parse(window.sessionStorage.getItem(storedKey)), enteredCommand);

      const stopped = fixture.runtimeStatus(A, 'stopped');
      const exactObservation = { action_request: { ...enteredCommand, phase: 'settled', outcome: 'returned' } };
      fixture.actionObservation(enteredCommand, { phase: 'settled', outcome: 'returned' });
      const commandRead = fixture.hold(A, `/action/${enteredCommand.deployment_id}/${enteredCommand.action_request_id}`);
      const checkedReceipt = fixture.hold(A, '');
      const beforeRecovery = fixture.calls.length;
      await click(button(document, 'Check installation status'));
      assert.equal(commandRead.started, true);
      assert.equal(checkedReceipt.started, false, 'Exact saved-command settlement is observed before its independent receipt');
      assertCachedEntriesPaused(document, originalLabels);
      await finish(commandRead, exactObservation);
      assert.equal(checkedReceipt.started, true);
      assertCachedEntriesPaused(document, originalLabels);
      await finish(checkedReceipt, stopped);
      assert.equal(window.sessionStorage.getItem(storedKey), null);
      assert.equal(document.querySelector('.deployment-panel').dataset.deploymentStatus, 'stopped');
      assert.equal(receipts.filter(Boolean).at(-1)?.deployment_id, enteredCommand.deployment_id);
      assert.equal(document.querySelector(`a[href="${entryUrl(A)}"]`) === null, true);
      assert.equal(document.querySelector(`a[href="${receiptFor(A).console_url}"]`) === null, true);
      assert.equal(taskField(document) === originalEditors.task, true);
      assert.equal(taskField(document).value, taskFor(A));
      if (!nativeTeam) assertDrafts(document, draftFor(A));
      const exactReads = fixture.calls.slice(beforeRecovery).filter(call => call.url.includes('/action/') || call.url === deploymentUrl(A));
      assert.deepEqual(exactReads.map(call => call.url), [
        `${deploymentUrl(A)}/action/${enteredCommand.deployment_id}/${enteredCommand.action_request_id}`, deploymentUrl(A)]);
      assert.equal(button(document, nativeTeam ? 'Start installed agent team' : 'Start installed application').disabled, false);

      // An independently checked running projection restores the same URLs;
      // this fixture does not dispatch Start or claim real runtime execution.
      fixture.runtimeStatus(A, 'running');
      await tick(1600);
      assert.equal(document.querySelector('.deployment-panel').dataset.deploymentStatus, 'running');
      assertCachedEntriesLive(document, originalUrls);
      assert.equal(taskField(document) === originalEditors.task, true);
      assert.equal(taskField(document).value, taskFor(A));
      if (!nativeTeam) { assertDrafts(document, draftFor(A)); assertStored(window, A); }
      assert.equal(button(document, 'Copy task').disabled, false);
      await click(button(document, 'Copy task'));
      assert.deepEqual(copied, [taskFor(A), taskFor(A)]);
      assert.equal(fixture.calls.filter(call => call.method === 'POST').length, 1,
        'All recovery is exact readback; Stop is never resent and Start is never invoked');
    }, { permitsStop: true });
  });
}
