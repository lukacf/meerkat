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
import { inputKey, manualKey, writeInput } from './wizard-state.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));

// Bundle only when this test module is run after the complete source review.
// App, GuidedSetup, Advanced controls, drafts and callbacks remain real. These
// replacements remove unrelated Console drawing and generated animation only.
// A file-backed bundle keeps failure stacks bounded; its linked map preserves
// the original source locations and contents for diagnostics.
const bundleDirectory = await mkdtemp(join(tmpdir(), 'mobkit-advanced-draft-'));
const appModulePath = join(bundleDirectory, 'advanced-draft-app.mjs');
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

const basePlan = () => ({
  schema_version: 2,
  project: { name: 'Original observing crew', slug: 'owned-advanced-observing',
    description: 'Prepare an observing plan using the supplied evidence.' },
  provider: 'openai', model: 'gpt-6.1-sol', target: 'mac',
  agent_classes: [{ id: 'planner', name: 'Observing planner',
    description: 'Keep continuing observing context.', instructions: 'Original planner instructions.',
    instances: 1, members: [], provider: 'openai', model: 'gpt-6.1-sol',
    tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
  topology: { mode: 'manual', entrypoint: 'planner:main', edges: [] },
  connections: [], flows: [], knowledge: { mode: 'none' }, advanced: { port: 4110 },
});
const initialGuide = (plan, running = true, action = 'chat') => ({
  id: 'owned-composed-advanced-guide', version: 5, status: 'question', stage: 'purpose',
  completed_stages: [], answers: [], messages: [], proposal: structuredClone(plan), blueprint: null,
  brief: { facts: {}, requirements: [] }, capabilities: { available: false, model: 'gpt-6.1-sol' },
  question: { id: 'observing-context', stage: 'purpose', title: 'Who should this help?',
    description: 'Describe the crew and continuing context.', multiple: false, allow_text: true,
    options: [{ id: 'crew', label: 'Our observing crew', description: 'A shared crew.' },
      { id: 'solo', label: 'Just me', description: 'My own continuing observing context.' }] },
  ...(running ? { operation: { status: 'running', request_id: 'a'.repeat(64), version: 5,
    started_at: '2026-10-06T20:30:00Z', request: { version: 5, action,
      text: action === 'manual' ? 'Original manual changes already admitted.' : 'The earlier submitted crew context.',
      selected: [], ...(action === 'manual' ? { blueprint: structuredClone(plan) } : {}) } } } : {}),
});
const humanInstructions = 'HUMAN-ONLY: Keep my exact lunar observing caveat.\nAsk before changing the equipment choice.';
const humanPurpose = 'HUMAN-ONLY: Plan a quiet observing evening with the synthetic crew.';
const unsentText = '  A separate unsent composer thought.\nKeep its whitespace and context.  ';
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

function fixture({ running = true, action = 'chat', pending = null, savedDraft = null, anonymous = false,
  normalizeSubmitted = value => value } = {}) {
  const original = basePlan(), guide = initialGuide(original, running, action);
  if (anonymous) {
    assert.equal(running, false, 'An anonymous setup has no admitted operation');
    guide.id = null;
    guide.version = 0;
  }
  const input = { key: inputKey(guide), draft: unsentText, selected: [], mode: 'chat' };
  const values = new Map([[input.key, JSON.stringify(input)]]);
  if (guide.id) values.set(GUIDE_KEY, guide.id);
  if (pending) values.set(manualKey(guide.id), JSON.stringify(pending));
  if (savedDraft) values.set(DRAFT_KEY, JSON.stringify(savedDraft));
  const storage = { getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key),
    key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
  const startedId = guide.id || 'owned-composed-started-advanced-guide';
  const guideUrl = `/api/guide/${startedId}`, respondUrl = `${guideUrl}/respond`;
  let server = structuredClone(guide), readFault = false, waitingRead = null, waitingWorkbench = null, waitingBody = null, guideReads = 0;
  let manualRefusal = null, lostManualResponse = null, draftImported = false, guideStarted = false;
  let draftToImport = savedDraft && structuredClone(savedDraft);
  const calls = [], unexpected = [];
  const read = async (url, options = {}) => {
    const method = options.method || 'GET', body = options.body ? JSON.parse(options.body) : undefined;
    calls.push({ url, method, ...(body === undefined ? {} : { body }) });
    if (method === 'POST' && url === '/api/guide/start' && anonymous && !guideStarted) {
      assert.deepEqual(Object.keys(body), ['start_request_id'],
        'Ordinary manual startup carries only its durable creation identity, never the human draft');
      assert.match(body.start_request_id, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
      guideStarted = true;
      server = { ...structuredClone(server), id: startedId, version: 1 };
      return structuredClone(server);
    }
    if (method === 'POST' && url === respondUrl) {
      assert.equal(body.action, 'manual', 'Only an explicitly reviewed manual return may submit in this fixture');
      if (manualRefusal) {
        const detail = manualRefusal;
        manualRefusal = null;
        return { httpError: 409, detail };
      }
      const previous = server;
      server = { ...structuredClone(previous), version: previous.version + 1,
        operation: { status: 'running', version: previous.version, request_id: 'b'.repeat(64),
          started_at: '2026-10-06T20:30:10Z', request: normalizeSubmitted(structuredClone(body)) } };
      if (lostManualResponse) {
        const detail = lostManualResponse;
        lostManualResponse = null;
        throw Error(detail);
      }
      return server;
    }
    if (method === 'POST' && url === '/api/blueprint/import' && draftToImport && !draftImported) {
      assert.deepEqual(body, draftToImport, 'A real reload validates exactly its saved Advanced draft once');
      draftImported = true;
      return { blueprint: structuredClone(draftToImport) };
    }
    if (method !== 'GET') {
      unexpected.push({ url, method });
      throw Error(`Unexpected offline mutation: ${method} ${url}`);
    }
    if (url === '/api/workbench' && waitingWorkbench) {
      const admitted = waitingWorkbench; waitingWorkbench = null; admitted.started = true; admitted.signal = options.signal; return admitted.promise;
    }
    if (url === '/api/workbench') return { blueprint: structuredClone(original),
      architect: { available: false, busy: false, model: 'gpt-6.1-sol', detail: 'Offline architect unavailable.' }, capabilities: {} };
    if (url === '/api/guide/new' && anonymous && !guideStarted) return structuredClone(server);
    if (url === guideUrl) {
      guideReads += 1;
      if (waitingRead) { const admitted = waitingRead; waitingRead = null; admitted.started = true; return admitted.promise; }
      if (readFault) throw Error('Synthetic saved-guide observation interrupted.');
      return structuredClone(server);
    }
    if (url === '/api/guide/list') return { setups: server.id ? [{ id: server.id, name: 'Observing crew', status: server.status }] : [] };
    if (url === '/api/provider-settings') return {
      providers: ['openai', 'anthropic', 'gemini'].map(provider => ({
        provider, configured: false, source: null, editable: true })),
      effective_provider: null, runtime_active: false, work_active: false,
      load_error: null, locked_provider: null, provider_binding_error: null, runtime_status: 'stopped',
    };
    if (url === '/api/architect/status') return { available: false, busy: false, model: 'gpt-6.1-sol' };
    if (url.startsWith(`${guideUrl}/timeline?`)) return { available: true, frames: [], bindings: [], exhausted: true };
    if (url === `${guideUrl}/gcp-deployment`) return null;
    if (['implementation', 'deployment', 'rehearsal', 'remote-deployment'].some(path => url === `${guideUrl}/${path}`))
      return { httpError: 404, detail: 'No saved activity in this composed offline fixture.' };
    // Actual background panels can read their initial anonymous owner before
    // ordinary guide startup. Admit only these exact observed no-record GETs.
    if (anonymous && ['deployment', 'rehearsal', 'gcp-deployment', 'remote-deployment']
      .some(path => url === `/api/guide/null/${path}`))
      return { httpError: 404, detail: 'No saved activity for the anonymous setup.' };
    if (url === '/api/capabilities') return { rehearsal: { start_request_replay: false } };
    if (url === '/api/checker-setup') return { status: 'ready', can_prepare: false, detail: 'Offline fixture checks ready.' };
    unexpected.push({ url, method });
    throw Error(`Unexpected offline read: ${method} ${url}`);
  };
  return { original, guide, input, values, storage, calls, unexpected, read, respondUrl,
    server: () => server, setServer: next => { server = structuredClone(next); },
    setReadFault: value => { readFault = value; }, reads: () => guideReads,
    refuseNextManual: detail => { manualRefusal = detail; },
    loseNextManualResponse: detail => { lostManualResponse = detail; },
    expectSavedDraftImport: draft => { draftToImport = structuredClone(draft); draftImported = false; },
    deferGuideRead: () => { waitingRead = { ...deferred(), started: false }; return waitingRead; },
    deferWorkbenchRead: () => { waitingWorkbench = { ...deferred(), started: false }; return waitingWorkbench; },
    deferGuideBody: () => { waitingBody = { ...deferred(), started: false }; return waitingBody; },
    responseBody: (url, body, signal) => {
      if (url === guideUrl && waitingBody) {
        const admitted = waitingBody; waitingBody = null; admitted.started = true; admitted.signal = signal; return admitted.promise;
      }
      return Promise.resolve(body);
    },
    manual: () => JSON.parse(values.get(manualKey(server.id)) || 'null'),
    draft: () => JSON.parse(values.get(DRAFT_KEY) || 'null'),
  };
}

async function mountedApp(t, state, verify) {
  t.mock.timers.enable({ apis: ['Date', 'setInterval', 'setTimeout'], now: Date.parse('2026-10-06T20:30:00Z') });
  const dom = new JSDOM('<div id="test-root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const replacements = { window: { value: dom.window }, document: { value: dom.window.document },
    navigator: { value: dom.window.navigator }, HTMLElement: { value: dom.window.HTMLElement },
    requestAnimationFrame: { value: dom.window.requestAnimationFrame.bind(dom.window) },
    cancelAnimationFrame: { value: dom.window.cancelAnimationFrame.bind(dom.window) },
    IS_REACT_ACT_ENVIRONMENT: { value: true }, crypto: { value: webcrypto },
    matchMedia: { value: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) },
    localStorage: { value: state.storage }, sessionStorage: { value: dom.window.sessionStorage },
    fetch: { value: async (url, options) => {
      const body = await state.read(url, options);
      return { ok: !body?.httpError, status: body?.httpError || 200,
        json: async () => body?.httpError ? { detail: body.detail } : state.responseBody(url, body, options?.signal) };
    } },
  };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, descriptor] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, ...descriptor });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  let root;
  try {
    // App's importable entry is evaluated under this owned DOM, never an ambient
    // document supplied by an unrelated test. No #app means no automatic mount.
    const { App } = await import(appModuleUrl);
    const { createRoot } = await import('react-dom/client');
    root = createRoot(dom.window.document.getElementById('test-root'));
    const render = async () => {
      await act(async () => { root.render(React.createElement(App)); await settle(); });
      await act(async () => { await settle(); });
    };
    await render();
    await verify(dom.window.document,
      async () => { await act(async () => { t.mock.timers.tick(2500); await settle(); }); },
      async () => {
        await act(async () => root.unmount());
        root = createRoot(dom.window.document.getElementById('test-root'));
        await render();
      });
    assert.deepEqual(state.unexpected, [], 'The composed path must use only the strict offline response allowlist');
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
  assert.ok(input, 'The real controlled editor field must be present');
  const prototype = input.tagName === 'TEXTAREA' ? input.ownerDocument.defaultView.HTMLTextAreaElement.prototype
    : input.ownerDocument.defaultView.HTMLInputElement.prototype;
  await act(async () => {
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(input, value);
    input.dispatchEvent(new input.ownerDocument.defaultView.Event('input', { bubbles: true }));
    await settle();
  });
}
async function editPurpose(document, text = humanPurpose) {
  const shell = document.querySelector('.app-shell');
  await act(async () => { shell.querySelector('.project-switch').click(); await settle(); });
  const modal = shell.querySelector('.modal');
  assert.ok(modal, 'ProjectModal belongs to the actual Advanced App');
  await inputValue(modal.querySelector('textarea'), text);
  await click(modal, 'Save project');
}
async function editInstructions(document, text = humanInstructions) {
  const shell = document.querySelector('.app-shell');
  await act(async () => { shell.querySelector('.class-row').click(); await settle(); });
  const inspector = shell.querySelector('.class-inspector');
  await click(inspector, 'instructions');
  await inputValue(inspector.querySelector('.prompt-editor'), text);
}
const posts = state => state.calls.filter(call => call.method === 'POST');
function assertUnsent(document, state) {
  assert.equal(document.querySelector('#setup-message').value, unsentText);
  assert.equal(JSON.parse(state.values.get(inputKey(state.server()))).draft, unsentText);
}
async function openFallback(document, state, poll) {
  state.setReadFault(true);
  await poll();
  assert.match(document.querySelector('.setup-error').textContent, /Synthetic saved-guide observation interrupted/);
  const control = button(document.querySelector('.setup-error'), 'Configure manually');
  assert.equal(control.disabled, false, 'Recovery permits Advanced access while the saved operation is still running');
  await act(async () => { control.click(); await settle(); });
  assert.equal(document.querySelector('.setup-shell') === null, true, 'GuidedSetup is hidden without being replaced by a test stub');
  assert.equal(document.querySelector('.app-shell').style.display, '');
}
function completed(state, plan) {
  return { ...structuredClone(state.server()), version: state.guide.version + 1,
    proposal: structuredClone(plan), operation: { ...state.guide.operation, status: 'completed' } };
}
function changedServerPlan(state) {
  const next = structuredClone(state.original);
  next.project.name = 'Server-completed observing crew';
  next.agent_classes[0].description = 'SERVER-ONLY-CORRECTION: coordinate the completed crew context.';
  return next;
}
const visibleProposal = document => JSON.parse(document.querySelector('[data-proposal-snapshot]').getAttribute('data-proposal-snapshot'));

