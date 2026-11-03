import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { webcrypto } from 'node:crypto';

const web = process.env.FORGE_RESUME_WEB || fileURLToPath(new URL('.', import.meta.url));
const sourcePath = process.env.FORGE_RESUME_SOURCE || join(web, 'guided.jsx');
const require = createRequire(join(web, 'package.json'));
const React = require('react'), { act } = React;
const { build } = require('esbuild');
const { JSDOM } = createRequire(resolve(web, '../../console/package.json'))('jsdom');
const { inputKey } = await import(pathToFileURL(join(web, 'wizard-state.mjs')));
const { GUIDE_KEY } = await import(pathToFileURL(join(web, 'guided-state.mjs')));
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
const directory = await mkdtemp(join(tmpdir(), 'forge-welcome-resume-'));
const modulePath = join(directory, 'guided.mjs');
after(() => rm(directory, { recursive: true, force: true }));
// Retain real guide/composer/navigation/storage owners. Only unrelated generated
// drawing and Console transcript rendering are inert, as in proposal-origin.
const built = await build({
  stdin: { contents: await readFile(sourcePath, 'utf8'), sourcefile: sourcePath, resolveDir: web, loader: 'jsx' },
  bundle: true, write: false, platform: 'node', format: 'esm', jsx: 'automatic',
  outfile: modulePath, sourcemap: 'linked', sourcesContent: true, logLevel: 'silent',
  loader: { '.css': 'empty' }, nodePaths: [join(web, 'node_modules')],
  alias: { '@console-core': resolve(web, '../../packages/console-core/src') },
  plugins: [{ name: 'actual-guide-inert-drawing', setup(builder) {
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
  if (output.path.endsWith('.map') && process.env.FORGE_RESUME_MAP)
    await writeFile(process.env.FORGE_RESUME_MAP, output.contents);
}
const { GuidedSetup } = await import(pathToFileURL(modulePath));
const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};
const response = (body, status = 200) => ({ ok: status < 400, status, json: async () => body });
const guide = id => ({ id, version: 1, status: 'question', stage: 'purpose',
  completed_stages: [], answers: [], messages: [], proposal: null, blueprint: null,
  brief: { facts: {}, requirements: [] }, capabilities: { available: false },
  question: { id: 'purpose', stage: 'purpose', title: 'What needs doing?', description: 'Describe a concrete task.',
    options: [], multiple: false, allow_text: true } });
const starter = { schema_version: 2, project: { name: 'Internal scaffold', slug: 'internal-scaffold', description: '' },
  provider: 'openai', model: 'gpt-6.1-sol', target: 'mac',
  agent_classes: [{ id: 'lead', name: 'Context owner', description: '', instructions: '', instances: 1,
    members: [], provider: 'openai', model: 'gpt-6.1-sol', tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
  topology: { mode: 'manual', entrypoint: 'lead:main', edges: [] }, connections: [], flows: [],
  knowledge: { mode: 'none' }, advanced: { port: 4110 } };

async function mounted(scenario, verify, timers = {}, { narrow = false, available = false, allowSend = false, startingInput = null, organization = null } = {}) {
  const a = guide('resume-opening-a'), b = { ...guide('resume-existing-b'),
    messages: [{ role: 'user', text: 'Keep the real saved observing setup.' }] };
  a.capabilities.available = available;
  if (organization) a.organization = structuredClone(organization);
  const list = deferred(), selection = deferred(), calls = [];
  const values = new Map([[GUIDE_KEY, a.id]]);
  if (startingInput) values.set(inputKey(a), JSON.stringify({ ...startingInput, key: inputKey(a) }));
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const document = dom.window.document;
  let firstList = true, root, nextInterval = 0;
  const intervals = new Map();
  const replacements = { window: dom.window, document, navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    localStorage: storage, sessionStorage: dom.window.sessionStorage, crypto: webcrypto, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => ({ matches: narrow, addEventListener() {}, removeEventListener() {} }),
    setInterval: callback => { const key = ++nextInterval; intervals.set(key, callback); return key; },
    clearInterval: key => intervals.delete(key),
    ...timers,
    fetch: async (url, options = {}) => {
      const method = options.method || 'GET', body = options.body && JSON.parse(options.body);
      calls.push({ url: String(url), method, body });
      if (method === 'POST') {
        assert.equal(allowSend, true, 'Resume navigation never creates/sends/starts work.');
        assert.equal(url, `/api/guide/${a.id}/respond`, 'Only the exact existing guide receives the explicit send.');
        return response({ ...structuredClone(a), version: a.version + 1,
          messages: [{ role: 'user', text: body.text }], operation: undefined });
      }
      assert.equal(method, 'GET', 'Resume navigation never creates/sends/starts work.');
      if (url === `/api/guide/${a.id}`) return response(structuredClone(a));
      if (url === `/api/guide/${b.id}`) return await selection.promise;
      if (url === '/api/guide/list') {
        if (firstList) {
          firstList = false;
          return scenario === 'list-timeout' ? { ok: true, status: 200, json: () => list.promise } : await list.promise;
        }
        return scenario === 'failed' ? response({ detail: 'Inert saved-list read failed.' }, 503)
          : response({ setups: scenario === 'empty' ? [] : scenario === 'only-current' ? [{ id: a.id, name: 'Blank saved start', status: 'question' }]
            : [{ id: a.id, name: 'Blank saved start', status: 'question' },
              { id: b.id, name: 'Existing observing setup', status: scenario },
              { id: 'resume-later-c', name: 'Later server-ordered setup', status: 'ready' }] });
      }
      if (String(url).includes('/timeline?')) return response({ frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true });
      if (url === '/api/architect/status') return response({ available, status: available ? 'ready' : 'unavailable', busy: false });
      if (url === '/api/provider-settings') return response({ providers: [], effective_provider: null, runtime_active: false, work_active: false });
      return response({ detail: 'No saved fixture operation.' }, 404);
    } };
  const old = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  try {
    const { createRoot } = require('react-dom/client');
    root = createRoot(document.getElementById('root'));
    await act(async () => { root.render(React.createElement(GuidedSetup, {
      active: true, bp: starter, Icon: () => null, onBlueprint: () => {}, onAdvanced: () => {},
      config: { architect: { available } } })); await settle(); });
    await act(async () => settle());
    const finish = async (wait, value) => {
      await act(async () => { wait.resolve(value); await settle(); }); await act(async () => settle());
    };
    await verify({ document, a, b, calls, values, list, selection, finish });
    if (!allowSend) assert.ok(calls.every(call => call.method === 'GET'));
    assert.equal(intervals.size > 20, false, 'The fixture does not create a polling campaign.');
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of old) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}
async function click(node) {
  assert.ok(node, 'The actual visible control exists.'); assert.equal(node.disabled, false);
  await act(async () => { node.click(); await settle(); }); await act(async () => settle());
}
const button = (document, label) => [...document.querySelectorAll('button')].find(item =>
    item.textContent.trim() === label || label === 'Guide settings' && item.textContent.trim().startsWith(label));

for (const change of ['none', 'new edit', 'focus elsewhere']) test(`restored welcome context ${change === 'none' ? 'puts the caret at the end' : `preserves ${change} before the caret frame`}`, async () => {
  const inserted = 'For a team, I would like help with repetitive work.';
  const draft = `${inserted}\n\nTurn our weekly project notes into a clear update.`;
  const frames = new Map();
  let frameId = 0;
  const timers = {
    requestAnimationFrame: callback => { const id = ++frameId; frames.set(id, callback); return id; },
    cancelAnimationFrame: id => frames.delete(id),
  };
  await mounted('empty', async ({ document, a, calls, values, list, finish }) => {
    await finish(list, response({ setups: [] }));
    assert.ok(document.querySelector('.setup-welcome'));
    const editor = document.querySelector('#setup-message');
    assert.equal(editor.value, draft, 'Exercise restored starting context in the actual Console composer.');
    await click(button(document, 'Change choices'));
    const choice = label => [...document.querySelectorAll('.forge-starting-help__choice')]
      .find(node => node.querySelector('strong')?.textContent === label);
    await click(choice('A team'));
    editor.setSelectionRange(0, 0);
    await click(choice('Repetitive work'));
    assert.equal(document.activeElement === editor, true, 'The choice retains immediate composer focus.');
    assert.equal(editor.value, draft, 'Selecting the same restored context does not change any draft byte.');
    assert.equal(JSON.parse(values.get(inputKey(a))).draft, draft);
    if (change === 'new edit') {
      const edited = `${draft}\nKeep this newer thought.`;
      await act(async () => {
        Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value').set.call(editor, edited);
        editor.dispatchEvent(new document.defaultView.Event('input', { bubbles: true })); await settle();
      });
      editor.setSelectionRange(4, 4);
    } else if (change === 'focus elsewhere') {
      await act(async () => { button(document, 'Guide settings').focus(); await settle(); });
    }
    const focus = document.activeElement;
    await act(async () => {
      const pending = [...frames.values()]; frames.clear();
      for (const callback of pending) callback(0);
      await settle();
    });
    assert.equal(document.activeElement === focus, true, 'A deferred caret handoff never takes focus back.');
    assert.equal(editor.selectionStart, change === 'none' ? draft.length : change === 'new edit' ? 4 : 0);
    assert.equal(editor.selectionEnd, editor.selectionStart);
    assert.equal(editor.value, change === 'new edit' ? `${draft}\nKeep this newer thought.` : draft);
    assert.ok(calls.every(call => call.method === 'GET'), 'Caret placement neither submits nor connects the guide.');
  }, timers, { narrow: true, available: true, startingInput: { draft, selected: [], mode: 'answer',
    starting_help: { audience_id: 'team', kind_id: 'repetitive', prefix: '', inserted } } });
});

for (const narrow of [true, false]) test(`${narrow ? 'phone Return keeps the idea unsent until explicit Send' : 'desktop Enter still explicitly sends the idea'}`, async () => {
  await mounted('empty', async ({ document, a, calls, values, list, finish }) => {
    await finish(list, response({ setups: [] }));
    assert.ok(document.querySelector('.setup-welcome'), 'Exercise the actual first-minute composer.');
    const textarea = document.querySelector('#setup-message'), send = document.querySelector('#setup-send');
    const draft = '  Plan our community garden.\nKeep this second thought unsent.  ';
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value').set.call(textarea, draft);
      textarea.dispatchEvent(new document.defaultView.Event('input', { bubbles: true })); await settle();
    });
    const posts = () => calls.filter(call => call.method === 'POST');
    const enter = async options => {
      const event = new document.defaultView.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true, ...options });
      await act(async () => { textarea.dispatchEvent(event); await settle(); });
      return event;
    };
    for (const options of [{ shiftKey: true }, { isComposing: true }]) {
      assert.equal((await enter(options)).defaultPrevented, false, 'Newline and composition keys retain browser ownership.');
      assert.deepEqual(posts(), []);
      assert.equal(textarea.value, draft);
      assert.equal(JSON.parse(values.get(inputKey(a))).draft, draft);
    }
    if (narrow) {
      assert.equal(document.querySelector('.forge-welcome-shortcuts') === null, true, 'Phone offers explicit Send without a misleading desktop shortcut.');
      assert.equal((await enter({})).defaultPrevented, false, 'Phone Return remains a native textarea newline.');
      assert.deepEqual(posts(), [], 'Phone Return never submits the unfinished idea.');
      assert.equal(textarea.value, draft);
      assert.equal(JSON.parse(values.get(inputKey(a))).draft, draft);
      assert.equal(send.title, 'Send to guide. Return adds a new line.');
      await click(send);
    } else {
      assert.match(document.querySelector('.forge-welcome-shortcuts').textContent, /Enter to send.*Shift \+ Enter for a new line/);
      assert.equal(send.title, 'Send to guide (Enter). Shift + Enter adds a new line.');
      assert.equal((await enter({})).defaultPrevented, true, 'Desktop Enter remains an explicit submission shortcut.');
    }
    assert.equal(posts().length, 1, 'Exactly one deliberate submission reaches the guide.');
    assert.deepEqual(posts()[0], { url: `/api/guide/${a.id}/respond`, method: 'POST',
      body: { version: a.version, action: 'answer', selected: [], text: draft } });
  }, {}, { narrow, available: true, allowSend: true });
});

test('a stalled saved-setup list enables retry and ignores its late body without changing the current draft', async () => {
  const realClearTimeout = globalThis.clearTimeout, deadlines = new Map();
  const timers = {
    setTimeout: (callback, delay, ...args) => {
      if (delay !== 30000) return realSetTimeout(callback, delay, ...args);
      const token = {}; deadlines.set(token, () => callback(...args)); return token;
    },
    clearTimeout: token => deadlines.has(token) ? deadlines.delete(token) : realClearTimeout(token),
  };
  await mounted('list-timeout', async ({ document, a, calls, values, list, finish }) => {
    const draft = '  My unsent idea\nKeep its whitespace.  ';
    const input = document.querySelector('#setup-message');
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value').set.call(input, draft);
      input.dispatchEvent(new document.defaultView.Event('input', { bubbles: true })); await settle();
    });
    await click(button(document, 'Your setups'));
    assert.match(document.querySelector('.setup-history-menu').textContent, /Reading saved setups/);
    await act(async () => {
      for (const [token, expire] of [...deadlines]) { deadlines.delete(token); expire(); }
      await settle();
    });
    assert.match(document.querySelector('.setup-history-menu').textContent, /Saved setups could not be read/);
    assert.equal(document.querySelector('.forge-welcome-resume') === null, true);
    await click(button(document, 'Retry saved setups'));
    assert.equal(document.querySelector('.forge-welcome-resume strong').textContent, 'Existing observing setup');
    assert.equal(values.get(GUIDE_KEY), a.id);
    assert.equal(input.value, draft);
    assert.equal(JSON.parse(values.get(inputKey(a))).draft, draft);
    await finish(list, { setups: [{ id: 'late-unrelated', name: 'Late old setup', status: 'ready' }] });
    assert.equal(document.querySelector('.forge-welcome-resume strong').textContent, 'Existing observing setup');
    assert.equal(document.querySelector('.setup-history-menu').textContent.includes('Late old setup'), false);
    assert.equal(input.value, draft);
    assert.equal(calls.filter(call => call.url === '/api/guide/list').length, 2);
    assert.equal(calls.filter(call => call.url === `/api/guide/${a.id}`).length, 1);
    assert.ok(calls.every(call => call.method === 'GET'));
  }, timers);
});

