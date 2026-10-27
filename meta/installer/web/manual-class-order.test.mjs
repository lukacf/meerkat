import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';

// The optional paths select isolated reviewed bytes. Maintained use defaults to
// this module's web siblings. Only the generated drawing is inert.
const web = process.env.FORGE_CLASS_ORDER_WEB
  ? pathToFileURL(`${process.env.FORGE_CLASS_ORDER_WEB}/`)
  : new URL('.', import.meta.url);
const source = process.env.FORGE_CLASS_ORDER_SOURCE
  ? pathToFileURL(process.env.FORGE_CLASS_ORDER_SOURCE)
  : new URL('state.mjs', web);
const require = createRequire(new URL('package.json', web));
const { build } = require('esbuild');
const React = require('react');
const { act } = React;
const { JSDOM } = createRequire(new URL('../../console/package.json', web))('jsdom');
const { blueprintDiff, resolvedEdges } = await import(source.href);
const { GUIDE_KEY } = await import(new URL('guided-state.mjs', web));
const { inputKey, manualKey } = await import(new URL('wizard-state.mjs', web));
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 5));
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('guided.jsx', web))],
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent',
  jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', web))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', web)) },
  plugins: [{ name: 'current-guide-with-inert-drawing', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onResolve({ filter: /(?:^|\/)state\.mjs$/ }, () => ({ path: fileURLToPath(source) }));
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: `import React from 'react';
        export function ProposalSimulation(props) {
          return React.createElement('div', {'data-class-order-preview': JSON.stringify(props.plan)});
        }`, loader: 'js',
    }));
  } }],
});
const { GuidedSetup } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);