test('opening read timeout enables explicit retry without losing or prematurely importing the saved draft', async t => {
  const savedDraft = basePlan();
  savedDraft.project.description = humanPurpose;
  const state = fixture({ running: false, savedDraft });
  const rawDraft = state.values.get(DRAFT_KEY);
  const stalled = state.deferWorkbenchRead();
  await mountedApp(t, state, async document => {
    assert.equal(stalled.started, true);
    assert.equal(document.querySelector('.loading-page button')?.textContent.trim(), 'Download saved draft');
    assert.deepEqual(posts(state), []);
    await act(async () => { t.mock.timers.tick(30000); await settle(); });
    assert.match(document.querySelector('[role="alert"]').textContent, /taking too long/);
    assert.equal(stalled.signal?.aborted, true, 'The expired read also aborts its fetch');
    assert.equal(state.values.get(DRAFT_KEY), rawDraft);
    assert.deepEqual(posts(state), [], 'Read timeout cannot import or replace the saved draft');
    await click(document.querySelector('.loading-page'), 'Try again');
    assertUnsent(document, state);
    assert.equal(posts(state).length, 1, 'Only deliberate retry validates the retained draft');
    assert.deepEqual(posts(state)[0], { url: '/api/blueprint/import', method: 'POST', body: savedDraft });
    stalled.resolve({ blueprint: changedServerPlan(state), architect: {} });
    await act(async () => { await settle(); });
    assertUnsent(document, state);
    assert.deepEqual(posts(state).map(call => call.body), [savedDraft], 'Late headers cannot import again');
  });
});

