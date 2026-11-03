import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { webcrypto } from 'node:crypto';
import { build } from 'esbuild';
import React, { act } from 'react';
import { GUIDE_KEY } from './guided-state.mjs';
import { GUIDE_CREATION_KEY, inputKey } from './wizard-state.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
const directory = await mkdtemp(join(tmpdir(), 'forge-guide-admission-'));
const modulePath = join(directory, 'guided.mjs');
after(() => rm(directory, { recursive: true, force: true }));
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('guided.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', outfile: modulePath,
  logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'real-guide-inert-drawing', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /conversation-transcript\.[tj]sx$/ }, () => ({
      contents: 'export const ConversationTranscript = () => null;', loader: 'js',
    }));
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: "import React from 'react'; export function ProposalSimulation(props) { return React.createElement('div', {'data-proposal-snapshot': JSON.stringify(props.plan)}); }",
      loader: 'js',
    }));
  } }],
});
for (const output of bundled.outputFiles) await writeFile(output.path, output.contents);
const { GuidedSetup } = await import(pathToFileURL(modulePath).href);
const A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const original = '  Help our neighbours share gardening tools.\nKeep this exact idea.  ';
const edited = `${original}\nA later, still unsent clarification.  `;
const clone = value => structuredClone(value);
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject, entered: false };
};
const storage = (values = new Map()) => ({ values,
  getItem: key => values.get(key) ?? null,
  setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key),
  key: index => [...values.keys()][index] ?? null, get length() { return values.size; },
});
function guide(id) {
  return { id, version: id ? 1 : 0, status: 'question', stage: 'purpose',
    completed_stages: [], answers: [], messages: [], blueprint: null, proposal: null,
    brief: { facts: {}, requirements: [] }, readiness: { ready: false, decision_blockers: [] },
    capabilities: { available: true }, question: { id: 'purpose', stage: 'purpose',
      title: 'What should your system do?', description: 'Describe one concrete job.',
      multiple: false, allow_text: true, options: [] } };
}
function fixture(stage, { holdInitial = true } = {}) {
  const initial = guide(stage === 'start' ? null : A);
  const guides = new Map([[A, guide(A)], [B, guide(B)]]);
  const local = storage(new Map(initial.id ? [[GUIDE_KEY, initial.id]] : []));
  const session = storage(), calls = [], held = deferred();
  let heldValue, creationResult = { httpError: 404, detail: 'No saved creation mapping yet.' };
  const laterBodies = [], getBodies = [], getFailures = new Map();
  const respondUrl = id => `/api/guide/${id}/respond`;
  const heldUrl = stage === 'start' ? '/api/guide/start' : respondUrl(A);
  const posts = () => calls.filter(call => call.method === 'POST');
  const fetch = async (url, options = {}) => {
    const method = options.method || 'GET';
    const multipart = options.body && typeof options.body.getAll === 'function';
    const call = { url, method, body: multipart ? options.body : options.body && JSON.parse(options.body), signal: options.signal };
    calls.push(call);
    let value;
    if (method === 'POST') {
      assert.ok(url === '/api/guide/start' || [...guides.keys()].some(id => url === respondUrl(id) || url === `/api/guide/${id}/attachments`),
        `No unrelated offline mutation is allowed: ${method} ${url}`);
      if (url === '/api/guide/start') {
        assert.deepEqual(Object.keys(call.body), ['start_request_id']);
        assert.equal(call.body.start_request_id, session.getItem(GUIDE_CREATION_KEY));
        value = clone(guides.get(A));
      } else if (url.endsWith('/attachments')) {
        assert.equal(multipart, true, 'Only this exact attachment route accepts multipart content.');
        assert.equal(url, `/api/guide/${A}/attachments`);
        const files = call.body.getAll('files');
        assert.equal(files.length, 1);
        value = { attachments: [{ id: 'd'.repeat(64), sha256: 'e'.repeat(64),
          filename: files[0].name, size: files[0].size, media_type: files[0].type,
          url: `/api/guide/${A}/attachments/${'d'.repeat(64)}` }] };
      } else {
        const id = [...guides.keys()].find(id => url === respondUrl(id)), current = guides.get(id);
        value = { ...clone(current), version: current.version + 1,
          messages: [{ id: 'saved-user-message', role: 'user', text: call.body.text }],
          operation: { status: 'completed', version: current.version, request_id: 'a'.repeat(64),
            request: { ...clone(call.body), stage: call.body.stage ?? null, blueprint: call.body.blueprint ?? null } } };
        guides.set(id, clone(value));
      }
    } else {
      assert.equal(method, 'GET');
      if (getFailures.has(url)) value = getFailures.get(url);
      else if (url.startsWith('/api/guide/creation/')) {
        assert.equal(url, `/api/guide/creation/${session.getItem(GUIDE_CREATION_KEY)}`);
        value = clone(creationResult);
      }
      else if (url === '/api/guide/new') value = clone(initial);
      else if (url === '/api/guide/list') value = { setups: [...guides.keys()].map(id => ({ id,
        name: id === A ? 'Saved setup A' : 'Saved setup B', status: 'question' })) };
      else if ([...guides.keys()].some(id => url === `/api/guide/${id}`))
        value = clone(guides.get([...guides.keys()].find(id => url === `/api/guide/${id}`)));
      else if ([...guides.keys()].some(id => url.startsWith(`/api/guide/${id}/timeline`)))
        value = { frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true };
      else if (url === '/api/architect/status') value = { available: true, status: 'ready' };
      else if (url === '/api/provider-settings') value = { runtime_active: true, work_active: false };
      else value = { httpError: 404, detail: 'No existing inert fixture activity.' };
    }
    return { ok: !value?.httpError, status: value?.httpError || 200,
      json: async () => {
        if (holdInitial && url === heldUrl && method === 'POST' && !held.entered) {
          held.entered = true; heldValue = clone(value);
          return held.promise;
        }
        if (method === 'POST' && laterBodies.length) {
          const body = laterBodies.find(item => !item.entered);
          if (body) { body.entered = true; body.value = clone(value); return body.promise; }
        }
        const reading = method === 'GET' && getBodies.find(item => !item.entered && item.url === url);
        if (reading) { reading.entered = true; reading.value = clone(value); return reading.promise; }
        return value;
      } };
  };
  return { initial, guides, local, session, calls, held, fetch, posts, getFailures,
    setCreation: value => { creationResult = value; },
    holdNextGet: url => { const body = deferred(); body.url = url; getBodies.push(body); return body; },
    holdNextPost: () => { const body = deferred(); laterBodies.push(body); return body; },
    settleOriginal: outcome => outcome === 'error'
      ? held.reject(new Error('Late original body failure.')) : held.resolve(clone(heldValue)),
    finish: () => {
      if (held.entered) held.resolve(clone(heldValue));
      for (const body of laterBodies) if (body.entered) body.resolve(clone(body.value));
      for (const body of getBodies) if (body.entered) body.resolve(clone(body.value));
    } };
}
async function mounted(t, state, verify) {
  t.mock.timers.enable({ apis: ['Date', 'setInterval', 'setTimeout'], now: Date.parse('2026-10-10T00:00:00Z') });
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  Object.defineProperties(dom.window, { localStorage: { configurable: true, value: state.local },
    sessionStorage: { configurable: true, value: state.session } });
  const replacements = { window: dom.window, document: dom.window.document,
    navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true, crypto: webcrypto, FormData: dom.window.FormData,
    localStorage: state.local, sessionStorage: state.session, fetch: state.fetch,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) };
  const before = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root'));
  let rootMounted = true;
  const document = dom.window.document;
  const click = async node => { assert.ok(node, 'The actual visible control exists.'); await act(async () => { node.click(); await settle(); }); };
  const write = async value => {
    const node = document.querySelector('#setup-message');
    assert.ok(node);
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(node, value);
      node.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
      await settle();
    });
  };
  const choose = async () => {
    const picker = document.querySelector('#setup-attachments-picker');
    assert.ok(picker);
    const file = new dom.window.File(['Exact inert evidence.\n'], 'observing-notes.txt', { type: 'text/plain' });
    Object.defineProperty(picker, 'files', { configurable: true, value: [file] });
    await act(async () => { picker.dispatchEvent(new dom.window.Event('change', { bubbles: true })); await settle(); });
    return file;
  };
  const props = { bp: { project: { name: 'Internal fixture only', slug: 'internal-fixture' }, agent_classes: [], topology: { edges: [] },
    flows: [], connections: [], knowledge: { mode: 'none' } },
    Icon: () => null, onBlueprint: () => {}, config: { architect: { available: true } } };
  const render = async active => {
    await act(async () => { root.render(React.createElement(GuidedSetup, { ...props, active })); await settle(); });
  };
  const unmount = async () => { await act(async () => { root.unmount(); await settle(); }); rootMounted = false; };
  try {
    await render(true);
    await act(async () => settle());
    await verify({ document, write, click, choose, render, unmount,
      invoke: async (callback, node) => { await act(async () => { callback({ currentTarget: node }); await settle(); }); },
      tick: async milliseconds => {
      await act(async () => { t.mock.timers.tick(milliseconds); await settle(); });
    } });
  } finally {
    await act(async () => { if (rootMounted) root.unmount(); state.finish(); await settle(); });
    dom.window.close();
    for (const [key, descriptor] of before) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}
