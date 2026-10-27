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
import { DRAFT_KEY } from './state.mjs';
import { GUIDE_KEY } from './guided-state.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));

// Bundle only when this test module is run after the complete source review.
// App, GuidedSetup, startup controls and draft persistence remain real. These
// replacements remove unrelated Console drawing and generated animation only.
// A file-backed bundle keeps failure stacks bounded; its linked map preserves
// the original source locations and contents for diagnostics.
const bundleDirectory = await mkdtemp(join(tmpdir(), 'mobkit-app-startup-'));
const appModulePath = join(bundleDirectory, 'startup-recovery-app.mjs');
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

const plan = name => ({
  schema_version: 2,
  project: { name, slug: name === 'Saved observing draft' ? 'saved-observing-draft' : 'fresh-startup-draft',
    description: 'Synthetic first-arrival draft recovery.' },
  provider: 'openai', model: 'gpt-6.1-sol', target: 'mac',
  agent_classes: [{ id: 'planner', name: 'Observing planner', description: 'Plan synthetic observations.',
    instructions: 'Keep the human observing caveat.', instances: 1, members: [],
    provider: 'openai', model: 'gpt-6.1-sol',
    tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
  topology: { mode: 'manual', entrypoint: 'planner:main', edges: [] },
  connections: [], flows: [], knowledge: { mode: 'none' }, advanced: { port: 4110 },
});
const savedPlan = () => plan('Saved observing draft');
const starterPlan = () => plan('Fresh starter draft');
const rawSaved = () => ` \n${JSON.stringify(savedPlan(), null, 2)}\n  `;
const deferred = () => {
  let resolve;
  const promise = new Promise(yes => { resolve = yes; });
  return { promise, resolve };
};
const workbench = () => ({ blueprint: starterPlan(), capabilities: {},
  architect: { available: false, busy: false, model: 'gpt-6.1-sol' } });

function fixture({ raw = null, workbenchReplies = [], importReplies = [], storageFailure = null, revisionRaw = null, projectRead = null } = {}) {
  let storageMode = storageFailure;
  const guideId = 'owned-startup-recovery-guide';
  const values = new Map([[GUIDE_KEY, guideId], ['owned-saved-installation-reference', 'keep-this-reference']]);
  if (raw !== null) values.set(DRAFT_KEY, raw);
  if (revisionRaw !== null) values.set('mobkit-launch-project-revision', revisionRaw);
  const calls = [], writes = [], unexpected = [], downloads = [], importSignals = [], blobs = new Map();
  const storage = {
    getItem(key) { if (storageMode === 'read') throw Error('Synthetic storage read refusal'); return values.get(key) ?? null; },
    setItem(key, value) {
      writes.push({ key, value });
      if (storageMode === 'write') throw Error('Synthetic browser quota refusal');
      values.set(key, value);
    },
    removeItem(key) { values.delete(key); }, key(index) { return [...values.keys()][index] ?? null; },
    get length() { return values.size; },
  };
  const read = async (url, options = {}) => {
    const method = options.method || 'GET', body = options.body ? JSON.parse(options.body) : undefined;
    const call = { url, method, ...(body === undefined ? {} : { body }) };
    calls.push(call);
    if (method === 'GET' && url === '/api/workbench') return workbenchReplies.length ? await workbenchReplies.shift() : workbench();
    if (method === 'POST' && url === '/api/blueprint/import') {
      importSignals.push(options.signal);
      if (importReplies.length) return await importReplies.shift();
      assert.deepEqual(body, savedPlan(), 'Startup validates the exact captured saved draft, not a starter or another request');
      return { blueprint: structuredClone(body) };
    }
    if (method !== 'GET') {
      unexpected.push(call);
      throw Error(`Unexpected offline mutation: ${method} ${url}`);
    }
    if (projectRead) {
      const result = await projectRead(url);
      if (result !== undefined) return result;
    }
    if (url === `/api/guide/${guideId}`) return {
      id: guideId, version: 1, status: 'question', stage: 'purpose',
      completed_stages: [], answers: [], messages: [], proposal: null, blueprint: null,
      brief: { facts: {}, requirements: [] }, capabilities: { available: false, model: 'gpt-6.1-sol' },
      question: { id: 'purpose', stage: 'purpose', title: 'Who should this help?', description: 'Synthetic first-arrival question.',
        options: [], multiple: false, allow_text: true },
    };
    if (url === '/api/guide/new') return {
      id: null, version: 0, status: 'question', stage: 'purpose', completed_stages: [], answers: [], messages: [],
      proposal: null, blueprint: null, brief: { facts: {}, requirements: [] }, capabilities: { available: false }, question: null,
    };
    if (url === '/api/guide/list') return { setups: [{ id: guideId, name: 'Existing saved conversation', status: 'question' }] };
    if (url.startsWith(`/api/guide/${guideId}/timeline?`)) return { available: true, frames: [], bindings: [], exhausted: true };
    if (url === '/api/provider-settings') return {
      providers: ['openai', 'anthropic', 'gemini'].map(provider => ({ provider, configured: false, source: null, editable: true })),
      effective_provider: null, runtime_active: false, work_active: false, load_error: null,
      locked_provider: null, provider_binding_error: null, runtime_status: 'stopped',
    };
    if (url === '/api/architect/status') return { available: false, busy: false, model: 'gpt-6.1-sol' };
    if (url === '/api/capabilities') return { rehearsal: { start_request_replay: false } };
    if (url === '/api/checker-setup') return { status: 'ready', can_prepare: false, detail: 'Offline fixture app checks ready.' };
    if (url === `/api/guide/${guideId}/gcp-deployment`) return null;
    if (['implementation', 'deployment', 'rehearsal', 'remote-deployment'].some(path => url === `/api/guide/${guideId}/${path}`) ||
        ['deployment', 'rehearsal', 'gcp-deployment', 'remote-deployment'].some(path => url === `/api/guide/null/${path}`))
      return { httpError: 404, detail: 'No saved activity in this offline startup fixture.' };
    unexpected.push(call);
    throw Error(`Unexpected offline read: ${method} ${url}`);
  };
  return { values, storage, calls, writes, unexpected, downloads, importSignals, blobs, read, guideId,
    setStorageFailure: value => { storageMode = value; },
    draftWrites: () => writes.filter(value => value.key === DRAFT_KEY),
    importCalls: () => calls.filter(value => value.url === '/api/blueprint/import'),
    workbenchCalls: () => calls.filter(value => value.url === '/api/workbench'),
  };
}

async function mountedApp(t, state, verify) {
  t.mock.timers.enable({ apis: ['Date', 'setInterval', 'setTimeout'], now: Date.parse('2026-10-06T22:40:00Z') });
  const dom = new JSDOM('<div id="test-root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const OriginalURL = globalThis.URL;
  class DownloadURL extends OriginalURL {}
  DownloadURL.createObjectURL = blob => { const url = `blob:owned-startup-${state.blobs.size + 1}`; state.blobs.set(url, blob); return url; };
  DownloadURL.revokeObjectURL = () => {};
  const replacements = { window: { value: dom.window }, document: { value: dom.window.document },
    navigator: { value: dom.window.navigator }, HTMLElement: { value: dom.window.HTMLElement },
    requestAnimationFrame: { value: dom.window.requestAnimationFrame.bind(dom.window) },
    cancelAnimationFrame: { value: dom.window.cancelAnimationFrame.bind(dom.window) },
    IS_REACT_ACT_ENVIRONMENT: { value: true }, crypto: { value: webcrypto }, URL: { value: DownloadURL },
    matchMedia: { value: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) },
    localStorage: { value: state.storage }, sessionStorage: { value: dom.window.sessionStorage },
    fetch: { value: async (url, options) => {
      const body = await state.read(url, options);
      if (body instanceof Error) throw body;
      if (typeof body?.json === 'function') return body;
      return { ok: !body?.httpError, status: body?.httpError || 200,
        json: async () => body?.httpError ? { detail: body.detail } : body };
    } },
  };
  if (state.storageGetterRefused)
    replacements.localStorage = { get() { throw Error('Synthetic browser storage getter refusal'); } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, descriptor] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, ...descriptor });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  dom.window.HTMLAnchorElement.prototype.click = function() { state.downloads.push({ href: this.href, name: this.download }); };
  let root;
  try {
    const { App } = await import(appModuleUrl);
    const { createRoot } = await import('react-dom/client');
    const render = async () => {
      root = createRoot(dom.window.document.getElementById('test-root'));
      await act(async () => { root.render(React.createElement(App)); await settle(); });
      await act(async () => { await settle(); });
    };
    const finish = async (waiting, value) => {
      await act(async () => { waiting.resolve(value); await settle(); });
      await act(async () => { await settle(); });
    };
    await render();
    await verify({ document: dom.window.document, finish,
      remount: async () => { await act(async () => root.unmount()); await render(); } });
    assert.deepEqual(state.unexpected, [], 'No guide/provider/native action or unlisted request is admitted by startup recovery');
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

function button(document, label) {
  const value = [...document.querySelectorAll('button')].find(node => node.textContent.trim() === label);
  assert.ok(value, `Missing actual startup recovery control: ${label}`);
  return value;
}
async function click(document, label) {
  const value = button(document, label);
  assert.equal(value.disabled, false, `The actual ${label} action must be available`);
  await act(async () => { value.click(); await settle(); });
  await act(async () => { await settle(); });
}
async function assertGuide(document, name) {
  assert.equal(document.querySelector('.loading-page') === null, true);
  const editor = document.querySelector('.app-shell');
  assert.ok(editor, 'The actual App editor retains the current startup draft.');
  if (editor.style.display === 'none') {
    assert.ok(document.querySelector('.setup-plan-pane'), 'The real GuidedSetup is mounted after startup');
    await click(document, 'Advanced editor');
  }
  assert.notEqual(editor.style.display, 'none', 'The human can inspect the recovered draft in the actual editor.');
  assert.equal(editor.querySelector('.project-switch').textContent.trim(), name,
    'The editor displays the exact recovered draft rather than the welcome illustration or workbench starter.');
}
function assertOtherState(state) {
  assert.equal(state.values.get(GUIDE_KEY), state.guideId, 'Recovery does not clear or replace the saved guide selection');
  assert.equal(state.values.get('owned-saved-installation-reference'), 'keep-this-reference');
  assert.ok(state.calls.filter(call => call.method === 'POST').every(call => call.url === '/api/blueprint/import'),
    'Draft validation cannot start a guide, provider, build, trial or installation');
}

test('failed initial workbench read preserves exact saved draft bytes and explicit retry opens that draft', async t => {
  const raw = rawSaved(), state = fixture({ raw, workbenchReplies: [new Error('Synthetic workbench unavailable.')] });
  await mountedApp(t, state, async ({ document }) => {
    assert.match(document.querySelector('[role="alert"]').textContent, /Synthetic workbench unavailable/);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    assert.deepEqual(state.draftWrites(), []);
    assert.equal(state.importCalls().length, 0);
    assert.equal(document.querySelector('.setup-plan-pane') === null, true);
    await click(document, 'Try again');
    await assertGuide(document, 'Saved observing draft');
    assert.equal(state.importCalls().length, 1);
    assert.deepEqual(state.importCalls()[0].body, savedPlan());
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), savedPlan());
    assertOtherState(state);
  });
});

