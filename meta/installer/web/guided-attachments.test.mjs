import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act } from 'react';
import { createGuideAttachmentStore, checkedGuideAttachment, guideAttachmentAccept, guideAttachmentSelectionError } from './guided-attachments.mjs';
import { GUIDE_KEY } from './guided-state.mjs';
import { inputKey } from './wizard-state.mjs';

const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const require = createRequire(import.meta.url);
const bundled = await build({ entryPoints: [fileURLToPath(new URL('guided.jsx', import.meta.url))], bundle: true, write: false,
  format: 'esm', platform: 'node', logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'shared-react', setup(builder) { builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
    path: pathToFileURL(require.resolve(args.path)).href, external: true })); } }] });
const { GuidedSetup } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
const guide = { id: 'synthetic-file-guide', version: 3, status: 'question', stage: 'knowledge', completed_stages: [], messages: [],
  brief: { facts: {}, requirements: [] }, capabilities: { available: true },
  question: { id: 'evidence', stage: 'knowledge', title: 'Which evidence?', description: 'Keep its source.', multiple: true,
    allow_text: true, options: [{ id: 'current', label: 'Current evidence', description: 'Use the current context.' }] } };
const ref = (name = 'notes.md', index = 1, id = guide.id) => ({ id: String(index).repeat(64), filename: name,
  media_type: name.endsWith('.png') ? 'image/png' : 'text/markdown', size: 12, sha256: 'a'.repeat(64),
  url: `/api/guide/${id}/attachments/${String(index).repeat(64)}` });
const storage = values => ({ getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
  removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } });
const pause = () => new Promise(resolve => setTimeout(resolve, 5));

test('uploaded references recover only for their exact guide and reject forged scoped URLs or metadata', () => {
  const values = new Map(), store = createGuideAttachmentStore(() => storage(values));
  store.add(guide.id, [ref()]);
  assert.deepEqual(createGuideAttachmentStore(() => storage(values)).read(guide.id), [ref()]);
  assert.deepEqual(store.read('another-guide'), []);
  assert.equal(checkedGuideAttachment(guide.id, { ...ref(), url: 'https://outside.invalid/file' }), null);
  assert.equal(checkedGuideAttachment(guide.id, { ...ref(), private_path: '/private/not-allowed' }), null);
  assert.equal(checkedGuideAttachment('another-guide', ref()), null);
  assert.equal([...values.values()].join('').includes('blob:'), false);
  store.add(guide.id, [ref('new.md', 2)]);
  store.admitted(guide.id, [ref()]);
  assert.deepEqual(store.read(guide.id), [ref('new.md', 2)], 'admission consumes only its submitted files');
});

test('file selection enforces count, per-file and combined limits and does not offer unsupported PDF', () => {
  const file = { name: 'notes.md', type: '', size: 8 * 1024 * 1024 };
  assert.equal(guideAttachmentSelectionError([file, file]), '');
  assert.match(guideAttachmentSelectionError([file, file, { ...file, size: 1 }]), /16 MiB/);
  assert.match(guideAttachmentSelectionError([{ ...file, size: file.size + 1 }]), /8 MiB/);
  assert.match(guideAttachmentSelectionError(Array(5).fill({ ...file, size: 1 })), /four/);
  const pdf = { name: 'report.pdf', type: 'application/pdf', size: 12 };
  assert.match(guideAttachmentSelectionError([pdf]), /PDF upload is unavailable/);
  assert.equal(guideAttachmentSelectionError([pdf], [], { media_types: ['application/pdf'] }), '');
  assert.doesNotMatch(guideAttachmentAccept({ media_types: [] }), /pdf/);
  assert.match(guideAttachmentAccept({ media_types: ['application/pdf'] }), /\.pdf/);
  assert.match(guideAttachmentSelectionError([{ ...file, size: 0 }]), /Empty/);
});

test('denied session storage retains uploaded files in page without promising reload recovery', () => {
  const store = createGuideAttachmentStore(() => { throw new Error('denied'); });
  store.add(guide.id, [ref()]);
  assert.deepEqual(store.read(guide.id), [ref()]);
  assert.equal(store.persistent(), false);
});

async function mount({ session = new Map(), state = structuredClone(guide), onPost, onRead, local: retainedLocal, draft = '  Keep my unsent context. Å  ', selected = ['current'], mode = 'chat', otherGuides = [], savedToolkit, narrow = false, setupDom } = {}, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const input = { key: inputKey(state), draft, selected, mode };
  const local = retainedLocal || new Map([[GUIDE_KEY, state.id], [input.key, JSON.stringify(input)]]);
  const calls = [];
  const mediaListeners = new Set();
  const media = { matches: narrow, addEventListener: (_, listener) => mediaListeners.add(listener),
    removeEventListener: (_, listener) => mediaListeners.delete(listener) };
  let server = state;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, FormData: dom.window.FormData,
    localStorage: storage(local), sessionStorage: storage(session), IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window), cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => media,
    fetch: async (url, options = {}) => {
      const method = options.method || 'GET';
      const body = options.body instanceof dom.window.FormData ? options.body : options.body && JSON.parse(options.body);
      calls.push({ url, method, body, signal: options.signal });
      let value;
      if (method === 'POST') {
        assert.ok(['/api/guide/start', ...[state.id, 'created-file-guide', ...otherGuides.map(item => item.id)].flatMap(id =>
          [`/api/guide/${id}/attachments`, `/api/guide/${id}/respond`])].includes(url));
        value = await onPost?.({ url, body, calls });
        if (!value && url.endsWith('/attachments')) value = { attachments: body.getAll('files').map((file, index) => ref(file.name, index + 1, state.id)) };
        if (!value) value = server = { ...structuredClone(server), version: server.version + 1,
          operation: { status: 'completed', request_id: 'b'.repeat(64), version: server.version,
            request: structuredClone(body) } };
      } else value = await onRead?.({ url, calls }) || (url === `/api/guide/${state.id}/toolkit-saved` && savedToolkit ? savedToolkit :
        url === '/api/guide/list' ? { setups: otherGuides.map(item => ({ id: item.id, name: item.name, status: item.status })) } :
        url === `/api/guide/${state.id}` || url === '/api/guide/new' ? server :
        url.includes('/timeline') ? { frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true } :
        otherGuides.find(item => url === `/api/guide/${item.id}`) || null);
      assert.ok(value, `Unexpected offline request ${method} ${url}`);
      if (typeof value.json === 'function') return value;
      return value.httpError ? { ok: false, status: value.httpError, json: async () => ({ detail: value.detail }) }
        : { ok: true, status: 200, json: async () => value };
    } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const objectURL = URL.createObjectURL, revokeURL = URL.revokeObjectURL;
  URL.createObjectURL = () => 'blob:offline-preview'; URL.revokeObjectURL = () => {};
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  setupDom?.(dom);
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root'));
  const choose = async name => {
    const picker = dom.window.document.querySelector('#setup-attachments-picker');
    const file = new dom.window.File(['safe fixture'], name, { type: name.endsWith('.png') ? 'image/png' : 'text/markdown' });
    Object.defineProperty(picker, 'files', { configurable: true, value: [file] });
    await act(async () => { picker.dispatchEvent(new dom.window.Event('change', { bubbles: true })); await pause(); });
  };
  try {
    await act(async () => { root.render(React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null,
      onBlueprint() {}, config: { architect: { available: true } } })); await pause(); });
    await act(pause);
    await verify({ document: dom.window.document, window: dom.window, calls, choose, input, session, local,
      setNarrow: async value => act(async () => { media.matches = value; for (const listener of mediaListeners) listener(); }) });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    URL.createObjectURL = objectURL; URL.revokeObjectURL = revokeURL;
    for (const [key, descriptor] of prior) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; }
  }
}