function named(document, label) {
  const result = [...document.querySelectorAll('button')].find(node => node.textContent.trim() === label ||
    node.querySelector('span')?.textContent === label);
  assert.ok(result, `The actual control is available: ${label}`);
  return result;
}
function clickHandler(node) {
  return node[Object.keys(node).find(key => key.startsWith('__reactProps$'))].onClick;
}

test('guide admission recovery: direct new draft keeps current anonymous writing and ordered choices without a POST', async t => {
  const state = fixture('start');
  state.initial.question = { ...state.initial.question, multiple: true, options: [
    { id: 'first', label: 'One household', description: 'A shared home context.' },
    { id: 'second', label: 'Our neighbours', description: 'A neighbourhood context.' },
  ] };
  state.local.setItem(inputKey(state.initial), JSON.stringify({ key: inputKey(state.initial),
    draft: original, selected: ['second', 'first'], mode: 'answer' }));
  await mounted(t, state, async controls => {
    const { document, write, click } = controls;
    await sendAndExpire(state, controls);
    await write(edited);
    assert.match(delivery(document).textContent, /The original request may still finish/);
    assert.ok(named(document, 'Check saved conversation'));
    await click(named(document, 'Open a new draft'));
    assert.equal(state.posts().length, 1, 'Opening writing only reads the anonymous draft; it never resends.');
    assert.equal(state.local.getItem(GUIDE_KEY), null);
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), null,
      'The old creation token is retired, not promised as still checkable here.');
    assert.equal(delivery(document) === null, true);
    assert.equal(document.querySelector('#setup-message').value, edited);
    const retained = JSON.parse(state.local.getItem(inputKey(state.initial)));
    assert.equal(retained.draft, edited);
    assert.deepEqual(retained.selected, ['second', 'first']);
    assert.equal(document.querySelector('#setup-send').disabled, false);
    assert.equal(state.calls.filter(call => call.url === '/api/guide/new').length, 2);
    assert.equal(state.calls.some(call => call.url.startsWith('/api/guide/creation/')), false);
  });
});

