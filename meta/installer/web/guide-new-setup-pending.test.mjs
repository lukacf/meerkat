import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';

const web = process.env.FORGE_NEW_SETUP_WEB || fileURLToPath(new URL('.', import.meta.url));
const selectedSource = process.env.FORGE_NEW_SETUP_SOURCE || join(web, 'guided.jsx');
const require = createRequire(join(web, 'package.json'));
const React = require('react'), { act } = React;
const { build } = require('esbuild');
const { JSDOM } = createRequire(resolve(web, '../../console/package.json'))('jsdom');
const { inputKey } = await import(pathToFileURL(join(web, 'wizard-state.mjs')).href);
const { GUIDE_KEY } = await import(pathToFileURL(join(web, 'guided-state.mjs')).href);
const source = await readFile(selectedSource, 'utf8');
console.log('Mounted guide source:', selectedSource, createHash('sha256').update(source).digest('hex'));
const directory = await mkdtemp(join(tmpdir(), 'forge-new-setup-pending-'));
const modulePath = join(directory, 'guided.mjs');
after(() => rm(directory, { recursive: true, force: true }));
const built = await build({
  stdin: { contents: source, sourcefile: selectedSource, resolveDir: web, loader: 'jsx' },
  bundle: true, write: false, platform: 'node', format: 'esm', jsx: 'automatic',
  outfile: modulePath, sourcemap: 'linked', sourcesContent: true, logLevel: 'silent',
  loader: { '.css': 'empty' }, nodePaths: [join(web, 'node_modules')],
  alias: { '@console-core': resolve(web, '../../packages/console-core/src') },
  plugins: [{ name: 'shared-real-react', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    // Drawing is inert. Actual guide controls, Console transcript and parser remain mounted.
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: 'export const ProposalSimulation = () => null;', loader: 'js',
    }));
  } }],
});
for (const output of built.outputFiles) {
  await writeFile(output.path, output.contents);
  if (output.path.endsWith('.map') && process.env.FORGE_NEW_SETUP_MAP)
    await writeFile(process.env.FORGE_NEW_SETUP_MAP, output.contents);
}
const { GuidedSetup } = await import(pathToFileURL(modulePath).href);

