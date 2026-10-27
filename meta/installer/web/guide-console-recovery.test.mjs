import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act } from 'react';
import { GUIDE_KEY } from './guided-state.mjs';
import { inputKey } from './wizard-state.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const webRoot = fileURLToPath(new URL('.', import.meta.url));
const compiled = await build({ stdin: { contents: 'export { GuidedSetup } from "./guided.jsx"; export { NativeGuideChat, useGuideTimeline } from "./native-guide-chat.jsx";',
  resolveDir: webRoot, loader: 'jsx' }, bundle: true, write: false, format: 'esm', platform: 'node',
  logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' }, nodePaths: [`${webRoot}/node_modules`],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'shared-real-react', setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { GuidedSetup, NativeGuideChat, useGuideTimeline } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString('base64')}`);
const deferred = () => { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; };
const settle = () => new Promise(resolve => setTimeout(resolve, 5));
const page = { available: true, frames: [], bindings: [], exhausted: true };
const failed = { available: true, status: 'failed', startup_retry_required: true, console_url: null, browser_console_url: null };
const ready = { available: true, status: 'ready', startup_retry_required: false, console_url: 'http://127.0.0.1:51842/console',
  browser_console_url: '/api/owned-console/guide/11111111-1111-4111-8111-111111111111/console' };
const guide = { id: 'offline-console-recovery', version: 5, status: 'question', stage: 'purpose', completed_stages: [], answers: [],
  brief: { facts: {}, requirements: [] }, capabilities: { available: true },
  messages: [{ id: 'saved-context', role: 'assistant', text: 'Your saved conversation is kept.' }],
  question: { id: 'who', stage: 'purpose', title: 'Who will use it?', multiple: false, allow_text: true,
    options: [{ id: 'household', label: 'My household', description: 'People with their own context.' }] } };

async function mount(onFetch, render, verify, { intervals } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const calls = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true, requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    fetch: async (url, options = {}) => {
      const call = { url, method: options.method || 'GET', body: options.body && JSON.parse(options.body), signal: options.signal };
      calls.push(call);
      return await onFetch(call);
    } };
  if (intervals) {
    replacements.setInterval = (callback, delay) => {
      const owner = { callback, delay, cancelled: false }; intervals.push(owner); return owner;
    };
    replacements.clearInterval = owner => { owner.cancelled = true; };
  }
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(document.getElementById('root'));
  const button = label => {
    const node = [...document.querySelectorAll('button')].find(item => item.textContent.trim() === label || item.getAttribute('aria-label') === label);
    assert.ok(node, `Missing actual recovery action: ${label}`); return node;
  };
  const click = node => act(async () => { node.click(); await settle(); });
  try {
    await act(async () => { root.render(render(dom.window)); await settle(); });
    await act(settle);
    await verify({ document, window: dom.window, calls, button, click,
      rerender: value => act(async () => { root.render(value); await settle(); }) });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}
const response = value => ({ ok: true, status: 200, json: async () => value });

test('held startup JSON can be checked independently without replaying Start or accepting its late old response', async () => {
  const body = deferred(); let bodyEntered = false, serverReady = false, checkReads = 0;
  function Conversation() {
    const snapshot = useGuideTimeline(guide);
    return React.createElement(NativeGuideChat, { state: guide, snapshot, Icon: () => null });
  }
  try {
    await mount(async call => {
      if (call.url.includes('/timeline?')) return serverReady ? response(page)
        : { ok: false, status: 503, json: async () => ({ detail: 'Startup requires a deliberate retry.' }) };
      if (call.url === '/api/architect/status' && call.method === 'GET') {
        if (bodyEntered) { checkReads += 1; serverReady = true; }
        return response(serverReady ? ready : failed);
      }
      assert.equal(call.url, '/api/architect/start'); assert.equal(call.method, 'POST'); assert.equal(call.body, undefined);
      return { ok: true, status: 200, json: async () => { bodyEntered = true; return await body.promise; } };
    }, () => React.createElement(Conversation), async ({ document, calls, button, click }) => {
      await click(button('Retry setup team'));
      assert.equal(bodyEntered, true, 'The actual mutating response decoder is still entered.');
      assert.equal(button('Retry setup team').disabled, true);
      const check = button('Check setup team status');
      assert.equal(check.disabled, false, 'A read-only escape remains reachable while the original body is held.');
      await click(check);
      assert.equal(checkReads, 1);
      assert.equal(calls.filter(call => call.method === 'POST').length, 1);
      assert.equal(calls.find(call => call.method === 'POST').signal, undefined, 'The original Start is retained without abort.');
      assert.ok(calls.filter(call => call.url.includes('/timeline?')).length >= 2, 'Fresh ready status resumes the real timeline owner.');
      assert.doesNotMatch(document.body.textContent, /Retrying setup team|Setup team could not start/);
      await act(async () => { body.resolve(failed); await settle(); });
      assert.doesNotMatch(document.body.textContent, /Retrying setup team|Setup team could not start/, 'The late old POST body cannot replace the newer observation.');
      assert.equal(calls.filter(call => call.method === 'POST').length, 1, 'Observation and late completion cannot replay Start or Send.');
    });
  } finally { body.resolve(ready); }
});

test('saved conversation Reload replaces old unavailable runtime before its exact preserved draft is sent', async () => {
  const draft = '  Help our household prepare tomorrow.\nKeep each person\'s context separate.  ';
  let current = structuredClone(guide);
  await mount(async call => {
    if (call.url === `/api/guide/${guide.id}`) return response(structuredClone(current));
    if (call.url === '/api/guide/list') return response({ setups: [] });
    if (call.url.includes('/timeline?')) return response(page);
    if (call.url === '/api/provider-settings') return response({ effective_provider: 'openai', providers: [{ provider: 'openai', source: 'environment' }],
      runtime_active: false, work_active: false });
    if (call.url === '/api/architect/status') return response(ready);
    assert.equal(call.url, `/api/guide/${guide.id}/respond`); assert.equal(call.method, 'POST');
    current = { ...structuredClone(current), version: current.version + 1, operation: { status: 'completed', request_id: 'b'.repeat(64),
      version: current.version, request: structuredClone(call.body) } };
    return response(current);
  }, window => {
    window.localStorage.setItem(GUIDE_KEY, guide.id);
    window.localStorage.setItem(inputKey(guide), JSON.stringify({ key: inputKey(guide), draft, selected: ['household'], mode: 'answer' }));
    return React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null, onBlueprint() {},
      appError: 'Forge restarted. Check the saved conversation.', config: { architect: { available: false, status: 'idle', console_url: null, browser_console_url: null } } });
  }, async ({ document, window, calls, button, click }) => {
    const editor = document.querySelector('#setup-message');
    const before = window.localStorage.getItem(inputKey(guide));
    assert.equal(editor.value, draft);
    await click(button('Reload saved conversation'));
    assert.ok(calls.some(call => call.url === '/api/architect/status'), 'Reload observes current runtime instead of retaining stale unavailable configuration.');
    assert.equal(document.querySelector('.setup-provider-model').textContent.includes('Provider setup needed'), false);
    assert.equal(document.querySelector('#setup-message') === editor, true);
    assert.equal(editor.value, draft);
    assert.equal(window.localStorage.getItem(inputKey(guide)), before);
    assert.deepEqual(calls.filter(call => call.method === 'POST'), [], 'Reload is read-only and retains the exact draft.');
    await click(button('Close provider settings'));
    await click(button('Send answer'));
    const posts = calls.filter(call => call.method === 'POST');
    assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { version: guide.version, action: 'answer', selected: ['household'], text: draft });
    assert.equal(posts[0].url, `/api/guide/${guide.id}/respond`);
  });
});