test('the narrow Menu retains existing controls and closes with Escape without changing the draft or admitting work', async () => {
  const state = { ...structuredClone(guide), capabilities: { available: true, model: 'gpt-6.1-sol' } };
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(state.id, [ref('keep-unsent.md')]);
  await mount({ state, session, narrow: true }, async ({ document, window, calls, input, local }) => {
    const menu = document.querySelector('.setup-mobile-menu-toggle'), actions = document.getElementById(menu.getAttribute('aria-controls'));
    const textarea = document.querySelector('#setup-message'), native = document.querySelector('[data-native-console-chat]');
    const before = local.get(input.key), attachment = document.querySelector('.setup-attachments a');
    assert.equal(menu.getAttribute('aria-expanded'), 'false');
    assert.equal(actions.classList.contains('is-open'), false);
    await act(async () => menu.click());
    assert.equal(menu.getAttribute('aria-expanded'), 'true');
    assert.equal(actions.classList.contains('is-open'), true);
    for (const label of ['Your setups', 'Guide settings', 'Saved tools', 'Configuration'])
      assert.ok([...actions.querySelectorAll('button')].some(button => button.textContent.includes(label)), label);
    assert.equal(actions.querySelector('.setup-provider-model').textContent, 'gpt-6.1-sol');
    const histories = actions.querySelector('.setup-history-control > button');
    await act(async () => { histories.click(); await pause(); });
    assert.equal(histories.getAttribute('aria-expanded'), 'true');
    assert.ok(calls.some(call => call.url === '/api/guide/list'));
    await act(async () => {
      histories.focus();
      histories.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    assert.equal(menu.getAttribute('aria-expanded'), 'false');
    assert.ok(document.activeElement === menu, 'Escape returns focus to the disclosure');
    assert.ok(document.querySelector('#setup-message') === textarea, 'The native textarea stays mounted');
    assert.ok(document.querySelector('[data-native-console-chat]') === native, 'The native transcript stays mounted');
    assert.ok(document.querySelector('.setup-attachments a') === attachment, 'The checked attachment stays mounted');
    assert.equal(textarea.value, input.draft);
    assert.equal(local.get(input.key), before);
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
  });
});

test('actual textarea focus compacts mobile chrome and preserves native mounts, accessible attachment control and growing draft', async () => {
  await mount({ narrow: true, draft: '', selected: [] }, async ({ document, window, calls, input, local, setNarrow }) => {
    const shell = document.querySelector('.setup-shell'), textarea = document.querySelector('#setup-message');
    const native = document.querySelector('[data-native-console-chat]'), header = document.querySelector('.setup-header');
    const attach = document.querySelector('[aria-label="Attach files"]');
    assert.equal(attach.classList.contains('cc-composer__pill-icon'), true);
    assert.equal(attach.getAttribute('title'), 'Attach files');
    assert.equal(attach.getAttribute('aria-controls'), 'setup-attachments-picker');
    assert.equal(shell.classList.contains('setup-composer-focused'), false);
    assert.equal(shell.classList.contains('setup-has-draft'), false);
    await act(async () => textarea.focus());
    assert.ok(document.activeElement === textarea, 'The real native textarea received focus');
    assert.equal(shell.classList.contains('setup-composer-focused'), true);
    await act(async () => attach.focus());
    assert.ok(document.activeElement === attach, 'The attachment control receives focus');
    assert.equal(shell.classList.contains('setup-composer-focused'), false, 'Another composer control does not count as typing');
    const note = '  My new unsent note.\nKeep the exact second line.  ';
    await act(async () => {
      textarea.focus();
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, note);
      textarea.dispatchEvent(new window.Event('input', { bubbles: true }));
    });
    assert.equal(shell.classList.contains('setup-has-draft'), true);
    assert.equal(JSON.parse(local.get(input.key)).draft, note);
    await act(async () => textarea.blur());
    assert.equal(shell.classList.contains('setup-composer-focused'), false);
    assert.equal(shell.classList.contains('setup-has-draft'), true, 'An unfocused draft keeps the larger input');
    await setNarrow(false);
    assert.equal(attach.textContent, 'Attach files', 'Desktop keeps its existing labeled toolbar');
    assert.equal(attach.classList.contains('cc-composer__pill'), true);
    assert.ok(document.querySelector('#setup-message') === textarea, 'Responsive controls preserve the native input');
    assert.ok(document.querySelector('[data-native-console-chat]') === native, 'Responsive controls preserve the native transcript');
    assert.ok(document.querySelector('.setup-header') === header, 'Responsive controls preserve the header');
    assert.equal(textarea.value, note);
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
  });
});

test('mobile New response sits above the conversation and only opening the exact native reply clears it', async () => {
  const state = { ...structuredClone(guide), version: 4,
    messages: [{ id: 'prior-user', role: 'user', text: 'Check the context.', created_at: '2026-10-05T10:00:00Z' },
      { id: 'native-reply', role: 'assistant', text: 'The actual saved native reply.', action: 'chat', created_at: '2026-10-05T10:01:00Z' }],
    operation: { status: 'completed', request_id: 'c'.repeat(64), version: 3, started_at: '2026-10-05T10:00:00Z',
      request: { action: 'chat', text: 'Check the context.', selected: [] } } };
  await mount({ state, narrow: true, setupDom(dom) {
    const proto = dom.window.HTMLElement.prototype;
    Object.defineProperty(proto, 'clientHeight', { configurable: true, get() { return this.classList.contains('setup-chat-scroll') ? 380 : 0; } });
    Object.defineProperty(proto, 'clientWidth', { configurable: true, get() { return this.classList.contains('setup-chat-scroll') ? 390 : 0; } });
    Object.defineProperty(proto, 'scrollHeight', { configurable: true, get() { return 3500; } });
    Object.defineProperty(proto, 'scrollTop', { configurable: true,
      get() { return this.ownedScrollTop ?? (this.classList.contains('setup-chat-scroll') ? 1100 : 0); },
      set(value) { this.ownedScrollTop = value; } });
    proto.getBoundingClientRect = function() {
      const scroll = dom.window.document.querySelector('.setup-chat-scroll'), offset = scroll?.scrollTop || 0;
      if (this.classList.contains('setup-chat-scroll')) return { top: 100, height: 380, bottom: 480 };
      if (this.classList.contains('cc-message--assistant')) return { top: 160 - offset, height: 900, bottom: 1060 - offset };
      if (this.classList.contains('setup-current-question')) return { top: 1200 - offset, height: 440, bottom: 1640 - offset };
      if (this.classList.contains('setup-choice')) return { top: 1330 - offset, height: 100, bottom: 1430 - offset };
      return { top: 0, height: 0, bottom: 0 };
    };
    proto.scrollTo = function(options) { this.scrollTop = options.top; };
  } }, async ({ document, window, calls, input, local }) => {
    await act(async () => new Promise(resolve => setTimeout(resolve, 30)));
    const notice = () => document.querySelector('.setup-new-response');
    const scroll = document.querySelector('.setup-chat-scroll'), before = local.get(input.key);
    assert.ok(scroll.querySelector('.cc-message--assistant'), 'The real native transcript contains the saved reply');
    assert.ok(notice(), 'Automatic decision arrival does not claim the off-screen reply was read');
    assert.equal(notice().parentElement.className, 'setup-mobile-response');
    assert.ok(notice().parentElement.compareDocumentPosition(scroll) & window.Node.DOCUMENT_POSITION_FOLLOWING);
    assert.equal(document.querySelector('.setup-composer-area .setup-new-response') === null, true);
    await act(async () => {
      document.querySelector('#setup-message').focus();
      scroll.dispatchEvent(new window.Event('scroll', { bubbles: true }));
    });
    assert.ok(notice(), 'Typing and seeing the choices leave the actual reply unseen');
    await act(async () => notice().click());
    assert.equal(scroll.querySelector('.cc-message--assistant').getBoundingClientRect().top, 116);
    assert.equal(notice() === null, true, 'Only the opened native reply clears its exact publication notice');
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(local.get(input.key), before);
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
  });
});

test('the real composer uploads multipart files, previews and removes them, then restores only host references after remount', async () => {
  const session = new Map();
  await mount({ session }, async ({ document, calls, choose, input }) => {
    const button = [...document.querySelectorAll('button')].find(node => node.textContent === 'Attach files');
    assert.ok(button && !button.disabled);
    assert.equal(button.getAttribute('aria-controls'), 'setup-attachments-picker');
    assert.doesNotMatch(document.querySelector('#setup-attachments-picker').accept, /pdf/);
    await choose('image.png');
    assert.equal(calls.filter(call => call.url.endsWith('/attachments')).length, 1);
    assert.equal(calls.find(call => call.url.endsWith('/attachments')).body.getAll('files')[0].name, 'image.png');
    assert.equal(document.querySelector('.setup-attachments img').getAttribute('src'), ref('image.png').url);
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length, 1);
  });
  await mount({ session }, async ({ document, calls }) => {
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'image.png');
    assert.equal(calls.filter(call => call.method === 'POST').length, 0, 'reload never uploads or sends');
    await act(async () => document.querySelector('[aria-label="Remove attachment image.png"]').click());
    assert.equal(document.querySelector('.setup-attachments a') === null, true);
  });
  assert.equal(session.size, 0);
});