test('starting a new setup retires only the outgoing pending bubble while preserving its running history and both drafts', async () => {
  const sentText = 'Prepare the workshop handover brief for the original team.';
  const nextText = '  Keep this later workshop thought unsent.\n\u00e5\u{1f9ed}  ';
  const anonymousText = '  My separate astronomy idea remains unsent.\nKeep its whitespace.  ';
  const a = { id: 'pending-new-setup-a', version: 4, status: 'question', stage: 'purpose',
    completed_stages: [], answers: [], brief: { facts: {}, requirements: [] }, capabilities: { available: true },
    messages: [{ id: 'prior-user', role: 'user', text: 'Our workshop helps several local teams.' },
      { id: 'prior-guide', role: 'assistant', text: 'Which handover should we explore first?' }],
    question: { id: 'workshop-purpose', stage: 'purpose', title: 'What should the workshop agents do?',
      description: 'Describe one useful job.', options: [], multiple: false, allow_text: true } };
  const opening = { id: null, version: 0, status: 'question', stage: 'purpose',
    completed_stages: [], answers: [], messages: [], brief: { facts: {}, requirements: [] }, capabilities: { available: true },
    question: { id: 'purpose', stage: 'purpose', title: 'What would you like to build?',
      description: 'Describe the outcome.', options: [], multiple: false, allow_text: true } };
  const initialA = { key: inputKey(a), draft: sentText, selected: [], mode: 'chat' };
  const anonymousInput = { key: inputKey(opening), draft: anonymousText, selected: [], mode: 'answer' };
  const values = new Map([[GUIDE_KEY, a.id], ...[initialA, anonymousInput].map(input => [input.key, JSON.stringify(input)])]);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const document = dom.window.document, calls = [];
  const realSetTimeout = globalThis.setTimeout;
  const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
  let serverA = a, running, openedNew = false, root, intervalId = 0;
  const intervals = new Map();
  const respondUrl = `/api/guide/${a.id}/respond`;
  const replacements = { window: dom.window, document, navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window), IS_REACT_ACT_ENVIRONMENT: true,
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    localStorage: storage, sessionStorage: dom.window.sessionStorage,
    setInterval(callback) { const id = ++intervalId; intervals.set(id, callback); return id; },
    clearInterval(id) { intervals.delete(id); },
    async fetch(url, options = {}) {
      const method = options.method || 'GET', body = options.body && JSON.parse(options.body);
      calls.push({ url, method, body });
      assert.ok(method === 'GET' || method === 'POST' && url === respondUrl,
        `No operation besides the original explicit answer is allowed: ${method} ${url}`);
      let result;
      if (url === respondUrl && method === 'POST') {
        assert.equal(calls.filter(call => call.method === 'POST').length, 1, 'Navigation cannot resubmit the original answer.');
        assert.equal(body.text, sentText); assert.equal(body.action, 'chat'); assert.equal(body.version, a.version);
        running = { ...structuredClone(a), operation: { status: 'running', request_id: 'a'.repeat(64), version: a.version,
          started_at: '2026-10-09T11:00:00Z', request: structuredClone(body), detail: 'The original guide is working.' } };
        serverA = running;
        result = structuredClone(running);
      } else if (url === `/api/guide/${a.id}`) result = structuredClone(serverA);
      else if (url === '/api/guide/new') { openedNew = true; result = structuredClone(opening); }
      else if (url === '/api/guide/list') result = { setups: [{ id: a.id, name: 'Original workshop setup', status: 'question' }] };
      else if (url.includes('/timeline')) result = openedNew ? {
        available: true, frames: [{ id: 'native-original-input', kind: 'user_input', payload: { content: sentText },
          interaction_id: 'original-running-turn', identity: 'architect:main', timestamp_ms: Date.parse(running.operation.started_at),
          cursor: 'console:1', frame_version: 1, source: { kind: 'runtime' } }],
        bindings: [{ interaction_id: 'original-running-turn', cursor: 'console:1', user_message_id: 'original-admitted-user',
          user_text: sentText, operation_request_id: running.operation.request_id, operation_started_at: running.operation.started_at }],
        next_cursor: 'console:1', latest_cursor: 'console:1', exhausted: true,
      } : { available: true, frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true };
      else if (url === '/api/architect/status') result = { available: true, status: 'ready', busy: true };
      assert.ok(result, `Unexpected offline request: ${method} ${url}`);
      return { ok: true, json: async () => result };
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  const composer = () => document.querySelector('#setup-message');
  const chat = () => document.querySelector('.setup-native-chat');
  const button = text => {
    const node = [...document.querySelectorAll('button')].find(item => item.textContent.trim() === text || item.querySelector('span')?.textContent === text);
    assert.ok(node, `Missing actual guide action: ${text}`); assert.equal(node.disabled, false);
    return node;
  };
  const click = node => act(async () => { node.click(); await settle(); });
  try {
    const { createRoot } = require('react-dom/client');
    root = createRoot(document.getElementById('root'));
    await act(async () => { root.render(React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null,
      onBlueprint() {}, config: { architect: { available: true, status: 'ready' } } })); await settle(); });
    await act(settle);
    assert.equal(composer().value, sentText);
    await click(document.querySelector('#setup-send'));
    assert.equal(calls.filter(call => call.method === 'POST').length, 1);
    assert.equal(document.querySelector('#setup-send').disabled, true, 'The original answer is admitted and running.');
    assert.ok(chat().textContent.includes(sentText), 'The real Console transcript shows the pending original answer.');
    assert.equal(composer().value, '', 'Exact admission still consumes the original draft.');
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(composer(), nextText);
      composer().dispatchEvent(new dom.window.Event('input', { bubbles: true }));
      await settle();
    });
    const retainedA = JSON.parse(values.get(inputKey(a)));
    assert.equal(retainedA.draft, nextText);
    assert.deepEqual(JSON.parse(values.get(anonymousInput.key)), anonymousInput);
    const originalRunning = structuredClone(running);

    await click(button('Your setups')); await click(button('+ Start a new setup'));
    assert.equal(composer().value, anonymousText, 'The anonymous draft is restored, not replaced by either A text.');
    assert.equal(values.get(GUIDE_KEY), undefined, 'The new conversation does not claim the original saved guide.');
    assert.ok(!chat().textContent.includes(sentText), 'The new anonymous guide must not inherit A\'s pending bubble.');
    assert.ok(!chat().textContent.includes(a.messages[0].text), 'The old saved history is not presented as new history.');
    assert.deepEqual(JSON.parse(values.get(inputKey(a))), retainedA);
    assert.deepEqual(JSON.parse(values.get(anonymousInput.key)), anonymousInput);
    assert.deepEqual(serverA, originalRunning, 'Navigation cannot alter the original running operation or saved history.');
    assert.equal(calls.filter(call => call.method !== 'GET').length, 1, 'No resubmit, stop or new-guide creation occurs.');

    await click(button('Your setups')); await click(button('Original workshop setup'));
    await act(settle);
    assert.equal(composer().value, nextText);
    assert.ok(chat().textContent.includes(a.messages[0].text), 'Returning restores original saved history.');
    assert.ok(chat().textContent.includes(sentText), 'The original running native admission remains in its own conversation.');
    assert.equal(document.querySelector('#setup-send').disabled, true, 'The original operation remains running.');
    assert.deepEqual(serverA, originalRunning);
    assert.deepEqual(JSON.parse(values.get(inputKey(a))), retainedA);
    assert.deepEqual(JSON.parse(values.get(anonymousInput.key)), anonymousInput);
    assert.equal(calls.filter(call => call.method !== 'GET').length, 1);
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
});