test('a delayed Reload runtime body cannot replace the newer current owner or the retained unsent draft', async () => {
  const body = deferred(); let entered = false;
  const draft = '  Keep the next question private.  ';
  const current = { ...ready, console_url: 'http://127.0.0.1:52614/console',
    browser_console_url: '/api/owned-console/guide/22222222-2222-4222-8222-222222222222/console' };
  const view = architect => React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null, onBlueprint() {},
    appError: 'Check the saved conversation after reconnecting.', config: { architect } });
  try {
    await mount(async call => {
      assert.equal(call.method, 'GET', 'Reload and owner updates cannot send or start anything.');
      if (call.url === `/api/guide/${guide.id}`) return response(structuredClone(guide));
      if (call.url === '/api/guide/list') return response({ setups: [] });
      if (call.url.includes('/timeline?')) return response(page);
      assert.equal(call.url, '/api/architect/status');
      return { ok: true, status: 200, json: async () => { entered = true; return await body.promise; } };
    }, window => {
      window.localStorage.setItem(GUIDE_KEY, guide.id);
      window.localStorage.setItem(inputKey(guide), JSON.stringify({ key: inputKey(guide), draft, selected: [], mode: 'chat' }));
      return view(ready);
    }, async ({ document, window, calls, button, click, rerender }) => {
      const editor = document.querySelector('#setup-message'), before = window.localStorage.getItem(inputKey(guide));
      await click(button('Reload saved conversation'));
      assert.equal(entered, true);
      await rerender(view(current));
      const link = () => document.querySelector('a[title="Inspect the live Forge setup team"]');
      assert.equal(link()?.href, new URL(current.browser_console_url, 'http://offline.test/').href);
      await act(async () => { body.resolve({ available: false, status: 'idle', console_url: null, browser_console_url: null }); await settle(); });
      assert.equal(link()?.href, new URL(current.browser_console_url, 'http://offline.test/').href, 'The old Reload read cannot retire the newer confirmed owner.');
      assert.equal(document.querySelector('.setup-provider-model').textContent.includes('Provider setup needed'), false);
      assert.equal(document.querySelector('#setup-message') === editor, true);
      assert.equal(editor.value, draft);
      assert.equal(window.localStorage.getItem(inputKey(guide)), before);
      assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
    });
  } finally { body.resolve(ready); }
});