test('welcome resumes only an explicitly selected checked saved setup and preserves the unsent draft', async () => {
  for (const scenario of ['empty', 'only-current', 'failed', 'ready', 'review', 'question']) {
    await mounted(scenario, async ({ document, a, b, calls, values, list, selection, finish }) => {
      assert.ok(document.querySelector('.setup-welcome'));
      assert.equal(document.querySelector('.forge-welcome-resume') === null, true, 'A pending list must not pretend a resume is available.');
      const rows = scenario === 'empty' ? [] : scenario === 'only-current' ? [{ id: a.id, name: 'Blank saved start', status: 'question' }]
        : [{ id: a.id, name: 'Blank saved start', status: 'question' },
          { id: b.id, name: 'Existing observing setup', status: scenario },
          { id: 'resume-later-c', name: 'Later server-ordered setup', status: 'ready' }];
      await finish(list, scenario === 'failed' ? response({ detail: 'Inert saved-list read failed.' }, 503) : response({ setups: rows }));
      assert.equal(values.get(GUIDE_KEY), a.id);
      assert.equal(calls.some(call => call.url === `/api/guide/${b.id}`), false, 'Reading the list never selects its first row.');
      const card = document.querySelector('.forge-welcome-resume');
      if (['empty', 'only-current', 'failed'].includes(scenario)) {
        assert.equal(card === null, true, 'Empty, current-only or failed reads offer no resume.'); return;
      }
      assert.ok(card, 'A checked existing setup has an explicit welcome resume affordance.');
      assert.equal(card.querySelector('strong').textContent, 'Existing observing setup', 'Use the first actual server-ordered other row.');
      assert.equal(card.querySelector('small').textContent, scenario === 'ready' ? 'Accepted plan' : scenario === 'review' ? 'Design ready to review' : 'Design in progress');
      assert.equal(card.querySelector('button').textContent.trim(), 'Open conversation');
      assert.equal(card.querySelector('button').getAttribute('aria-label'), 'Continue Existing observing setup');
      if (scenario !== 'question') return;
      const raw = ' \tKeep my new idea separate from the saved observing setup.\n\u00e5  ';
      const textarea = document.querySelector('#setup-message');
      await act(async () => {
        Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value').set.call(textarea, raw);
        textarea.dispatchEvent(new document.defaultView.Event('input', { bubbles: true })); await settle();
      });
      assert.equal(JSON.parse(values.get(inputKey(a))).draft, raw);
      await click(button(document, 'Guide settings'));
      assert.equal(document.querySelector('.forge-welcome-resume') === null, true, 'Model settings have their own first-minute action.');
      await click(button(document, 'Back to my idea'));
      assert.equal(document.querySelector('#setup-message').value, raw);
      await click(document.querySelector('.forge-welcome-resume button'));
      assert.equal(calls.filter(call => call.url === `/api/guide/${b.id}`).length, 1);
      assert.equal(document.querySelector('#setup-send').disabled, true, 'Existing navigation owns pending submission admission.');
      assert.equal(document.querySelector('.forge-welcome-resume') === null, true, 'Pending selection cannot offer another resume.');
      assert.equal(JSON.parse(values.get(inputKey(a))).draft, raw, 'Navigation retains the exact unsent draft through its existing key.');
      await finish(selection, response(structuredClone(b)));
      assert.equal(values.get(GUIDE_KEY), b.id);
      assert.equal(document.querySelector('#setup-message').value, '', 'The saved setup never receives the other idea.');
      assert.equal(document.querySelector('.forge-welcome-resume') === null, true);
      await click(button(document, 'Your setups'));
      const original = [...document.querySelectorAll('.setup-history-menu button')]
        .find(item => item.querySelector('span')?.textContent === 'Blank saved start');
      await click(original);
      assert.equal(values.get(GUIDE_KEY), a.id);
      assert.equal(document.querySelector('#setup-message').value, raw, 'Returning restores the retained draft without sending it.');
      assert.ok(calls.every(call => call.method === 'GET'));
    });
  }
});