test('failed draft import retains bytes and a deferred explicit retry excludes same-turn duplicate submissions', async t => {
  const raw = rawSaved(), waiting = deferred();
  const state = fixture({ raw, importReplies: [{ httpError: 503, detail: 'Local draft validation is temporarily unavailable.' }, waiting.promise] });
  await mountedApp(t, state, async ({ document, finish }) => {
    assert.match(document.querySelector('[role="alert"]').textContent, /temporarily unavailable/);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    assert.deepEqual(state.draftWrites(), []);
    const retry = button(document, 'Try again');
    await act(async () => { retry.click(); retry.click(); await settle(); });
    assert.equal(state.workbenchCalls().length, 2, 'Only one deliberate retry is admitted synchronously');
    assert.equal(state.importCalls().length, 2);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    assert.equal(document.querySelector('.setup-plan-pane') === null, true);
    assert.equal(button(document, 'Use a fresh draft instead').disabled, true);
    await finish(waiting, { blueprint: savedPlan() });
    await assertGuide(document, 'Saved observing draft');
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), savedPlan());
    assertOtherState(state);
  });
});

for (const held of ['response headers', 'JSON body']) test(`saved draft import deadline releases recovery after held ${held} and excludes the expired reply`, async t => {
  const raw = rawSaved(), expired = deferred(), retryReply = deferred();
  let bodyReads = 0;
  const firstReply = held === 'response headers' ? expired.promise : {
    ok: true, status: 200,
    json: () => { ++bodyReads; return expired.promise; },
  };
  const state = fixture({ raw, importReplies: [firstReply, retryReply.promise] });
  await mountedApp(t, state, async ({ document, finish }) => {
    assert.equal(state.importCalls().length, 1);
    assert.deepEqual(state.importCalls()[0].body, savedPlan());
    assert.equal(bodyReads, held === 'JSON body' ? 1 : 0, 'The body case is held after successful response headers');
    assert.equal(state.importSignals[0]?.aborted, false);
    assert.equal(button(document, 'Use a fresh draft instead').disabled, true);
    await act(async () => { t.mock.timers.tick(29999); await settle(); });
    assert.equal(document.querySelector('[role="alert"]') === null, true, 'The selected deadline has not elapsed');
    await act(async () => { t.mock.timers.tick(1); await settle(); });
    assert.match(document.querySelector('[role="alert"]').textContent, /taking too long to respond\. Try again\./);
    assert.equal(state.importSignals[0].aborted, true);
    assert.equal(button(document, 'Try again').disabled, false, 'The original startup latch is released at the read deadline');
    assert.equal(button(document, 'Use a fresh draft instead').disabled, false);
    assert.equal(document.querySelector('.setup-plan-pane') === null, true);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    assert.deepEqual(state.draftWrites(), []);
    await act(async () => { t.mock.timers.tick(30000); await settle(); });
    assert.equal(state.workbenchCalls().length, 1, 'Elapsed time cannot retry startup automatically');
    assert.equal(state.importCalls().length, 1, 'Elapsed time cannot repeat validation automatically');
    await click(document, 'Download saved draft');
    assert.equal(await state.blobs.get(state.downloads[0].href).text(), raw);
    const retry = button(document, 'Try again');
    await act(async () => { retry.click(); retry.click(); await settle(); });
    assert.equal(state.workbenchCalls().length, 2, 'One explicit retry owns the next startup read');
    assert.equal(state.importCalls().length, 2, 'A same-turn duplicate retry is excluded');
    assert.deepEqual(state.importCalls()[1].body, savedPlan());
    assert.equal(state.importSignals[1].aborted, false);
    assert.equal(button(document, 'Use a fresh draft instead').disabled, true);
    const retired = savedPlan(); retired.project.name = 'Expired import must not appear';
    await finish(expired, { blueprint: retired });
    assert.equal(document.querySelector('.setup-plan-pane') === null, true, 'The expired reply cannot publish while the explicit retry is pending');
    assert.equal(document.querySelector('[role="alert"]') === null, true, 'The expired request cannot restore an old error');
    assert.equal(button(document, 'Use a fresh draft instead').disabled, true, 'The expired request cannot release the newer startup latch');
    assert.equal(state.values.get(DRAFT_KEY), raw);
    assert.deepEqual(state.draftWrites(), []);
    assert.doesNotMatch(document.body.textContent, /Expired import must not appear/);
    await finish(retryReply, { blueprint: savedPlan() });
    await assertGuide(document, 'Saved observing draft');
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), savedPlan());
    assert.equal(state.importCalls().length, 2);
    assertOtherState(state);
  });
});