test('a fresh anonymous setup keeps Saved tools closed and never reads a null guide', async () => {
  await mount({ state: { ...structuredClone(guide), id: null } }, async ({ document, calls, input }) => {
    const button = [...document.querySelectorAll('button')].find(node => node.textContent === 'Saved tools');
    assert.ok(button);
    assert.equal(button.disabled, true);
    assert.equal(button.getAttribute('aria-expanded'), 'false');
    assert.equal(document.querySelector('.setup-saved-tools') === null, true);
    await act(async () => { button.click(); await pause(); });
    assert.equal(button.getAttribute('aria-expanded'), 'false');
    assert.equal(document.querySelector('.setup-saved-tools') === null, true);
    assert.equal(calls.some(call => call.url.includes('/toolkit-saved')), false);
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(calls.some(call => call.method === 'POST'), false);
  });
});

test('discussing an approved saved-tool review preserves the real unsent note, choices and attachment drafts', async () => {
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(guide.id, [ref('keep-unsent.md')]);
  const savedToolkit = { available: true, status: 'review_required', host: { target: 'macos-arm64', families: ['services'] },
    selection: { request_id: '1'.repeat(32), request_sha256: 'a'.repeat(64), version: guide.version },
    review: { status: 'review_required', profile: 'gmail', values: {}, prior_operations: ['old_operation'], unapplied: [] } };
  await mount({ session, savedToolkit }, async ({ document, calls, input }) => {
    await act(async () => { [...document.querySelectorAll('button')].find(node => node.textContent === 'Saved tools').click(); await pause(); });
    assert.equal(calls.filter(call => call.method === 'POST').length, 0);
    await act(async () => { [...document.querySelectorAll('button')].find(node => node.textContent === 'Discuss how this fits my mob').click(); await pause(); });
    const posted = calls.find(call => call.url.endsWith('/respond')).body;
    assert.equal(posted.version, guide.version);
    assert.equal(posted.action, 'chat');
    assert.deepEqual(posted.attachments, []);
    assert.deepEqual(posted.selected, []);
    assert.doesNotMatch(posted.text, /Keep my unsent context/);
    assert.match(posted.text, /distinct expertise or private context/);
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length, 1);
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'keep-unsent.md');
    assert.deepEqual(createGuideAttachmentStore(() => storage(session)).read(guide.id), [ref('keep-unsent.md')]);
  });
});

test('failed upload and failed answer admission preserve the actual note, selection and uploaded references', async () => {
  let failUpload = true;
  await mount({ onPost: ({ url }) => url.endsWith('/attachments') && failUpload
    ? { httpError: 422, detail: 'The image could not be validated.' }
    : url.endsWith('/respond') ? { httpError: 409, detail: 'The guide version changed.' } : null }, async ({ document, choose, input, session }) => {
    await choose('notes.md');
    assert.match(document.querySelector('.setup-attachments [role="alert"]').textContent, /validated/);
    assert.equal(document.querySelector('#setup-send').disabled, true, 'unuploaded evidence is never silently omitted');
    failUpload = false;
    await act(async () => { [...document.querySelectorAll('button')].find(node => node.textContent === 'Retry upload').click(); await pause(); });
    assert.ok(document.querySelector('.setup-attachments a'));
    await act(async () => document.querySelector('#setup-send').click());
    assert.match(document.body.textContent, /guide version changed/);
    assert.ok(document.querySelector('.setup-attachments a'));
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length, 1);
    assert.equal(JSON.parse([...session.values()][0]).attachments[0].id, ref().id);
  });
});

test('Help discusses the actual checked attachments without consuming files, note or choices', async () => {
  const session = new Map();
  await mount({ session }, async ({ document, calls, choose, input }) => {
    await choose('notes.md');
    await act(async () => [...document.querySelectorAll('button')].find(node => node.textContent === 'Help me decide').click());
    const send = calls.find(call => call.url.endsWith('/respond'));
    assert.equal(send.body.action, 'chat'); assert.deepEqual(send.body.selected, []);
    assert.deepEqual(send.body.attachments, [ref()]);
    assert.ok(send.body.text.includes(input.draft));
    assert.ok(document.querySelector('.setup-attachments a'));
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length, 1);
  });
  await mount({ session }, ({ document }) => assert.ok(document.querySelector('.setup-attachments a')));
});

test('ordinary admission consumes only its files, retaining a new attachment selected while the POST is pending', async () => {
  let finish, announced;
  const started = new Promise(resolve => { announced = resolve; });
  await mount({ onPost: ({ url, body }) => {
    if (url.endsWith('/respond')) { announced(); return new Promise(resolve => { finish = () => resolve({ ...guide,
      version: 4, operation: { status: 'running', version: 3, request_id: 'c'.repeat(64), request: body } }); }); }
    if (body.getAll('files')[0].name === 'later.md') return { attachments: [ref('later.md', 2)] };
  } }, async ({ document, calls, choose, session }) => {
    await choose('notes.md');
    await act(async () => { document.querySelector('#setup-send').click(); await started; });
    await choose('later.md');
    await act(async () => { finish(); await pause(); });
    const send = calls.find(call => call.url.endsWith('/respond'));
    assert.deepEqual(send.body.attachments, [ref()]);
    assert.deepEqual([...document.querySelectorAll('.setup-attachments a')].map(node => node.textContent), ['later.md']);
    assert.deepEqual(JSON.parse([...session.values()][0]).attachments, [ref('later.md', 2)]);
  });
});

for (const action of ['answer', 'chat']) test(`lost ${action} response reconciles completed files without consuming later context or historical reuse`, async () => {
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(guide.id, [ref()]);
  let finishLost, announced, completed;
  const started = new Promise(resolve => { announced = resolve; });
  await mount({ session, mode: action, selected: action === 'answer' ? ['current'] : [],
    otherGuides: [{ ...structuredClone(guide), name: 'Recovered files' }], onPost: ({ url, body }) => {
    if (url.endsWith('/respond')) {
      announced();
      return new Promise((_, reject) => { finishLost = () => {
        completed = { ...structuredClone(guide), version: 4,
          question: { ...guide.question, id: 'later-evidence', title: 'What should happen next?' },
          operation: { status: 'completed', request_id: 'c'.repeat(64), version: 3,
            request: { stage: null, blueprint: null, ...structuredClone(body) } } };
        reject(new Error('The response was interrupted after the server accepted it.'));
      }; });
    }
    return { attachments: [body.getAll('files')[0].name === 'later.md' ? ref('later.md', 2) : ref()] };
  }, onRead: ({ url }) => url === `/api/guide/${guide.id}` && completed },
  async ({ document, window, calls, choose, local }) => {
    await act(async () => { document.querySelector('#setup-send').click(); await started; });
    await choose('later.md');
    const laterDraft = '  This is a different unsent follow-up. Å  ';
    await act(async () => {
      const textarea = document.querySelector('#setup-message');
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, laterDraft);
      textarea.dispatchEvent(new window.Event('input', { bubbles: true }));
      finishLost(); await pause();
    });
    const files = () => [...document.querySelectorAll('.setup-attachments a')].map(node => node.textContent);
    assert.deepEqual(files(), ['notes.md', 'later.md'], 'An uncertain response cannot consume either attachment');
    const delivery = document.querySelector('[aria-label="Message delivery status"]');
    assert.equal(delivery.querySelector('[role="status"]').textContent, 'Message delivery is not confirmed');
    assert.doesNotMatch(delivery.textContent, /Your submitted answer is saved|Guide request running/);
    await act(async () => [...delivery.querySelectorAll('button')].find(node => node.textContent === 'Check saved conversation').click());
    assert.deepEqual(files(), ['later.md'], 'Authoritative completion consumes the original snapshot only');
    assert.equal(document.querySelector('#setup-message').value, laterDraft);
    assert.equal(local.get(`mobkit-setup-completed-v1:${guide.id}`), completed.operation.request_id);
    assert.deepEqual(createGuideAttachmentStore(() => storage(session)).read(guide.id), [ref('later.md', 2)]);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 1, 'Recovery observes without resending');
    await choose('notes.md');
    await act(async () => [...document.querySelectorAll('button')].find(node => node.textContent.includes('Your setups')).click());
    await act(async () => [...document.querySelectorAll('.setup-history-menu button')].find(node => node.textContent.includes('Recovered files')).click());
    assert.deepEqual(files(), ['later.md', 'notes.md'], 'An already acknowledged completion cannot consume deliberate reuse');
  });
});