const workplace = { schema_version: 1, id: 'workplace', version: '1.0.0', label: 'Example workplace',
  starting_examples: [{ id: 'triage', title: 'Find work that needs attention', outcome: 'A clear weekly view.',
    initial_brief: 'Help our team find the commitments that need attention. Ask who it is for.' }] };

for (const narrow of [false, true]) test(`organization opening replaces stock examples and sends only after explicit Send (${narrow ? 'phone' : 'desktop'})`, async () => {
  await mounted('empty', async ({ document, calls, list, finish }) => {
    await finish(list, response({ setups: [] }));
    assert.ok(document.querySelector('.setup-welcome'));
    assert.equal(document.querySelectorAll('.forge-organization-start').length, 1);
    assert.doesNotMatch(document.body.textContent, /household|school dates|Run a club/);
    assert.equal(document.querySelector('.forge-example-disclosure') === null, true, '.forge-example-disclosure stays absent');
    const pick = document.querySelector('[aria-label="Use Find work that needs attention"]');
    await click(pick);
    const editor = document.querySelector('#setup-message');
    assert.equal(editor.value, workplace.starting_examples[0].initial_brief);
    assert.ok(document.activeElement === editor, `Expected composer focus; got ${document.activeElement?.tagName} ${document.activeElement?.id}`);
    assert.equal(calls.filter(call => call.method !== 'GET').length, 0);
    await click(document.querySelector('#setup-send'));
    const writes = calls.filter(call => call.method !== 'GET');
    assert.equal(writes.length, 1);
    assert.equal(writes[0].body.text, workplace.starting_examples[0].initial_brief);
    assert.equal(editor.value, '', 'Accepted Send clears the normal composer.');
  }, {}, { narrow, available: true, allowSend: true, organization: workplace });
});