test('a newer busy status observation fences a held Reload runtime body without changing saved writing', async () => {
  const body = deferred(), intervals = []; let reads = 0;
  const draft = '  Keep this follow-up while the admitted work finishes.  ';
  const active = { ...structuredClone(guide), operation: { status: 'running', request_id: 'c'.repeat(64), version: 4,
    request: { version: 4, action: 'chat', text: 'The already admitted original request.', selected: [] } } };
  const current = { ...ready, console_url: 'http://127.0.0.1:53914/console',
    browser_console_url: '/api/owned-console/guide/33333333-3333-4333-8333-333333333333/console' };
  try {
    await mount(async call => {
      assert.equal(call.method, 'GET', 'The fixture observes saved work without admitting any operation.');
      if (call.url === `/api/guide/${guide.id}`) return response(structuredClone(active));
      if (call.url === '/api/guide/list') return response({ setups: [] });
      if (call.url.includes('/timeline?')) return response(page);
      assert.equal(call.url, '/api/architect/status');
      if (++reads === 1) return { ok: true, status: 200, json: async () => await body.promise };
      return response(current);
    }, window => {
      window.localStorage.setItem(GUIDE_KEY, guide.id);
      window.localStorage.setItem(inputKey(active), JSON.stringify({ key: inputKey(active), draft, selected: [], mode: 'chat' }));
      return React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null, onBlueprint() {},
        appError: 'Check saved work after reconnecting.', config: { architect: ready } });
    }, async ({ document, window, calls, button, click }) => {
      const before = window.localStorage.getItem(inputKey(active));
      await click(button('Reload saved conversation'));
      assert.equal(reads, 1, 'The earlier Reload body is held.');
      const poll = intervals.findLast(owner => owner.delay === 4000 && !owner.cancelled);
      assert.ok(poll, 'The real current busy status owner remains mounted.');
      await act(async () => { await poll.callback(); await settle(); });
      const link = () => document.querySelector('a[title="Inspect the live Forge setup team"]');
      assert.equal(link()?.href, new URL(current.browser_console_url, 'http://offline.test/').href);
      await act(async () => { body.resolve({ available: false, status: 'idle', console_url: null, browser_console_url: null }); await settle(); });
      assert.equal(link()?.href, new URL(current.browser_console_url, 'http://offline.test/').href, 'The earlier read cannot overwrite a newer accepted current-status observation.');
      assert.equal(document.querySelector('#setup-message').value, draft);
      assert.equal(window.localStorage.getItem(inputKey(active)), before);
      assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
    }, { intervals });
  } finally { body.resolve(ready); }
});