for (const admission of ['immediate', 'running']) test(`direct ${admission} admission preserves reattached originals through later completion and remount`, async () => {
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(guide.id, [ref()]);
  const state = { ...structuredClone(guide), name: 'My checked files' };
  let accepted, retainedLocal;
  await mount({ session, state, selected: [], mode: 'chat', onPost: ({ url, body }) => {
    if (url.endsWith('/respond')) {
      accepted = { ...structuredClone(state), version: 4, operation: { status: 'running', request_id: 'd'.repeat(64),
        version: 3, request: structuredClone(body) } };
      return admission === 'running' ? accepted : { ...structuredClone(state), version: 4 };
    }
  } }, async ({ document, choose, local }) => {
    await act(async () => document.querySelector('#setup-send').click());
    assert.equal(document.querySelector('.setup-attachments a') === null, true, 'Direct acceptance consumes the first use');
    await choose('notes.md');
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'notes.md');
    retainedLocal = local;
  });
  const completed = { ...accepted, operation: { ...accepted.operation, status: 'completed',
    request: { stage: null, blueprint: null, ...accepted.operation.request } } };
  await mount({ session, local: retainedLocal, state: completed }, async ({ document, calls }) => {
    assert.equal(document.querySelector('.setup-attachments a')?.textContent, 'notes.md',
      'First completion after remount cannot consume an attachment added after direct acceptance');
    assert.equal(calls.filter(call => call.method === 'POST').length, 0);
    assert.equal(retainedLocal.get(`mobkit-setup-completed-v1:${guide.id}`), completed.operation.request_id);
  });
});

test('lost Help response preserves its discussion files and tentative input after authoritative completion', async () => {
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(guide.id, [ref()]);
  let completed;
  await mount({ session, onPost: ({ body }) => {
    completed = { ...structuredClone(guide), version: 4,
      operation: { status: 'completed', request_id: 'e'.repeat(64), version: 3, request: structuredClone(body) } };
    throw new Error('The discussion response was interrupted.');
  }, onRead: ({ url }) => url === `/api/guide/${guide.id}` && completed },
  async ({ document, calls, input }) => {
    await act(async () => [...document.querySelectorAll('button')].find(node => node.textContent === 'Help me decide').click());
    const delivery = document.querySelector('[aria-label="Message delivery status"]');
    assert.equal(delivery.querySelector('[role="status"]').textContent, 'Message delivery is not confirmed');
    await act(async () => [...delivery.querySelectorAll('button')].find(node => node.textContent === 'Check saved conversation').click());
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'notes.md');
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(document.querySelectorAll('.setup-choice.selected').length, 1);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 1);
  });
});

test('lost response retains an original deliberately removed and reattached before completion recovery across remount', async () => {
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(guide.id, [ref()]);
  let completed, retainedLocal;
  await mount({ session, selected: [], mode: 'chat', onPost: ({ url, body }) => {
    if (url.endsWith('/respond')) {
      completed = { ...structuredClone(guide), version: 4, operation: { status: 'completed', request_id: '9'.repeat(64),
        version: 3, request: structuredClone(body) } };
      throw new Error('The admission response was interrupted.');
    }
  } }, async ({ document, choose, local }) => {
    await act(async () => document.querySelector('#setup-send').click());
    await act(async () => document.querySelector('[aria-label="Remove attachment notes.md"]').click());
    await choose('notes.md');
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'notes.md');
    retainedLocal = local;
  });
  await mount({ session, local: retainedLocal, state: completed }, async ({ document, calls }) => {
    assert.equal(document.querySelector('.setup-attachments a')?.textContent, 'notes.md',
      'The same host reference now belongs to a newer user attachment choice');
    assert.equal(calls.filter(call => call.method === 'POST').length, 0);
  });
});

for (const status of [undefined, 'running', 'failed', 'interrupted']) test(`lost admission preserves files until completed receipt (${status || 'unknown'})`, async () => {
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(guide.id, [ref()]);
  let observed;
  await mount({ session, selected: [], mode: 'chat', onPost: ({ body }) => {
    observed = { ...structuredClone(guide), ...(status ? { operation: { status, request_id: 'f'.repeat(64),
      version: 3, request: structuredClone(body) } } : {}) };
    throw new Error('The admission response was interrupted.');
  }, onRead: ({ url }) => url === `/api/guide/${guide.id}` && observed },
  async ({ document, calls, input }) => {
    await act(async () => document.querySelector('#setup-send').click());
    const delivery = document.querySelector('[aria-label="Message delivery status"]');
    assert.equal(delivery.querySelector('[role="status"]').textContent, 'Message delivery is not confirmed');
    assert.doesNotMatch(delivery.textContent, /Your submitted answer is saved|Guide request running/);
    await act(async () => [...delivery.querySelectorAll('button')].find(node => node.textContent === 'Check saved conversation').click());
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'notes.md');
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 1);
  });
});

test('attachment-only send discusses neutral evidence without confirming an offered choice or inventing an analysis', async () => {
  await mount({ selected: [], mode: 'answer' }, async ({ document, window, calls, choose }) => {
    await choose('notes.md');
    const textarea = document.querySelector('#setup-message');
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, '');
      textarea.dispatchEvent(new window.Event('input', { bubbles: true }));
    });
    await act(async () => document.querySelector('#setup-send').click());
    const send = calls.find(call => call.url.endsWith('/respond'));
    assert.equal(send.body.text, 'Consider the attached evidence.');
    assert.equal(send.body.action, 'chat'); assert.deepEqual(send.body.selected, []);
    assert.deepEqual(send.body.attachments, [ref()]);
    assert.equal(document.querySelector('.setup-attachments a') === null, true);
  });
});

test('the first upload starts the anonymous setup once and keeps its original unsent context in the new guide', async () => {
  const initial = { ...structuredClone(guide), id: null, input_surface: { media_types: ['application/pdf'] } };
  await mount({ state: initial, onPost: ({ url }) => url === '/api/guide/start'
    ? { ...structuredClone(guide), id: 'created-file-guide', input_surface: initial.input_surface }
    : url.endsWith('/attachments') ? { attachments: [ref('notes.md', 1, 'created-file-guide')] } : null },
  async ({ document, calls, choose, input, session }) => {
    assert.match(document.querySelector('#setup-attachments-picker').accept, /\.pdf/);
    await choose('notes.md');
    assert.equal(calls.filter(call => call.url === '/api/guide/start').length, 1);
    assert.equal(calls.filter(call => call.url === '/api/guide/created-file-guide/attachments').length, 1);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 0);
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(JSON.parse([...session.values()][0]).guide_id, 'created-file-guide');
  });
});

test('actual pending upload announces progress and disables send until its checked response arrives', async () => {
  let finish;
  await mount({ onPost: ({ url }) => url.endsWith('/attachments')
    ? new Promise(resolve => { finish = () => resolve({ attachments: [ref()] }); }) : null },
  async ({ document, calls, choose }) => {
    await choose('notes.md');
    assert.match(document.querySelector('.setup-attachments [role="status"]').textContent, /Uploading files/);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 0);
    await act(async () => { finish(); await pause(); });
    assert.equal(document.querySelector('.setup-attachments [role="status"]') === null, true);
    assert.equal(document.querySelector('#setup-send').disabled, false);
  });
});

