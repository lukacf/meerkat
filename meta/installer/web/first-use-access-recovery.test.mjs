import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
const webDir = existsSync(new URL('package.json', import.meta.url))
  ? dirname(fileURLToPath(import.meta.url)) : fileURLToPath(new URL('../../../web/', import.meta.url));
const require = createRequire(join(webDir, 'package.json'));
const { build } = require('esbuild');
const { JSDOM } = createRequire(join(webDir, '../../console/package.json'))('jsdom');
const { default: React, act } = await import(pathToFileURL(require.resolve('react')).href);
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
const directory = await mkdtemp(join(tmpdir(), 'forge-first-use-access-recovery-'));
after(() => rm(directory, { recursive: true, force: true }));
const modulePath = join(directory, 'components.mjs');
const bundled = await build({
  stdin: { contents: 'export { DeploymentPanel } from "./deployment-panel.jsx"; export { InstalledNativeTeamUse } from "./deployment-use.jsx";',
    resolveDir: webDir, sourcefile: 'first-use-access-recovery-entry.mjs' },
  bundle: true, write: false, format: 'esm', platform: 'node', outfile: modulePath,
  sourcemap: 'linked', sourcesContent: true, logLevel: 'silent', loader: { '.css': 'empty' },
  nodePaths: [join(webDir, 'node_modules')], plugins: [
    { name: 'shared-real-react', setup(builder) {
      builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } },
  ],
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

function offlineFixture() {
  const base = deploymentUrl(A), calls = [], unexpected = [], held = new Map();
  const responses = new Map([
    [base, receiptFor(A)], [base + `/options?build_id=${buildFor(A).build_id}`, optionsFor(A)],
    [base + '/experiences', experiencesFor(A)], [base + '/use', metadataFor(A)],
    [`/api/guide/${A}/gcp-deployment`, null],
    [`/api/guide/${A}/remote-deployment`, { httpError: 404, detail: 'No saved remote deployment.' }],
  ]);
  return { calls, unexpected,
    count(tail) { return calls.filter(call => call.url === base + tail).length; },
    holdBody(tail) {
      const url = base + tail;
      assert.ok(responses.has(url));
      let resolve;
      const promise = new Promise(done => { resolve = done; });
      const read = { promise, resolve, started: false, signal: null };
      held.set(url, read);
      return read;
    },
    async fetch(url, options = {}) {
      const call = { url: String(url), method: options.method || 'GET' };
      calls.push(call);
      if (call.method !== 'GET' || options.body != null || !responses.has(call.url)) {
        unexpected.push(call); throw Error(`Unexpected inert request: ${call.method} ${call.url}`);
      }
      const value = clone(responses.get(call.url)), read = held.get(call.url);
      if (read) { held.delete(call.url); read.signal = options.signal; }
      return { ok: !value?.httpError, status: value?.httpError || 200,
        json: async () => {
          if (read) { read.started = true; return clone(await read.promise); }
          return value?.httpError ? { detail: value.detail } : value;
        } };
    },
  };
}

async function mounted(t, fixture, verify, { nativeTeam = false } = {}) {
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
  let reactRoot;
  try {
    const { DeploymentPanel, InstalledNativeTeamUse } = await import(pathToFileURL(modulePath).href);
    const { createRoot } = await import(pathToFileURL(require.resolve('react-dom/client')).href);
    const admissions = [], receipts = [];
    const props = nativeTeam
      ? { state: guideFor(A), receipt: nativeReceipt(), Icon: () => null }
      : { state: guideFor(A), plan: planFor(A), build: buildFor(A), designBusy: false, pendingManual: false,
        Icon: () => null, onRepairReceipt: value => admissions.push(value), onReceipt: value => receipts.push(value) };
    reactRoot = createRoot(dom.window.document.getElementById('test-root'));
    await act(async () => { reactRoot.render(React.createElement(nativeTeam ? InstalledNativeTeamUse : DeploymentPanel, props)); await settle(); });
    await act(async () => { await settle(); });
    await verify({ document: dom.window.document, window: dom.window, receipts,
      tick: async milliseconds => {
        await act(async () => { t.mock.timers.tick(milliseconds); await settle(); });
        await act(async () => { await settle(); });
      } });
    assert.deepEqual(fixture.unexpected, []);
    assert.ok(fixture.calls.every(call => call.method === 'GET'), 'Checking and retrying metadata must never send an installation, job or feedback command');
    assert.deepEqual(admissions, []);
    if (!nativeTeam) assert.ok(receipts.every(row => row === null || row.deployment_id === receiptFor(A).deployment_id && row.status === 'running'),
      'Metadata recovery must not change the original installation');
  } finally {
    if (reactRoot) await act(async () => reactRoot.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

const useView = document => document.querySelector('.deployment-use');
const taskField = document => useView(document)?.querySelector('.deployment-use-field textarea');
function findButton(document, label) {
  return [...document.querySelectorAll('button')].find(item => item.textContent.trim() === label);
}
function button(document, label) {
  const node = findButton(document, label); assert.ok(node, `Missing actual recovery control: ${label}`); return node;
}
async function click(node) {
  await act(async () => { node.click(); await settle(); });
  await act(async () => { await settle(); });
}
async function editTask(document, window) {
  const field = taskField(document); assert.ok(field); assert.equal(field.disabled, false);
  const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
  await act(async () => { setter.call(field, taskFor(A)); field.dispatchEvent(new window.Event('input', { bubbles: true })); await settle(); });
  assert.equal(taskField(document)?.value, taskFor(A));
}
async function finish(read, value) {
  assert.equal(read.started, true);
  await act(async () => { read.resolve(clone(value)); await settle(); });
  await act(async () => { await settle(); });
}
function nativeReceipt() { return { ...receiptFor(A), source_kind: 'native-team', source_id: '9'.repeat(64), build_id: null }; }
function nativeMetadata() { return { ...metadataFor(A), source_kind: 'native-team', source_id: '9'.repeat(64), build_id: null,
  continuation: { available: false }, success_reporting_available: false }; }

test('a stalled application UI metadata body releases its actual Retry UI links control and keeps the unsent job', async t => {
  const fixture = offlineFixture(), held = fixture.holdBody('/experiences');
  await mounted(t, fixture, async ({ document, window, tick }) => {
    assert.equal(held.started, true);
    assert.equal(button(document, 'Retry UI links').disabled, true);
    await editTask(document, window);
    await tick(29999);
    assert.equal(button(document, 'Retry UI links').disabled, true);
    await tick(1);
    assert.equal(button(document, 'Retry UI links').disabled, false, 'A stalled metadata body must release the existing explicit retry');
    assert.match(useView(document).textContent, /Declared application UI links could not be loaded/);
    assert.equal(taskField(document).value, taskFor(A));
    const before = fixture.count('/experiences');
    await click(button(document, 'Retry UI links'));
    assert.equal(fixture.count('/experiences'), before + 1, 'Only the explicit retry repeats the metadata read');
    assert.equal(document.querySelector(`a[href="${entryUrl(A)}"]`)?.href, entryUrl(A));
    assert.equal(taskField(document).value, taskFor(A));
    await finish(held, experiencesFor(A, false));
    assert.equal(document.querySelector(`a[href="${entryUrl(A)}"]`)?.href, entryUrl(A), 'An obsolete body must not overwrite the retried link');
    assert.equal(taskField(document).value, taskFor(A));
  });
});

test('a stalled application first-use body exposes Check first-use details and preserves its draft through recovery', async t => {
  const fixture = offlineFixture(), held = fixture.holdBody('/use');
  await mounted(t, fixture, async ({ document, window, tick }) => {
    assert.equal(held.started, true);
    assert.match(useView(document).textContent, /Loading this installed application's first-use details/);
    assert.equal(findButton(document, 'Check first-use details') === undefined, true);
    await editTask(document, window);
    await tick(29999);
    assert.equal(findButton(document, 'Check first-use details') === undefined, true);
    await tick(1);
    assert.equal(button(document, 'Check first-use details').disabled, false, 'A stalled first-use body must expose its existing recovery action');
    assert.match(useView(document).textContent, /first-use details could not be checked/);
    assert.equal(taskField(document).value, taskFor(A));
    const before = fixture.count('/use');
    await click(button(document, 'Check first-use details'));
    assert.equal(fixture.count('/use'), before + 1);
    assert.equal(document.querySelector(`a[href="${entryUrl(A)}"]`)?.href, entryUrl(A));
    assert.equal(taskField(document).value, taskFor(A));
    await finish(held, { ...metadataFor(A), project: { name: 'OBSOLETE_LATE_BODY' } });
    assert.doesNotMatch(useView(document).textContent, /OBSOLETE_LATE_BODY/);
    assert.equal(taskField(document).value, taskFor(A));
  });
});

test('a stalled agent-team first-use body exposes explicit recheck and only the recovered exact Console owner', async t => {
  const fixture = offlineFixture(), held = fixture.holdBody('/use');
  // This direct actual native-team view only reads its source-bound /use endpoint.
  const originalFetch = fixture.fetch.bind(fixture);
  fixture.fetch = (url, options) => originalFetch(url, options).then(response => ({ ...response,
    json: async () => { const value = await response.json(); return value?.build_id ? nativeMetadata() : value; } }));
  await mounted(t, fixture, async ({ document, tick }) => {
    assert.equal(held.started, true);
    assert.match(useView(document).textContent, /Checking this installed team's conversation owner/);
    assert.equal(findButton(document, 'Recheck agent team details') === undefined, true);
    assert.equal(useView(document).querySelector('a[href]') === null, true);
    await tick(29999);
    assert.equal(findButton(document, 'Recheck agent team details') === undefined, true);
    await tick(1);
    assert.equal(button(document, 'Recheck agent team details').disabled, false, 'A stalled native-team metadata read must become explicitly recheckable');
    assert.match(useView(document).textContent, /conversation owner could not be checked/);
    assert.equal(useView(document).querySelector('a[href]') === null, true);
    await click(button(document, 'Recheck agent team details'));
    assert.equal(fixture.count('/use'), 2);
    assert.equal(useView(document).querySelector('a[href]')?.href, nativeReceipt().console_url);
    assert.match(useView(document).textContent, /Open the Console and select Observing lead/);
    await finish(held, { ...nativeMetadata(), entrypoint: { identity: 'wrong:owner', name: 'OBSOLETE_LATE_OWNER' } });
    assert.doesNotMatch(useView(document).textContent, /OBSOLETE_LATE_OWNER/);
    assert.equal(useView(document).querySelector('a[href]')?.href, nativeReceipt().console_url);
  }, { nativeTeam: true });
});