for (const outcome of ['success', 'error'])
  test(`guide admission recovery: direct new draft fences late original ${outcome} from a newer anonymous Send`, async t => {
    const state = fixture('start');
    await mounted(t, state, async controls => {
      const { document, write, click } = controls;
      await sendAndExpire(state, controls);
      const originalToken = state.session.getItem(GUIDE_CREATION_KEY);
      await write(edited);
      await click(named(document, 'Open a new draft'));
      assert.equal(state.posts().length, 1);
      const newer = state.holdNextPost();
      await click(document.querySelector('#setup-send'));
      assert.equal(newer.entered, true);
      const newToken = state.session.getItem(GUIDE_CREATION_KEY);
      assert.notEqual(newToken, originalToken);
      await act(async () => { state.settleOriginal(outcome); await settle(); });
      assert.equal(state.local.getItem(GUIDE_KEY), null);
      assert.equal(state.session.getItem(GUIDE_CREATION_KEY), newToken);
      assert.equal(document.querySelector('#setup-message').value, edited);
      assert.equal(document.querySelector('#setup-send').disabled, true,
        'Late cleanup cannot release the newer original anonymous send owner.');
      assert.equal(state.posts().length, 2, 'Only the deliberate newer Send created its own request.');
      assert.doesNotMatch(document.body.textContent, /Late original body failure/);
    });
  });