test('a reviewed pipeline class reorder reaches the guide with the exact edited proposal', async () => {
  const agent = (id, name) => ({ id, name, description: `Own ${name.toLowerCase()} context.`,
    instructions: 'Use the supplied evidence.', instances: 1, members: [],
    provider: 'openai', model: 'gpt-6.1-sol',
    tools: { memory: true, workspace: false, notes: 'none', mcp: [] } });
  const original = { schema_version: 2, project: { name: 'Evidence review', slug: 'evidence-review',
    description: 'Intake, analysis and independent review.' }, provider: 'openai', model: 'gpt-6.1-sol',
    target: 'mac', agent_classes: [agent('intake', 'Intake'), agent('analysis', 'Analysis'), agent('review', 'Review')],
    topology: { mode: 'pipeline', entrypoint: 'intake:main', edges: [] },
    connections: [], flows: [], knowledge: { mode: 'none' }, advanced: { port: 4110 } };
  const edited = structuredClone(original);
  edited.agent_classes = [edited.agent_classes[0], edited.agent_classes[2], edited.agent_classes[1]];
  const untouched = structuredClone(original);
  assert.deepEqual(resolvedEdges(original), [
    { a: 'intake:main', b: 'analysis:main' }, { a: 'analysis:main', b: 'review:main' },
  ]);
  assert.deepEqual(resolvedEdges(edited), [
    { a: 'intake:main', b: 'review:main' }, { a: 'review:main', b: 'analysis:main' },
  ]);
  const guide = { id: 'owned-pipeline-order', version: 5, status: 'refining', stage: 'review', question: null,
    proposal: original, blueprint: null, messages: [], answers: [], completed_stages: [],
    brief: { facts: {}, requirements: [] }, capabilities: { available: true },
    readiness: { ready: true, context_ready: true, decision_blockers: [], implementation_blockers: [] } };
  const input = { key: inputKey(guide), draft: '  Keep my next thought unsent.\n  ', selected: [], mode: 'chat' };
  const values = new Map([[GUIDE_KEY, guide.id], [input.key, JSON.stringify(input)]]);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null,
    get length() { return values.size; } };
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const calls = [], publications = [], intervals = new Map();
  let server = structuredClone(guide), nextTimer = 0;
  const respondUrl = `/api/guide/${guide.id}/respond`;
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true, localStorage: storage,
    sessionStorage: dom.window.sessionStorage,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    setInterval: (callback, delay) => { const id = ++nextTimer; intervals.set(id, { callback, delay }); return id; },
    clearInterval: id => intervals.delete(id),
    fetch: async (url, options = {}) => {
      const method = options.method || 'GET', body = options.body && JSON.parse(options.body);
      calls.push({ url, method, ...(body ? { body } : {}) });
      let result;
      if (method === 'POST' && url === '/api/blueprint/import') result = { blueprint: structuredClone(body) };
      else if (method === 'POST' && url === respondUrl) {
        server = { ...server, operation: { status: 'running', version: guide.version,
          request_id: 'a'.repeat(64), request: structuredClone(body) } };
        result = structuredClone(server);
      } else {
        assert.equal(method, 'GET', `Unrelated mutation refused: ${method} ${url}`);
        if (url === `/api/guide/${guide.id}`) result = structuredClone(server);
        else if (url === '/api/guide/list') result = { setups: [] };
        else if (url === '/api/architect/status') result = { state: 'ready', available: true };
        else if (url === '/api/capabilities') result = { rehearsal: { start_request_replay: false } };
        else if (url === '/api/checker-setup') result = { status: 'ready', can_prepare: false };
        else if (url.startsWith(`/api/guide/${guide.id}/timeline`))
          result = { available: true, frames: [], bindings: [], exhausted: true };
        else if (['implementation', 'deployment', 'rehearsal'].some(kind => url === `/api/guide/${guide.id}/${kind}`))
          result = { httpError: 404, detail: 'No saved fixture operation.' };
        else assert.fail(`Unexpected offline read: ${url}`);
      }
      return { ok: !result.httpError, status: result.httpError || 200, json: async () => result };
    },
  };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  const { createRoot } = require('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root'));
  const waitFor = async predicate => {
    for (let i = 0; i < 60 && !predicate(); i++) await act(async () => settle());
    assert.ok(predicate(), 'The real manual configuration request reached its expected boundary.');
  };
  try {
    await act(async () => { root.render(React.createElement(GuidedSetup, { active: true, bp: null,
      Icon: () => null, onBlueprint: value => publications.push(structuredClone(value)),
      config: { architect: { available: true } } })); await settle(); });
    const document = dom.window.document;
    await waitFor(() => document.querySelector('[data-class-order-preview]'));
    assert.deepEqual(JSON.parse(document.querySelector('[data-class-order-preview]').dataset.classOrderPreview), original);
    const configuration = [...document.querySelectorAll('button')].find(button => button.textContent.includes('Configuration'));
    assert.ok(configuration && !configuration.disabled);
    await act(async () => { configuration.click(); await settle(); });
    const editor = document.querySelector('#setup-config-source');
    assert.deepEqual(JSON.parse(editor.value), original);
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(editor, JSON.stringify(edited, null, 2));
      editor.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
      await settle();
    });
    const send = document.querySelector('[aria-labelledby="setup-config-title"] footer .setup-action');
    assert.ok(send && !send.disabled);
    await act(async () => { send.click(); await settle(); });
    await waitFor(() => calls.some(call => call.url === respondUrl));
    const mutations = calls.filter(call => call.method === 'POST');
    assert.deepEqual(mutations.map(call => call.url), ['/api/blueprint/import', respondUrl]);
    assert.deepEqual(mutations[0].body, edited, 'Import checks exactly the deliberately reordered draft.');
    assert.equal(mutations[1].body.version, guide.version);
    assert.equal(mutations[1].body.action, 'manual');
    assert.deepEqual(mutations[1].body.blueprint, edited, 'The guide receives the exact reordered proposal.');
    assert.match(mutations[1].body.text, /Agent order/);
    assert.deepEqual(JSON.parse(values.get(manualKey(guide.id))).blueprint, edited);
    assert.deepEqual(publications.at(-1), edited);
    assert.deepEqual(JSON.parse(document.querySelector('[data-class-order-preview]').dataset.classOrderPreview), edited,
      'The actual guide supplies the edited plan to its preview; generated animation stays inert.');
    assert.equal(document.querySelector('[aria-labelledby="setup-config-title"]') === null, true);
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.deepEqual(JSON.parse(values.get(input.key)), input);
    assert.deepEqual(original, untouched);
    assert.deepEqual(blueprintDiff(original, edited), ['Agent order']);
    const added = { ...structuredClone(original), agent_classes: [...structuredClone(original.agent_classes), agent('archive', 'Archive')] };
    assert.deepEqual(blueprintDiff(original, added), ['Add Archive (1 agent)']);
    const removed = { ...structuredClone(original), agent_classes: original.agent_classes.slice(0, 2) };
    assert.deepEqual(blueprintDiff(original, removed), ['Remove Review']);
    const updated = structuredClone(original); updated.agent_classes[1].instructions = 'Keep the original uncertainty.';
    assert.deepEqual(blueprintDiff(original, updated), ['Update Analysis']);
    assert.deepEqual(blueprintDiff(original, original), []);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});
