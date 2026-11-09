import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { webcrypto } from 'node:crypto';

const repository = process.env.FORGE_EXPORT_TEST_REPO_ROOT || fileURLToPath(new URL('../../', import.meta.url));
const web = join(repository, 'installer/web');
const require = createRequire(join(web, 'package.json'));
const { build } = require('esbuild');
const React = require('react');
const { act } = React;
const { JSDOM } = createRequire(join(repository, 'console/package.json'))('jsdom');
const { DRAFT_KEY } = await import(pathToFileURL(join(web, 'state.mjs')));
const { GUIDE_KEY } = await import(pathToFileURL(join(web, 'guided-state.mjs')));
const source = process.env.FORGE_EXPORT_APP_SOURCE || join(web, 'app.jsx');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
const bundleDirectory = await mkdtemp(join(tmpdir(), 'forge-advanced-export-'));
const appModulePath = join(bundleDirectory, 'app.mjs');
after(() => rm(bundleDirectory, { recursive: true, force: true }));
// The actual App, guide, editor, validation and download handlers remain real.
// Only unrelated Console drawing/generated animation are replaced. No server,
// agent, archive producer, credential or deployment operation is exercised.
const bundled = await build({
  entryPoints: [join(web, 'app.jsx')], bundle: true, write: false, format: 'esm', platform: 'node',
  outfile: appModulePath, sourcemap: 'linked', sourcesContent: true,
  logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' }, nodePaths: [join(web, 'node_modules')],
  alias: { '@console-core': join(repository, 'packages/console-core/src') },
  plugins: [{ name: 'actual-app-inert-export', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /\/installer\/web\/app\.jsx$/ }, async () => ({
      contents: await readFile(source, 'utf8'), loader: 'jsx', resolveDir: web,
    }));
    builder.onLoad({ filter: /console-workbench\.[tj]sx$/ }, () => ({
      contents: `import React from 'react'; export function ConsoleWorkbench(props) {
        return React.createElement('div', {'data-inert-console-drawing': true}, props.launcherHeader,
          props.launcher, props.launcherFooter, props.mainHeader, props.main, props.mainFooter,
          props.activityRailHeader, props.activityRail);
      }`, loader: 'js',
    }));
    builder.onLoad({ filter: /topology-panel\.[tj]sx$/ }, () => ({ contents: 'export const TopologyPanel = () => null;', loader: 'js' }));
    builder.onLoad({ filter: /conversation-pane\.[tj]sx$/ }, () => ({ contents: 'export const ConversationPane = () => null;', loader: 'js' }));
    builder.onLoad({ filter: /conversation-transcript\.[tj]sx$/ }, () => ({ contents: 'export const ConversationTranscript = () => null;', loader: 'js' }));
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: `import React from 'react'; export function ProposalSimulation(props) {
        return React.createElement('div', {'data-plan': JSON.stringify(props.plan)});
      }`, loader: 'js',
    }));
  } }],
});
for (const output of bundled.outputFiles) await writeFile(output.path, output.contents);
// Retain the exact test-bundle input map when an isolated evidence path is supplied.
if (process.env.FORGE_EXPORT_SOURCE_MAP) {
  const map = bundled.outputFiles.find(output => output.path.endsWith('.map'));
  await writeFile(process.env.FORGE_EXPORT_SOURCE_MAP, map.contents);
}
const appModuleUrl = pathToFileURL(appModulePath).href;
const plan = () => ({
  schema_version: 2, project: { name: 'Owned export draft', slug: 'owned-export-draft', description: 'Inert Advanced export ownership.' },
  provider: 'openai', model: 'gpt-6.1-sol', target: 'mac',
  agent_classes: [{ id: 'planner', name: 'Planner', description: 'Plan the job.', instructions: 'Keep the user context.',
    instances: 1, members: [], provider: 'openai', model: 'gpt-6.1-sol',
    tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
  topology: { mode: 'manual', entrypoint: 'planner:main', edges: [] }, connections: [], flows: [],
  knowledge: { mode: 'none' }, advanced: { port: 4110 },
});
const deferred = () => {
  let resolve;
  const promise = new Promise(yes => { resolve = yes; });
  return { promise, resolve };
};
const jsonResponse = (body, status = 200) => ({ ok: status < 400, status, json: async () => body });
const validation = () => jsonResponse({ files: [], checks: ['Inert package preview'], identities: 1 });
const packageResponse = () => ({ ok: true, status: 200, blob: async () => new Blob(['inert package marker']) });
function fixture() {
  const guideId = 'owned-export-guide', values = new Map([[GUIDE_KEY, guideId], [DRAFT_KEY, JSON.stringify(plan())]]);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
  const calls = [], unexpected = [], downloads = [], blobs = new Map();
  let validationReply = () => validation(), packageReply = () => packageResponse();
  const fetch = async (url, options = {}) => {
    const method = options.method || 'GET', body = options.body ? JSON.parse(options.body) : undefined;
    const call = { url: String(url), method, ...(body === undefined ? {} : { body }) };
    calls.push(call);
    if (method === 'POST' && url === '/api/blueprint/import') return jsonResponse({ blueprint: body });
    if (method === 'POST' && url === '/api/blueprint/validate') return await validationReply();
    if (method === 'POST' && url === '/api/blueprint/package') return await packageReply();
    if (method !== 'GET') { unexpected.push(call); throw Error(`Unexpected inert mutation: ${method} ${url}`); }
    if (url === '/api/workbench') return jsonResponse({ blueprint: plan(), capabilities: {}, architect: { available: false, busy: false, model: 'gpt-6.1-sol' } });
    if (url === `/api/guide/${guideId}`) return jsonResponse({
      id: guideId, version: 1, status: 'question', stage: 'purpose', completed_stages: [], answers: [], messages: [],
      proposal: null, blueprint: null, brief: { facts: {}, requirements: [] }, capabilities: { available: false, model: 'gpt-6.1-sol' },
      question: { id: 'purpose', stage: 'purpose', title: 'What should this help with?', description: 'Inert saved guide.', options: [], multiple: false, allow_text: true },
    });
    if (url === '/api/guide/new') return jsonResponse({ id: null, version: 0, status: 'question', stage: 'purpose',
      completed_stages: [], answers: [], messages: [], proposal: null, blueprint: null,
      brief: { facts: {}, requirements: [] }, capabilities: { available: false }, question: null });
    if (url === '/api/guide/list') return jsonResponse({ setups: [{ id: guideId, name: 'Owned export guide', status: 'question' }] });
    if (String(url).startsWith(`/api/guide/${guideId}/timeline?`)) return jsonResponse({ available: true, frames: [], bindings: [], exhausted: true });
    if (url === '/api/provider-settings') return jsonResponse({
      providers: ['openai', 'anthropic', 'gemini'].map(provider => ({ provider, configured: false, source: null, editable: true })),
      effective_provider: null, runtime_active: false, work_active: false, load_error: null,
      locked_provider: null, provider_binding_error: null, runtime_status: 'stopped',
    });
    if (url === '/api/architect/status') return jsonResponse({ available: false, busy: false, model: 'gpt-6.1-sol' });
    if (url === '/api/capabilities') return jsonResponse({ rehearsal: { start_request_replay: false } });
    if (url === '/api/checker-setup') return jsonResponse({ status: 'ready', can_prepare: false, detail: 'Inert checker ready.' });
    if (url === `/api/guide/${guideId}/gcp-deployment`) return jsonResponse(null);
    if (['implementation', 'deployment', 'rehearsal', 'remote-deployment'].some(path => url === `/api/guide/${guideId}/${path}`) ||
        ['deployment', 'rehearsal', 'gcp-deployment', 'remote-deployment'].some(path => url === `/api/guide/null/${path}`))
      return jsonResponse({ detail: 'No saved activity in this inert export fixture.' }, 404);
    unexpected.push(call); throw Error(`Unexpected inert read: ${method} ${url}`);
  };
  return { values, storage, calls, unexpected, downloads, blobs, fetch, guideId,
    validationReply: value => { validationReply = value; }, packageReply: value => { packageReply = value; } };
}
async function mountedApp(t, state, verify) {
  const dom = new JSDOM('<div id="test-root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const OriginalURL = globalThis.URL;
  class DownloadURL extends OriginalURL {}
  DownloadURL.createObjectURL = blob => { const url = `blob:inert-export-${state.blobs.size + 1}`; state.blobs.set(url, blob); return url; };
  DownloadURL.revokeObjectURL = () => {};
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window), IS_REACT_ACT_ENVIRONMENT: true, crypto: webcrypto,
    URL: DownloadURL, matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    localStorage: state.storage, sessionStorage: dom.window.sessionStorage, fetch: state.fetch };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  dom.window.HTMLAnchorElement.prototype.click = function() { state.downloads.push({ href: this.href, name: this.download }); };
  let root;
  try {
    const { App } = await import(appModuleUrl);
    const { createRoot } = require('react-dom/client');
    root = createRoot(dom.window.document.getElementById('test-root'));
    await act(async () => { root.render(React.createElement(App)); await settle(); });
    await act(async () => { await settle(); });
    const finish = async (waiting, value) => {
      await act(async () => { waiting.resolve(value); await settle(); });
      await act(async () => { await settle(); });
    };
    await verify({ document: dom.window.document, finish });
    assert.deepEqual(state.unexpected, [], 'Only explicitly mocked reads/import/preview/package requests are allowed');
    assert.ok(state.calls.filter(call => call.method !== 'GET').every(call =>
      ['/api/blueprint/import', '/api/blueprint/validate', '/api/blueprint/package'].includes(call.url)),
      'Export never starts a guide, model, provider, installation or native operation');
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}
function button(document, label) {
  const node = [...document.querySelectorAll('button')].find(node => node.textContent.trim() === label);
  assert.ok(node, `Missing actual control: ${label}`); return node;
}
async function click(document, label) {
  const node = button(document, label); assert.equal(node.disabled, false, `${label} is available`);
  await act(async () => { node.click(); await settle(); }); await act(async () => { await settle(); });
}
async function enterAdvanced(document) {
  await click(document, 'Advanced editor'); await click(document, 'Deploy your system');
  assert.equal(document.querySelector('.app-shell .project-switch').textContent.trim(), 'Owned export draft');
}
async function newIdenticalOwner(document) {
  await click(document, 'Back to guided setup'); await enterAdvanced(document);
}
const alerts = document => [...document.querySelectorAll('.error-banner, .setup-global-error')].map(node => node.textContent).join(' ');
const success = document => [...document.querySelectorAll('.toast')].some(node => node.textContent.includes('Your deployment package is ready.'));

// One mounted journey exercises the same existing selection owner across all
// three awaits, including a replacement owner with byte-identical blueprint.
test('Advanced export belongs to its exact editor and blueprint through preview, response and body awaits', async t => {
  t.mock.timers.enable({ apis: ['Date', 'setInterval', 'setTimeout'], now: Date.parse('2026-10-09T08:00:00Z') });
  for (const scenario of ['response-owner', 'body-blueprint', 'validation-owner', 'package-error-owner', 'review-error-owner', 'current']) {
    const state = fixture();
    await mountedApp(t, state, async ({ document, finish }) => {
      await enterAdvanced(document);
      const waiting = deferred();
      if (scenario === 'response-owner') {
        state.packageReply(() => waiting.promise);
        await click(document, 'Download deployment');
        const captured = state.calls.find(call => call.url === '/api/blueprint/package').body;
        await newIdenticalOwner(document);
        assert.deepEqual(JSON.parse(state.values.get(DRAFT_KEY)), captured, 'Replacement owner has identical blueprint bytes');
        await finish(waiting, packageResponse());
        assert.equal(state.downloads.length, 0, 'A package response for a retired Advanced owner must not download');
        assert.equal(success(document), false, 'A stale response must not toast over the new owner');
      } else if (scenario === 'body-blueprint') {
        state.packageReply(() => ({ ok: true, status: 200, blob: () => waiting.promise }));
        await click(document, 'Download deployment');
        await act(async () => { document.querySelector('.project-switch').click(); await settle(); });
        const input = document.querySelector('.modal-body input');
        assert.ok(input);
        await act(async () => {
          Object.getOwnPropertyDescriptor(document.defaultView.HTMLInputElement.prototype, 'value').set.call(input, 'Newer export draft');
          input.dispatchEvent(new document.defaultView.Event('input', { bubbles: true })); await settle();
        });
        await click(document, 'Save project');
        assert.equal(document.querySelector('.project-switch').textContent.trim(), 'Newer export draft');
        await finish(waiting, new Blob(['old inert package marker']));
        assert.equal(state.downloads.length, 0, 'A body for an edited blueprint must not download');
        assert.equal(success(document), false); assert.equal(alerts(document), '');
        assert.equal(JSON.parse(state.values.get(DRAFT_KEY)).project.name, 'Newer export draft');
      } else if (scenario === 'validation-owner' || scenario === 'review-error-owner') {
        state.validationReply(() => waiting.promise);
        await click(document, scenario === 'review-error-owner' ? 'Review package' : 'Download deployment');
        await newIdenticalOwner(document);
        await finish(waiting, jsonResponse({ detail: 'Old owner validation refusal.' }, 400));
        assert.equal(alerts(document), '', 'Late validation errors cannot overwrite an identical replacement owner');
        assert.equal(state.calls.filter(call => call.url === '/api/blueprint/package').length, 0);
        assert.equal(state.downloads.length, 0); assert.equal(success(document), false);
      } else if (scenario === 'package-error-owner') {
        state.packageReply(() => ({ ok: false, status: 400, json: () => waiting.promise }));
        await click(document, 'Download deployment'); await newIdenticalOwner(document);
        await finish(waiting, { detail: 'Old owner package refusal.' });
        assert.equal(alerts(document), '', 'Late package error JSON cannot overwrite a replacement owner');
        assert.equal(state.downloads.length, 0); assert.equal(success(document), false);
      } else {
        await click(document, 'Download deployment');
        assert.equal(state.downloads.length, 1, 'A current package still downloads exactly once');
        assert.equal(state.downloads[0].name, 'owned-export-draft.zip');
        assert.equal(await state.blobs.get(state.downloads[0].href).text(), 'inert package marker');
        assert.deepEqual(state.calls.find(call => call.url === '/api/blueprint/package').body, plan());
        assert.equal(success(document), true);
        state.validationReply(() => jsonResponse({ detail: 'Current validation refusal.' }, 400));
        await click(document, 'Review package'); assert.match(alerts(document), /Current validation refusal/);
        state.validationReply(() => validation()); state.packageReply(() => jsonResponse({ detail: 'Current package refusal.' }, 400));
        await click(document, 'Download deployment'); assert.match(alerts(document), /Current package refusal/);
        assert.equal(state.downloads.length, 1, 'Failed requests do not duplicate the successful current export');
      }
      assert.equal(state.values.get(GUIDE_KEY), state.guideId, 'The saved guide selection is preserved');
    });
  }
});