for (const [kind, raw, detail] of [
  ['malformed', '  { "schema_version": 2, THIS IS THE RETAINED HUMAN COPY \n', /not valid JSON/],
  ['unsupported', ' \n{"schema_version":1,"project":{"name":"Older retained draft"}}\n ', /unsupported format/],
]) test(`${kind} saved draft can be exported exactly and replaced only after deliberate confirmation`, async t => {
  const state = fixture({ raw });
  await mountedApp(t, state, async ({ document }) => {
    assert.match(document.querySelector('[role="alert"]').textContent, detail);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    assert.deepEqual(state.draftWrites(), []);
    assert.equal(state.importCalls().length, 0);
    await click(document, 'Download saved draft');
    assert.equal(state.downloads.length, 1);
    assert.equal(state.downloads[0].name, 'saved-launch-draft.json');
    assert.equal(await state.blobs.get(state.downloads[0].href).text(), raw);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    await click(document, 'Use a fresh draft instead');
    assert.equal(state.workbenchCalls().length, 1, 'Opening the replacement choice does not replace anything');
    await click(document, 'Keep saved draft');
    assert.equal(state.values.get(DRAFT_KEY), raw);
    await click(document, 'Use a fresh draft instead');
    const replace = button(document, 'Replace saved draft');
    await act(async () => { replace.click(); replace.click(); await settle(); });
    await act(async () => { await settle(); });
    assert.equal(state.workbenchCalls().length, 2, 'The explicit replacement is admitted once');
    await assertGuide(document, 'Fresh starter draft');
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), starterPlan());
    assert.equal(state.importCalls().length, 0, 'Explicit replacement never pretends the invalid draft was validated');
    assertOtherState(state);
  });
});

