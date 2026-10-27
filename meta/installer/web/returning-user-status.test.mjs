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

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 25));
const directory = await mkdtemp(join(tmpdir(), 'forge-returning-user-status-'));
after(() => rm(directory, { recursive: true, force: true }));
const modulePath = join(directory, 'guided.mjs');
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('guided.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', outfile: modulePath,
  logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'same-react-inert-drawing', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: 'export const ProposalSimulation = () => null;', loader: 'js',
    }));
  } }],
});
for (const output of bundled.outputFiles) await writeFile(output.path, output.contents);
const { GuidedSetup } = await import(pathToFileURL(modulePath).href);

test('returning-user status: actual guide keeps current navigation outside its long historical transcript', async () => {
  const id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
  const oldReply = 'Your application is currently stopped and native runtime availability is false.\n\n' +
    'This is a retained historical explanation. '.repeat(45);
  const plan = { schema_version: 2, project: { name: 'Synthetic retained application', slug: 'synthetic-retained-application' },
    target: 'mac', provider: 'openai', model: 'gpt-6.1-sol', agent_classes: [],
    topology: { mode: 'manual', edges: [] }, connections: [], flows: [], knowledge: { mode: 'none' } };
  const guide = { id, version: 4, status: 'ready', stage: 'review', proposal: plan, blueprint: null,
    question: null, completed_stages: [], answers: [], brief: { facts: {}, requirements: [] },
    readiness: { ready: true, decision_blockers: [] }, capabilities: { available: true },
    messages: [{ id: 'old-request', role: 'user', text: 'How is this application doing?', created_at: '2026-10-01T09:59:00Z' },
      { id: 'old-reply', role: 'assistant', text: oldReply, created_at: '2026-10-01T10:00:00Z' }] };
  const deployment = { guide_id: id, guide_version: 4, deployment_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
    build_id: 'original-build', current: true, status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:4110/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } },
    source_digest: 'a'.repeat(64), package_sha256: 'b'.repeat(64), destination: '/synthetic/original-application' };
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  dom.window.localStorage.setItem(GUIDE_KEY, id);
  const calls = [], revealed = [];
  const fetch = async (url, options = {}) => {
    const method = options.method || 'GET';
    calls.push({ url, method });
    assert.equal(method, 'GET', 'The status anchor must never submit work.');
    let value;
    if (url === `/api/guide/${id}`) value = guide;
    else if (url === `/api/guide/${id}/deployment`) value = deployment;
    else if (url.startsWith(`/api/guide/${id}/timeline?`)) value = { available: true, frames: [], bindings: [], exhausted: true };
    else if (url === '/api/architect/status') value = { available: true, status: 'ready' };
    else if (url === '/api/provider-settings') value = { runtime_active: true, work_active: false };
    else if (url === '/api/capabilities') value = {};
    else value = { httpError: 404, detail: 'No additional synthetic activity.' };
    return { ok: !value.httpError, status: value.httpError || 200, json: async () => structuredClone(value) };
  };
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    fetch, crypto: webcrypto, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() { revealed.push(this); };
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root'));
  const props = { active: true, bp: plan, Icon: () => null, onBlueprint() {}, config: { architect: { available: true } } };
  const render = async changes => { await act(async () => { root.render(React.createElement(GuidedSetup, { ...props, ...changes })); await settle(); }); };
  try {
    await render();
    await act(async () => settle());
    const document = dom.window.document, scroll = document.querySelector('.setup-chat-scroll');
    const anchor = document.querySelector('.setup-current-status');
    assert.ok(anchor, 'The actual mounted guide renders the current projection.');
    assert.equal(anchor.parentNode === scroll.parentNode, true);
    assert.equal(scroll.contains(anchor), false);
    assert.ok(anchor.compareDocumentPosition(scroll) & dom.window.Node.DOCUMENT_POSITION_FOLLOWING);
    assert.match(anchor.textContent, /Your installed custom application is running/);
    assert.match(scroll.textContent, /Your application is currently stopped and native runtime availability is false/);
    assert.ok(scroll.textContent.includes('This is a retained historical explanation. '.repeat(45).trim()));
    assert.equal(guide.messages[1].text, oldReply, 'Historical source text stays exact.');
    assert.ok(scroll.querySelector('.setup-progress-card'), 'The full current status card remains available.');
    assert.equal(document.querySelector('.setup-conversation-history-boundary').parentNode === scroll.parentNode, true);
    const button = anchor.querySelector('button');
    assert.equal(button.textContent.trim(), 'Use your application');
    assert.equal(button.disabled, false);
    await act(async () => { button.click(); await settle(); });
    const target = document.querySelector('#setup-deployment');
    assert.deepEqual(revealed, [target], 'The existing action only reveals the original deployment panel.');
    assert.equal(target.closest('details').open, true);
    assert.equal(calls.filter(call => call.method !== 'GET').length, 0);
    const captured = button[Object.keys(button).find(key => key.startsWith('__reactProps$'))].onClick;
    await render({ appError: 'Current application request failed.' });
    assert.equal(document.querySelector('.setup-current-status') === null, true, 'A transient error remains primary.');
    assert.match(document.querySelector('.setup-chat-top').textContent, /The guide needs your attention/);
    await act(async () => { captured({ currentTarget: button }); await settle(); });
    assert.equal(revealed.length, 1, 'A detached old anchor cannot reveal another status panel.');
    guide.operation = { status: 'failed', detail: 'Your installed custom application is running' };
    const reload = [...document.querySelectorAll('button')].find(node => node.textContent === 'Reload saved conversation');
    await act(async () => { reload.click(); await settle(); });
    await render();
    assert.equal(document.querySelector('.setup-current-status') === null, true,
      'Saved failure wording identical to the application title cannot become an application-status anchor.');
    assert.match(document.querySelector('.setup-chat-top').textContent, /Your installed custom application is running/);
    await render({ active: false });
    await act(async () => { captured({ currentTarget: button }); await settle(); });
    assert.equal(revealed.length, 1, 'An inactive guide cannot reuse the captured navigation.');
    assert.equal(calls.filter(call => call.method !== 'GET').length, 0);
  } finally {
    await act(async () => { root.unmount(); await settle(); });
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});