test('opening saved-guide body timeout enables retry and ignores the late old body', async t => {
  const state = fixture({ running: false });
  writeInput(state.storage, state.input);
  const originalInput = state.values.get(state.input.key);
  const stalled = state.deferGuideBody();
  const oldBody = structuredClone(state.server());
  await mountedApp(t, state, async document => {
    assert.equal(stalled.started, true, 'The HTTP headers arrived but its JSON body stalled');
    assert.match(document.querySelector('.setup-loading').textContent, /Opening your conversation/);
    await act(async () => { t.mock.timers.tick(30000); await settle(); });
    assert.match(document.querySelector('.setup-loading').textContent, /taking too long/);
    assert.equal(stalled.signal?.aborted, true, 'The deadline covers body completion too');
    assert.equal(state.values.get(GUIDE_KEY), state.guide.id);
    assert.equal(state.values.get(state.input.key), originalInput);
    const next = structuredClone(oldBody);
    next.version += 1;
    next.question.title = 'Recovered current crew context';
    state.setServer(next);
    await click(document.querySelector('.setup-loading'), 'Try again');
    assertUnsent(document, state);
    assert.match(document.querySelector('.setup-shell').textContent, /Recovered current crew context/);
    assert.equal(state.calls.filter(call => call.url === '/api/guide/new').length, 0);
    stalled.resolve(oldBody);
    await act(async () => { await settle(); });
    assert.match(document.querySelector('.setup-shell').textContent, /Recovered current crew context/);
    assertUnsent(document, state);
    assert.deepEqual(posts(state), [], 'Opening and retrying a saved conversation cannot start another operation');
  });
});

