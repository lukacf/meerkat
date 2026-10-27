import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';

const webRoot = process.env.FORGE_TEST_WEB_ROOT || fileURLToPath(new URL('.', import.meta.url));
const sourcePath = process.env.FORGE_COMPOSE_SOURCE || join(webRoot, 'guided.jsx');
const require = createRequire(join(webRoot, 'package.json'));
const React = require('react'), { act } = React;
const { build } = require('esbuild');
const { JSDOM } = createRequire(resolve(webRoot, '../../console/package.json'))('jsdom');
const { inputKey } = await import(pathToFileURL(join(webRoot, 'wizard-state.mjs')).href);
const { GUIDE_KEY } = await import(pathToFileURL(join(webRoot, 'guided-state.mjs')).href);
const source = await readFile(sourcePath, 'utf8');
console.log('Mounted guide source:', sourcePath, createHash('sha256').update(source).digest('hex'));
const directory = await mkdtemp(join(tmpdir(), 'forge-compose-mode-ownership-'));
const modulePath = join(directory, 'guided.mjs');
after(() => rm(directory, { recursive: true, force: true }));
const built = await build({
  stdin: { contents: source, sourcefile: sourcePath, resolveDir: webRoot, loader: 'jsx' },
  bundle: true, write: false, platform: 'node', format: 'esm', jsx: 'automatic',
  outfile: modulePath, sourcemap: 'linked', sourcesContent: true, logLevel: 'silent',
  loader: { '.css': 'empty' }, nodePaths: [join(webRoot, 'node_modules')],
  alias: { '@console-core': resolve(webRoot, '../../packages/console-core/src') },
  plugins: [{ name: 'shared-real-react', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
for (const output of built.outputFiles) {
  await writeFile(output.path, output.contents);
  if (output.path.endsWith('.map') && process.env.FORGE_COMPOSE_MAP)
    await writeFile(process.env.FORGE_COMPOSE_MAP, output.contents);
}
const { GuidedSetup } = await import(pathToFileURL(modulePath).href);

test('a delayed original completed response preserves the newer chat draft through exact readback, a saved-guide round-trip and its actual next send', async () => {
  const firstText = 'Our workshop starts with two coordinators.';
  const nextText = ' \tBefore deciding scale, explain how each person keeps private context.\n\u00e5\u{1f9ed}  ';
  const guide = (id, title) => ({ id, version: 5, status: 'question', stage: 'purpose',
    completed_stages: [], answers: [], brief: { facts: {}, requirements: [] }, capabilities: { available: true },
    messages: [{ id: `job-${id}`, role: 'user', text: 'Coordinate community workshop repair requests.' }],
    question: { id: `people-${id}`, stage: 'purpose', title, description: 'Describe the people this helps.',
      multiple: false, allow_text: true, options: [{ id: 'coordinators', label: 'Coordinators', description: 'People with their own context.' }] } });
  const a = guide('owned-compose-a', 'Who should the workshop help?');
  const b = guide('owned-compose-b', 'Who should the observing system help?');
  const inputA = { key: inputKey(a), draft: firstText, selected: [], mode: 'answer' };
  const inputB = { key: inputKey(b), draft: '  Keep this observing idea separate.  ', selected: [], mode: 'answer' };
  const values = new Map([[GUIDE_KEY, a.id], ...[inputA, inputB].map(input => [input.key, JSON.stringify(input)])]);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const document = dom.window.document, calls = [];
  let serverA = a, completed, resolveOriginalBody, responseDeadline, bodyEntered = false, postCount = 0, root;
  const originalBody = new Promise(resolve => { resolveOriginalBody = resolve; });
  const respondUrl = `/api/guide/${a.id}/respond`;
  const realSetTimeout = globalThis.setTimeout;
  const settle = () => new Promise(resolve => realSetTimeout(resolve, 8));
  const replacements = { window: dom.window, document, navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window), IS_REACT_ACT_ENVIRONMENT: true,
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    localStorage: storage, sessionStorage: dom.window.sessionStorage,
    setTimeout(callback, delay, ...args) {
      if (delay === 30000) {
        responseDeadline = callback;
        return realSetTimeout(() => {}, 60000);
      }
      return realSetTimeout(callback, delay, ...args);
    },
    async fetch(url, options = {}) {
      const method = options.method || 'GET', body = options.body && JSON.parse(options.body);
      calls.push({ url, method, body });
      assert.ok(method === 'GET' || method === 'POST' && url === respondUrl,
        `Only the selected offline guide may receive explicit sends: ${method} ${url}`);
      let result;
      if (url === respondUrl && method === 'POST') {
        postCount += 1;
        if (postCount === 1) {
          completed = { ...structuredClone(a), version: 6,
            question: { ...a.question, id: 'scale-a', title: 'What scale should this support?',
              options: [{ id: 'small', label: 'A small group', description: 'A few people sharing work.' }] },
            messages: [...a.messages, { id: 'first-admitted', role: 'user', text: firstText }],
            operation: { status: 'completed', request_id: 'b'.repeat(64), version: a.version,
              request: { stage: null, blueprint: null, ...structuredClone(body) } } };
          serverA = completed;
          return { ok: true, status: 200, json: async () => {
            bodyEntered = true;
            return await originalBody;
          } };
        } else {
          assert.equal(postCount, 2, 'Readback cannot replay the uncertain first send; only one later deliberate send is permitted.');
          result = serverA = { ...structuredClone(completed), operation: { status: 'running', request_id: 'c'.repeat(64),
            version: completed.version, started_at: new Date().toISOString(), request: body } };
        }
      } else if (url === `/api/guide/${a.id}`) result = structuredClone(serverA);
      else if (url === `/api/guide/${b.id}`) result = structuredClone(b);
      else if (url === '/api/guide/list') result = { setups: [{ id: a.id, name: 'Workshop setup A', status: 'question' },
        { id: b.id, name: 'Observing setup B', status: 'question' }] };
      else if (url.includes('/timeline')) result = { frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true };
      else if (url === '/api/architect/status') result = { state: 'working' };
      assert.ok(result, `Unexpected offline request: ${method} ${url}`);
      return { ok: true, json: async () => result };
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  const composer = () => document.querySelector('#setup-message');
  const button = text => {
    const node = [...document.querySelectorAll('button')].find(item => item.textContent.trim() === text || item.querySelector('span')?.textContent === text);
    assert.ok(node, `Missing actual guide action: ${text}`);
    return node;
  };
  const click = node => act(async () => { node.click(); await settle(); });
  const edit = text => act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(composer(), text);
    composer().dispatchEvent(new dom.window.Event('input', { bubbles: true }));
    composer().dispatchEvent(new dom.window.Event('change', { bubbles: true }));
  });
  const assertNextDraft = () => {
    assert.equal(composer().value, nextText, 'The different next draft retains exact text.');
    assert.equal(button('Ask or suggest a change').getAttribute('aria-pressed'), 'true',
      'A late completed reply must not change the newer draft from chat to answer.');
    assert.equal(document.querySelector('#setup-send').getAttribute('aria-label'), 'Send message');
    const savedInput = JSON.parse(values.get(inputKey(completed)));
    assert.equal(savedInput.draft, nextText); assert.equal(savedInput.mode, 'chat');
  };
  try {
    const { createRoot } = require('react-dom/client');
    root = createRoot(document.getElementById('root'));
    await act(async () => { root.render(React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null,
      onBlueprint() {}, config: { architect: { available: true } } })); await settle(); });
    await act(settle);
    assert.equal(composer().value, firstText);
    await click(document.querySelector('#setup-send'));
    assert.equal(bodyEntered, true, 'The original HTTP response body remains held after server completion.');
    assert.equal(postCount, 1);
    assert.equal(composer().value, firstText, 'Pending acknowledgement retains the exact first intent.');
    assert.equal(composer().disabled, false, 'The next draft is editable while the completed reply is delayed.');
    await edit(nextText);
    await click(button('Ask or suggest a change'));
    assert.equal(button('Ask or suggest a change').getAttribute('aria-pressed'), 'true');
    assert.equal(document.querySelector('#setup-send').disabled, true, 'Editing next-turn context cannot submit during the delayed response.');
    assert.equal(typeof responseDeadline, 'function');
    await act(async () => { responseDeadline(); await settle(); });
    const delivery = document.querySelector('[aria-label="Message delivery status"]');
    assert.equal(delivery.querySelector('[role="status"]').textContent, 'Message delivery is not confirmed');
    assert.doesNotMatch(delivery.textContent, /Your submitted answer is saved|Guide request running/);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.equal(composer().value, nextText);
    await click(button('Check saved conversation'));
    assert.equal(document.querySelector('[aria-label="Message delivery status"]') === null, true,
      'Only exact saved request readback releases the uncertain Send.');
    assert.equal(postCount, 1, 'Checking the saved guide does not repeat the admitted request.');
    assert.match(document.body.textContent, /What scale should this support/);
    assertNextDraft();
    await act(async () => { resolveOriginalBody(structuredClone(completed)); await settle(); });
    assert.equal(postCount, 1, 'Releasing the retired original body cannot automatically send the new draft.');
    assertNextDraft();

    await click(button('Your setups')); await click(button('Observing setup B'));
    assert.equal(composer().value, inputB.draft);
    await click(button('Your setups')); await click(button('Workshop setup A'));
    assertNextDraft();
    assert.equal(postCount, 1, 'Saved-guide switching is read-only and never sends either draft.');
    assert.deepEqual(JSON.parse(values.get(inputB.key)), inputB);

    await click(document.querySelector('#setup-send'));
    const later = calls.filter(call => call.method === 'POST')[1];
    assert.equal(later.url, respondUrl); assert.equal(later.body.version, completed.version);
    assert.equal(later.body.action, 'chat', 'The real next POST must discuss the draft instead of answering the scale question.');
    assert.equal(later.body.text, nextText); assert.deepEqual(later.body.selected, []);
    assert.equal(postCount, 2);
    assert.equal(composer().value, '', 'Actual admission still clears the exact submitted next draft.');
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
});