test('guide admission recovery: checking disables new draft and retires captured handlers after the read settles', async t => {
  const state = fixture('start');
  await mounted(t, state, async controls => {
    const { document, click, invoke } = controls;
    await sendAndExpire(state, controls);
    const token = state.session.getItem(GUIDE_CREATION_KEY);
    const button = named(document, 'Open a new draft'), oldHandler = clickHandler(button);
    const reading = state.holdNextGet(`/api/guide/creation/${token}`);
    await click(named(document, 'Check saved conversation'));
    assert.equal(reading.entered, true);
    assert.equal(named(document, 'Open a new draft').disabled, true);
    const before = state.calls.length;
    await invoke(oldHandler, button);
    assert.equal(state.calls.length, before);
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), token);
    await act(async () => { reading.resolve(reading.value); await settle(); });
    assert.equal(named(document, 'Open a new draft').disabled, false);
    const afterRead = state.calls.length;
    await invoke(oldHandler, button);
    assert.equal(state.calls.length, afterRead, 'A retired callback cannot navigate or start another request after the check settles.');
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), token);
    await click(named(document, 'Open a new draft'));
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), null);
    assert.equal(state.posts().length, 1);
  });
});

test('guide admission recovery: new-draft navigation admits only one read and detaches its old callback', async t => {
  const state = fixture('start');
  await mounted(t, state, async controls => {
    const { document, write, click, invoke } = controls;
    await sendAndExpire(state, controls);
    await write(edited);
    const button = named(document, 'Open a new draft'), oldHandler = clickHandler(button);
    const reading = state.holdNextGet('/api/guide/new');
    await click(button);
    assert.equal(reading.entered, true);
    assert.equal(button.isConnected, false);
    const before = state.calls.length;
    await invoke(oldHandler, button);
    assert.equal(state.calls.length, before, 'Pending new-draft navigation cannot start a second read.');
    await act(async () => { reading.resolve(reading.value); await settle(); });
    const settled = state.calls.length;
    await invoke(oldHandler, button);
    assert.equal(state.calls.length, settled, 'A detached old recovery card cannot reopen a later draft.');
    assert.equal(document.querySelector('#setup-message').value, edited);
    assert.equal(state.posts().length, 1);
  });
});

for (const boundary of ['guide', 'inactive', 'unmounted', 'creation token'])
  test(`guide admission recovery: captured new-draft callback refuses changed ${boundary}`, async t => {
    const state = fixture('start');
    await mounted(t, state, async controls => {
      const { document, write, click, invoke, render, unmount } = controls;
      await sendAndExpire(state, controls);
      await write(edited);
      const button = named(document, 'Open a new draft'), oldHandler = clickHandler(button);
      if (boundary === 'guide') {
        await click(named(document, 'Your setups'));
        await click(named(document, 'Saved setup B'));
        await write('  B keeps its own writing.  ');
      } else if (boundary === 'inactive') {
        await render(false);
        assert.equal(button.isConnected, false, 'Inactive GuidedSetup removes its old controls.');
      } else if (boundary === 'unmounted') await unmount();
      else state.session.setItem(GUIDE_CREATION_KEY, 'cccccccc-cccc-4ccc-8ccc-cccccccccccc');
      const before = state.calls.length, creation = state.session.getItem(GUIDE_CREATION_KEY);
      const draft = state.local.getItem(inputKey(state.initial));
      await invoke(oldHandler, button);
      assert.equal(state.calls.length, before);
      assert.equal(state.session.getItem(GUIDE_CREATION_KEY), creation);
      assert.equal(state.local.getItem(inputKey(state.initial)), draft);
      assert.equal(state.posts().length, 1);
      if (boundary === 'guide') {
        assert.equal(state.local.getItem(GUIDE_KEY), B);
        assert.equal(document.querySelector('#setup-message').value, '  B keeps its own writing.  ');
      }
    });
  });

test('guide admission recovery: existing-guide delivery uncertainty keeps only its original check action', async t => {
  const state = fixture('respond');
  await mounted(t, state, async controls => {
    await sendAndExpire(state, controls);
    assert.ok(named(controls.document, 'Check saved conversation'));
    assert.equal([...delivery(controls.document).querySelectorAll('button')]
      .some(button => button.textContent === 'Open a new draft'), false);
    assert.equal(state.posts().length, 1);
  });
});

