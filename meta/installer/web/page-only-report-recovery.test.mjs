import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build as bundle } from 'esbuild';
import React, { act } from 'react';
import { createInstalledUseImprovementStore, createInstalledUseSuccessStore, getInstalledUsePageStores,
  installedUseScopeKey, installedUseFeedback, installedUseImprovementHint,
  installedUseSuccessBody, installedUseUrl } from './deployment-use.mjs';
import { remoteInstalledUseBinding, remoteInstalledUseDraftScope, remoteInstalledUseFeedback,
  remoteInstalledUseImprovementHint, remoteInstalledUseSuccessBody, remoteInstalledUseUrl,
  createRemoteInstalledUseImprovementStore, createRemoteInstalledUseSuccessStore } from './remote-installed-use.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
const directory = await mkdtemp(join(tmpdir(), 'mobkit-page-only-report-recovery-'));
const modulePath = join(directory, 'installed-report-controls.mjs');
after(() => rm(directory, { recursive: true, force: true }));

// This module is executed only after the complete source batch is reviewed.
// Actual local and remote components share React/ReactDOM with the harness;
// the file-backed bundle and linked map keep failure diagnostics bounded.
const bundled = await bundle({
  stdin: { contents: 'export { InstalledApplicationUseMount } from "./deployment-use.jsx";\n' +
    'export { RemoteInstalledUse } from "./remote-installed-use.jsx";\n',
    resolveDir: fileURLToPath(new URL('./', import.meta.url)), sourcefile: 'installed-report-controls.jsx', loader: 'jsx' },
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

const state = { id: '11111111-1111-4111-8111-111111111111', version: 4, status: 'ready', operation: null };
const source = 'a'.repeat(64), archive = 'b'.repeat(64);
const checked = { guide_id: state.id, guide_version: state.version, current: true,
  source_verified: true, build_id: 'accepted-observing-source', source_digest: source,
  status: 'completed', cleanup_pending: false };
const local = { guide_id: state.id, guide_version: state.version,
  deployment_id: '22222222-2222-4222-8222-222222222222', source_kind: 'application',
  build_id: checked.build_id, source_digest: source, package_sha256: archive,
  status: 'stopped', worker_live: false, console_url: null, current: true };
const remote = { protocol: 1, target: 'linux', guide_id: state.id, guide_version: state.version,
  deployment_id: local.deployment_id, build_id: checked.build_id,
  accepted_source_digest: source, package_sha256: archive,
  transferred_source_digest: 'c'.repeat(64), controller_sha256: 'd'.repeat(64),
  endpoint: { host: 'synthetic-linux.local', user: 'fixture-user', ssh_port: 22 },
  host: { uid: 1001, home: '/home/fixture-user', destination: '/home/fixture-user/src/observing',
    platform: 'linux', python: '3.11.9' }, destination: '/home/fixture-user/src/observing',
  host_key_fingerprint: 'SHA256:' + 'A'.repeat(43), status: 'stopped', last_known_status: 'stopped',
  remote_admission_requested: true, admitted: true, worker_live: false,
  observation: { available: false }, source_current: false, ready: false,
  application_experiences: { protocol: 1, supported: true, status: 'none', source_digest: source,
    installed_source_digest: 'c'.repeat(64), metadata_sha256: null, entries: [] } };
const firstUse = { scope: 'authored-first-use-suggestions', metadata_sha256: 'e'.repeat(64),
  tasks: [{ label: 'Review observing notes', request: 'Compare café notes with 觀測 🌌.\nKeep the context.',
    description: 'Explain one useful next step from my notes.' }], detail: null };
const draft = { task: '  My café observing task with 觀測 🌌.\nKeep these exact spaces.  ',
  outcome: 'partly', improvement: '  Explain the evidence for this result.\nKeep my report and spacing.  ' };
const requestId = '33333333-3333-4333-8333-333333333333';

function metadata(kind, fixture = { state, checked, local, remote }) {
  const { state, checked, local, remote } = fixture;
  const receipt = kind === 'local' ? local : remote;
  const common = { guide_id: state.id, version: state.version, guide_version: receipt.guide_version,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id, source_digest: source,
    package_sha256: archive, status: receipt.status, current: true,
    project: { name: 'Observing notes' }, entrypoint: { identity: 'lead:main', name: 'Observing lead' },
    application_outcomes_verified: false, native_runtime_available: false,
    success_report: null, success_report_detail: null, success_reporting_available: true,
    first_use: structuredClone(firstUse), continuation: { build_id: checked.build_id, source_digest: checked.source_digest,
      available: true, requires_acknowledgement: false } };
  return kind === 'local' ? { ...common, scope: 'verified-installed-source', console_url: null }
    : { ...common, protocol: 1, scope: 'frozen-remote-package-source', accepted_source_digest: source,
      transferred_source_digest: remote.transferred_source_digest, controller_sha256: remote.controller_sha256,
      remote_binding: JSON.parse(remoteInstalledUseBinding(remote, state)), installed_source_verified: false,
      observation: { scope: 'saved-destination-observation', status: remote.status, available: false,
        freshness: 'unavailable', admitted: false, source_current: false } };
}
const receiptFor = kind => kind === 'local' ? local : remote;
const scopeFor = kind => kind === 'local' ? installedUseScopeKey(local, state) : remoteInstalledUseDraftScope(remote, state);
const fixtureA = { state, checked, local, remote };
const useUrl = (kind, operation = 'use', fixture = fixtureA) => kind === 'local' ? installedUseUrl(fixture.state.id, operation)
  : remoteInstalledUseUrl(fixture.state.id, fixture.remote.deployment_id, operation);
function otherFixture(kind) {
  const next = structuredClone(fixtureA);
  if (kind === 'local') {
    next.state.id = '55555555-5555-4555-8555-555555555555';
    next.checked.guide_id = next.state.id; next.local.guide_id = next.state.id;
    next.local.deployment_id = '66666666-6666-4666-8666-666666666666';
  } else {
    // All draft-scope fields are identical. Only the exact destination binding differs.
    next.remote.endpoint.host = 'other-synthetic-linux.local';
    next.remote.destination = '/home/fixture-user/src/another-observing';
    next.remote.host.destination = next.remote.destination;
  }
  return next;
}
const improvementLabel = kind => kind === 'local' ? 'Improve this behavior' : 'Ask builder to improve it';
const successResponse = body => ({ ...body, scope: 'human-reported-installed-success', guide_id: state.id,
  outcome: 'worked', application_outcomes_verified: false, recorded_at: '2026-10-07T01:30:00Z' });
const reply = (value, status = 200) => ({ ok: status >= 200 && status < 300, status,
  json: async () => structuredClone(value) });

function deniedStorage() {
  const calls = [];
  return { calls,
    getItem(key) { calls.push(['get', key]); throw Error('PRIVATE_BROWSER_STORAGE'); },
    setItem(key) { calls.push(['set', key]); throw Error('PRIVATE_BROWSER_STORAGE'); },
    removeItem(key) { calls.push(['remove', key]); throw Error('PRIVATE_BROWSER_STORAGE'); },
  };
}
async function mounted(kind, t, post, verify, options = {}) {
  t.mock.timers.enable({ apis: ['setInterval', 'setTimeout'] });
  const dom = new JSDOM('<div id="test-root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const calls = [], unexpected = [], publications = [], storage = options.storage ?? deniedStorage();
  Object.defineProperty(dom.window, 'sessionStorage', options.storageGetterDenied
    ? { configurable: true, get() { throw Error('PRIVATE_BROWSER_STORAGE'); } }
    : { configurable: true, value: storage });
  const fixtureB = otherFixture(kind);
  const allowedGets = new Map([[useUrl(kind), metadata(kind)], [`/api/guide/${state.id}/implementation`, checked],
    [useUrl(kind, 'use', fixtureB), metadata(kind, fixtureB)], [`/api/guide/${fixtureB.state.id}/implementation`, fixtureB.checked]]);
  allowedGets.set(useUrl(kind), metadata(kind));
  const allowedPosts = new Set([useUrl(kind, 'feedback'), useUrl(kind, 'success')]);
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: options.globalStorage ?? storage, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (url, options = {}) => {
      const target = String(url), method = options.method || 'GET';
      const body = options.body == null ? null : JSON.parse(options.body);
      const call = { target, method, body, signal: options.signal }; calls.push(call);
      if (method === 'GET' && body === null && allowedGets.has(target)) {
        const value = allowedGets.get(target);
        return typeof value === 'function' ? value(call) : reply(value);
      }
      if (method === 'POST' && body !== null && allowedPosts.has(target)) return post(call, calls);
      unexpected.push(call); throw Error(`Unexpected offline request: ${method} ${target}`);
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  let root;
  try {
    const loaded = await import(pathToFileURL(modulePath).href);
    const Component = kind === 'local' ? loaded.InstalledApplicationUseMount : loaded.RemoteInstalledUse;
    const { createRoot } = await import('react-dom/client');
    root = createRoot(dom.window.document.getElementById('test-root'));
    const render = async fixture => {
      allowedGets.set(useUrl(kind, 'use', fixture), metadata(kind, fixture));
      const props = { state: fixture.state, receipt: kind === 'local' ? fixture.local : fixture.remote,
        build: fixture.checked, blocked: false, Icon: () => null, selectionScope: options.selectionScope,
        onRepairReceipt: value => publications.push(value) };
      if (kind === 'local') props.experiences = { deployment_id: fixture.local.deployment_id, build_id: fixture.local.build_id,
        source_digest: source, package_sha256: archive, metadata_sha256: null, scope: 'declared-source',
        entries: [], verified: false };
      await act(async () => { root.render(React.createElement(Component, props)); await settle(); });
      await act(async () => { await settle(); });
    };
    await render(fixtureA);
    const button = label => [...dom.window.document.querySelectorAll('button')]
      .find(node => node.textContent.trim() === label || node.querySelector('strong')?.textContent === label);
    const click = async label => {
      const node = button(label); assert.ok(node, `Missing actual control: ${label}`);
      await act(async () => { node.click(); await settle(); });
      await act(async () => { await settle(); });
    };
    const writeDraft = async (next = draft) => {
      const disclosure = dom.window.document.querySelector('.deployment-use-feedback-disclosure');
      if (!disclosure.open) {
        await act(async () => { disclosure.querySelector('summary').click(); await settle(); });
      }
      assert.equal(disclosure.open, true);
      const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set;
      await act(async () => {
        for (const [selector, value] of [['.deployment-use-field textarea', next.task],
          ['.deployment-use-feedback textarea', next.improvement]]) {
          const field = dom.window.document.querySelector(selector);
          assert.ok(field); setter.call(field, value); field.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
        }
        dom.window.document.querySelector(`.deployment-use-feedback input[value="${next.outcome}"]`).click();
        await settle();
      });
      assertDrafts(dom.window.document, next);
    };
    const edit = async (next = draft) => {
      await click(firstUse.tasks[0].label);
      assert.equal(dom.window.document.querySelector('.deployment-use-field textarea').value, firstUse.tasks[0].request);
      await writeDraft(next);
    };
    await verify({ document: dom.window.document, window: dom.window, button, click, edit, writeDraft, calls, storage, publications,
      select: owner => render(owner === 'A' ? fixtureA : fixtureB),
      selectBuild: value => render({ ...fixtureA, checked: value }),
      remount: async () => {
        await act(async () => { root.render(null); await settle(); });
        await render(fixtureA);
      },
      observeBuild: value => allowedGets.set(`/api/guide/${state.id}/implementation`, value),
      idle: async () => { await act(async () => { t.mock.timers.tick(2000); await settle(); }); } });
    assert.deepEqual(unexpected, [], 'Only exact installed-source, Build readback and human-report routes are allowed');
    assert.ok(calls.every(call => call.method === 'GET' || allowedPosts.has(call.target)));
    assert.doesNotMatch(dom.window.document.body.textContent, /PRIVATE_BROWSER_STORAGE|PRIVATE_REFUSAL_DETAIL/);
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    t.mock.timers.reset();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}
function assertDrafts(document, expected = draft) {
  assert.equal(document.querySelector('.deployment-use-field textarea')?.value, expected.task);
  assert.equal(document.querySelector('.deployment-use-feedback textarea')?.value, expected.improvement);
  assert.equal(document.querySelector(`.deployment-use-feedback input[value="${expected.outcome}"]`)?.checked, true);
}
const postsFor = (calls, kind, operation) => calls.filter(call => call.method === 'POST' && call.target === useUrl(kind, operation));

function heldJson() {
  let resolve, reject, entered = false, pending = true;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { response: { ok: true, status: 200, json: () => { entered = true; return promise; } },
    entered: () => entered, pending: () => pending,
    resolve(value) { pending = false; resolve(value); }, reject(error) { pending = false; reject(error); } };
}
const finishedImprovement = { ...checked, build_id: 'checked-improvement-candidate', source_digest: 'f'.repeat(64),
  request: 'An independently checked candidate request.' };

for (const kind of ['local', 'remote']) {
  for (const persistent of [false, true]) {
    test(`late exact improvement custody ${kind} retains admission after Check Build with ${persistent ? 'persistent' : 'page-only'} tracking`, async t => {
      const original = heldJson(), reading = heldJson();
      const pendingDraft = { ...newerDraft, outcome: draft.outcome };
      await mounted(kind, t, () => original.response,
        async ({ document, button, click, edit, writeDraft, observeBuild, calls, publications, idle, storage }) => {
          await edit(); await click(improvementLabel(kind));
          await click('Check Build status');
          await writeDraft(pendingDraft);
          observeBuild(() => reading.response); await click('Check Build status');
          assert.equal(reading.entered(), true);
          const before = structuredClone(publications);
          await act(async () => { original.resolve(finishedImprovement); await settle(); });
          await idle(); assertDrafts(document, pendingDraft);
          assert.ok(document.querySelector('.installed-improvement-handoff'), 'The exact late POST still proves its own admission');
          assert.deepEqual(publications, before, 'The newer Build read owns candidate publication');
          assert.equal(button('Checking Build status...')?.disabled, true, 'POST completion cannot release the pending GET');
          if (persistent) {
            const saved = kind === 'local' ? createInstalledUseImprovementStore(storage).read(scopeFor(kind))
              : createRemoteInstalledUseImprovementStore(storage, remoteInstalledUseBinding(remote, state)).read(scopeFor(kind));
            assert.equal(saved.buildId, finishedImprovement.build_id);
            assert.equal(saved.hostRequest, finishedImprovement.request);
          }
          await act(async () => { reading.resolve(checked); await settle(); });
          await idle(); assertDrafts(document, pendingDraft);
          assert.equal(publications.at(-1).build_id, checked.build_id, 'The independent GET publishes its own receipt');
          assert.equal(!!button('Prepare another report'), false, 'The predecessor observation cannot finish the newly admitted report');
          assert.equal(postsFor(calls, kind, 'feedback').length, 1);
          assert.equal(postsFor(calls, kind, 'success').length, 0);
        }, persistent ? { storage: memoryStorage() } : {});
    });

    test(`late exact improvement custody ${kind} releases a coded refusal after Check Build with ${persistent ? 'persistent' : 'page-only'} tracking`, async t => {
      const original = heldJson(), reading = heldJson();
      const pendingDraft = { ...newerDraft, outcome: draft.outcome };
      original.response.ok = false; original.response.status = 422;
      await mounted(kind, t, () => original.response,
        async ({ document, button, click, edit, writeDraft, observeBuild, calls, publications, idle }) => {
          await edit(); await click(improvementLabel(kind)); await click('Check Build status');
          await writeDraft(pendingDraft);
          observeBuild(() => reading.response); await click('Check Build status');
          const before = structuredClone(publications);
          await act(async () => { original.resolve({ detail: 'PRIVATE_REFUSAL_DETAIL', code: 'installed_feedback_not_admitted' }); await settle(); });
          await idle(); assertDrafts(document, pendingDraft);
          assert.match(document.body.textContent, /The builder did not receive this report/);
          assert.equal(button(improvementLabel(kind)).disabled, false, 'An owner-proven refusal permits an explicit retry');
          await act(async () => { reading.resolve(finishedImprovement); await settle(); });
          await idle(); assertDrafts(document, pendingDraft);
          assert.deepEqual(publications, before, 'A read belonging to the refused report cannot revive its tracking');
          assert.equal(postsFor(calls, kind, 'feedback').length, 1);
          assert.equal(postsFor(calls, kind, 'success').length, 0);
        }, persistent ? { storage: memoryStorage() } : {});
    });
  }

  for (const result of ['success', 'error', 'refusal']) {
    test(`pending improvement response recovery ${kind} explicitly prepares a new report without replacing the original body after late ${result}`, async t => {
      const original = heldJson();
      if (result === 'refusal') { original.response.ok = false; original.response.status = 422; }
      await mounted(kind, t, call => {
        assert.equal(call.target, useUrl(kind, 'feedback'));
        return original.response;
      }, async ({ document, button, click, edit, writeDraft, selectBuild, observeBuild, calls, publications, idle }) => {
        await edit(); await click(improvementLabel(kind));
        assert.equal(original.entered(), true); assert.equal(original.pending(), true);
        assert.equal(postsFor(calls, kind, 'feedback')[0].signal, undefined);
        assert.equal(button('Check Build status').disabled, false, 'Observation is available while the original JSON body is withheld');
        await selectBuild(finishedImprovement); observeBuild(finishedImprovement);
        await click('Check Build status');
        assertDrafts(document);
        assert.equal(document.querySelector('.installed-improvement-handoff') === null, true, 'A newer unrelated Build is not admission of this report');
        assert.match(document.body.textContent, /does not confirm whether your earlier report was admitted/);
        assert.equal(button('Prepare another report')?.disabled, false);
        assert.equal(document.querySelector('.deployment-use-feedback button[type="submit"]').disabled, true);
        assert.equal(original.pending(), true, 'Readback retains the original entered mutation');
        assert.match(document.body.textContent, /original response remains pending; preparing another report does not cancel it/);
        await click('Prepare another report');
        assertDrafts(document);
        assert.equal(button(improvementLabel(kind)).disabled, true, 'The unchanged earlier report is not resent');
        await writeDraft(newerDraft);
        const beforeLate = structuredClone(publications), beforeReads = calls.filter(call => call.method === 'GET').length;
        await act(async () => {
          if (result === 'error') original.reject(Error('PRIVATE_REFUSAL_DETAIL'));
          else if (result === 'refusal') original.resolve({ detail: 'PRIVATE_REFUSAL_DETAIL', code: 'installed_feedback_not_admitted' });
          else original.resolve({ ...finishedImprovement, build_id: 'late-original-build', status: 'queued', request: 'Late original admission.' });
          await settle();
        });
        await idle();
        assertDrafts(document, newerDraft);
        assert.equal(document.querySelector('.installed-improvement-handoff') === null, true);
        assert.deepEqual(publications, beforeLate);
        assert.equal(calls.filter(call => call.method === 'GET').length, beforeReads);
        assert.equal(postsFor(calls, kind, 'feedback').length, 1);
        assert.equal(postsFor(calls, kind, 'success').length, 0);
        assert.doesNotMatch(document.body.textContent, /Admission of your improvement request is unconfirmed|could not be confirmed/);
      });
    });
  }

  for (const result of ['success', 'refusal']) {
    test(`late exact improvement custody ${kind} ignores ${result} after persistent tracking is superseded`, async t => {
      const original = heldJson(), storage = memoryStorage();
      if (result === 'refusal') { original.response.ok = false; original.response.status = 422; }
      await mounted(kind, t, () => original.response,
        async ({ document, click, edit, calls, publications, idle }) => {
          await edit(); await click(improvementLabel(kind)); await click('Check Build status');
          const other = kind === 'local' ? createInstalledUseImprovementStore(storage)
            : createRemoteInstalledUseImprovementStore(storage, remoteInstalledUseBinding(remote, state));
          const newer = { ...other.read(scopeFor(kind)), request: 'A distinct report from the returning owner.' };
          assert.equal(other.save(scopeFor(kind), newer), true);
          const before = structuredClone(publications), bytes = [...storage.values];
          await act(async () => {
            original.resolve(result === 'success' ? finishedImprovement
              : { detail: 'PRIVATE_REFUSAL_DETAIL', code: 'installed_feedback_not_admitted' });
            await settle();
          });
          await idle(); assertDrafts(document);
          assert.deepEqual([...storage.values], bytes, 'A late reply cannot overwrite or remove newer retained tracking');
          assert.deepEqual(publications, before);
          assert.equal(document.querySelector('.installed-improvement-handoff') === null, true);
          assert.doesNotMatch(document.body.textContent, /The builder did not receive this report/);
          assert.equal(postsFor(calls, kind, 'feedback').length, 1);
        }, { storage });
    });
  }

  test(`pending improvement response recovery ${kind} retains uncertainty through inconclusive reads and late POST failure`, async t => {
    const original = heldJson();
    await mounted(kind, t, () => original.response,
      async ({ document, button, click, edit, observeBuild, selectBuild, calls, publications, idle }) => {
        await edit(); await click(improvementLabel(kind));
        assert.equal(button('Check Build status').disabled, false);
        await click('Check Build status');
        assert.equal(button('Prepare another report') === undefined, true, 'The original continuation Build cannot release the report');
        observeBuild(() => { throw Error('PRIVATE_REFUSAL_DETAIL'); });
        await click('Check Build status');
        assert.match(document.body.textContent, /Build status could not be checked/);
        assert.equal(button('Prepare another report') === undefined, true);
        assert.equal(button('Check Build status').disabled, false);
        observeBuild({ ...finishedImprovement, guide_id: 'foreign-guide' });
        await click('Check Build status');
        assert.equal(button('Prepare another report') === undefined, true);
        await selectBuild(finishedImprovement);
        for (const patch of [{ cleanup_pending: true }, { source_verified: false }, { status: 'running' }, { build_id: 'unselected-new-build' }]) {
          observeBuild({ ...finishedImprovement, ...patch }); await click('Check Build status');
          assert.equal(button('Prepare another report') === undefined, true, 'Only the exact selected, terminal, checked and cleaned source can prepare a new report');
        }
        const before = structuredClone(publications), finalRead = heldJson();
        observeBuild(() => finalRead.response); await click('Check Build status');
        assert.equal(finalRead.entered(), true);
        await act(async () => { original.reject(Error('PRIVATE_REFUSAL_DETAIL')); await settle(); });
        await idle(); assertDrafts(document);
        assert.deepEqual(publications, before);
        assert.equal(document.querySelector('.installed-improvement-handoff') === null, true);
        assert.equal(postsFor(calls, kind, 'feedback').length, 1);
        assert.equal(button('Checking Build status...').disabled, true, 'The original POST finally cannot release the entered GET');
        await act(async () => { finalRead.resolve(finishedImprovement); await settle(); });
        await idle();
        assert.equal(button('Prepare another report')?.disabled, false);
      });
  });

  test(`pending improvement response recovery ${kind} fences held readback and response after selection changes and remount`, async t => {
    const original = heldJson(), reading = heldJson();
    let epoch = 1, current = true;
    const selectionScope = { token: () => epoch,
      capture: (_state, token = epoch) => { const captured = epoch; return () => current && captured === epoch && token === epoch; } };
    await mounted(kind, t, () => original.response,
      async ({ document, button, click, edit, writeDraft, observeBuild, select, remount, calls, publications, idle }) => {
        await edit(); await click(improvementLabel(kind));
        observeBuild(() => reading.response);
        await click('Check Build status');
        assert.equal(reading.entered(), true);
        assert.equal(button('Checking Build status...').disabled, true);
        current = false; epoch++;
        await act(async () => { reading.resolve(finishedImprovement); await settle(); });
        await idle();
        assert.deepEqual(publications, []);
        assert.equal(button('Prepare another report') === undefined, true);
        current = true;
        observeBuild(checked);
        await select('B'); await edit(otherDraft);
        await select('A'); await idle(); assertDrafts(document);
        await writeDraft(newerDraft); await remount(); await idle(); assertDrafts(document, newerDraft);
        const before = structuredClone(publications);
        await act(async () => { original.resolve({ ...finishedImprovement, build_id: 'late-remounted-build', request: 'Late original report.' }); await settle(); });
        await idle(); assertDrafts(document, newerDraft);
        assert.deepEqual(publications, before, 'Retired original publication cannot replace the remounted owner');
        assert.equal(document.querySelector('.installed-improvement-handoff') === null, true);
        assert.equal(button(improvementLabel(kind)).disabled, true);
        assert.equal(postsFor(calls, kind, 'feedback').length, 1);
        await select('B'); assertDrafts(document, otherDraft);
      }, { selectionScope });
  });
}

for (const kind of ['local', 'remote']) {
  test(`page-only report recovery ${kind} coded feedback refusal releases only its exact unsaved hint for deliberate retry`, async t => {
    const status = kind === 'local' ? 409 : 422;
    await mounted(kind, t, (call, calls) => {
      assert.equal(call.target, useUrl(kind, 'feedback'));
      return postsFor(calls, kind, 'feedback').length === 1
        ? reply({ code: 'installed_feedback_not_admitted', detail: 'PRIVATE_REFUSAL_DETAIL' }, status)
        : reply({ guide_id: state.id, guide_version: state.version, build_id: 'new-explicit-improvement',
          status: 'queued', request: 'Host accepted this explicit improvement.' });
    }, async ({ document, button, click, edit, calls, idle }) => {
      await edit(); assert.equal(button(improvementLabel(kind)).disabled, false);
      await click(improvementLabel(kind));
      assertDrafts(document);
      assert.match(document.body.textContent, /did not receive this report/);
      assert.match(document.body.textContent, /Browser draft storage is unavailable/);
      assert.equal(button('Retry clearing report tracking') === undefined, true);
      assert.equal(button(improvementLabel(kind)).disabled, false);
      assert.equal(postsFor(calls, kind, 'feedback').length, 1);
      await idle(); assert.equal(postsFor(calls, kind, 'feedback').length, 1, 'A host refusal never automatically resends');
      await click(improvementLabel(kind));
      const posts = postsFor(calls, kind, 'feedback');
      assert.equal(posts.length, 2); assert.deepEqual(posts[1].body, posts[0].body);
      assert.equal(posts[0].body.task, draft.task.trim()); assert.equal(posts[0].body.request, draft.improvement.trim());
      assert.equal(postsFor(calls, kind, 'success').length, 0);
      assert.equal(document.querySelector('.installed-improvement-handoff')?.dataset.buildId, 'new-explicit-improvement');
      assert.match(document.body.textContent, /open page only/); assertDrafts(document);
    });
  });

  test(`page-only report recovery ${kind} coded success refusal releases its exact page retry without claiming durable removal`, async t => {
    await mounted(kind, t, (call, calls) => {
      assert.equal(call.target, useUrl(kind, 'success'));
      return postsFor(calls, kind, 'success').length === 1
        ? reply({ code: 'installed_success_not_admitted', detail: 'PRIVATE_REFUSAL_DETAIL' }, 409)
        : reply(successResponse(call.body));
    }, async ({ document, button, click, edit, calls, idle, publications }) => {
      await edit(); assert.equal(button('It worked').disabled, false);
      await click('It worked'); assertDrafts(document);
      assert.match(document.querySelector('.deployment-use-success').textContent, /earlier success report was not saved/);
      assert.equal(button('Clear unsaved confirmation') === undefined, true);
      assert.equal(button('Retry success confirmation') === undefined, true);
      assert.equal(button('It worked').disabled, false);
      await idle(); assert.equal(postsFor(calls, kind, 'success').length, 1);
      await click('It worked');
      const posts = postsFor(calls, kind, 'success');
      assert.equal(posts.length, 2); assert.notEqual(posts[1].body.request_id, posts[0].body.request_id);
      const { request_id: firstId, ...first } = posts[0].body;
      const { request_id: secondId, ...second } = posts[1].body;
      assert.deepEqual(second, first); assert.equal(first.task, draft.task.trim());
      assert.match(document.querySelector('.deployment-use-success').textContent, /An earlier success report is saved for this job/);
      assert.match(document.body.textContent, /Browser draft storage is unavailable/);
      assertDrafts(document); assert.deepEqual(publications, []);
      assert.equal(postsFor(calls, kind, 'feedback').length, 0);
    });
  });

  test(`page-only report recovery ${kind} ambiguous feedback and success stay retained without automatic new admission`, async t => {
    await mounted(kind, t, call => {
      if (call.target === useUrl(kind, 'feedback')) return reply({ detail: 'PRIVATE_REFUSAL_DETAIL' }, kind === 'local' ? 409 : 422);
      assert.equal(call.target, useUrl(kind, 'success')); throw Error('Synthetic response lost before confirmation.');
    }, async ({ document, button, click, edit, calls, idle }) => {
      await edit(); await click(improvementLabel(kind));
      assertDrafts(document); assert.equal(button(improvementLabel(kind)).disabled, true);
      assert.equal(button('Retry clearing report tracking') === undefined, true);
      await click('Check Build status');
      assert.equal(button(improvementLabel(kind)).disabled, true, 'Unchanged old Build cannot release an unknown report');
      await click(improvementLabel(kind));
      assert.equal(postsFor(calls, kind, 'feedback').length, 1);
      await click('It worked');
      assert.equal(button('Retry success confirmation').disabled, false);
      assert.equal(button('Clear unsaved confirmation') === undefined, true);
      assert.equal(document.querySelector('[aria-label="Task awaiting success confirmation"]')?.textContent, draft.task.trim());
      const original = postsFor(calls, kind, 'success')[0].body;
      await click('Recheck success reports'); await idle();
      assert.equal(postsFor(calls, kind, 'feedback').length, 1);
      assert.equal(postsFor(calls, kind, 'success').length, 1);
      assert.deepEqual(postsFor(calls, kind, 'success')[0].body, original);
      assert.equal(button('Retry success confirmation').disabled, false);
      assertDrafts(document); assert.doesNotMatch(document.querySelector('.deployment-use-success').textContent, /Your success report is saved/);
    });
  });
}

function memoryStorage() {
  const values = new Map([['unrelated-browser-data', 'Preserve this exact value.']]), calls = [];
  return { values, calls,
    getItem(key) { calls.push(['get', key]); return values.get(key) ?? null; },
    setItem(key, value) { calls.push(['set', key]); values.set(key, value); },
    removeItem(key) { calls.push(['remove', key]); values.delete(key); },
  };
}
function storeFixture(kind, family, storage) {
  const scope = scopeFor(kind), binding = kind === 'remote' ? remoteInstalledUseBinding(remote, state) : null;
  const create = () => family === 'improvement'
    ? kind === 'local' ? createInstalledUseImprovementStore(storage) : createRemoteInstalledUseImprovementStore(storage, binding)
    : kind === 'local' ? createInstalledUseSuccessStore(storage) : createRemoteInstalledUseSuccessStore(storage, binding);
  const body = family === 'improvement'
    ? (kind === 'local' ? installedUseFeedback(metadata(kind), local, state, checked, { ...draft, request: draft.improvement })
      : remoteInstalledUseFeedback(metadata(kind), remote, state, checked, { ...draft, request: draft.improvement }))
    : (kind === 'local' ? installedUseSuccessBody : remoteInstalledUseSuccessBody)(metadata(kind), receiptFor(kind), state, draft.task, requestId);
  const value = family === 'improvement'
    ? kind === 'local' ? installedUseImprovementHint(scope, body) : remoteInstalledUseImprovementHint(remote, state, body)
    : body;
  assert.ok(value, 'The real source-bound helper supplies the test record');
  return { scope, create, value,
    save: (store, next = value) => family === 'improvement' ? store.save(scope, next) : store.prepare(scope, next),
    release: (store, next = value) => family === 'improvement' ? store.forgetRejected(scope, next) : store.reject(scope, next),
    newer: family === 'improvement' ? { ...value, request: 'A distinct newer uncertain report.' }
      : { ...value, request_id: '44444444-4444-4444-8444-444444444444', task: 'A distinct newer success report.' } };
}

test('page-only report recovery real stores preserve persistent, changed, confirmed and corrupt records on exact-refusal cleanup', () => {
  for (const kind of ['local', 'remote']) for (const family of ['improvement', 'success']) {
    for (const condition of ['cache-mismatch', 'confirmed', 'corrupt-cache', 'remove-failure', 'newer-disk', 'corrupt-disk']) {
      const storage = memoryStorage(), fixture = storeFixture(kind, family, storage), store = fixture.create();
      assert.equal(fixture.save(store), true); assert.equal(store.persistent(fixture.scope), true);
      const key = [...storage.values.keys()].find(item => item !== 'unrelated-browser-data');
      let target = store, expected = fixture.value;
      if (condition === 'cache-mismatch') expected = fixture.newer;
      if (condition === 'confirmed') {
        if (family === 'improvement') assert.equal(fixture.save(store, { ...fixture.value,
          buildId: 'confirmed-host-build', hostRequest: 'An admitted host request.' }), true);
        else { assert.equal(store.clear(fixture.scope, fixture.value), true); assert.equal(fixture.save(store, fixture.newer), true); }
      }
      if (condition === 'corrupt-cache') {
        storage.values.set(key, 'malformed'); target = fixture.create();
        assert.deepEqual(target.read(fixture.scope), family === 'improvement' ? { corrupt: true } : null);
      }
      if (condition === 'remove-failure') storage.removeItem = item => {
        storage.calls.push(['remove', item]); throw Error('Synthetic removal refused.');
      };
      if (condition === 'newer-disk') {
        const other = fixture.create();
        if (family === 'success') assert.equal(other.clear(fixture.scope, fixture.value), true);
        assert.equal(fixture.save(other, fixture.newer), true);
      }
      if (condition === 'corrupt-disk') storage.values.set(key, 'malformed');
      const before = [...storage.values], cached = target.read(fixture.scope);
      assert.equal(fixture.release(target, expected), false, `${kind} ${family} ${condition}`);
      assert.deepEqual([...storage.values], before, `${condition} keeps all browser bytes intact`);
      assert.deepEqual(target.read(fixture.scope), cached);
    }
  }
});

test('page-only report recovery real stores clear only exact cached refusal without touching denied or uncertain browser writes', () => {
  for (const kind of ['local', 'remote']) for (const family of ['improvement', 'success']) {
    for (const condition of ['denied', 'write-succeeded-readback-failed']) {
      const storage = condition === 'denied' ? deniedStorage() : memoryStorage();
      if (condition !== 'denied') {
        let readbackFailed = false;
        const get = storage.getItem, set = storage.setItem;
        storage.getItem = key => {
          if (readbackFailed) { storage.calls.push(['get', key]); throw Error('Synthetic readback unavailable.'); }
          return get(key);
        };
        storage.setItem = (key, value) => { set(key, value); readbackFailed = true; };
      }
      const fixture = storeFixture(kind, family, storage), store = fixture.create();
      assert.equal(fixture.save(store), true); assert.equal(store.persistent(fixture.scope), false);
      const beforeCalls = storage.calls.slice(), beforeBytes = storage.values ? [...storage.values] : [];
      assert.equal(fixture.release(store, fixture.newer), false, 'An exact host witness cannot release a different cached record');
      assert.deepEqual(store.read(fixture.scope), fixture.value);
      assert.equal(fixture.release(store), true, `${kind} ${family} ${condition} releases its exact page-only record`);
      assert.equal(store.read(fixture.scope), null); assert.equal(store.persistent(fixture.scope), false);
      assert.deepEqual(storage.calls, beforeCalls, 'Page-only release never re-reads, deletes or writes browser storage');
      assert.deepEqual(storage.values ? [...storage.values] : [], beforeBytes, 'Even a possibly written record stays byte-for-byte intact');
      if (condition !== 'denied') assert.equal(beforeBytes.length, 2, 'The uncertain write really reached browser storage');
    }
  }
});

const otherDraft = { task: '  A separate installation task.  ', outcome: 'blocked', improvement: ' Keep this other report. ' };
const newerDraft = { task: '  My later edited task.  ', outcome: 'not_worked', improvement: ' Keep my later explanation. ' };
for (const kind of ['local', 'remote']) {
  test(`confirmed-feedback page custody ${kind} retains a mounted confirmation across A to B to A without borrowing another owner`, async t => {
    const admitted = { ...checked, build_id: 'confirmed-page-improvement',
      request: 'Host admitted the exact café and 觀測 🌌 report.', source_digest: 'f'.repeat(64) };
    await mounted(kind, t, call => {
      assert.equal(call.target, useUrl(kind, 'feedback'));
      return reply(admitted);
    }, async ({ document, button, click, edit, select, observeBuild, calls, idle }) => {
      await edit(); await click(improvementLabel(kind));
      assert.equal(document.querySelector('.installed-improvement-handoff')?.dataset.buildId, admitted.build_id,
        'The exact host response is confirmed while its original view is still mounted');
      assert.equal(button(improvementLabel(kind)).disabled, true);
      observeBuild(admitted);
      await select('B');
      assert.equal(document.querySelector('.installed-improvement-handoff') === null, true,
        'A different installed source or exact remote destination cannot inherit the confirmed link');
      assert.equal(document.querySelector('.deployment-use-submitted') === null, true);
      assert.equal(document.querySelector('.deployment-use-field textarea').value, '');
      await edit(otherDraft);
      const readsBeforeReturn = calls.filter(call => call.method === 'GET' && call.target === `/api/guide/${state.id}/implementation`).length;
      await select('A'); await idle(); assertDrafts(document);
      assert.ok(calls.filter(call => call.method === 'GET' && call.target === `/api/guide/${state.id}/implementation`).length > readsBeforeReturn,
        'Return observes actual Build state instead of treating the retained admission as checked work');
      assert.equal(document.querySelector('.installed-improvement-handoff')?.dataset.buildId, admitted.build_id);
      assert.equal(document.querySelector('.installed-improvement-handoff a')?.getAttribute('href'), '#setup-implementation');
      assert.equal(button(improvementLabel(kind)).disabled, true);
      const posts = postsFor(calls, kind, 'feedback');
      assert.equal(posts.length, 1, 'Navigation and Build readback never resend the original report');
      assert.equal(posts[0].body.task, draft.task.trim()); assert.equal(posts[0].body.request, draft.improvement.trim());
      assert.equal(postsFor(calls, kind, 'success').length, 0);
      assert.match(document.body.textContent, /open page only/);
      await select('B'); assertDrafts(document, otherDraft);
      assert.equal(document.querySelector('.installed-improvement-handoff') === null, true);
      await select('A'); assertDrafts(document);
      assert.equal(document.querySelector('.installed-improvement-handoff')?.dataset.buildId, admitted.build_id);
      assert.equal(postsFor(calls, kind, 'feedback').length, 1);
    });
  });
}

for (const kind of ['local', 'remote']) {
  test(`installed-use page custody ${kind} restores drafts and walkthrough after A to B to A with denied storage`, async t => {
    await mounted(kind, t, () => { throw Error('No report was requested.'); }, async ({ document, edit, select, calls }) => {
      await edit();
      assert.ok(document.querySelector('.deployment-use-walkthrough[aria-label="First-job walkthrough: Review observing notes"]'));
      await select('B');
      assert.equal(document.querySelector('.deployment-use-field textarea').value, '');
      assert.equal(document.querySelector('.deployment-use-feedback textarea').value, '');
      assert.equal(document.querySelector('.deployment-use-walkthrough')?.getAttribute('aria-label'), 'Prepare your own first job');
      await edit(otherDraft);
      await select('A'); assertDrafts(document);
      assert.equal(document.querySelector('.deployment-use-walkthrough')?.getAttribute('aria-label'),
        'First-job walkthrough: Review observing notes');
      await select('B'); assertDrafts(document, otherDraft);
      assert.equal(calls.filter(call => call.method === 'POST').length, 0);
    });
  });

  test(`installed-use page custody ${kind} retains entered reports across remount and fences late replies from newer drafts`, async t => {
    let feedbackReply, successReply;
    const feedback = new Promise(resolve => { feedbackReply = resolve; });
    const success = new Promise(resolve => { successReply = resolve; });
    await mounted(kind, t, call => call.target === useUrl(kind, 'feedback') ? feedback : success,
      async ({ document, button, click, edit, writeDraft, select, calls, publications, idle }) => {
        await edit(); await click(improvementLabel(kind)); await click('It worked');
        const originalSuccess = postsFor(calls, kind, 'success')[0].body;
        await select('B');
        assert.equal(button(improvementLabel(kind)).disabled, true, 'An empty separate draft is not a report');
        assert.equal(document.querySelector('[aria-label="Task awaiting success confirmation"]') === null, true);
        await select('A');
        assert.equal(document.querySelector('[aria-label="Task awaiting success confirmation"]')?.textContent, draft.task.trim(),
          'The exact entered success report remains pending after its first owner unmounted');
        assert.equal(button('Retry success confirmation')?.disabled, false);
        assertDrafts(document);
        assert.equal(button(improvementLabel(kind)).disabled, true, 'The original unknown report cannot be resubmitted');
        await writeDraft(newerDraft);
        const beforeLateResponse = structuredClone(publications);
        await act(async () => {
          feedbackReply(reply({ guide_id: state.id, guide_version: state.version, build_id: 'late-original-improvement',
            status: 'queued', request: 'The host admitted the original report.' }));
          successReply(reply(successResponse(originalSuccess))); await settle();
        });
        await idle(); assertDrafts(document, newerDraft);
        assert.deepEqual(publications, beforeLateResponse, 'An unmounted response cannot publish a late candidate');
        assert.equal(postsFor(calls, kind, 'feedback').length, 1);
        assert.equal(postsFor(calls, kind, 'success').length, 1);
        await select('B'); await select('A'); assertDrafts(document, newerDraft);
        assert.equal(button(improvementLabel(kind)).disabled, true);
        assert.equal(document.querySelector('[aria-label="Task awaiting success confirmation"]')?.textContent, draft.task.trim());
        assert.doesNotMatch(document.querySelector('.deployment-use-success').textContent, /Your success report is saved/);
        assert.equal(postsFor(calls, kind, 'feedback').length, 1);
        assert.equal(postsFor(calls, kind, 'success').length, 1);
        assert.deepEqual(postsFor(calls, kind, 'success')[0].body, originalSuccess);
      });
  });

  test(`installed-use page custody ${kind} does not transfer denied-storage drafts or uncertain reports to another window`, async t => {
    await mounted(kind, t, () => { throw Error('Synthetic response lost.'); }, async ({ edit, click }) => {
      await edit(); await click(improvementLabel(kind)); await click('It worked');
    });
    await mounted(kind, t, () => { throw Error('No new report was requested.'); }, async ({ document, calls }) => {
      assert.equal(document.querySelector('.deployment-use-field textarea').value, '');
      assert.equal(document.querySelector('.deployment-use-feedback textarea').value, '');
      assert.equal(document.querySelector('[aria-label="Task awaiting success confirmation"]') === null, true);
      assert.equal(document.querySelector('.deployment-use-walkthrough')?.getAttribute('aria-label'), 'Prepare your own first job');
      assert.equal(calls.filter(call => call.method === 'POST').length, 0);
    });
  });
}

for (const kind of ['local', 'remote']) {
  test(`installed-use page custody ${kind} retains window-owned drafts when its storage getter throws without using global storage`, async t => {
    const unrelatedStorage = memoryStorage();
    await mounted(kind, t, () => { throw Error('No report was requested.'); }, async ({ document, edit, select }) => {
      await edit(); await select('B'); await select('A'); assertDrafts(document);
      assert.match(document.body.textContent, /Browser draft storage is unavailable/);
      assert.deepEqual(unrelatedStorage.calls, [], 'The mounted owner cannot borrow another global storage object');
      assert.deepEqual([...unrelatedStorage.values], [['unrelated-browser-data', 'Preserve this exact value.']]);
    }, { storageGetterDenied: true, globalStorage: unrelatedStorage });
  });
}

test('installed-use page custody explicit window getter binds actual storage and isolates exact source and denied-window records', () => {
  const unrelated = memoryStorage(), storageA = memoryStorage(), storageB = memoryStorage();
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'sessionStorage');
  Object.defineProperty(globalThis, 'sessionStorage', { configurable: true, value: unrelated });
  try {
    const pageA = { sessionStorage: storageA }, pageB = { sessionStorage: storageB };
    const pageDenied = { get sessionStorage() { throw Error('PRIVATE_BROWSER_STORAGE'); } };
    const scope = scopeFor('local'), hint = storeFixture('local', 'improvement', storageA).value;
    const body = storeFixture('local', 'success', storageA).value;
    for (const page of [pageA, pageDenied]) {
      const stores = getInstalledUsePageStores(page);
      for (const [field, value] of Object.entries(draft)) assert.equal(stores.drafts.update(scope, field, value), true);
      assert.equal(stores.successStore.prepare(scope, body), true);
      assert.equal(stores.improvementStore.save(scope, hint), true);
      const restored = getInstalledUsePageStores(page); assert.equal(restored, stores);
      assert.deepEqual(restored.drafts.read(scope), draft);
      assert.deepEqual(restored.successStore.read(scope), body);
      assert.deepEqual(restored.improvementStore.read(scope), hint);
      assert.equal(restored.drafts.persistent(scope), page === pageA);
      assert.equal(restored.successStore.persistent(scope), page === pageA);
      assert.equal(restored.improvementStore.persistent(scope), page === pageA);
      const otherSource = installedUseScopeKey({ ...local, package_sha256: 'f'.repeat(64) }, state);
      assert.deepEqual(restored.drafts.read(otherSource), { task: '', outcome: '', improvement: '' });
      assert.equal(restored.successStore.read(otherSource), null);
      assert.equal(restored.improvementStore.read(otherSource), null);
    }
    const otherWindow = getInstalledUsePageStores(pageB);
    assert.deepEqual(otherWindow.drafts.read(scope), { task: '', outcome: '', improvement: '' });
    assert.equal(otherWindow.successStore.read(scope), null); assert.equal(otherWindow.improvementStore.read(scope), null);
    assert.deepEqual(unrelated.calls, [], 'An explicit owner always reads that same owner storage');
    const standalone = getInstalledUsePageStores(null);
    assert.equal(standalone.drafts.update(scope, 'task', draft.task), true);
    assert.deepEqual(getInstalledUsePageStores(null).drafts.read(scope), { task: '', outcome: '', improvement: '' });
    assert.deepEqual(unrelated.calls, [], 'Missing page ownership cannot fall back to a different global store');
  } finally {
    if (previous) Object.defineProperty(globalThis, 'sessionStorage', previous);
    else delete globalThis.sessionStorage;
  }
});