test('the composed App opens real Advanced edit and Back controls from an interrupted running guide', async t => {
  const state = fixture();
  await mountedApp(t, state, async (document, poll) => {
    assertUnsent(document, state);
    await openFallback(document, state, poll);
    await editPurpose(document);
    assert.equal(state.draft().project.description, humanPurpose);
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assertUnsent(document, state);
    assert.equal(posts(state).length, 0, 'Returning while a saved operation runs does not submit another manual request');
    assert.equal(state.manual().manual_intent.draft.project.description, humanPurpose);
  });
});

test('a guide poll admitted before Advanced opens cannot replace the real edited draft when it recovers running', async t => {
  const state = fixture();
  await mountedApp(t, state, async (document, poll) => {
    state.setReadFault(true);
    await poll();
    state.setReadFault(false);
    const oldRead = state.deferGuideRead();
    await poll();
    assert.equal(oldRead.started, true, 'The real running poll was admitted while the guide callback still had its earlier App render');
    await click(document.querySelector('.setup-error'), 'Configure manually');
    await editPurpose(document);
    await editInstructions(document);
    const edited = state.draft();
    const recovered = { ...structuredClone(state.server()), proposal: changedServerPlan(state) };
    state.setServer(recovered);
    await act(async () => { oldRead.resolve(recovered); await settle(); });
    assert.deepEqual(state.draft(), edited, 'The stale poll callback must read current editor ownership rather than its old mode closure');
    assert.equal(document.querySelector('.app-shell .prompt-editor').value, humanInstructions);
    assert.equal(posts(state).length, 0);
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assertUnsent(document, state);
    assert.deepEqual(state.manual().manual_intent.draft, edited);
  });
});

test('changed completed server proposal is never attributed to an unchanged Advanced draft as a manual edit', async t => {
  const state = fixture();
  await mountedApp(t, state, async (document, poll) => {
    await openFallback(document, state, poll);
    const current = changedServerPlan(state);
    state.setReadFault(false);
    state.setServer(completed(state, current));
    await poll();
    assert.deepEqual(state.draft(), state.original, 'Hidden completion does not overwrite the untouched owned editor draft');
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assert.equal(posts(state).length, 0, 'No human edit means no manual POST, even after an authoritative completion changed the plan');
    assert.equal(state.manual(), null);
    assert.deepEqual(visibleProposal(document), current);
    assertUnsent(document, state);
  });
});

test('edited draft survives changed completion and deliberate review sends exact human intent without replacing the current full plan', async t => {
  const state = fixture();
  await mountedApp(t, state, async (document, poll) => {
    await openFallback(document, state, poll);
    await editPurpose(document);
    await editInstructions(document);
    const edited = state.draft(), current = changedServerPlan(state);
    state.setReadFault(false);
    state.setServer(completed(state, current));
    await poll();
    assert.deepEqual(state.draft(), edited);
    assert.equal(document.querySelector('.app-shell .prompt-editor').value, humanInstructions);
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assert.equal(posts(state).length, 0, 'Back retains the edit and does not automatically resubmit an obsolete full blueprint');
    assert.deepEqual(state.manual().manual_intent, { base: state.original, draft: edited });
    assert.deepEqual(visibleProposal(document), current, 'The proposal continues to show authoritative server context');
    assertUnsent(document, state);
    await click(document.querySelector('.setup-manual-note'), 'Review changes against current plan');
    assert.equal(posts(state).length, 1);
    const admission = posts(state)[0];
    assert.equal(admission.url, state.respondUrl);
    assert.equal(admission.body.action, 'manual');
    assert.equal(admission.body.version, 6);
    assert.equal(Object.hasOwn(admission.body, 'blueprint'), false, 'Intent review cannot replace the newer server blueprint before agent review');
    assert.deepEqual(admission.body.manual_intent, { base: state.original, draft: edited });
    assert.equal(admission.body.manual_intent.draft.agent_classes[0].instructions, humanInstructions);
    assert.equal(admission.body.manual_intent.draft.project.description, humanPurpose);
    assertUnsent(document, state);
  });
});

test('normal idle same-owner Advanced return keeps ordinary manual blueprint submission', async t => {
  const state = fixture({ running: false });
  await mountedApp(t, state, async document => {
    assertUnsent(document, state);
    await click(document, 'Advanced editor');
    await editPurpose(document);
    await editInstructions(document);
    const edited = state.draft();
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assert.equal(posts(state).length, 1);
    assert.equal(posts(state)[0].body.version, state.guide.version);
    assert.equal(posts(state)[0].body.action, 'manual');
    assert.deepEqual(posts(state)[0].body.blueprint, edited);
    assert.equal(Object.hasOwn(posts(state)[0].body, 'manual_intent'), false);
    assertUnsent(document, state);
  });
});

test('an idle pending manual blueprint remains the real Advanced base for another deliberate edit', async t => {
  const pendingPlan = basePlan();
  pendingPlan.project.description = 'Earlier pending manual purpose.';
  const pending = { blueprint: pendingPlan, text: 'Earlier manually adjusted purpose.', changes: ['Project and purpose'] };
  const state = fixture({ running: false, pending });
  await mountedApp(t, state, async document => {
    await click(document.querySelector('.setup-manual-note'), 'Adjust changes');
    assert.deepEqual(state.draft(), pendingPlan);
    await editInstructions(document);
    const edited = state.draft();
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assert.equal(posts(state).length, 1);
    assert.deepEqual(posts(state)[0].body.blueprint, edited);
    assert.equal(posts(state)[0].body.blueprint.project.description, pendingPlan.project.description);
    assert.equal(posts(state)[0].body.blueprint.agent_classes[0].instructions, humanInstructions);
    assertUnsent(document, state);
  });
});