for (const stage of ['start', 'respond']) test(`guide admission recovery: held ${stage} JSON cannot silently block Send after explicit saved-state navigation`, async t => {
  const state = fixture(stage);
  await mounted(t, state, async ({ document, write, click, tick }) => {
    await write(original);
    await click(document.querySelector('#setup-send'));
    assert.equal(state.held.entered, true, 'The original POST headers returned but its JSON body remains pending.');
    assert.equal(state.posts().length, 1);
    if (stage === 'respond') assert.equal(state.posts()[0].body.text, original);
    await write(edited);
    assert.equal(JSON.parse(state.local.getItem(inputKey(state.initial))).draft, edited,
      'Exact writing typed while admission waits remains in the canonical draft envelope.');
    await tick(60000);
    assert.equal(state.posts().length, 1, 'Passing time must not automatically replay the entered mutation.');
    assert.equal(state.posts()[0].signal?.aborted, true,
      'The explicit admission deadline bounds even a held JSON body without replaying its mutation.');
    await click(named(document, 'Your setups'));
    const selected = stage === 'start' ? B : A;
    await click(named(document, selected === A ? 'Saved setup A' : 'Saved setup B'));
    assert.ok(state.calls.some(call => call.method === 'GET' && call.url === `/api/guide/${selected}`),
      'Explicit navigation reads actual saved guide state without replaying the old POST.');
    assert.equal(state.posts().length, 1);
    if (stage === 'respond') {
      assert.equal(document.querySelector('#setup-message').value, edited,
        'Readback of the completed original preserves later custom writing.');
      assert.deepEqual(state.guides.get(A).operation.request, { ...state.posts()[0].body, stage: null, blueprint: null },
        'The same-guide readback carries the exact submitted operation, not an unrelated completion.');
    } else {
      assert.equal(JSON.parse(state.local.getItem(inputKey(state.initial))).draft, edited,
        'Navigating to a different setup keeps the original anonymous draft.');
      await write('  A separate task for saved setup B.\nStill an explicit Send.  ');
    }
    const send = document.querySelector('#setup-send');
    assert.equal(send.disabled, false, 'The fresh idle saved state presents Send as available.');
    await click(send);
    assert.equal(state.posts().length, 2,
      'A genuinely enabled explicit Send must admit the current task rather than silently returning on the old processing latch.');
    assert.equal(state.posts()[1].url, `/api/guide/${selected}/respond`);
  });
});

const delivery = document => document.querySelector('[aria-label="Message delivery status"]');
async function sendAndExpire(state, controls, text = original) {
  await controls.write(text);
  await controls.click(controls.document.querySelector('#setup-send'));
  assert.equal(state.held.entered, true);
  await controls.tick(30001);
  assert.equal(state.posts().length, 1, 'A deadline never automatically resends the submitted task.');
  assert.equal(state.posts()[0].signal?.aborted, true, 'The deadline includes response.json.');
}

test('guide admission recovery: exact completed readback preserves later writing and releases only the original task', async t => {
  const state = fixture('respond');
  await mounted(t, state, async controls => {
    const { document, write, click, tick } = controls;
    await sendAndExpire(state, controls);
    assert.match(delivery(document)?.textContent || '', /Message delivery is not confirmed/);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    await write(edited);
    const beforeReads = state.calls.length;
    await click(named(document, 'Check saved conversation'));
    assert.ok(state.calls.slice(beforeReads).some(call => call.method === 'GET' && call.url === `/api/guide/${A}`));
    assert.equal(state.posts().length, 1, 'Checking saved conversation does not send the task again.');
    assert.equal(document.querySelector('#setup-message').value, edited);
    assert.equal(JSON.parse(state.local.getItem(inputKey(state.guides.get(A)))).draft, edited);
    assert.equal(delivery(document) === null, true);
    assert.equal(document.querySelector('#setup-send').disabled, false);
    const second = state.holdNextPost();
    await click(document.querySelector('#setup-send'));
    assert.equal(second.entered, true);
    assert.equal(state.posts().length, 2);
    await act(async () => { state.settleOriginal('success'); await settle(); });
    assert.equal(document.querySelector('#setup-message').value, edited,
      'A late old body cannot consume writing owned by the newer Send.');
    assert.equal(document.querySelector('#setup-send').disabled, true);
    await tick(1000);
    assert.equal(state.posts().length, 2);
  });
});

test('guide admission recovery: exact running readback resumes existing status polling without another POST', async t => {
  const state = fixture('respond');
  await mounted(t, state, async controls => {
    const { document, click, tick, write } = controls;
    await sendAndExpire(state, controls);
    const saved = state.guides.get(A);
    state.guides.set(A, { ...saved, version: state.initial.version,
      operation: { ...saved.operation, status: 'running' } });
    await click(named(document, 'Check saved conversation'));
    assert.equal(delivery(document) === null, true, 'Exact running admission is confirmed, not left uncertain.');
    assert.equal(document.querySelector('#setup-message').value, original,
      'Original writing remains until saved completion; running admission resumes status without resending.');
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.match(document.querySelector('.setup-guide-wait')?.textContent || '', /Guide request running/);
    await write('  My next question, while the current task runs.  ');
    state.guides.set(A, saved);
    await tick(2500);
    assert.equal(state.posts().length, 1);
    assert.equal(document.querySelector('#setup-message').value, '  My next question, while the current task runs.  ');
    assert.equal(document.querySelector('#setup-send').disabled, false);
  });
});