test('a stored draft changed during import is preserved and must be opened by a fresh explicit retry', async t => {
  const oldRaw = rawSaved(), waiting = deferred();
  const newer = savedPlan(); newer.project.name = 'Newer human draft';
  const newerRaw = `\n${JSON.stringify(newer, null, 2)}\n`;
  const state = fixture({ raw: oldRaw, importReplies: [waiting.promise, { blueprint: newer }] });
  await mountedApp(t, state, async ({ document, finish }) => {
    assert.equal(state.importCalls().length, 1);
    state.values.set(DRAFT_KEY, newerRaw);
    await finish(waiting, { blueprint: savedPlan() });
    assert.match(document.querySelector('[role="alert"]').textContent, /changed while it was opening/);
    assert.equal(state.values.get(DRAFT_KEY), newerRaw);
    assert.deepEqual(state.draftWrites(), []);
    assert.equal(document.querySelector('.setup-plan-pane') === null, true);
    await click(document, 'Try again');
    await assertGuide(document, 'Newer human draft');
    assert.deepEqual(state.importCalls()[1].body, newer);
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), newer);
    assertOtherState(state);
  });
});

test('replacement refuses a changed saved draft instead of overwriting another browser edit', async t => {
  const raw = 'INVALID saved draft', state = fixture({ raw });
  const newerRaw = rawSaved();
  await mountedApp(t, state, async ({ document }) => {
    await click(document, 'Use a fresh draft instead');
    state.values.set(DRAFT_KEY, newerRaw);
    await click(document, 'Replace saved draft');
    assert.match(document.querySelector('[role="alert"]').textContent, /saved draft changed or could not be read/);
    assert.equal(state.values.get(DRAFT_KEY), newerRaw);
    assert.deepEqual(state.draftWrites(), []);
    assert.equal(state.workbenchCalls().length, 1, 'A mismatched replacement intent cannot begin another startup read');
    await click(document, 'Try again');
    await assertGuide(document, 'Saved observing draft');
    assertOtherState(state);
  });
});

