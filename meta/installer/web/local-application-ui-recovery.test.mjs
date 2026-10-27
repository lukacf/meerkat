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
const directory = await mkdtemp(join(tmpdir(), 'mobkit-local-application-ui-recovery-'));
const modulePath = join(directory, 'deployment-panel.mjs');
after(() => rm(directory, { recursive: true, force: true }));

// Execute this file-backed build only after the complete source batch is
// reviewed. The real panel and all local, remote and GCP children are bundled;
// React and ReactDOM remain shared with the harness. Linked maps retain useful
// source positions without printing an entire data-URL bundle in a failure.
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('deployment-panel.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node',
  outfile: modulePath, sourcemap: 'linked', sourcesContent: true,
  logLevel: 'silent', loader: { '.css': 'empty' },
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
const ideaFor = id => ({ label: `Review observing notes ${suffix(id).toUpperCase()}`,
  request: `Review my café notes for ${suffix(id).toUpperCase()}.\nExplain one useful next step.`,
  description: 'Start with a useful summary of my own notes.' });

function planFor(id) {
  return { schema_version: 2, target: 'mac', provider: 'openai', model: 'gpt-6.1-sol',
    project: { name: `Observing notes ${suffix(id).toUpperCase()}`, slug: `ui-recovery-${suffix(id)}`,
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

function metadataFor(id, nativeAvailable = true) {
  const receipt = receiptFor(id), build = buildFor(id);
  return { scope: 'verified-installed-source', application_outcomes_verified: false,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: receipt.source_digest, package_sha256: receipt.package_sha256,
    guide_version: version, current_guide_version: version, version, current: true,
    status: 'running', project: { name: planFor(id).project.name },
    entrypoint: { identity: 'lead:main', name: 'Observing lead' },
    native_runtime_available: nativeAvailable, console_url: nativeAvailable ? receipt.console_url : null,
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
      reason: available ? null : 'The native Console observation was temporarily unavailable.',
      status: 'declared', verified: false }] };
}

function offlineFixture({ experiences = experiencesFor(A, false), metadata = metadataFor(A) } = {}) {
  const responses = new Map(), waiting = new Map(), calls = [], unexpected = [];
  for (const id of [A, B]) {
    const base = deploymentUrl(id);
    responses.set(base, receiptFor(id));
    responses.set(`${base}/options?build_id=${buildFor(id).build_id}`, optionsFor(id));
    responses.set(`${base}/experiences`, id === A ? experiences : experiencesFor(id));
    responses.set(`${base}/use`, id === A ? metadata : metadataFor(id));
    responses.set(`/api/guide/${id}/gcp-deployment`, null);
    responses.set(`/api/guide/${id}/remote-deployment`, { httpError: 404, detail: 'No saved remote deployment.' });
  }
  return { calls, unexpected,
    count: (id, tail = '') => calls.filter(call => call.url === deploymentUrl(id) + tail).length,
    hold(id, tail) {
      const url = deploymentUrl(id) + tail;
      assert.ok(responses.has(url), 'Only a known exact read can be deferred');
      let resolve, reject;
      const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
      const read = { promise, resolve, reject, started: false };
      waiting.set(url, [...(waiting.get(url) || []), read]);
      return read;
    },
    async fetch(url, options = {}) {
      const call = { url: String(url), method: options.method || 'GET' };
      calls.push(call);
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

async function mountedPanel(t, fixture, verify) {
  t.mock.timers.enable({ apis: ['setInterval', 'setTimeout'] });
  const dom = new JSDOM('<div id="test-root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const replacements = { window: dom.window, document: dom.window.document,
    navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    sessionStorage: dom.window.sessionStorage, localStorage: dom.window.localStorage,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: fixture.fetch.bind(fixture) };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  let root;
  try {
    const { DeploymentPanel } = await import(pathToFileURL(modulePath).href);
    const { createRoot } = await import('react-dom/client');
    const admissions = [];
    function Parent() {
      const [id, select] = useState(A);
      return React.createElement('main', { 'data-selected-guide': id },
        React.createElement('nav', null,
          React.createElement('button', { type: 'button', onClick: () => select(A) }, 'Select setup A'),
          React.createElement('button', { type: 'button', onClick: () => select(B) }, 'Select setup B')),
        React.createElement(DeploymentPanel, { state: guideFor(id), plan: planFor(id), build: buildFor(id),
          designBusy: false, pendingManual: false, Icon: () => null,
          onRepairReceipt: value => admissions.push(value) }));
    }
    root = createRoot(dom.window.document.getElementById('test-root'));
    await act(async () => { root.render(React.createElement(Parent)); await settle(); });
    await act(async () => { await settle(); });
    const tick = async milliseconds => {
      await act(async () => { t.mock.timers.tick(milliseconds); await settle(); });
      await act(async () => { await settle(); });
    };
    await verify({ document: dom.window.document, window: dom.window, fixture, tick });
    assert.deepEqual(fixture.unexpected, [], 'Only the exact local and inert remote/GCP GET routes are allowed');
    assert.ok(fixture.calls.length > 0);
    assert.ok(fixture.calls.every(call => call.method === 'GET'), 'UI-link recovery admits no application, build or runtime mutation');
    assert.deepEqual(admissions, [], 'Editing and recovering application links never admits a build');
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
const appLink = document => [...(useView(document)?.querySelectorAll('.deployment-use-interfaces a') || [])].find(link => !new URL(link.href).hash.startsWith('#mobkit-task=')) || null;
const taskLink = document => [...(useView(document)?.querySelectorAll('a') || [])]
  .find(link => link.textContent === 'Continue with this task');
function button(document, label) {
  const found = [...document.querySelectorAll('button')].find(node => node.textContent.trim() === label);
  assert.ok(found, `Missing actual control: ${label}`);
  return found;
}
async function click(node, twice = false) {
  await act(async () => { node.click(); if (twice) node.click(); await settle(); });
  await act(async () => { await settle(); });
}
async function finish(read, value) {
  assert.equal(read.started, true, 'The deferred response belongs to an actual admitted GET');
  await act(async () => { read.resolve(clone(value)); await settle(); });
  await act(async () => { await settle(); });
}
async function fail(read) {
  assert.equal(read.started, true);
  await act(async () => { read.reject(Error('Synthetic link observation unavailable.')); await settle(); });
  await act(async () => { await settle(); });
}
async function writeDrafts(document, window, id) {
  const idea = useView(document)?.querySelector('.deployment-use-ideas button');
  assert.ok(idea, 'The real authored starting point is visible');
  await click(idea);
  assert.equal(taskField(document)?.value, ideaFor(id).request);
  const setValue = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
  await act(async () => {
    for (const [field, value] of [[taskField(document), taskFor(id)], [reportField(document), reportFor(id)]]) {
      assert.ok(field, 'The actual installed-use editor remains mounted');
      setValue.call(field, value);
      field.dispatchEvent(new window.Event('input', { bubbles: true }));
    }
    useView(document).querySelector('.deployment-use-outcomes input[value="partly"]').click();
    await settle();
  });
  assertDrafts(document, window, id);
}
function assertDrafts(document, window, id) {
  assert.equal(taskField(document)?.value, taskFor(id));
  assert.equal(reportField(document)?.value, reportFor(id));
  assert.equal(useView(document)?.querySelector('input[value="partly"]')?.checked, true);
  const store = createInstalledUseDraftStore(window.sessionStorage), scope = installedUseScopeKey(receiptFor(id), guideFor(id));
  assert.deepEqual(store.read(scope), { task: taskFor(id), outcome: 'partly', improvement: reportFor(id) });
  assert.deepEqual(store.selectedIdea(scope), { catalogue_sha256: 'e'.repeat(64), index: 0 });
}
function assertLinks(document, id) {
  assert.equal(appLink(document)?.getAttribute('href'), entryUrl(id));
  assert.equal(appLink(document)?.textContent.trim(), `Open ${entryLabel(id)}`);
  const handoff = taskLink(document);
  assert.ok(handoff, 'The restored exact authored entry receives the current editable human task');
  assert.equal(handoff.getAttribute('aria-label'), `Continue with this task in ${entryLabel(id)}`);
  assert.equal(handoff.href.split('#')[0], entryUrl(id));
  assert.deepEqual(JSON.parse(decodeURIComponent(new URL(handoff.href).hash.slice('#mobkit-task='.length))),
    { version: 1, task: taskFor(id) });
}
async function beginRetry(document, fixture, id = A) {
  const experiences = fixture.hold(id, '/experiences'), use = fixture.hold(id, '/use');
  const before = { experiences: fixture.count(id, '/experiences'), use: fixture.count(id, '/use') };
  const retry = button(document, 'Retry UI links');
  assert.equal(retry.disabled, false, 'A settled unavailable observation has an explicit recovery action');
  await click(retry, true);
  assert.equal(fixture.count(id, '/experiences'), before.experiences + 1);
  assert.equal(fixture.count(id, '/use'), before.use + 1, 'One manual retry refreshes the independent first-use observation too');
  assert.equal(button(document, 'Retry UI links').disabled, true);
  assert.match(useView(document).textContent, /Checking this installed package's declared application UI links/);
  await click(button(document, 'Retry UI links'));
  assert.equal(fixture.count(id, '/experiences'), before.experiences + 1);
  assert.equal(fixture.count(id, '/use'), before.use + 1, 'Pending clicks admit no repeated read');
  return { experiences, use };
}
async function healthyParentPoll(document, fixture, tick, id = A) {
  const before = { parent: fixture.count(id), experiences: fixture.count(id, '/experiences'), use: fixture.count(id, '/use') };
  await tick(1500);
  assert.ok(fixture.count(id) > before.parent, 'The real parent polled its still-healthy receipt');
  assert.match(document.querySelector('.deployment-health')?.textContent || '', /process and native MobKit Console are available/);
  assert.equal(fixture.count(id, '/experiences'), before.experiences, 'An unchanged healthy parent key does not automatically retry UI metadata');
  assert.equal(fixture.count(id, '/use'), before.use, 'Parent polling does not automatically retry first-use health');
}

test('local application UI recovery manually refreshes held null links and both observations without losing human drafts', async t => {
  const fixture = offlineFixture(), initial = fixture.hold(A, '/experiences');
  await mountedPanel(t, fixture, async ({ document, window, tick }) => {
    assert.equal(initial.started, true);
    await writeDrafts(document, window, A);
    await finish(initial, experiencesFor(A, false));
    assert.equal(appLink(document) === null, true);
    assert.equal(taskLink(document) === undefined, true);
    await healthyParentPoll(document, fixture, tick);
    const retry = await beginRetry(document, fixture);
    assertDrafts(document, window, A);
    await finish(retry.use, metadataFor(A));
    assert.equal(button(document, 'Retry UI links').disabled, true, 'Ready first-use health cannot unblock still-pending UI metadata');
    await finish(retry.experiences, experiencesFor(A));
    assertLinks(document, A);
    assertDrafts(document, window, A);
  });
});

test('local application UI recovery is offered when first-use native health alone withdraws healthy declared links', async t => {
  const fixture = offlineFixture({ experiences: experiencesFor(A), metadata: metadataFor(A, false) });
  await mountedPanel(t, fixture, async ({ document, window, tick }) => {
    await writeDrafts(document, window, A);
    assert.equal(appLink(document) === null, true);
    assert.equal(taskLink(document) === undefined, true);
    assert.equal([...useView(document).querySelectorAll('a')].some(link => link.textContent.trim().startsWith('Open native Console')), false);
    assert.equal(button(document, 'Retry UI links').disabled, false, 'Recovery is not conditional on a first-use Console URL');
    await healthyParentPoll(document, fixture, tick);
    const retry = await beginRetry(document, fixture);
    await finish(retry.experiences, experiencesFor(A));
    assert.equal(button(document, 'Retry UI links').disabled, true, 'Healthy UI metadata cannot unblock a still-pending independent first-use read');
    assert.equal(taskLink(document) === undefined, true);
    await finish(retry.use, metadataFor(A));
    assertLinks(document, A);
    assertDrafts(document, window, A);
  });
});

test('local application UI recovery retains reports after a failed retry until a later deliberate retry succeeds', async t => {
  const fixture = offlineFixture();
  await mountedPanel(t, fixture, async ({ document, window, tick }) => {
    await writeDrafts(document, window, A);
    const failed = await beginRetry(document, fixture);
    await finish(failed.use, metadataFor(A));
    await fail(failed.experiences);
    assert.match(useView(document).textContent, /Declared application UI links could not be loaded/);
    assert.equal(button(document, 'Retry UI links').disabled, false);
    assert.equal(appLink(document) === null, true);
    assert.equal(taskLink(document) === undefined, true);
    assertDrafts(document, window, A);
    await healthyParentPoll(document, fixture, tick);
    const next = await beginRetry(document, fixture);
    await finish(next.experiences, experiencesFor(A));
    await finish(next.use, metadataFor(A));
    assertLinks(document, A);
    assertDrafts(document, window, A);
  });
});

test('local application UI recovery discards late A observations after the real parent selects B and retains B exact drafts and links', async t => {
  const fixture = offlineFixture();
  await mountedPanel(t, fixture, async ({ document, window }) => {
    await writeDrafts(document, window, A);
    await click(button(document, 'Select setup B'));
    await writeDrafts(document, window, B);
    assertLinks(document, B);
    await click(button(document, 'Select setup A'));
    assertDrafts(document, window, A);
    const waiting = await beginRetry(document, fixture);
    await click(button(document, 'Select setup B'));
    assert.equal(document.querySelector('main').dataset.selectedGuide, B);
    assert.equal(useView(document)?.querySelector('h3')?.textContent, `Use ${planFor(B).project.name}`);
    assertDrafts(document, window, B);
    assertLinks(document, B);
    const before = { experiences: fixture.count(B, '/experiences'), use: fixture.count(B, '/use') };
    await finish(waiting.experiences, experiencesFor(A));
    await finish(waiting.use, metadataFor(A));
    assert.equal(fixture.count(B, '/experiences'), before.experiences);
    assert.equal(fixture.count(B, '/use'), before.use);
    assert.equal(useView(document)?.querySelector('h3')?.textContent, `Use ${planFor(B).project.name}`);
    assertDrafts(document, window, B);
    assertLinks(document, B);
    assert.equal(button(document, 'Select setup B').disabled, false);
  });
});