test('guide admission recovery: unrelated same-version or failed readback keeps delivery uncertain and preserves the task', async t => {
  const state = fixture('respond');
  await mounted(t, state, async controls => {
    const { document, click } = controls;
    await sendAndExpire(state, controls);
    const saved = state.guides.get(A);
    state.guides.set(A, { ...saved, version: state.initial.version,
      operation: { ...saved.operation, request: { ...saved.operation.request, text: 'A different task.' } } });
    await click(named(document, 'Check saved conversation'));
    assert.ok(delivery(document));
    assert.equal(document.querySelector('#setup-message').value, original);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    state.getFailures.set(`/api/guide/${A}`, { httpError: 503, detail: 'Saved state temporarily unreadable.' });
    await click(named(document, 'Check saved conversation'));
    assert.ok(delivery(document));
    assert.equal(document.querySelector('#setup-message').value, original);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.equal(state.posts().length, 1, 'Neither failed nor unrelated evidence allows automatic replay.');
    state.getFailures.delete(`/api/guide/${A}`);
    state.guides.set(A, saved);
    await click(named(document, 'Check saved conversation'));
    assert.equal(delivery(document) === null, true);
    assert.equal(document.querySelector('#setup-message').value, '', 'Only an exact later check can consume the original task.');
    assert.equal(state.posts().length, 1);
  });
});

test('guide admission recovery: a later saved version excludes old CAS without claiming or consuming its task', async t => {
  const state = fixture('respond');
  await mounted(t, state, async controls => {
    const { document, click } = controls;
    await sendAndExpire(state, controls);
    const saved = state.guides.get(A);
    state.guides.set(A, { ...saved, operation: null,
      messages: [{ id: 'other-message', role: 'user', text: 'Independent saved progress.' }] });
    await click(named(document, 'Check saved conversation'));
    assert.equal(delivery(document) === null, true);
    assert.equal(document.querySelector('#setup-message').value, original,
      'Version exclusion is not proof that this submitted writing was admitted.');
    assert.equal(document.querySelector('#setup-send').disabled, false);
    assert.equal(state.posts().length, 1);
  });
});

test('guide admission recovery: complete request matching preserves ordered choices and attachments until exact native admission', async t => {
  const state = fixture('respond');
  state.initial.question = { ...state.initial.question, multiple: true, options: [
    { id: 'first', label: 'One household', description: 'A shared home context.' },
    { id: 'second', label: 'Our neighbours', description: 'A neighbourhood context.' },
  ] };
  state.guides.get(A).question = clone(state.initial.question);
  const refs = ['b', 'c'].map((digit, index) => ({ id: digit.repeat(64), sha256: 'f'.repeat(64),
    filename: `context-${index}.txt`, media_type: 'text/plain', size: 24,
    url: `/api/guide/${A}/attachments/${digit.repeat(64)}` }));
  state.session.setItem(`mobkit-guide-attachments-v1:${A}`,
    JSON.stringify({ guide_id: A, attachments: refs }));
  await mounted(t, state, async controls => {
    const { document, click } = controls;
    const option = label => [...document.querySelectorAll('.setup-choice-grid button')]
      .find(node => node.querySelector('strong')?.textContent === label);
    await click(option('Our neighbours'));
    await click(option('One household'));
    await sendAndExpire(state, controls);
    const exact = state.guides.get(A), request = exact.operation.request;
    assert.deepEqual(request.selected, ['second', 'first']);
    assert.deepEqual(request.attachments, refs);
    for (const [label, mutate] of [
      ['choice order', operation => { operation.request.selected.reverse(); }],
      ['attachment order', operation => { operation.request.attachments.reverse(); }],
      ['request version', operation => { operation.version += 1; }],
      ['host request ID', operation => { operation.request_id = 'not-a-host-operation-id'; }],
      ['default type', operation => { operation.request.stage = false; }],
      ['extra request field', operation => { operation.request.extra = 'unrecognized'; }],
    ]) {
      const candidate = { ...clone(exact), version: state.initial.version,
        operation: { ...clone(exact.operation), status: 'running' } };
      mutate(candidate.operation);
      state.guides.set(A, candidate);
      await click(named(document, 'Check saved conversation'));
      assert.ok(delivery(document), `${label} is not proof that the original submitted request was admitted.`);
      assert.equal(document.querySelector('#setup-message').value, original);
      assert.deepEqual(JSON.parse(state.local.getItem(inputKey(state.initial))).selected, ['second', 'first']);
      assert.deepEqual(JSON.parse(state.session.getItem(`mobkit-guide-attachments-v1:${A}`)).attachments, refs);
      assert.equal(state.posts().length, 1);
    }
    state.guides.set(A, exact);
    await click(named(document, 'Check saved conversation'));
    assert.equal(delivery(document) === null, true);
    assert.equal(document.querySelector('#setup-message').value, '');
    assert.deepEqual(JSON.parse(state.local.getItem(inputKey(state.initial))).selected, []);
    assert.equal(state.session.getItem(`mobkit-guide-attachments-v1:${A}`), null,
      'Only the exact normalized admission consumes the submitted checked attachment references.');
    assert.equal(state.posts().length, 1);
  });
});