test('organization examples preserve existing writing and custom idea focus', async () => {
  const own = 'Keep my own unusual idea exactly.  ';
  await mounted('empty', async ({ document, calls, list, finish }) => {
    await finish(list, response({ setups: [] }));
    const editor = document.querySelector('#setup-message');
    await click(document.querySelector('[aria-label="Preview Find work that needs attention"]'));
    assert.equal(editor.value, own);
    await click(button(document, 'Keep my idea'));
    assert.ok(document.activeElement === editor, `Expected composer focus; got ${document.activeElement?.tagName} ${document.activeElement?.id}`);
    assert.equal(editor.value, own);
    await click(document.querySelector('[aria-label="Preview Find work that needs attention"]'));
    await click(button(document, 'Add inspiration to my idea'));
    assert.equal(editor.value, own + '\n\n' + workplace.starting_examples[0].initial_brief);
    await click(document.querySelector('.forge-organization-start__header > button'));
    assert.ok(document.activeElement === editor, `Expected composer focus; got ${document.activeElement?.tagName} ${document.activeElement?.id}`);
    assert.equal(calls.filter(call => call.method !== 'GET').length, 0);
  }, {}, { organization: workplace, startingInput: { draft: own, mode: 'answer' } });
});

test('organization with no examples keeps custom input and does not restore stock examples', async () => {
  await mounted('empty', async ({ document, list, finish }) => {
    await finish(list, response({ setups: [] }));
    assert.equal(document.querySelectorAll('.forge-organization-start .setup-starting-points > button').length, 0);
    assert.doesNotMatch(document.body.textContent, /household|school dates|Run a club|Choose one|An editable starting point/);
    await click(document.querySelector('.forge-organization-start__header > button'));
    assert.ok(document.activeElement === document.querySelector('#setup-message'));
  }, {}, { organization: { ...workplace, starting_examples: [] } });
});


test('organization opening drops saved stock context while preserving and sending the raw draft', async () => {
  const draft = '  Discover projects I might care about.  ';
  await mounted('empty', async ({ document, a, calls, values, list, finish }) => {
    await finish(list, response({ setups: [] }));
    assert.equal(document.querySelector('.forge-starting-context') === null, true, '.forge-starting-context stays absent');
    assert.doesNotMatch(document.querySelector('#setup-message').placeholder, /For example:/);
    assert.doesNotMatch(document.body.textContent, /For a team, help with repetitive work/);
    assert.equal(document.querySelector('#setup-message').value, draft);
    assert.equal(JSON.parse(values.get(inputKey(a))).starting_help, undefined);
    await click(document.querySelector('#setup-send'));
    const writes = calls.filter(call => call.method !== 'GET');
    assert.equal(writes.length, 1);
    assert.equal(writes[0].body.text, draft);
  }, {}, { available: true, allowSend: true, organization: workplace,
    startingInput: { draft, mode: 'answer', selected: [],
      starting_help: { version: 2, audience_id: 'team', kind_id: 'repetitive' } } });
});