for (const late of ['success', 'failure']) test(`an unmounted pending startup ${late} cannot import or publish into a later mounted App`, async t => {
  const waiting = deferred(), raw = rawSaved();
  const state = fixture({ raw, workbenchReplies: [waiting.promise, workbench()] });
  await mountedApp(t, state, async ({ document, finish, remount }) => {
    assert.equal(document.querySelector('.setup-plan-pane') === null, true);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    await remount();
    await assertGuide(document, 'Saved observing draft');
    const before = state.values.get(DRAFT_KEY), imports = state.importCalls().length;
    const retired = late === 'success' ? workbench() : new Error('Retired late startup failure');
    if (late === 'success') retired.blueprint.project.name = 'Retired late startup';
    await finish(waiting, retired);
    assert.equal(state.importCalls().length, imports, 'The retired request cannot admit another validation');
    assert.equal(state.values.get(DRAFT_KEY), before);
    await assertGuide(document, 'Saved observing draft');
    assert.doesNotMatch(document.body.textContent, /Retired late startup/);
    assertOtherState(state);
  });
});

test('a known retained draft stays recoverable when browser storage becomes unreadable before retry', async t => {
  const raw = rawSaved(), state = fixture({ raw, importReplies: [{ httpError: 503, detail: 'Synthetic import refusal.' }] });
  await mountedApp(t, state, async ({ document }) => {
    state.setStorageFailure('read');
    await click(document, 'Try again');
    assert.match(document.querySelector('[role="alert"]').textContent, /retained copy is still available to download/);
    assert.equal(state.values.get(DRAFT_KEY), raw);
    assert.deepEqual(state.draftWrites(), []);
    assert.equal(state.workbenchCalls().length, 1, 'An unreadable known draft cannot silently start a fallback');
    assert.equal(document.querySelector('.setup-plan-pane') === null, true);
    await click(document, 'Download saved draft');
    assert.equal(await state.blobs.get(state.downloads[0].href).text(), raw);
    state.setStorageFailure(null);
    await click(document, 'Try again');
    await assertGuide(document, 'Saved observing draft');
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), savedPlan());
    assert.ok(state.draftWrites().length > 0, 'Availability is restored only after actual draft persistence succeeds');
    assert.doesNotMatch(document.body.textContent, /This browser cannot save your local draft/);
    assertOtherState(state);
  });
});