for (const stage of ['headers', 'body']) for (const outcome of ['success', 'error']) {
  test(`upload escape: held ${stage}, late ${outcome} cannot replace an explicit new upload`, async () => {
    const session = new Map(), kept = ref('already-kept.md', 3);
    createGuideAttachmentStore(() => storage(session)).add(guide.id, [kept]);
    let releaseOld, releaseNew, uploads = 0;
    const oldResponse = new Promise(resolve => { releaseOld = resolve; });
    const newResponse = new Promise(resolve => { releaseNew = resolve; });
    await mount({ session, onPost: ({ url }) => {
      if (!url.endsWith('/attachments')) return null;
      if (++uploads === 1) return stage === 'headers' ? oldResponse : {
        ok: outcome === 'success', status: outcome === 'success' ? 200 : 413,
        json: () => oldResponse,
      };
      return newResponse;
    } }, async ({ document, window, calls, choose, input, local }) => {
      await choose('waiting.png');
      const oldUpload = calls.find(call => call.url.endsWith('/attachments'));
      const editor = document.querySelector('#setup-message');
      const written = '  My latest unsent task.\nKeep every character.  ';
      await act(async () => {
        Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(editor, written);
        editor.dispatchEvent(new window.Event('input', { bubbles: true }));
      });
      const stop = [...document.querySelectorAll('.setup-attachments button')].find(button => button.textContent === 'Stop waiting');
      assert.ok(stop, 'The current upload must have an explicit escape while headers or its body are held.');
      await act(async () => { stop.click(); await pause(); });
      assert.equal(oldUpload.signal.aborted, true, 'Only the entered client upload is aborted.');
      assert.equal(document.querySelector('.setup-attachment-wait') === null, true);
      assert.match(document.querySelector('.setup-attachment-recovery[role="status"]').textContent, /Upload not confirmed.*server may still finish.*Retry the upload or remove the files/i);
      assert.equal(document.querySelector('.setup-attachments [role="alert"]') === null, true, 'Stopping the browser wait is a calm recovery state, not an upload failure.');
      assert.equal(document.querySelector('[aria-label="Remove attachment waiting.png"]').disabled, false);
      assert.ok([...document.querySelectorAll('.setup-attachments button')].some(button => button.textContent === 'Retry upload'));
      assert.equal(document.querySelector('#setup-send').disabled, true, 'Retained pending files still need removal or an explicit retry.');
      assert.equal(editor.value, written);
      assert.deepEqual(JSON.parse(local.get(input.key)).selected, input.selected);
      assert.deepEqual(createGuideAttachmentStore(() => storage(session)).read(guide.id), [kept]);

      let newName;
      if (outcome === 'success') {
        newName = 'waiting.png';
        await act(async () => {
          [...document.querySelectorAll('.setup-attachments button')].find(button => button.textContent === 'Retry upload').click();
          await pause();
        });
      } else {
        await act(async () => document.querySelector('[aria-label="Remove attachment waiting.png"]').click());
        assert.equal(document.querySelector('#setup-send').disabled, false);
        assert.equal(editor.value, written);
        newName = 'replacement.png';
        await choose(newName);
      }
      const newUpload = calls.filter(call => call.url.endsWith('/attachments'))[1];
      assert.ok(newUpload, 'Only deliberate Retry or a new file choice admits another upload.');
      await act(async () => {
        releaseOld(outcome === 'success' ? { attachments: [ref('waiting.png')] }
          : stage === 'headers' ? { httpError: 413, detail: 'Old upload failure must stay hidden.' }
            : { detail: 'Old upload failure must stay hidden.' });
        await pause();
      });
      assert.equal(newUpload.signal.aborted, false);
      assert.match(document.querySelector('.setup-attachments [role="status"]').textContent, /Uploading files/);
      assert.equal(document.querySelector('.setup-attachments [role="alert"]') === null, true);
      assert.deepEqual(createGuideAttachmentStore(() => storage(session)).read(guide.id), [kept],
        'The abandoned response cannot publish refs or consume the new owner pending files.');
      assert.equal(document.querySelector(`[aria-label="Remove attachment ${newName}"]`).disabled, true);
      await act(async () => { releaseNew({ attachments: [ref(newName, 2)] }); await pause(); });
      assert.equal(document.querySelector('.setup-attachments [role="status"]') === null, true);
      assert.deepEqual(createGuideAttachmentStore(() => storage(session)).read(guide.id), [kept, ref(newName, 2)]);
      assert.equal(editor.value, written);
      assert.equal(JSON.parse(local.get(input.key)).draft, written);
      assert.deepEqual(JSON.parse(local.get(input.key)).selected, input.selected);
      assert.equal(calls.filter(call => call.url.endsWith('/attachments')).length, 2);
      assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 0);
    });
  });
}

test('unavailable pixel analysis is disclosed during image upload and after it without blocking previews or evidence admission', async () => {
  let finish;
  const detail = 'Image originals are kept, but the current guide cannot read their pixels. Paste relevant measurements or text.';
  await mount({ state: { ...structuredClone(guide), input_surface: { image_analysis: false, image_analysis_detail: detail } },
    onPost: ({ url }) => url.endsWith('/attachments')
      ? new Promise(resolve => { finish = () => resolve({ attachments: [ref('image.png')] }); }) : null },
  async ({ document, calls, choose }) => {
    assert.equal(document.querySelector('.setup-attachment-analysis-note') === null, true, 'no image means no notice');
    assert.match(document.querySelector('#setup-attachments-picker').accept, /image\/png/);
    await choose('image.png');
    assert.equal(document.querySelector('.setup-attachment-analysis-note').textContent, detail);
    assert.equal(document.querySelector('.setup-attachments img').getAttribute('src'), 'blob:offline-preview');
    await act(async () => { finish(); await pause(); });
    assert.equal(document.querySelector('.setup-attachment-analysis-note').textContent, detail);
    assert.equal(document.querySelector('.setup-attachments img').getAttribute('src'), ref('image.png').url);
    assert.equal(document.querySelector('#setup-send').disabled, false);
    await act(async () => document.querySelector('#setup-send').click());
    assert.deepEqual(calls.find(call => call.url.endsWith('/respond')).body.attachments, [ref('image.png')]);
    assert.equal(document.querySelector('.setup-attachment-analysis-note') === null, true, 'admitted files no longer belong to the next message');
  });
});

test('only positively available image analysis suppresses the notice, and documents do not acquire an image warning', async () => {
  for (const capability of [null, true]) {
    const detail = 'Pixel analysis is not verified for this guide. Image originals remain available.';
    await mount({ state: { ...structuredClone(guide), input_surface: { image_analysis: capability, image_analysis_detail: detail } } },
    async ({ document, choose }) => {
      await choose('notes.md');
      assert.equal(document.querySelector('.setup-attachment-analysis-note') === null, true);
      await choose('image.png');
      const notice = document.querySelector('.setup-attachment-analysis-note');
      assert.equal(notice?.textContent ?? null, capability === true ? null : detail);
      assert.ok(document.querySelector('.setup-attachments img'));
    });
  }
});