test('guide admission recovery: anonymous creation 404 remains uncertain and exact GET mapping restores latest draft without auto-send', async t => {
  const state = fixture('start');
  await mounted(t, state, async controls => {
    const { document, write, click } = controls;
    await sendAndExpire(state, controls);
    const creation = state.session.getItem(GUIDE_CREATION_KEY);
    assert.match(creation, /^[0-9a-f-]{36}$/);
    assert.match(document.body.textContent, /Conversation creation is not confirmed/);
    await write(edited);
    await click(named(document, 'Check saved conversation'));
    assert.ok(state.calls.some(call => call.method === 'GET' && call.url === `/api/guide/creation/${creation}`));
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), creation);
    assert.equal(document.querySelector('#setup-message').value, edited);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.equal(state.posts().length, 1);
    state.setCreation(clone(state.guides.get(A)));
    await click(named(document, 'Check saved conversation'));
    assert.equal(state.local.getItem(GUIDE_KEY), A);
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), null);
    assert.equal(document.querySelector('#setup-message').value, edited);
    assert.equal(JSON.parse(state.local.getItem(inputKey(state.guides.get(A)))).draft, edited);
    assert.equal(document.querySelector('#setup-send').disabled, false);
    assert.equal(state.posts().length, 1, 'Recovering creation must not implicitly send the first answer.');
    const second = state.holdNextPost();
    await click(document.querySelector('#setup-send'));
    assert.equal(second.entered, true);
    assert.equal(state.posts().length, 2);
    assert.equal(state.posts()[1].body.text, edited);
    await act(async () => { state.settleOriginal('success'); await settle(); });
    assert.equal(document.querySelector('#setup-send').disabled, true,
      'The delayed original creation body cannot release the newer message admission.');
    assert.equal(state.local.getItem(GUIDE_KEY), A);
  });
});

for (const stage of ['start', 'respond']) for (const outcome of ['success', 'error'])
  test(`guide admission recovery: late ${stage} ${outcome} after navigation cannot release a newer held Send`, async t => {
    const state = fixture(stage);
    await mounted(t, state, async ({ document, write, click, tick }) => {
      await write(original);
      await click(document.querySelector('#setup-send'));
      await write(edited);
      await tick(1000);
      await click(named(document, 'Your setups'));
      await click(named(document, 'Saved setup B'));
      const newText = '  B owns this new task and its pending wait.  ';
      await write(newText);
      const newer = state.holdNextPost();
      await click(document.querySelector('#setup-send'));
      assert.equal(newer.entered, true);
      assert.equal(state.posts().length, 2);
      await act(async () => { state.settleOriginal(outcome); await settle(); });
      assert.equal(document.querySelector('#setup-message').value, newText);
      assert.equal(document.querySelector('#setup-send').disabled, true,
        'Old finally cleanup must not release another exact admission owner.');
      assert.equal(state.local.getItem(GUIDE_KEY), B);
      assert.equal(JSON.parse(state.local.getItem(inputKey(state.initial))).draft, edited);
      assert.doesNotMatch(document.body.textContent, /Late original body failure/);
      await tick(1000);
      assert.equal(state.posts().length, 2);
    });
  });

