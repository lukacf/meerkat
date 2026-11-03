import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { webcrypto } from 'node:crypto';

const web = process.env.FORGE_BUILD_RETURN_WEB || fileURLToPath(new URL('.', import.meta.url));
const sourcePath = process.env.FORGE_BUILD_RETURN_SOURCE || join(web, 'guided.jsx');
const require = createRequire(join(web, 'package.json'));
const React = require('react'), { act } = React;
const { build } = require('esbuild');
const { JSDOM } = createRequire(resolve(web, '../../console/package.json'))('jsdom');
const { inputKey } = await import(pathToFileURL(join(web, 'wizard-state.mjs')));
const { GUIDE_KEY } = await import(pathToFileURL(join(web, 'guided-state.mjs')));
const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
const settle = () => new Promise(done => realSetTimeout(done, 10));
const directory = await mkdtemp(join(tmpdir(), 'forge-guided-build-return-'));
const modulePath = join(directory, 'guided.mjs');
after(() => rm(directory, { recursive: true, force: true }));

// The parent, saved-build component, canonical receipt reader, load gate and
// implementation panel are real. Unrelated drawing/transcript rendering is inert.
const built = await build({
  stdin: { contents: await readFile(sourcePath, 'utf8'), sourcefile: sourcePath, resolveDir: web, loader: 'jsx' },
  bundle: true, write: false, platform: 'node', format: 'esm', jsx: 'automatic',
  outfile: modulePath, sourcemap: 'linked', sourcesContent: true, logLevel: 'silent',
  loader: { '.css': 'empty' }, nodePaths: [join(web, 'node_modules')],
  alias: { '@console-core': resolve(web, '../../packages/console-core/src') },
  plugins: [{ name: 'actual-guided-build-return', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /conversation-transcript\.[tj]sx$/ }, () => ({
      contents: 'export const ConversationTranscript = () => null;', loader: 'js',
    }));
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: 'export const ProposalSimulation = () => null;', loader: 'js',
    }));
  } }],
});
for (const output of built.outputFiles) {
  await writeFile(output.path, output.contents);
  if (output.path.endsWith('.map') && process.env.FORGE_BUILD_RETURN_MAP)
    await writeFile(process.env.FORGE_BUILD_RETURN_MAP, output.contents);
}
const { GuidedSetup } = await import(pathToFileURL(modulePath));
const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};
const response = (body, status = 200) => ({ ok: status < 400, status, json: async () => structuredClone(body) });
const guide = (id, version = 1) => ({ id, version, status: 'question', stage: 'purpose',
  completed_stages: [], answers: [], messages: [], proposal: null, blueprint: null,
  brief: { facts: {}, requirements: [] }, capabilities: { available: false },
  question: { id: 'purpose', stage: 'purpose', title: 'What needs doing?', description: 'Describe a useful job.',
    options: [], multiple: false, allow_text: true } });