test('actual navigation cancels the old upload, hides its late failure and allows the new guide upload without losing old checked refs', async () => {
  const other = { ...structuredClone(guide), id: 'other-file-guide', name: 'Other saved setup',
    question: { ...guide.question, title: 'Other saved question' } };
  const session = new Map();
  createGuideAttachmentStore(() => storage(session)).add(guide.id, [ref('kept.md')]);
  let failOld;
  await mount({ session, otherGuides: [other], onPost: ({ url }) => {
    if (url === `/api/guide/${guide.id}/attachments`) return new Promise(resolve => {
      failOld = () => resolve({ httpError: 422, detail: 'Old guide upload failed.' });
    });
    if (url === `/api/guide/${other.id}/attachments`) return { attachments: [ref('new.md', 2, other.id)] };
  } }, async ({ document, choose, calls }) => {
    await choose('late.png');
    const oldUpload = calls.find(call => call.url === `/api/guide/${guide.id}/attachments`);
    assert.match(document.querySelector('.setup-attachments [role="status"]').textContent, /Uploading/);
    await act(async () => [...document.querySelectorAll('button')].find(node => node.textContent.includes('Your setups')).click());
    await act(async () => { [...document.querySelectorAll('.setup-history-menu button')].find(node => node.textContent.includes('Other saved setup')).click(); await pause(); });
    assert.match(document.body.textContent, /Other saved question/);
    assert.equal(oldUpload.signal.aborted, true);
    assert.equal(document.querySelector('.setup-attachments [role="status"]') === null, true);
    assert.equal(document.querySelector('.setup-attachments [role="alert"]') === null, true);
    assert.equal(document.querySelector('.setup-attachments a') === null, true);
    await choose('new.md');
    assert.equal(document.querySelector('.setup-attachments a').getAttribute('href'), ref('new.md', 2, other.id).url);
    await act(async () => { failOld(); await pause(); });
    assert.doesNotMatch(document.body.textContent, /Old guide upload failed/);
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'new.md');
    const recovered = createGuideAttachmentStore(() => storage(session));
    assert.deepEqual(recovered.read(guide.id), [ref('kept.md')]);
    assert.deepEqual(recovered.read(other.id), [ref('new.md', 2, other.id)]);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 0);
  });
});

test('guide submission waits for anonymous startup and response confirmation while preserving the next exact draft', async () => {
  const sent = 'Help our workshop turn repair requests into a useful handover.';
  const duringStartup = '  A later thought, written while the setup is starting.\nKeep this whitespace.  ';
  const duringResponse = '  My next unsent question, changed again before confirmation.\nKeep its exact context.  ';
  const initial = { ...structuredClone(guide), id: null, version: 0, stage: 'purpose',
    answers: [], question: { id: 'purpose', stage: 'purpose', title: 'What would you like to get done?',
      description: 'Describe one useful job.', options: [], allow_text: true, multiple: false } };
  const created = { ...structuredClone(initial), id: 'created-file-guide', version: 1 };
  let releaseStartup, releaseResponse, announceStartup, announceResponse, posted, server = initial;
  const startupEntered = new Promise(resolve => { announceStartup = resolve; });
  const responseEntered = new Promise(resolve => { announceResponse = resolve; });
  const startup = new Promise(resolve => { releaseStartup = () => { server = created; resolve(structuredClone(created)); }; });
  const response = new Promise(resolve => { releaseResponse = () => {
    server = { ...structuredClone(created), operation: { status: 'running', version: created.version,
      request_id: 'd'.repeat(64), started_at: new Date().toISOString(), request: structuredClone(posted) } };
    resolve(structuredClone(server));
  }; });
  await mount({ state: initial, draft: sent, selected: [], mode: 'answer',
    onPost: ({ url, body }) => {
      if (url === '/api/guide/start') { announceStartup(); return startup; }
      assert.equal(url, '/api/guide/created-file-guide/respond');
      posted = structuredClone(body); announceResponse(); return response;
    }, onRead: ({ url }) => url === '/api/architect/status' ? { available: true, status: 'ready', busy: true }
      : url === '/api/guide/created-file-guide' ? structuredClone(server) : null },
  async ({ document, window, calls, local, input }) => {
    const row = () => document.querySelector('.setup-guide-wait');
    const edit = text => act(async () => {
      const textarea = document.querySelector('#setup-message');
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, text);
      textarea.dispatchEvent(new window.Event('input', { bubbles: true })); await pause();
    });
    const assertSending = () => {
      assert.equal(row()?.querySelector('[role="status"]')?.textContent, 'Sending to your guide');
      assert.match(row().textContent, /Waiting for confirmation\. You can keep writing below\./);
      assert.doesNotMatch(row().textContent, /saved|Guide request running|elapsed|Using tool|Last tool/i);
      assert.equal(row().querySelector('.setup-guide-wait-clock') === null, true);
      assert.equal(row().hasAttribute('data-request-id'), false, 'Local sending cannot invent a saved request binding.');
      assert.equal(document.querySelector('#setup-send').disabled, true);
      assert.equal(document.querySelector('#setup-message').disabled, false);
      assert.ok(document.querySelector('.setup-native-chat').textContent.includes(sent), 'The actual Console transcript retains the submitted text.');
    };
    await act(async () => { document.querySelector('#setup-send').click(); await startupEntered; });
    assertSending();
    await edit(duringStartup);
    assert.equal(JSON.parse(local.get(input.key)).draft, duringStartup);
    assert.equal(calls.filter(call => call.url === '/api/guide/start').length, 1);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 0);
    await act(async () => { document.querySelector('#setup-send').click(); releaseStartup(); await responseEntered; });
    assertSending();
    assert.equal(document.querySelector('#setup-message').value, duringStartup);
    await edit(duringResponse);
    await act(async () => { document.querySelector('#setup-send').click(); releaseResponse(); await pause(); });
    assert.equal(row().querySelector('[role="status"]').textContent, 'Saved - your guide is working');
    assert.equal(row().querySelector('small') === null, true, 'The first idea does not repeat the saved-answer hint.');
    assert.equal(document.querySelector('.forge-next-draft-hint').textContent,
      'Your next draft stays here. Send it when your guide replies.');
    assert.doesNotMatch(row().textContent, /Sending to your guide|Waiting for confirmation/);
    assert.equal(row().getAttribute('data-request-id'), server.operation.request_id);
    assert.equal(document.querySelector('#setup-message').value, duringResponse);
    assert.equal(JSON.parse(local.get(inputKey(created))).draft, duringResponse);
    assert.equal(calls.filter(call => call.url === '/api/guide/start').length, 1);
    const submissions = calls.filter(call => call.url.endsWith('/respond'));
    assert.equal(submissions.length, 1, 'Editing and clicking a disabled Send cannot duplicate the submitted turn.');
    assert.deepEqual(submissions[0].body, { version: 1, action: 'answer', selected: [], text: sent });
  });
});

test('guide submission lost acknowledgement preserves the edited draft without claiming saved or running work', async () => {
  const sent = 'Explore how our workshop handles requests from several teams.';
  const next = '  A different unsent question written before acknowledgement.\nKeep this exact draft.  ';
  const state = { ...structuredClone(guide), messages: [{ id: 'earlier-context', role: 'user', text: 'Our workshop supports local teams.' }] };
  let announce, rejectResponse;
  const entered = new Promise(resolve => { announce = resolve; });
  const response = new Promise((_, reject) => { rejectResponse = reject; });
  await mount({ state, draft: sent, selected: [], mode: 'chat', onPost: ({ url }) => {
    assert.equal(url, `/api/guide/${state.id}/respond`); announce(); return response;
  }, onRead: ({ url }) => url === '/api/architect/status' ? { available: true, status: 'ready', busy: true } : null },
  async ({ document, window, calls, local, input }) => {
    await act(async () => { document.querySelector('#setup-send').click(); await entered; });
    const row = document.querySelector('.setup-guide-wait');
    assert.equal(row?.querySelector('[role="status"]')?.textContent, 'Sending to your guide');
    assert.doesNotMatch(row.textContent, /saved|Guide request running|elapsed|Using tool|Last tool/i);
    await act(async () => {
      const textarea = document.querySelector('#setup-message');
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, next);
      textarea.dispatchEvent(new window.Event('input', { bubbles: true })); await pause();
    });
    await act(async () => { rejectResponse(new Error('Offline acknowledgement lost; acceptance could not be confirmed.')); await pause(); });
    const delivery = document.querySelector('[aria-label="Message delivery status"]');
    assert.equal(delivery.querySelector('[role="status"]').textContent, 'Message delivery is not confirmed');
    assert.equal(document.querySelector('.setup-error') === null, true, 'Unconfirmed delivery uses its honest status instead of exposing the transport error.');
    assert.doesNotMatch(delivery.textContent, /Your submitted answer is saved|Guide request running|Sending to your guide/);
    assert.doesNotMatch(document.querySelector('.setup-native-chat').textContent, /Guide request running|Your submitted answer is saved|Sending to your guide/);
    assert.equal(document.querySelector('#setup-message').value, next);
    assert.equal(JSON.parse(local.get(input.key)).draft, next);
    assert.equal(document.querySelector('#setup-send').disabled, true, 'Unconfirmed admission blocks another Send without claiming running work.');
    await act(async () => [...delivery.querySelectorAll('button')].find(node => node.textContent === 'Check saved conversation').click());
    assert.equal(document.querySelector('[aria-label="Message delivery status"] [role="status"]').textContent,
      'Message delivery is not confirmed', 'An unchanged guide without the exact receipt cannot resolve admission.');
    assert.equal(document.querySelector('#setup-message').value, next);
    assert.equal(JSON.parse(local.get(input.key)).draft, next);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    const mutations = calls.filter(call => call.method === 'POST');
    assert.equal(mutations.length, 1, 'Failure and draft edits cannot automatically retry a possibly admitted request.');
    assert.deepEqual(mutations[0].body, { version: state.version, action: 'chat', selected: [], text: sent });
  });
});