test('completion of an older manual request cannot consume distinct new Advanced intent returned while it was running', async t => {
  const oldPlan = basePlan();
  oldPlan.project.description = 'The old manual purpose already being reviewed.';
  const oldPending = { blueprint: oldPlan, text: 'Original manual changes already admitted.', changes: ['Project and purpose'] };
  const state = fixture({ action: 'manual', pending: oldPending });
  // Match the retained operation to the older pending blueprint, not the new edit.
  state.guide.operation.request.blueprint = structuredClone(oldPlan);
  state.setServer(state.guide);
  await mountedApp(t, state, async (document, poll) => {
    await openFallback(document, state, poll);
    assert.deepEqual(state.draft(), oldPlan);
    await editInstructions(document);
    const newDraft = state.draft();
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    const newPending = state.manual();
    assert.deepEqual(newPending.manual_intent, { base: oldPlan, draft: newDraft });
    assert.equal(posts(state).length, 0);
    const current = changedServerPlan(state);
    state.setReadFault(false);
    state.setServer(completed(state, current));
    await poll();
    assert.deepEqual(state.manual(), newPending, 'The older manual completion clears only its own submitted pending feedback');
    assert.ok(button(document.querySelector('.setup-manual-note'), 'Review changes against current plan'));
    assert.deepEqual(visibleProposal(document), current);
    assert.equal(posts(state).length, 0, 'Observing old completion never resends the new intent');
    assertUnsent(document, state);
  });
});

test('undoing already-submitted manual intent retains a new entry-draft delta after the older operation completes', async t => {
  const original = basePlan(), oldDraft = structuredClone(original);
  oldDraft.project.description = 'Older submitted purpose that the human now undoes.';
  oldDraft.agent_classes[0].instructions = 'Older submitted instructions that the human now undoes.';
  const oldPending = { blueprint: oldDraft, text: 'Original manual intent already admitted.',
    changes: ['Project and purpose', 'Update Observing planner'],
    manual_intent: { base: original, draft: oldDraft } };
  const state = fixture({ action: 'manual', pending: oldPending });
  state.guide.operation.request = { version: state.guide.version, action: 'manual', selected: [],
    text: oldPending.text, manual_intent: structuredClone(oldPending.manual_intent) };
  state.setServer(state.guide);
  await mountedApp(t, state, async (document, poll) => {
    await openFallback(document, state, poll);
    assert.deepEqual(state.draft(), oldDraft, 'Fallback edits the actual pending draft, not the visible current server plan');
    await editPurpose(document, original.project.description);
    await editInstructions(document, original.agent_classes[0].instructions);
    assert.deepEqual(state.draft(), original, 'The human deliberately undoes all previously submitted changes through real fields');
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    const undo = state.manual();
    assert.deepEqual(undo.manual_intent, { base: oldDraft, draft: original },
      'The new human delta is from the editor entry draft, not an empty comparison against the old intent base');
    assert.equal(posts(state).length, 0);
    state.setReadFault(false);
    state.setServer(completed(state, oldDraft));
    await poll();
    assert.deepEqual(state.manual(), undo, 'Older completion cannot consume the distinct pending undo');
    assert.deepEqual(visibleProposal(document), oldDraft);
    assertUnsent(document, state);
    await click(document.querySelector('.setup-manual-note'), 'Review changes against current plan');
    assert.equal(posts(state).length, 1);
    assert.equal(posts(state)[0].body.version, 6);
    assert.equal(Object.hasOwn(posts(state)[0].body, 'blueprint'), false);
    assert.deepEqual(posts(state)[0].body.manual_intent, { base: oldDraft, draft: original });
    assertUnsent(document, state);
  });
});

test('a saved drift intent survives actual App reload and review refusal until an explicit retry', async t => {
  const original = basePlan(), edited = structuredClone(original);
  edited.project.description = humanPurpose;
  edited.agent_classes[0].instructions = humanInstructions;
  const pending = { blueprint: edited, text: 'Saved human project and instruction adjustments.',
    changes: ['Project and purpose', 'Update Observing planner'], proposal_changed: true,
    manual_intent: { base: original, draft: edited } };
  const current = changedServerPlan({ original });
  const state = fixture({ pending, savedDraft: current });
  state.setServer(completed(state, current));
  await mountedApp(t, state, async document => {
    assert.equal(state.calls.filter(call => call.url === '/api/blueprint/import').length, 1,
      'The composed reload uses the ordinary saved draft validation path');
    assert.equal(state.calls.filter(call => call.url === state.respondUrl).length, 0,
      'Restoring a pending intent never automatically resends it');
    assert.deepEqual(state.manual(), pending);
    assert.deepEqual(visibleProposal(document), current);
    assertUnsent(document, state);
    state.refuseNextManual('Synthetic current-plan review refused before admission.');
    await click(document.querySelector('.setup-manual-note'), 'Review changes against current plan');
    const reviews = () => state.calls.filter(call => call.url === state.respondUrl);
    assert.equal(reviews().length, 1);
    assert.match(document.querySelector('.setup-error').textContent, /Synthetic current-plan review refused before admission/);
    assert.deepEqual(state.manual(), pending, 'A refused review retains the exact recoverable human intent');
    assert.deepEqual(visibleProposal(document), current);
    assertUnsent(document, state);
    await click(document.querySelector('.setup-manual-note'), 'Review changes against current plan');
    assert.equal(reviews().length, 2, 'Only another explicit review click retries a refused admission');
    assert.deepEqual(reviews()[1].body, reviews()[0].body);
    assert.equal(reviews()[1].body.version, 6);
    assert.equal(Object.hasOwn(reviews()[1].body, 'blueprint'), false);
    assert.deepEqual(reviews()[1].body.manual_intent, pending.manual_intent);
    assertUnsent(document, state);
  });
});