test('ordinary successful startup without a saved draft opens the fresh guide without importing or admitting agent work', async t => {
  const state = fixture();
  await mountedApp(t, state, async ({ document }) => {
    await assertGuide(document, 'Fresh starter draft');
    assert.equal(state.workbenchCalls().length, 1);
    assert.equal(state.importCalls().length, 0);
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), starterPlan());
    assertOtherState(state);
  });
});

test('successful saved-draft startup opens exactly the imported draft rather than the workbench starter', async t => {
  const state = fixture({ raw: rawSaved() });
  await mountedApp(t, state, async ({ document }) => {
    await assertGuide(document, 'Saved observing draft');
    assert.equal(state.importCalls().length, 1);
    assert.deepEqual(state.importCalls()[0].body, savedPlan());
    assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), savedPlan());
    assertOtherState(state);
  });
});

test('readable saved draft and matching project revision survive denied persistence without taking down App', async t => {
  const raw = rawSaved(), revisionRaw = ` \n${JSON.stringify({ slug: savedPlan().project.slug, revision: 4 })}\n`;
  const state = fixture({ raw, revisionRaw, storageFailure: 'write' });
  await mountedApp(t, state, async ({ document }) => {
    await assertGuide(document, 'Saved observing draft');
    assert.match(document.body.textContent, /This browser cannot save your local draft/);
    assert.equal(state.values.get(DRAFT_KEY), raw, 'Denied writes cannot replace the original local draft bytes');
    assert.equal(state.values.get('mobkit-launch-project-revision'), revisionRaw);
    assert.ok(state.writes.some(write => write.key === 'mobkit-launch-project-revision'),
      'The actual matching-revision persistence effect is exercised, rather than avoided by the fixture');
    await click(document, 'Download draft');
    assert.deepEqual(JSON.parse(await state.blobs.get(state.downloads[0].href).text()), savedPlan());
    await assertGuide(document, 'Saved observing draft');
    assertOtherState(state);
  });
});

for (const storageFailure of ['read', 'write', 'getter']) test(`unavailable browser ${storageFailure} storage allows honest page-only startup and draft export`, async t => {
  const state = fixture({ storageFailure });
  state.storageGetterRefused = storageFailure === 'getter';
  await mountedApp(t, state, async ({ document }) => {
    await assertGuide(document, 'Fresh starter draft');
    assert.match(document.body.textContent, /This browser cannot save your local draft/);
    assert.equal(state.values.has(DRAFT_KEY), false);
    assert.equal(state.importCalls().length, 0);
    await click(document, 'Download draft');
    assert.equal(state.downloads.length, 1);
    assert.deepEqual(JSON.parse(await state.blobs.get(state.downloads[0].href).text()), starterPlan());
    assert.ok(state.calls.filter(call => call.method === 'POST').length === 0);
  });
});