test('guide sending navigation retires local confirmation after a failed selection and refuses the late original acknowledgement', async () => {
  const sent = 'Explore the workshop handover for the current saved setup.';
  const next = '  My later workshop thought stays in this setup.\nKeep its exact whitespace.  ';
  const state = { ...structuredClone(guide), messages: [{ id: 'current-context', role: 'user', text: 'Keep the original workshop context.' }] };
  const other = { ...structuredClone(guide), id: 'failed-selection-guide', name: 'Another saved setup',
    question: { ...guide.question, title: 'The other setup question.' } };
  let announceResponse, releaseResponse, announceSelection, rejectSelection, posted;
  const responseEntered = new Promise(resolve => { announceResponse = resolve; });
  const selectionEntered = new Promise(resolve => { announceSelection = resolve; });
  const response = new Promise(resolve => { releaseResponse = () => resolve({ ...structuredClone(state),
    operation: { status: 'running', version: state.version, request_id: 'e'.repeat(64),
      started_at: new Date().toISOString(), request: structuredClone(posted) } }); });
  const selection = new Promise((_, reject) => { rejectSelection = reject; });
  await mount({ state, draft: sent, selected: [], mode: 'chat', otherGuides: [other],
    onPost: ({ url, body }) => {
      assert.equal(url, `/api/guide/${state.id}/respond`);
      posted = structuredClone(body); announceResponse(); return response;
    }, onRead: ({ url }) => {
      if (url === `/api/guide/${other.id}`) { announceSelection(); return selection; }
      return url === '/api/architect/status' ? { available: true, status: 'ready', busy: true } : null;
    } }, async ({ document, window, calls, local, input }) => {
    const row = () => document.querySelector('.setup-guide-wait');
    await act(async () => { document.querySelector('#setup-send').click(); await responseEntered; });
    assert.equal(row()?.querySelector('[role="status"]')?.textContent, 'Sending to your guide');
    await act(async () => {
      const textarea = document.querySelector('#setup-message');
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, next);
      textarea.dispatchEvent(new window.Event('input', { bubbles: true })); await pause();
    });
    await act(async () => {
      [...document.querySelectorAll('button')].find(node => node.textContent.includes('Your setups')).click(); await pause();
    });
    await act(async () => {
      [...document.querySelectorAll('.setup-history-menu button')].find(node => node.textContent.includes(other.name)).click();
      await selectionEntered;
    });
    assert.doesNotMatch(row()?.textContent || '', /Sending to your guide|Guide request running|Your submitted answer is saved/,
      'Selection loading cannot attribute the old local send to the new setup.');
    assert.equal(document.querySelector('[aria-label="Message delivery status"] button').disabled, true);
    await act(async () => { rejectSelection(new Error('Offline selected setup could not be opened.')); await pause(); });
    assert.match(document.body.textContent, /Offline selected setup could not be opened/);
    assert.ok(document.querySelector('.setup-native-chat').textContent.includes(state.messages[0].text),
      'A failed selection leaves the original saved conversation displayed.');
    assert.equal(document.querySelector('[aria-label="Message delivery status"] [role="status"]').textContent,
      'Message delivery is not confirmed', 'Retiring presentation preserves the unresolved admission without claiming running work.');
    assert.doesNotMatch(row()?.textContent || '', /Sending to your guide|Guide request running|Your submitted answer is saved/);
    assert.equal(document.querySelector('#setup-message').value, next);
    assert.equal(JSON.parse(local.get(input.key)).draft, next);
    assert.equal(local.get(GUIDE_KEY), state.id);
    await act(async () => { releaseResponse(); await pause(); });
    assert.equal(document.querySelector('[aria-label="Message delivery status"] [role="status"]').textContent,
      'Message delivery is not confirmed', 'The old acknowledgement remains fenced after the failed navigation.');
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.doesNotMatch(document.querySelector('.setup-native-chat').textContent, /Guide request running|Your submitted answer is saved/);
    assert.equal(document.querySelector('#setup-message').value, next);
    assert.equal(JSON.parse(local.get(input.key)).draft, next);
    assert.equal(calls.filter(call => call.url === `/api/guide/${other.id}`).length, 1);
    const mutations = calls.filter(call => call.method === 'POST');
    assert.equal(mutations.length, 1, 'Navigation, failure and late acknowledgement cannot retry the original request.');
    assert.deepEqual(mutations[0].body, { version: state.version, action: 'chat', selected: [], text: sent });
  });
});

test('first turn transition keeps the real submitted message and editable draft through startup, saved running and remount', async () => {
  const sent = 'Help our workshop turn scattered repair requests into a useful handover.';
  const next = '  A separate unsent workshop thought.\nKeep its exact whitespace.  ';
  const initial = { ...structuredClone(guide), id: null, version: 0, stage: 'purpose', answers: [],
    question: { id: 'purpose', stage: 'purpose', title: 'What would you like to get done?',
      description: 'Describe one useful job.', options: [], multiple: false, allow_text: true } };
  const created = { ...structuredClone(initial), id: 'created-file-guide', version: 1 };
  let announceStartup, releaseStartup, server = initial, retainedLocal, posted;
  const startupEntered = new Promise(resolve => { announceStartup = resolve; });
  const startup = new Promise(resolve => { releaseStartup = () => { server = created; resolve(structuredClone(created)); }; });
  const read = ({ url }) => {
    if (url === '/api/architect/status') return { available: true, status: 'ready', busy: true };
    if (url === '/api/guide/created-file-guide') return structuredClone(server);
    if (url.includes('/timeline')) {
      const operation = server.operation;
      return { available: true, frames: operation ? [{ id: 'first-turn-native-input', kind: 'user_input',
        payload: { content: sent }, interaction_id: 'first-turn-native-interaction', identity: 'architect:main',
        timestamp_ms: Date.parse(operation.started_at), cursor: 'console:1', frame_version: 1,
        source: { kind: 'runtime' } }] : [], bindings: operation ? [{ interaction_id: 'first-turn-native-interaction',
        cursor: 'console:1', user_message_id: 'first-turn-admitted-user', user_text: sent,
        operation_request_id: operation.request_id, operation_started_at: operation.started_at }] : [],
        next_cursor: operation ? 'console:1' : null, latest_cursor: operation ? 'console:1' : null, exhausted: true };
    }
    return null;
  };
  await mount({ state: initial, draft: sent, selected: [], mode: 'answer', onRead: read,
    onPost: ({ url, body }) => {
      if (url === '/api/guide/start') { announceStartup(); return startup; }
      assert.equal(url, '/api/guide/created-file-guide/respond');
      posted = structuredClone(body);
      server = { ...structuredClone(created), operation: { status: 'running', version: created.version,
        request_id: 'f'.repeat(64), started_at: new Date().toISOString(), request: structuredClone(body) } };
      return structuredClone(server);
    } }, async ({ document, window, calls, local }) => {
    assert.equal(document.querySelector('.setup-current-question') !== null, true, 'The original opening is present before an explicit submission.');
    await act(async () => { document.querySelector('#setup-send').click(); await startupEntered; });
    assert.equal(document.querySelector('.setup-current-question') === null, true, 'First local submission removes the obsolete opening question.');
    assert.equal(document.querySelector('.setup-starting-points') === null, true, 'Already-submitted users do not see disabled starting examples.');
    assert.equal(document.querySelector('.setup-question-return') === null, true);
    assert.equal(document.querySelector('.setup-guide-wait [role="status"]').textContent, 'Sending to your guide');
    assert.ok(document.querySelector('.setup-native-chat').textContent.includes(sent), 'The actual Console transcript still shows the submitted text.');
    const textarea = document.querySelector('#setup-message');
    assert.equal(textarea.disabled, false);
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, next);
      textarea.dispatchEvent(new window.Event('input', { bubbles: true })); await pause();
    });
    await act(async () => { releaseStartup(); await pause(); });
    assert.equal(document.querySelector('.setup-current-question') === null, true, 'A saved running receipt with unchanged opening context keeps the old question suppressed.');
    assert.equal(document.querySelector('.setup-starting-points') === null, true);
    assert.equal(document.querySelector('.setup-guide-wait [role="status"]').textContent, 'Saved - your guide is working');
    assert.ok(document.querySelector('.setup-native-chat').textContent.includes(sent));
    assert.equal(document.querySelector('#setup-message').value, next);
    assert.equal(JSON.parse(local.get(inputKey(created))).draft, next);
    assert.equal(calls.filter(call => call.url === '/api/guide/start').length, 1);
    const submissions = calls.filter(call => call.url.endsWith('/respond'));
    assert.equal(submissions.length, 1);
    assert.deepEqual(submissions[0].body, { version: 1, action: 'answer', selected: [], text: sent });
    retainedLocal = local;
  });
  await mount({ state: structuredClone(server), local: retainedLocal, draft: next, selected: [], mode: 'answer', onRead: read },
    ({ document, calls }) => {
      assert.equal(document.querySelector('.setup-current-question') === null, true, 'Remount uses the saved running binding, without a local sending token.');
      assert.equal(document.querySelector('.setup-starting-points') === null, true);
      assert.equal(document.querySelector('.setup-question-return') === null, true);
      assert.equal(document.querySelector('.setup-guide-wait').getAttribute('data-request-id'), server.operation.request_id);
      assert.ok(document.querySelector('.setup-native-chat').textContent.includes(sent), 'The real native frame restores the submitted text on remount.');
      assert.equal(document.querySelector('#setup-message').value, next);
      assert.equal(document.querySelector('#setup-message').disabled, false);
      assert.equal(calls.filter(call => call.method === 'POST').length, 0, 'Restoring a running first turn cannot resubmit it.');
    });
});