test('normal anonymous Advanced return starts one guide and transfers human feedback and unsent composer to that owner', async t => {
  const state = fixture({ running: false, anonymous: true });
  await mountedApp(t, state, async document => {
    assertUnsent(document, state);
    await click(document, 'Advanced editor');
    await editPurpose(document);
    await editInstructions(document);
    const edited = state.draft();
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assert.deepEqual(posts(state).map(call => call.url), ['/api/guide/start', state.respondUrl],
      'Only the existing startup and ordinary manual admission are allowed');
    const admitted = posts(state)[1].body;
    assert.equal(admitted.version, 1);
    assert.equal(admitted.action, 'manual');
    assert.deepEqual(admitted.blueprint, edited);
    assert.equal(Object.hasOwn(admitted, 'manual_intent'), false);
    assert.deepEqual(state.manual().blueprint, edited, 'Pending human feedback belongs to the newly admitted guide');
    assert.equal(state.values.has(manualKey(null)), false, 'Anonymous feedback has transferred rather than shadowing the saved owner');
    assert.equal(state.values.has(state.input.key), false, 'The old anonymous composer is retired after its exact owner transfer');
    assert.equal(state.values.get(GUIDE_KEY), state.server().id);
    assertUnsent(document, state);
  });
});

test('a real newClass manual request binds its normalized running receipt and clears only its own saved note after remount and completion', async t => {
  const state = fixture({ running: false, normalizeSubmitted: request => {
    for (const agent of request.blueprint.agent_classes) {
      agent.members ??= [];
      agent.tools = { workspace: false, notes: 'none', builtins: false, shell: false,
        memory: true, workgraph: false, schedule: false, knowledge: false, mcp: [], ...agent.tools };
    }
    return request;
  } });
  await mountedApp(t, state, async (document, poll, remount) => {
    await click(document, 'Advanced editor');
    const add = document.querySelector('.app-shell [title="Add an agent class"]');
    assert.ok(add, 'The actual Advanced sidebar supplies the normal newClass control');
    await act(async () => { add.click(); await settle(); });
    const edited = state.draft(), newAgent = edited.agent_classes[1];
    assert.equal(newAgent.id, 'specialist');
    assert.equal(Object.hasOwn(newAgent, 'members'), false);
    assert.equal(Object.hasOwn(newAgent.tools, 'memory'), false,
      'The real newClass draft omits defaults that the server will normalize');
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    const reviews = () => state.calls.filter(call => call.url === state.respondUrl);
    assert.equal(reviews().length, 1);
    assert.deepEqual(reviews()[0].body.blueprint, edited);
    const normalized = structuredClone(state.server().operation.request.blueprint);
    assert.notDeepEqual(normalized, edited, 'The running receipt genuinely has server-populated defaults');
    assert.deepEqual(normalized.agent_classes[1].members, []);
    assert.equal(normalized.agent_classes[1].tools.memory, true);
    assert.deepEqual(state.manual().blueprint, edited, 'Normalization never replaces the saved human draft');
    assert.equal(state.manual().submitted_request_id, state.server().operation.request_id,
      'The admitted server receipt identity binds this exact pending snapshot');
    assertUnsent(document, state);
    state.expectSavedDraftImport(edited);
    await remount();
    assert.equal(reviews().length, 1, 'Recovering the existing running receipt never resubmits the adjustment');
    assert.deepEqual(state.manual().blueprint, edited);
    assert.equal(state.manual().submitted_request_id, state.server().operation.request_id);
    assertUnsent(document, state);
    const admitted = state.server();
    state.setServer({ ...structuredClone(admitted), version: admitted.version + 1,
      proposal: normalized, operation: { ...admitted.operation, status: 'completed' } });
    await poll();
    assert.equal(state.manual(), null, 'Its exact completed request clears the bound note despite normalized blueprint defaults');
    assert.equal(document.querySelector('.setup-manual-note') === null, true);
    assert.deepEqual(visibleProposal(document), normalized);
    assert.equal(reviews().length, 1);
    assertUnsent(document, state);
  });
});