test('guide admission recovery: explicit return to a new setup keeps anonymous writing and retires only the old creation owner', async t => {
  const state = fixture('start');
  await mounted(t, state, async ({ document, write, click, tick }) => {
    await write(original);
    await click(document.querySelector('#setup-send'));
    const originalCreation = state.session.getItem(GUIDE_CREATION_KEY);
    await write(edited);
    await tick(1000);
    await click(named(document, 'Your setups'));
    await click(named(document, 'Saved setup B'));
    const textB = '  B has its own separate unsent task.  ';
    await write(textB);
    await click(named(document, 'Your setups'));
    await click(named(document, '+ Start a new setup'));
    assert.equal(document.querySelector('#setup-message').value, edited,
      'The existing anonymous draft returns without borrowing B\'s writing.');
    assert.equal(JSON.parse(state.local.getItem(inputKey(state.guides.get(B)))).draft, textB);
    assert.equal(state.local.getItem(GUIDE_KEY), null);
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), null,
      'Explicit New setup retires the earlier uncertain creation token.');
    assert.equal(state.posts().length, 1, 'Returning to writing does not send it.');
    const newer = state.holdNextPost();
    await click(document.querySelector('#setup-send'));
    assert.equal(newer.entered, true);
    assert.equal(state.posts().length, 2);
    assert.equal(state.posts()[1].url, '/api/guide/start');
    const newCreation = state.session.getItem(GUIDE_CREATION_KEY);
    assert.notEqual(newCreation, originalCreation, 'A deliberate new setup owns its own creation request.');
    await act(async () => { state.settleOriginal('success'); await settle(); });
    assert.equal(document.querySelector('#setup-message').value, edited);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.equal(state.local.getItem(GUIDE_KEY), null);
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), newCreation);
    assert.equal(state.posts().length, 2, 'A late old creation body cannot auto-send the original answer.');
  });
});

test('guide admission recovery: first-visit file selection creates once and uploads the exact file without sending the draft', async t => {
  const state = fixture('start', { holdInitial: false });
  await mounted(t, state, async ({ document, write, choose }) => {
    await write(original);
    const file = await choose();
    assert.equal(state.posts().length, 2);
    assert.equal(state.posts()[0].url, '/api/guide/start');
    assert.match(state.posts()[0].body.start_request_id, /^[0-9a-f-]{36}$/);
    assert.equal(state.posts()[1].url, `/api/guide/${A}/attachments`);
    const uploaded = state.posts()[1].body.getAll('files')[0];
    assert.equal(uploaded.name, file.name);
    assert.equal(uploaded.size, file.size);
    assert.equal(uploaded.type, file.type);
    assert.equal(document.querySelector('.setup-attachments a')?.textContent, file.name);
    assert.equal(state.calls.some(call => call.url.endsWith('/respond')), false,
      'Choosing a file never sends the written first answer.');
    assert.equal(document.querySelector('#setup-message').value, original);
    assert.equal(JSON.parse(state.local.getItem(inputKey(state.guides.get(A)))).draft, original);
    assert.equal(state.session.getItem(GUIDE_CREATION_KEY), null);
    assert.equal(delivery(document) === null, true);
  });
});

test('guide admission recovery: first-visit file creation timeout checks existing mapping without uploading or resending', async t => {
  const state = fixture('start');
  await mounted(t, state, async ({ document, write, click, choose, tick }) => {
    await write(original);
    await choose();
    assert.equal(state.held.entered, true);
    assert.equal(state.posts().length, 1);
    await write(edited);
    await tick(30001);
    assert.equal(state.posts()[0].signal?.aborted, true);
    assert.match(delivery(document)?.textContent || '', /Conversation creation is not confirmed/);
    assert.equal(document.querySelector('#setup-message').value, edited);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    state.setCreation(clone(state.guides.get(A)));
    await click(named(document, 'Check saved conversation'));
    assert.equal(state.local.getItem(GUIDE_KEY), A);
    assert.equal(document.querySelector('#setup-message').value, edited);
    assert.equal(delivery(document) === null, true);
    assert.equal(state.posts().length, 1,
      'Creation recovery does not resume the old file upload or send the first answer.');
    await act(async () => { state.settleOriginal('success'); await settle(); });
    assert.equal(state.posts().length, 1, 'A late creation body cannot start the abandoned upload.');
    assert.equal(document.querySelector('.setup-attachments a') === null, true);
    assert.equal(document.querySelector('#setup-message').value, edited);
    await choose();
    assert.equal(state.posts().length, 2, 'Only a fresh deliberate file selection uploads to the recovered saved guide.');
    assert.equal(state.posts()[1].url, `/api/guide/${A}/attachments`);
    assert.equal(state.calls.some(call => call.url.endsWith('/respond')), false);
  });
});
