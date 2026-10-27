import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { GUIDE_KEY } from './guided-state.mjs';
import { inputKey } from './wizard-state.mjs';

const web = fileURLToPath(new URL('./', import.meta.url));
const require = createRequire(new URL('./package.json', import.meta.url));
const React = require('react'), { act } = React;
const { build } = require('esbuild');
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const hookSource = resolve(process.env.FORGE_ATTACHMENT_DRAFT_SOURCE || fileURLToPath(new URL('./guided-attachments.jsx', import.meta.url)));
const bundled = await build({ entryPoints: [resolve(web, 'guided.jsx')], bundle: true, write: false,
  outfile: fileURLToPath(new URL('./mounted-control.mjs', import.meta.url)), sourcemap: 'external', sourcesContent: true,
  format: 'esm', platform: 'node', logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [resolve(web, 'node_modules')],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'shared-react-and-selected-hook', setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    builder.onResolve({ filter: /guided-attachments\.jsx$/ }, () => ({ path: hookSource }));
    builder.onLoad({ filter: /guided-attachments\.jsx$/ }, async args => ({ contents: await readFile(args.path, 'utf8'), loader: 'jsx', resolveDir: web }));
  } }] });
if (process.env.FORGE_ATTACHMENT_DRAFT_MAP)
  await writeFile(process.env.FORGE_ATTACHMENT_DRAFT_MAP, bundled.outputFiles.find(file => file.path.endsWith('.map')).contents);
const script = bundled.outputFiles.find(file => file.path.endsWith('mounted-control.mjs'));
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
  const choose = async (name, content = 'safe fixture') => {
    const picker = dom.window.document.querySelector('#setup-attachments-picker');
    const file = new dom.window.File([content], name, { type: name.endsWith('.png') ? 'image/png' : 'text/markdown' });
    Object.defineProperty(picker, 'files', { configurable: true, value: [file] });
    await act(async () => { picker.dispatchEvent(new dom.window.Event('change', { bubbles: true })); await pause(); });
    const deadline = Date.now() + 5000;
    while (dom.window.document.querySelector('.setup-attachment-wait') && Date.now() < deadline) await act(pause);
    assert.equal(dom.window.document.querySelector('.setup-attachment-wait') === null, true,
      'Wait for the actual FileReader/upload response before checking its retained draft.');
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

test('a failed attachment draft survives another guide upload and same-name pending removal, then retries only its original bytes', async () => {
  const other = { ...structuredClone(guide), id: 'attachment-owner-b', name: 'Attachment guide B',
    question: { ...guide.question, title: 'B evidence' } };
  const sent = [], attempts = new Map();
  const responseRef = (id, file) => ({ id: (id === guide.id ? '1' : '2').repeat(64), filename: file.name,
    media_type: file.type, size: file.size, sha256: createHash('sha256').update(file.text).digest('hex'),
    url: `/api/guide/${id}/attachments/${(id === guide.id ? '1' : '2').repeat(64)}` });
  await mount({ otherGuides: [other, guide], onPost: ({ url, files }) => {
    assert.ok(url.endsWith('/attachments'), 'This scenario never sends an answer or admits guide work.');
    const id = url.split('/')[3];
    const count = (attempts.get(id) || 0) + 1; attempts.set(id, count);
    sent.push({ id, files: structuredClone(files) });
    assert.equal(files.length, 1);
    if (count === 1) return { httpError: 422, detail: `${id === guide.id ? 'A' : 'B'} upload failed; keep its draft.` };
    return { attachments: [responseRef(id, files[0])] };
  } }, async ({ document, choose, calls }) => {
    const switchTo = async label => {
      await act(async () => { [...document.querySelectorAll('button')].find(node => node.textContent.includes('Your setups')).click(); });
      await act(async () => { [...document.querySelectorAll('.setup-history-menu button')].find(node => node.textContent.includes(label)).click(); await pause(); });
    };
    await choose('shared.md', 'Original A evidence');
    assert.match(document.querySelector('.setup-attachments [role="alert"]').textContent, /A upload failed/);
    assert.ok([...document.querySelectorAll('button')].find(node => node.textContent === 'Retry upload'));
    await switchTo('Attachment guide B');
    assert.equal(document.querySelector('.setup-attachments li') === null, true, 'A pending files are hidden in B.');
    await choose('shared.md', 'Unsent B evidence');
    assert.match(document.querySelector('.setup-attachments [role="alert"]').textContent, /B upload failed/);
    await act(async () => { document.querySelector('button[aria-label="Remove attachment shared.md"]').click(); await pause(); });
    assert.equal(document.querySelector('.setup-attachments li') === null, true, 'Removing B clears only its colliding pending item.');
    await choose('shared.md', 'Confirmed B evidence');
    assert.equal(document.querySelector('.setup-attachments a').getAttribute('href'), responseRef(other.id, sent.at(-1).files[0]).url);
    await switchTo('Attachment guide A');
    assert.equal(document.querySelector('.setup-attachments li span')?.textContent, 'shared.md', 'A failed, unsent File must survive B activity.');
    assert.match(document.querySelector('.setup-attachments [role="alert"]').textContent, /A upload failed/);
    assert.equal(document.querySelector('#setup-send').disabled, true, 'The failed A file cannot be silently omitted.');
    await act(async () => { [...document.querySelectorAll('button')].find(node => node.textContent === 'Retry upload').click(); await pause(); });
    for (let step = 0; !document.querySelector('.setup-attachments a') && step < 40; step++) await act(pause);
    assert.ok(document.querySelector('.setup-attachments a'), `A retry must settle to its checked reference. Files: ${JSON.stringify(sent)}; UI: ${document.querySelector('.setup-attachments').textContent}`);
    assert.equal(document.querySelector('.setup-attachments a').getAttribute('href'), responseRef(guide.id, sent.at(-1).files[0]).url);
    assert.equal(document.querySelector('.setup-attachments [role="alert"]') === null, true);
    assert.deepEqual(sent.map(({ id, files }) => [id, files[0].name, files[0].text]), [
      [guide.id, 'shared.md', 'Original A evidence'],
      [other.id, 'shared.md', 'Unsent B evidence'],
      [other.id, 'shared.md', 'Confirmed B evidence'],
      [guide.id, 'shared.md', 'Original A evidence'],
    ], 'Each upload/retry keeps its actual guide and File bytes.');
    assert.equal(calls.filter(call => call.method === 'POST').length, 4);
    assert.equal(calls.filter(call => call.url.endsWith('/respond')).length, 0);
  });
});