test('a lost newClass response recovers current server context and retains exact human intent for deliberate review without a stale full-plan POST', async t => {
  const state = fixture({ running: false, normalizeSubmitted: request => {
    if (request.blueprint) for (const agent of request.blueprint.agent_classes) {
      agent.members ??= [];
      agent.tools = { workspace: false, notes: 'none', builtins: false, shell: false,
        memory: true, workgraph: false, schedule: false, knowledge: false, mcp: [], ...agent.tools };
    }
    return request;
  } });
  await mountedApp(t, state, async document => {
    await click(document, 'Advanced editor');
    const add = document.querySelector('.app-shell [title="Add an agent class"]');
    assert.ok(add, 'The human uses the normal Advanced newClass control');
    await act(async () => { add.click(); await settle(); });
    const edited = state.draft();
    assert.equal(edited.agent_classes[1].id, 'specialist');
    assert.equal(Object.hasOwn(edited.agent_classes[1], 'members'), false);
    assert.equal(Object.hasOwn(edited.agent_classes[1].tools, 'memory'), false);
    state.loseNextManualResponse('Synthetic manual response lost after the server admitted it.');
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    const reviews = () => state.calls.filter(call => call.url === state.respondUrl);
    assert.equal(reviews().length, 1);
    assert.deepEqual(reviews()[0].body.blueprint, edited, 'The initial ordinary manual admission contains the actual human draft');
    const recovery = document.querySelector('.setup-delivery-recovery');
    assert.ok(!!recovery, 'An uncertain response exposes the existing read-only recovery control');
    assert.match(recovery.textContent, /Message delivery is not confirmed/);
    assert.equal(state.server().operation.status, 'running', 'The server retained admission before its response was lost');
    assert.notDeepEqual(state.server().operation.request.blueprint, edited,
      'The lost receipt contains genuine server normalization, so exact raw blueprint comparison cannot acknowledge it');
    assert.deepEqual(state.manual().blueprint, edited);
    assert.equal(Object.hasOwn(state.manual(), 'submitted_request_id'), false,
      'A lost response cannot bind a receipt that this App never observed');
    assertUnsent(document, state);
    const admitted = state.server(), current = structuredClone(admitted.operation.request.blueprint);
    current.project.name = 'Server-completed crew with an unrelated correction';
    current.agent_classes[0].description = 'SERVER-ONLY-CORRECTION: preserve the updated evidence boundary.';
    state.setServer({ ...structuredClone(admitted), version: admitted.version + 1, proposal: current,
      operation: { ...admitted.operation, status: 'completed' } });
    await click(recovery, 'Check saved conversation');
    assert.equal(reviews().length, 1, 'Recovering the retained completed receipt never resends the old full blueprint');
    assert.deepEqual(state.manual().blueprint, edited, 'The unbound human adjustment stays recoverable');
    assert.deepEqual(state.manual().manual_intent, { base: state.original, draft: edited });
    assert.deepEqual(visibleProposal(document), current, 'Unrelated newer server context remains the visible authority');
    assertUnsent(document, state);
    await click(document.querySelector('.setup-manual-note'), 'Review changes against current plan');
    assert.equal(reviews().length, 2, 'Only explicit current-plan review submits the retained human intent');
    const retry = reviews()[1].body;
    assert.equal(retry.version, admitted.version + 1);
    assert.equal(retry.action, 'manual');
    assert.equal(Object.hasOwn(retry, 'blueprint'), false, 'Review cannot overwrite the newer full proposal with an obsolete human draft');
    assert.deepEqual(retry.manual_intent, { base: state.original, draft: edited });
    assert.equal(retry.manual_intent.draft.project.name, state.original.project.name);
    assert.equal(current.project.name, 'Server-completed crew with an unrelated correction');
    assertUnsent(document, state);
  });
});

test('a legacy ambiguous full-plan note keeps its draft available but cannot review stale context over a newer completed proposal', async t => {
  const legacyDraft = basePlan();
  legacyDraft.project.description = 'An older saved manual purpose with no retained comparison base.';
  const legacy = { blueprint: legacyDraft, text: 'Original manual changes already admitted.', changes: ['Project and purpose'] };
  const state = fixture({ action: 'manual', pending: legacy });
  const normalized = structuredClone(legacyDraft);
  normalized.agent_classes[0].members = [];
  normalized.agent_classes[0].tools = { workspace: false, notes: 'none', builtins: false,
    shell: false, memory: true, workgraph: false, schedule: false, knowledge: false, mcp: [] };
  state.guide.operation.request.blueprint = normalized;
  state.setServer(state.guide);
  const current = structuredClone(normalized);
  current.project.name = 'Newer authoritative crew context';
  current.agent_classes[0].description = 'SERVER-ONLY-CORRECTION: preserve the later observing constraint.';
  state.setServer(completed(state, current));
  await mountedApp(t, state, async document => {
    const note = document.querySelector('.setup-manual-note');
    assert.ok(note, 'The ambiguous historical adjustment remains available for human recovery');
    const review = [...note.querySelectorAll('button')].find(node => node.textContent.trim().startsWith('Review changes'));
    assert.ok(review, 'The existing review control exposes its recovery refusal');
    assert.equal(review.disabled, true, 'Without an exact historical base, stale full-plan review must be unavailable');
    await act(async () => { review.click(); await settle(); });
    assert.equal(posts(state).length, 0, 'A disabled ambiguous review cannot overwrite the newer full proposal');
    assert.deepEqual(state.manual().blueprint, legacyDraft);
    assert.equal(Object.hasOwn(state.manual(), 'manual_intent'), false,
      'Recovery cannot invent the missing historical comparison base');
    assert.deepEqual(visibleProposal(document), current);
    assertUnsent(document, state);
    await click(note, 'Adjust changes');
    assert.deepEqual(state.draft(), legacyDraft, 'The actual Advanced editor still exposes the retained human draft');
    await click(document.querySelector('.app-shell'), 'Back to guided setup');
    assert.equal(posts(state).length, 0, 'Opening and returning the unchanged saved draft is not a new manual admission');
    assert.deepEqual(state.manual().blueprint, legacyDraft);
    assert.deepEqual(visibleProposal(document), current);
    assertUnsent(document, state);
  });
});