// Named revision reads use the same actual mounted App and browser draft
// persistence as startup. Only these exact fixture-owned GETs are added.
const projectA = { slug: 'revision-project-a', name: 'Revision project A', revision: 2 };
const projectB = { slug: 'revision-project-b', name: 'Revision project B', revision: 2 };
const revisionBlueprint = (project, revision) => ({ ...savedPlan(), project: {
  ...savedPlan().project, slug: project.slug, name: `${project.name} revision ${revision}` } });
const revisionResult = (project, revision) => ({ blueprint: revisionBlueprint(project, revision), revision,
  updated_at: '2026-10-08T03:00:00Z', hash: `synthetic-${project.slug}-${revision}` });
function projectFixture(overrides = new Map()) {
  const history = { revisions: [2, 1].map(revision => ({ revision, updated_at: '2026-10-08T03:00:00Z' })) };
  const replies = new Map([
    ['/api/projects', () => ({ projects: [projectA, projectB] })],
    ...[projectA, projectB].flatMap(project => [
      [`/api/projects/${project.slug}/history`, () => structuredClone(history)],
      ...[1, 2].map(revision => [`/api/projects/${project.slug}?revision=${revision}`,
        () => revisionResult(project, revision)]),
    ]),
    ...overrides,
  ]);
  return fixture({ raw: rawSaved(), projectRead: url => replies.get(url)?.() });
}
const historyModal = document => document.querySelector('[role="dialog"][aria-label="Projects and revision history"]');
async function clickNode(node) {
  assert.ok(node, 'The actual revision-history control exists');
  assert.equal(node.disabled, false, 'The deliberate current control is available');
  await act(async () => { node.click(); await settle(); });
  await act(async () => { await settle(); });
}
async function openHistory(document, { advanced = true } = {}) {
  if (advanced) await click(document, 'Advanced editor');
  await clickNode(document.querySelector('.projects-link'));
  assert.ok(historyModal(document));
}
async function restoreRevision(document, revision) {
  const row = [...historyModal(document).querySelectorAll('.revision-list > div')]
    .find(node => node.querySelector('span')?.textContent.trim().startsWith(`Revision ${revision}`));
  await clickNode(row?.querySelector('button'));
}
async function selectProject(document, project) {
  const node = [...historyModal(document).querySelectorAll('.saved-projects button')]
    .find(node => node.querySelector('small')?.textContent === project.slug);
  await clickNode(node);
}
function assertCurrentDraft(document, state, expected) {
  assert.equal(document.querySelector('.project-switch').textContent.trim(), expected.project.name);
  assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), expected, 'The actual persisted App draft retains the latest deliberate choice');
  assertOtherState(state);
}

test('revision history selecting its active project retains usable Restore rows', async t => {
  const state = projectFixture();
  await mountedApp(t, state, async ({ document }) => {
    await openHistory(document);
    assert.equal(historyModal(document).querySelectorAll('.revision-list > div').length, 2);
    const reads = state.calls.filter(call => call.url === `/api/projects/${projectA.slug}/history`).length;
    await selectProject(document, projectA);
    assert.equal(historyModal(document).querySelectorAll('.revision-list > div').length, 2,
      'A click on the already-active project must not strand its loaded history.');
    assert.equal(state.calls.filter(call => call.url === `/api/projects/${projectA.slug}/history`).length, reads,
      'Selecting the same project leaves its current history and existing request lifetime intact.');
    assertCurrentDraft(document, state, savedPlan());
    await restoreRevision(document, 1);
    assertCurrentDraft(document, state, revisionBlueprint(projectA, 1));
  });
});