test('first turn transition restores the initial question after refusal or interruption and shows actual published context while running', async () => {
  const sent = 'Help a community workshop coordinate repair requests.';
  const next = '  A later unsent thought survives the refused first turn.\nKeep it exact.  ';
  const initial = { ...structuredClone(guide), id: 'created-file-guide', version: 1, stage: 'purpose', answers: [],
    operation: { status: 'failed', version: 1, request_id: '9'.repeat(64), started_at: '2026-10-09T13:00:00Z',
      detail: 'The earlier first turn failed.', request: { version: 1, action: 'answer', selected: [], text: sent } },
    question: { id: 'purpose', stage: 'purpose', title: 'What would you like to get done?',
      description: 'Describe one useful job.', options: [], multiple: false, allow_text: true } };
  let announceResponse, releaseRefusal, retainedLocal;
  const responseEntered = new Promise(resolve => { announceResponse = resolve; });
  const response = new Promise(resolve => { releaseRefusal = () => resolve({ httpError: 409, detail: 'Offline first answer refused before admission.' }); });
  const read = ({ url }) => url === '/api/architect/status' ? { available: true, status: 'ready', busy: true } : null;
  await mount({ state: initial, draft: sent, selected: [], mode: 'answer', onRead: read,
    onPost: ({ url }) => { assert.equal(url, '/api/guide/created-file-guide/respond'); announceResponse(); return response; } },
    async ({ document, window, calls, local, input }) => {
      const help = [...document.querySelectorAll('.setup-current-question button')].find(node => node.textContent.includes('Help me describe it'));
      assert.ok(help, 'A failed first turn keeps its existing opening help control.');
      help.focus();
      assert.equal(document.activeElement === help, true);
      await act(async () => { help.click(); await responseEntered; });
      assert.equal(document.activeElement === document.querySelector('#setup-message'), true,
        'An explicit first-turn help submission moves focus from its removed question to the existing composer.');
      assert.equal(document.querySelector('.setup-current-question') === null, true, 'A local first-turn wait suppresses only the obsolete question subtree.');
      assert.equal(document.querySelector('.setup-starting-points') === null, true);
      assert.ok(document.querySelector('.setup-native-chat').textContent.includes(sent), 'The native submitted help still contains the user context.');
      await act(async () => {
        const textarea = document.querySelector('#setup-message');
        Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, next);
        textarea.dispatchEvent(new window.Event('input', { bubbles: true })); await pause();
      });
      await act(async () => { releaseRefusal(); await pause(); });
      assert.match(document.querySelector('.setup-error').textContent, /Offline first answer refused before admission/);
      assert.equal(document.querySelector('.setup-current-question') !== null, true, 'Refused admission restores the original question.');
      assert.equal(document.querySelector('.setup-starting-points') !== null, true);
      assert.equal(document.querySelector('.setup-guide-wait') === null, true);
      assert.equal(document.querySelector('#setup-message').value, next);
      assert.equal(JSON.parse(local.get(input.key)).draft, next);
      assert.equal(calls.filter(call => call.method === 'POST').length, 1, 'Refusal never automatically retries the first turn.');
      retainedLocal = local;
    });
  const operation = { status: 'interrupted', version: 1, request_id: 'a'.repeat(64),
    started_at: '2026-10-09T14:00:00Z', detail: 'The saved first turn was interrupted.',
    request: { version: 1, action: 'answer', selected: [], text: sent } };
  await mount({ state: { ...structuredClone(initial), operation }, local: retainedLocal, draft: next, selected: [], mode: 'answer', onRead: read },
    ({ document, calls }) => {
      assert.equal(document.querySelector('.setup-current-question') !== null, true, 'An interrupted saved operation is not a current waiting binding.');
      assert.ok(document.querySelector('.setup-current-question').textContent.includes(initial.question.title));
      assert.equal(document.querySelector('.setup-starting-points') !== null, true);
      assert.equal(document.querySelector('.setup-guide-wait') === null, true);
      assert.equal(document.querySelector('#setup-message').value, next);
      assert.equal(calls.filter(call => call.method === 'POST').length, 0);
    });
  const published = { ...structuredClone(initial), version: 2, operation: { ...operation, status: 'running' },
    messages: [{ id: 'actual-first-user', role: 'user', text: sent },
      { id: 'actual-context-question', role: 'assistant', text: 'Who should the workshop support?' }],
    brief: { facts: { purpose: { summary: 'Coordinate the workshop repair requests.' } }, requirements: [] },
    question: { id: 'audience', stage: 'people', title: 'Who should the workshop support?', description: 'Choose the actual audience.',
      multiple: false, allow_text: true, options: [{ id: 'coordinators', label: 'Workshop coordinators', description: 'People handling the requests.' }] } };
  await mount({ state: published, draft: next, selected: [], mode: 'answer', onRead: read }, ({ document, calls }) => {
    assert.equal(document.querySelector('.setup-current-question') !== null, true, 'Actual published context releases first-turn suppression even while a saved operation is running.');
    assert.ok(document.querySelector('.setup-current-question').textContent.includes(published.question.title));
    assert.equal(document.querySelector('.setup-choice').disabled, true, 'Existing running-state eligibility still pauses the published choice.');
    assert.equal(document.querySelector('.setup-guide-wait [role="status"]').textContent, 'Guide request running');
    assert.ok(document.querySelector('.setup-native-chat').textContent.includes(sent));
    assert.equal(document.querySelector('#setup-message').value, next);
    assert.equal(calls.filter(call => call.method === 'POST').length, 0);
  });
});