// These read-only recovery cases reuse the composed App fixture above. Headers
// and bodies deliberately ignore AbortSignal so late server responses also run.
test('guide read deadlines release the mounted timeline and its startup recovery observation', async t => {
  for (const stage of ['headers', 'body']) await t.test(stage, async t => {
    const state = fixture({ running: false });
    const timeline = deferred(), status = deferred();
    const read = state.read, body = state.responseBody;
    let timelineStarted = false, statusStarted = false, timelineReads = 0;
    const emptyPage = { available: true, frames: [], bindings: [], exhausted: true };
    const failedStatus = { available: true, status: 'failed', startup_retry_required: true };
    state.read = async (url, options) => {
      if (url.includes('/timeline?')) {
        timelineReads += 1;
        if (!timelineStarted) {
          timelineStarted = true; timeline.signal = options.signal;
          if (stage === 'headers') return timeline.promise;
          await read(url, options); return emptyPage;
        }
      }
      if (url === '/api/architect/status' && !statusStarted) {
        statusStarted = true; status.signal = options.signal;
        if (stage === 'body') return status.promise;
        await read(url, options); return failedStatus;
      }
      return read(url, options);
    };
    state.responseBody = (url, value, signal) => {
      if (url.includes('/timeline?') && stage === 'body' && timelineReads === 1) return timeline.promise;
      if (url === '/api/architect/status' && stage === 'headers') return status.promise;
      return body(url, value, signal);
    };
    await mountedApp(t, state, async document => {
      assert.equal(timelineStarted, true, 'The actual useGuideTimeline read is withheld');
      await act(async () => { t.mock.timers.tick(30000); await settle(); });
      assert.equal(statusStarted, true, 'Timeline expiry reaches the actual startup-recovery read');
      assert.equal(timeline.signal?.aborted, true);
      assert.match(document.querySelector('.setup-stream-status')?.textContent || '', /Reconnecting/);
      await act(async () => { t.mock.timers.tick(30000); await settle(); });
      assert.equal(status.signal?.aborted, true, 'The recovery catch has its own bounded read');
      await act(async () => { t.mock.timers.tick(2500); await settle(); });
      assert.equal(timelineReads, 2, 'The same timeline owner reconnects after both reads settle');
      assert.equal(document.querySelector('.setup-stream-status') === null, true);
      timeline.resolve(emptyPage); status.resolve(failedStatus);
      await act(async () => { await settle(); });
      assert.equal(document.querySelector('.setup-stream-status') === null, true, 'Late expired observations cannot restore a failed startup latch');
      assert.equal([...document.querySelectorAll('button')].some(node => node.textContent.trim() === 'Retry setup team'), false);
      assertUnsent(document, state);
      assert.deepEqual(posts(state), [], 'Read recovery never retries start or the admitted answer');
    });
  });
});

test('guide read deadlines release a mounted running-operation poll and ignore its late old reply', async t => {
  for (const stage of ['headers', 'body']) await t.test(stage, async t => {
    const state = fixture(), read = state.read, body = state.responseBody;
    const stalled = deferred();
    let withhold = false, started = false;
    const guideUrl = `/api/guide/${state.guide.id}`, oldReply = structuredClone(state.server());
    state.read = async (url, options) => {
      if (url === guideUrl && withhold && !started) {
        started = true; stalled.signal = options.signal;
        if (stage === 'headers') return stalled.promise;
        await read(url, options); return oldReply;
      }
      return read(url, options);
    };
    state.responseBody = (url, value, signal) => {
      if (url === guideUrl && stage === 'body' && withhold && started && !stalled.bodyStarted) {
        stalled.bodyStarted = true; return stalled.promise;
      }
      return body(url, value, signal);
    };
    await mountedApp(t, state, async (document, poll) => {
      withhold = true;
      await poll();
      assert.equal(started, true, 'Opening succeeded; the actual running-state interval is now withheld');
      await act(async () => { t.mock.timers.tick(30000); await settle(); });
      assert.equal(stalled.signal?.aborted, true);
      assert.match(document.querySelector('.setup-error')?.textContent || '', /Connection interrupted.*taking too long/);
      assertUnsent(document, state);
      const next = completed(state, changedServerPlan(state));
      state.setServer(next);
      await poll();
      assert.equal(visibleProposal(document).project.name, next.proposal.project.name);
      assert.equal(document.querySelector('.setup-native-phase') === null, true, 'Completion clears the retained running presentation');
      assert.equal(document.querySelector('.setup-error') === null, true);
      stalled.resolve(oldReply);
      await act(async () => { await settle(); });
      assert.equal(visibleProposal(document).project.name, next.proposal.project.name);
      assert.equal(document.querySelector('.setup-native-phase') === null, true);
      assertUnsent(document, state);
      assert.deepEqual(posts(state), [], 'Observation timeout and recovery do not admit another turn');
    });
  });
});

test('guide read deadlines keep the mounted busy architect poll single-flight and prevent late status rollback', async t => {
  for (const stage of ['headers', 'body']) await t.test(stage, async t => {
    const state = fixture(), read = state.read, body = state.responseBody;
    const held = deferred();
    let statusReads = 0;
    const stale = { available: true, busy: true, status: 'ready', model: 'Obsolete held model' };
    const current = { available: true, busy: true, status: 'ready', model: 'Recovered current model' };
    state.read = async (url, options) => {
      if (url === '/api/architect/status') {
        statusReads += 1;
        await read(url, options);
        if (statusReads === 1) {
          held.signal = options.signal;
          if (stage === 'headers') return held.promise;
          return stale;
        }
        return current;
      }
      return read(url, options);
    };
    state.responseBody = (url, value, signal) => {
      if (url === '/api/architect/status' && stage === 'body' && statusReads === 1) return held.promise;
      return body(url, value, signal);
    };
    await mountedApp(t, state, async document => {
      await act(async () => { t.mock.timers.tick(4000); await settle(); });
      assert.equal(statusReads, 1);
      await act(async () => { t.mock.timers.tick(28000); await settle(); });
      assert.equal(statusReads, 1, 'Further interval ticks cannot pile up held architect reads');
      await act(async () => { t.mock.timers.tick(2000); await settle(); });
      assert.equal(held.signal?.aborted, true);
      await act(async () => { t.mock.timers.tick(2000); await settle(); });
      assert.equal(statusReads, 2, 'The existing interval resumes after its read deadline');
      assert.match(document.querySelector('.setup-provider-model').textContent, /Recovered current model/);
      held.resolve(stale);
      await act(async () => { await settle(); });
      assert.match(document.querySelector('.setup-provider-model').textContent, /Recovered current model/);
      assert.doesNotMatch(document.querySelector('.setup-shell').textContent, /Obsolete held model/);
      assertUnsent(document, state);
      assert.deepEqual(posts(state), [], 'Status polling retains read-only authority');
    });
  });
});