const plan = { schema_version: 2, project: { name: 'Handover notes', slug: 'handover-notes', description: 'Keep handovers useful.' },
  provider: 'openai', model: 'gpt-6.1-sol', target: 'mac',
  agent_classes: [{ id: 'lead', name: 'Handover owner', description: '', instructions: '', instances: 1,
    members: [], provider: 'openai', model: 'gpt-6.1-sol', tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
  topology: { mode: 'manual', entrypoint: 'lead:main', edges: [] }, connections: [], flows: [],
  knowledge: { mode: 'none' }, advanced: { port: 4110 } };
const rawDraft = '  Keep my new idea separate from the running handover build.\n\u00e5  ';
const button = (document, label) => [...document.querySelectorAll('button')]
  .find(item => item.textContent.trim() === label);
async function click(node) {
  assert.ok(node, 'The actual user control exists.');
  assert.equal(node.disabled, false, 'The selected navigation control is available.');
  await act(async () => { node.click(); await settle(); });
  await act(async () => settle());
}

async function mounted(verify, { historical = false, guideOperation = false } = {}) {
  const a = guide('build-return-a'), b = { ...guide('build-return-b', historical ? 4 : 3),
    proposal: structuredClone(plan), messages: [{ role: 'user', text: 'Make our handover notes useful.' }] };
  const c = { ...guide('build-return-c', 6), proposal: { ...structuredClone(plan),
    project: { name: 'Current setup C', slug: 'current-c', description: 'A different saved job.' } },
    messages: [{ role: 'user', text: 'Keep setup C selected.' }] };
  if (guideOperation) b.operation = { status: 'running', request_id: 'b'.repeat(64), version: b.version,
    started_at: '2026-10-13T10:00:00Z', request: { version: b.version, action: 'chat',
      selected: [], text: 'Clarify the handover audience.', attachments: [] } };
  const original = { guide_id: b.id, guide_version: historical ? 2 : b.version,
    current_guide_version: b.version, build_id: 'original-handover-build', current: !historical,
    status: historical ? 'completed' : 'running', phase: historical ? 'completed' : 'implementation',
    source_digest: 'a'.repeat(64), source_verified: false, ready_to_package: false, files: [], checks: [], progress: [] };
  const bSelection = deferred(), freshBuild = deferred(), calls = [], scrolls = [];
  const values = new Map([[GUIDE_KEY, a.id], [inputKey(a), JSON.stringify({
    key: inputKey(a), draft: rawDraft, selected: [], mode: 'answer' })]]);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null,
    get length() { return values.size; } };
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const document = dom.window.document;
  let root, bReads = 0, freshReads = 0, selectedRead = false, nextGuide = structuredClone(b), sequence = 0;
  const frames = new Map(), intervals = new Map(), timers = new Map();
  const replacements = { window: dom.window, document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: storage, sessionStorage: dom.window.sessionStorage,
    crypto: webcrypto, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: callback => { const key = ++sequence; frames.set(key, callback); return key; },
    cancelAnimationFrame: key => frames.delete(key),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    setInterval: (callback, delay) => { const key = ++sequence; intervals.set(key, { callback, delay }); return key; },
    clearInterval: key => intervals.delete(key),
    // Explicitly controlled UI polling/deadline timers, with no native work.
    setTimeout: (callback, delay, ...args) => {
      if (delay < 1000) return realSetTimeout(callback, delay, ...args);
      const key = { id: ++sequence }; timers.set(key, { callback: () => callback(...args), delay }); return key;
    },
    clearTimeout: key => { if (!timers.delete(key)) realClearTimeout(key); },
    fetch: async (target, options = {}) => {
      const url = String(target), method = options.method || 'GET';
      calls.push({ url, method });
      assert.equal(method, 'GET', 'Return to build cannot submit, resume, stop, install or start any work.');
      if (url === '/api/guide/list') return response({ setups: [
        { id: b.id, name: 'Handover notes', status: 'review' },
        { id: c.id, name: 'Current setup C', status: 'question' },
        { id: a.id, name: 'My new idea', status: 'question' }] });
      if (url === `/api/guide/${a.id}`) return response(a);
      if (url === `/api/guide/${b.id}`) {
        bReads += 1;
        return bReads === 1 ? await bSelection.promise : response(nextGuide);
      }
      if (url === `/api/guide/${c.id}`) return response(c);
      if (url === `/api/guide/${b.id}/implementation`) {
        if (!selectedRead) return response(original);
        freshReads += 1;
        return await freshBuild.promise;
      }
      if (url === '/api/checker-setup') return response({ status: 'ready', can_prepare: false, detail: 'Inert checker observation.' });
      if (url.includes('/timeline?')) return response({ frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true });
      if (url === '/api/architect/status') return response({ available: false, status: 'unavailable', busy: false });
      if (url === '/api/provider-settings') return response({ providers: [], effective_provider: null, runtime_active: false, work_active: false });
      return response({ detail: 'No saved fixture operation.' }, 404);
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {
    if ([...this.children].some(child => child.id === 'setup-implementation'))
      scrolls.push({ selectedGuide: values.get(GUIDE_KEY), connected: this.isConnected });
  };
  const finish = async (wait, value) => {
    await act(async () => { wait.resolve(value); await settle(); }); await act(async () => settle());
  };
  const selectB = async (value = b) => {
    selectedRead = true;
    nextGuide = structuredClone(value);
    await finish(bSelection, response(value));
  };
  const flushFrames = async () => {
    const batch = [...frames]; frames.clear();
    await act(async () => { for (const [, callback] of batch) callback(0); await settle(); });
    await act(async () => settle());
  };
  const chooseC = async () => {
    await click(button(document, 'Your setups'));
    const control = [...document.querySelectorAll('.setup-history-conversation')]
      .find(item => item.querySelector('span')?.textContent === 'Current setup C');
    await click(control);
    assert.equal(values.get(GUIDE_KEY), c.id, 'The actual setup chooser selected C.');
  };
  try {
    const { createRoot } = require('react-dom/client');
    root = createRoot(document.getElementById('root'));
    await act(async () => { root.render(React.createElement(GuidedSetup, {
      active: true, bp: plan, Icon: () => null, onBlueprint() {}, onAdvanced() {},
      config: { architect: { available: false } } })); await settle(); });
    await act(async () => settle());
    assert.ok(document.querySelector('.forge-welcome-resume .saved-setup-build-status'),
      'The actual welcome card observes its saved build through the production component.');
    await verify({ a, b, c, original, document, values, calls, scrolls, frames, intervals,
      freshBuild, finish, selectB, flushFrames, chooseC,
      freshReads: () => freshReads, updateGuide: value => { nextGuide = structuredClone(value); } });
    assert.ok(calls.every(call => call.method === 'GET'), 'Every request remained a read.');
    assert.equal(JSON.parse(values.get(inputKey(a))).draft, rawDraft, 'The new idea was never transferred or consumed.');
  } finally {
    if (root) await act(async () => root.unmount());
    bSelection.resolve(response(b)); freshBuild.resolve(response(original));
    await settle();
    frames.clear(); timers.clear(); intervals.clear(); dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test('Return to build waits for the selected guide and fresh canonical build read before opening its existing panel', async () => {
  await mounted(async ({ b, original, document, values, scrolls, freshBuild, finish, selectB, flushFrames, freshReads }) => {
    await click(button(document, 'Return to build'));
    await flushFrames();
    assert.equal(scrolls.length, 0, 'The chooser observation alone cannot navigate.');
    await selectB();
    assert.equal(values.get(GUIDE_KEY), b.id);
    assert.ok(freshReads() > 0, 'The mounted actual implementation owner requests a fresh canonical receipt.');
    await flushFrames();
    assert.equal(scrolls.length, 0, 'Loading the guide alone cannot navigate before its build read.');
    await finish(freshBuild, response(original));
    await flushFrames();
    assert.deepEqual(scrolls, [{ selectedGuide: b.id, connected: true }]);
    assert.equal(document.querySelector('.setup-proposal-actions').open, true);
    assert.ok(document.querySelector('#setup-implementation textarea'), 'The existing implementation panel is mounted.');
    await flushFrames();
    assert.equal(scrolls.length, 1, 'The explicit navigation is consumed once.');
  });
});

test('selecting C cancels Return to build B while the guide or fresh receipt is still held', async () => {
  for (const boundary of ['guide', 'receipt']) await mounted(async ({ b, c, original, document, values,
    scrolls, freshBuild, finish, selectB, flushFrames, chooseC }) => {
    await click(button(document, 'Return to build'));
    if (boundary === 'receipt') await selectB();
    await chooseC();
    if (boundary === 'guide') await selectB();
    await finish(freshBuild, response(original));
    await flushFrames();
    assert.equal(values.get(GUIDE_KEY), c.id, `A late ${boundary} result cannot reselect B.`);
    assert.equal(scrolls.length, 0, `A late ${boundary} result cannot scroll C's build area.`);
    assert.ok(document.querySelector('#setup-implementation').textContent.includes(c.proposal.project.name));
    assert.notEqual(values.get(GUIDE_KEY), b.id);
  });
});

test('a queued Return to build animation frame cannot act after C replaces the selected setup', async () => {
  await mounted(async ({ c, original, document, values, scrolls, frames, freshBuild, finish, selectB, flushFrames, chooseC }) => {
    await click(button(document, 'Return to build'));
    await selectB();
    await finish(freshBuild, response(original));
    assert.ok(frames.size > 0, 'The actual parent has queued its navigation frame.');
    assert.equal(scrolls.length, 0);
    await chooseC();
    await flushFrames();
    assert.equal(values.get(GUIDE_KEY), c.id);
    assert.equal(scrolls.length, 0, 'No earlier connected or reused panel may receive the queued jump.');
  });
});

test('a same-guide version advance cancels the old Return to build intent even when the fresh build is current', async () => {
  await mounted(async ({ b, original, document, values, scrolls, intervals, freshBuild, finish,
    selectB, flushFrames, updateGuide }) => {
    await click(button(document, 'Return to build'));
    await selectB();
    const advanced = { ...structuredClone(b), version: b.version + 1,
      operation: { ...structuredClone(b.operation), status: 'completed' } };
    updateGuide(advanced);
    const readers = [...intervals.values()].filter(timer => timer.delay === 2500);
    assert.ok(readers.length > 0, 'The actual pending guide observer is mounted.');
    await act(async () => { for (const timer of readers) await timer.callback(); await settle(); });
    await act(async () => settle());
    await finish(freshBuild, response({ ...original, current_guide_version: advanced.version }));
    await flushFrames();
    assert.equal(values.get(GUIDE_KEY), b.id);
    assert.equal(document.querySelector('.setup-current-question').dataset.guideVersion, String(advanced.version),
      'The real guide operation observer accepted the newer version.');
    assert.equal(scrolls.length, 0, 'A fresh current receipt cannot renew an older navigation intent.');
  }, { guideOperation: true });
});

test('the same running build can change its source digest during repair without losing read-only return navigation', async () => {
  await mounted(async ({ b, original, document, scrolls, freshBuild, finish, selectB, flushFrames }) => {
    await click(button(document, 'Return to build'));
    await selectB();
    const repaired = { ...original, phase: 'automatic_repair', source_digest: 'd'.repeat(64) };
    assert.notEqual(repaired.source_digest, original.source_digest);
    await finish(freshBuild, response(repaired));
    await flushFrames();
    assert.deepEqual(scrolls, [{ selectedGuide: b.id, connected: true }]);
    assert.match(document.querySelector('#setup-implementation').textContent, /findings from the independent review/);
  });
});

test('a historical completed build opens only its same canonical saved build without claiming current package readiness', async () => {
  await mounted(async ({ b, original, document, scrolls, freshBuild, finish, selectB, flushFrames }) => {
    assert.match(document.querySelector('.saved-setup-build-status').textContent, /earlier design/);
    await click(button(document, 'Return to build'));
    await selectB();
    await finish(freshBuild, response(original));
    await flushFrames();
    assert.deepEqual(scrolls, [{ selectedGuide: b.id, connected: true }]);
    assert.match(document.querySelector('#setup-implementation').textContent, /proposal has changed since this build/);
    assert.equal(button(document, 'Download checked application') === undefined, true,
      'Returning to historical source is not a package or installation grant.');
  }, { historical: true });
});

test('a replacement build for the same guide and version cannot inherit the selected original build return', async () => {
  await mounted(async ({ original, document, scrolls, freshBuild, finish, selectB, flushFrames }) => {
    await click(button(document, 'Return to build'));
    await selectB();
    await finish(freshBuild, response({ ...original, build_id: 'replacement-handover-build' }));
    await flushFrames();
    assert.equal(scrolls.length, 0, 'The fresh receipt must match the original selected build, not merely its guide/version.');
  });
});

test('the compact menu build action moves unchanged source-button focus to the named existing application build', async () => {
  await mounted(async ({ b, original, document, scrolls, freshBuild, finish, selectB, flushFrames }) => {
    await click(button(document, 'Your setups'));
    const entry = [...document.querySelectorAll('.setup-history-entry')]
      .find(item => item.querySelector('.setup-history-conversation span')?.textContent === 'Handover notes');
    const source = [...entry.querySelectorAll('button')]
      .find(item => item.getAttribute('aria-label') === 'Return to build for Handover notes');
    assert.ok(source, 'The actual menu names the full build action and exact setup accessibly.');
    assert.equal(source.textContent.trim(), 'Open build', 'Compact menu copy stays distinct from opening the conversation.');
    await act(async () => { source.focus(); await settle(); });
    assert.equal(document.activeElement === source, true);
    await click(source);
    await selectB();
    assert.equal(source.isConnected, false, 'The original focused menu control is detached by its own selection.');
    await finish(freshBuild, response(original));
    await flushFrames();
    const target = document.querySelector('[role="region"][aria-label="Application build"]');
    assert.ok(target, 'The existing implementation area has a focusable name.');
    assert.deepEqual(scrolls, [{ selectedGuide: b.id, connected: true }]);
    assert.equal(document.activeElement === target, true, 'An unchanged or naturally detached source focus reaches that same build area.');
    assert.equal(target.querySelector('#setup-implementation')?.isConnected, true);
  });
});

test('moving focus while the selected build read or navigation frame is held cancels the pending build jump', async () => {
  for (const boundary of ['read', 'frame']) await mounted(async ({ original, document, scrolls,
    frames, freshBuild, finish, selectB, flushFrames }) => {
    await click(button(document, 'Your setups'));
    const entry = [...document.querySelectorAll('.setup-history-entry')]
      .find(item => item.querySelector('.setup-history-conversation span')?.textContent === 'Handover notes');
    const source = [...entry.querySelectorAll('button')]
      .find(item => item.getAttribute('aria-label') === 'Return to build for Handover notes');
    assert.ok(source, 'The actual compact action remains bound to Handover notes.');
    await act(async () => { source.focus(); await settle(); });
    await click(source);
    await selectB();
    if (boundary === 'frame') {
      await finish(freshBuild, response(original));
      assert.ok(frames.size > 0, 'The real parent has queued navigation before the user moves focus.');
    }
    const editor = document.querySelector('#setup-message');
    assert.ok(editor, 'The current guide retains its real composer.');
    await act(async () => { editor.focus(); await settle(); });
    assert.equal(document.activeElement === editor, true);
    if (boundary === 'read') await finish(freshBuild, response(original));
    await flushFrames();
    assert.equal(scrolls.length, 0, `New focus cancels the ${boundary}-pending scroll as well as focus transfer.`);
    assert.equal(document.activeElement === editor, true, 'Return navigation cannot steal a newer user focus.');
  });
});