test('revision history newer Restore wins after older response settles and its modal unmounts', async t => {
  const older = deferred(), state = projectFixture(new Map([
    [`/api/projects/${projectA.slug}?revision=1`, () => older.promise],
  ]));
  await mountedApp(t, state, async ({ document, finish }) => {
    await openHistory(document);
    await restoreRevision(document, 1);
    assert.equal(state.calls.filter(call => call.url === `/api/projects/${projectA.slug}?revision=1`).length, 1);
    await restoreRevision(document, 2);
    assert.equal(historyModal(document) === null, true, 'The newer successful restoration closes its own modal');
    assertCurrentDraft(document, state, revisionBlueprint(projectA, 2));
    const writes = state.draftWrites().length;
    await finish(older, revisionResult(projectA, 1));
    assertCurrentDraft(document, state, revisionBlueprint(projectA, 2));
    assert.equal(state.draftWrites().length, writes, 'The old reply cannot persist a replacement draft');
    assert.equal(historyModal(document) === null, true);
    assert.match(document.querySelector('.toast').textContent, /Project loaded/);
  });
});

for (const outcome of ['success', 'refusal']) test(`revision history project switch ignores an older Restore ${outcome}`, async t => {
  const older = deferred(), state = projectFixture(new Map([
    [`/api/projects/${projectA.slug}?revision=1`, () => older.promise],
  ]));
  await mountedApp(t, state, async ({ document, finish }) => {
    await openHistory(document);
    await restoreRevision(document, 1);
    await selectProject(document, projectB);
    const writes = state.draftWrites().length;
    await finish(older, outcome === 'success' ? revisionResult(projectA, 1) : new Error('Old project A Restore refused.'));
    assertCurrentDraft(document, state, savedPlan());
    assert.equal(state.draftWrites().length, writes);
    assert.ok(historyModal(document), 'The actual newly selected project history stays open');
    assert.equal(historyModal(document).querySelector('.saved-projects .active small').textContent, projectB.slug);
    assert.doesNotMatch(historyModal(document).textContent, /Old project A Restore refused/);
    await restoreRevision(document, 2);
    assertCurrentDraft(document, state, revisionBlueprint(projectB, 2));
  });
});

for (const dismissal of ['Close', 'Escape', 'backdrop']) test(`revision history ${dismissal} dismissal and reopening fence the original Restore`, async t => {
  const older = deferred(), state = projectFixture(new Map([
    [`/api/projects/${projectA.slug}?revision=1`, () => older.promise],
  ]));
  await mountedApp(t, state, async ({ document, finish }) => {
    await openHistory(document);
    await restoreRevision(document, 1);
    if (dismissal === 'Close') await clickNode(historyModal(document).querySelector('footer button'));
    else await act(async () => {
      if (dismissal === 'Escape') document.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      else document.querySelector('.modal-backdrop').dispatchEvent(new window.MouseEvent('mousedown', { bubbles: true }));
      await settle();
    });
    assert.equal(historyModal(document) === null, true);
    assertCurrentDraft(document, state, savedPlan());
    await openHistory(document, { advanced: false });
    const reopened = historyModal(document), writes = state.draftWrites().length;
    await finish(older, revisionResult(projectA, 1));
    assert.equal(historyModal(document) === reopened, true, 'An older modal cannot dismiss the new instance');
    assertCurrentDraft(document, state, savedPlan());
    assert.equal(state.draftWrites().length, writes);
    assert.equal(document.querySelector('.toast') === null, true, 'Stale restoration cannot announce success');
    await restoreRevision(document, 2);
    assertCurrentDraft(document, state, revisionBlueprint(projectA, 2));
  });
});

test('revision history current Restore refusal remains explicit and retry preserves real Undo', async t => {
  let attempts = 0;
  const state = projectFixture(new Map([
    [`/api/projects/${projectA.slug}?revision=2`, () => ++attempts === 1
      ? new Error('Current revision read refused.') : revisionResult(projectA, 2)],
  ]));
  await mountedApp(t, state, async ({ document }) => {
    await openHistory(document);
    await restoreRevision(document, 2);
    assertCurrentDraft(document, state, savedPlan());
    assert.match(historyModal(document).textContent, /Current revision read refused/);
    await restoreRevision(document, 2);
    assert.equal(attempts, 2);
    assert.equal(historyModal(document) === null, true);
    assertCurrentDraft(document, state, revisionBlueprint(projectA, 2));
    await clickNode(document.querySelector('[aria-label="Undo last change"]'));
    assertCurrentDraft(document, state, savedPlan());
  });
});
