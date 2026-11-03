import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve } from 'node:path';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';

const web = resolve(process.env.FORGE_ATTACHMENT_RETRY_WEB || dirname(fileURLToPath(import.meta.url)));
const require = createRequire(resolve(web, 'package.json'));
const { GUIDE_KEY } = await import(pathToFileURL(resolve(web, 'guided-state.mjs')).href);
const { inputKey } = await import(pathToFileURL(resolve(web, 'wizard-state.mjs')).href);
const React = require('react'), { act } = React;
const { build } = require('esbuild');
const { JSDOM } = createRequire(resolve(web, '../../console/package.json'))('jsdom');
const hookSource = resolve(process.env.FORGE_ATTACHMENT_RETRY_SOURCE || resolve(web, 'guided-attachments.jsx'));
const bundled = await build({ absWorkingDir: web, entryPoints: [resolve(web, 'guided.jsx')], bundle: true, write: false,
  outfile: resolve(web, 'attachment-retry-control.mjs'), sourcemap: 'external', sourcesContent: true,
  format: 'esm', platform: 'node', logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [resolve(web, 'node_modules')],
  alias: { '@console-core': resolve(web, '../../packages/console-core/src') },
  plugins: [{ name: 'shared-react-and-selected-hook', setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    builder.onResolve({ filter: /guided-attachments\.jsx$/ }, () => ({ path: hookSource }));
    builder.onLoad({ filter: /guided-attachments\.jsx$/ }, async args => ({ contents: await readFile(args.path, 'utf8'), loader: 'jsx', resolveDir: web }));
  } }] });
const script = bundled.outputFiles.find(file => file.path.endsWith('.mjs'));
if (process.env.FORGE_ATTACHMENT_RETRY_BUNDLE) {
  await writeFile(process.env.FORGE_ATTACHMENT_RETRY_BUNDLE, script.contents);
  await writeFile(`${process.env.FORGE_ATTACHMENT_RETRY_BUNDLE}.map`, bundled.outputFiles.find(file => file.path.endsWith('.map')).contents);
}
const { GuidedSetup } = await import(`data:text/javascript;base64,${Buffer.from(script.text).toString('base64')}`);
const guide = { id: 'attachment-owner-a', name: 'Attachment guide A', version: 3, status: 'question', stage: 'knowledge', completed_stages: [], messages: [],
  brief: { facts: {}, requirements: [] }, capabilities: { available: true },
  question: { id: 'evidence', stage: 'knowledge', title: 'Which evidence?', description: 'Keep its source.', multiple: true,
    allow_text: true, options: [{ id: 'current', label: 'Current evidence', description: 'Use the current context.' }] } };
const ref = (name = 'notes.md', index = 1, id = guide.id) => ({ id: String(index).repeat(64), filename: name,
  media_type: name.endsWith('.png') ? 'image/png' : 'text/markdown', size: 12, sha256: 'a'.repeat(64),
  url: `/api/guide/${id}/attachments/${String(index).repeat(64)}` });
const storage = values => ({ getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
  removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } });
const pause = () => new Promise(resolve => setTimeout(resolve, 5));

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
      const files = body instanceof dom.window.FormData ? await Promise.all(body.getAll('files').map(file => new Promise((resolve, reject) => {
        const reader = new dom.window.FileReader(); reader.onload = () => resolve({ name: file.name, type: file.type, size: file.size, text: reader.result }); reader.onerror = reject; reader.readAsText(file);
      }))) : [];
      let value;
      if (method === 'POST') {
        assert.ok(['/api/guide/start', ...[state.id, 'created-file-guide', ...otherGuides.map(item => item.id)].flatMap(id =>
          [`/api/guide/${id}/attachments`, `/api/guide/${id}/respond`])].includes(url));
        value = await onPost?.({ url, body, calls, files });
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
  const { createRoot } = await import(pathToFileURL(require.resolve('react-dom/client')).href);
  const root = createRoot(dom.window.document.getElementById('root'));
  const choose = async files => {
    const picker = dom.window.document.querySelector('#setup-attachments-picker');
    const selectedFiles = files.map(({ name, text }) => new dom.window.File([text], name, { type: 'text/markdown' }));
    Object.defineProperty(picker, 'files', { configurable: true, value: selectedFiles });
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

test('removing one rejected file leaves an explicit retry for the remaining attachment and preserves unsent input', async () => {
  const sent = [];
  let finishRetry;
  await mount({ onPost: ({ url, files }) => {
    assert.equal(url, `/api/guide/${guide.id}/attachments`, 'This case must never send guide work.');
    sent.push(structuredClone(files));
    if (sent.length === 1) return { httpError: 422, detail: 'bad.md is not readable. Remove it and retry the other file.' };
    assert.equal(sent.length, 2, 'Only explicit Retry starts the second upload.');
    return new Promise(resolve => { finishRetry = () => resolve({ attachments: [{
      id: '1'.repeat(64), filename: files[0].name, media_type: files[0].type,
      size: files[0].size, sha256: createHash('sha256').update(files[0].text).digest('hex'),
      url: `/api/guide/${guide.id}/attachments/${'1'.repeat(64)}`,
    }] }); });
  } }, async ({ document, calls, choose, input, local, session }) => {
    const retry = () => [...document.querySelectorAll('.setup-attachments button')].find(node => node.textContent === 'Retry upload');
    const retained = local.get(input.key);
    await choose([{ name: 'bad.md', text: 'Rejected fixture' }, { name: 'keep.md', text: '  Useful source context.\nKeep these original bytes.  ' }]);
    assert.match(document.querySelector('.setup-attachments [role="alert"]').textContent, /bad.md is not readable/);
    assert.ok(retry(), 'The original failed batch offers its existing Retry action.');
    assert.equal(document.querySelectorAll('.setup-attachments li').length, 2);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    await act(async () => { document.querySelector('[aria-label="Remove attachment bad.md"]').click(); await pause(); });
    assert.equal(document.querySelector('.setup-attachments [role="alert"]') === null, true, 'Removing the rejected file keeps existing error-clearing semantics.');
    assert.equal(document.querySelectorAll('.setup-attachments li').length, 1);
    assert.equal(document.querySelector('.setup-attachments li span').textContent, 'keep.md');
    assert.equal(document.querySelector('#setup-send').disabled, true, 'Unuploaded remaining evidence still cannot be silently omitted.');
    assert.ok(retry(), 'Removing one failed file must not hide Retry for the remaining unsent file.');
    assert.equal(sent.length, 1, 'Removal does not upload automatically.');
    assert.equal(session.size, 0, 'Raw failed Files are never persisted.');
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.deepEqual(JSON.parse(local.get(input.key)).selected, input.selected);
    assert.equal(local.get(input.key), retained);
    await act(async () => { retry().click(); await pause(); });
    // The inert transport reads File bytes asynchronously before its response hook.
    for (let step = 0; sent.length < 2 && step < 40; step++) await act(pause);
    assert.equal(sent.length, 2);
    assert.equal(retry() === undefined, true, 'An active upload cannot start another Retry.');
    assert.equal(document.querySelector('[aria-label="Remove attachment keep.md"]').disabled, true);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    await act(async () => { finishRetry(); await pause(); });
    assert.equal(document.querySelector('.setup-attachments a').textContent, 'keep.md');
    assert.equal(document.querySelector('.setup-attachments li span') === null, true, 'Only checked refs replace the remaining pending File.');
    assert.equal(retry() === undefined, true);
    assert.equal(document.querySelector('#setup-send').disabled, false);
    assert.equal(document.querySelector('#setup-message').value, input.draft);
    assert.equal(local.get(input.key), retained);
    assert.deepEqual(sent.map(files => files.map(({ name, text }) => [name, text])), [
      [['bad.md', 'Rejected fixture'], ['keep.md', '  Useful source context.\nKeep these original bytes.  ']],
      [['keep.md', '  Useful source context.\nKeep these original bytes.  ']],
    ]);
    assert.equal(calls.filter(call => call.method === 'POST').length, 2);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 0);
  });
});
