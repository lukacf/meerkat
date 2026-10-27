import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act } from 'react';
import { GUIDE_KEY } from './guided-state.mjs';
import { inputKey } from './wizard-state.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const webRoot = fileURLToPath(new URL('.', import.meta.url));
const compiled = await build({ entryPoints: [`${webRoot}/guided.jsx`], bundle: true, write: false,
  format: 'esm', platform: 'node', logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [`${webRoot}/node_modules`],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'shared-real-react', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { GuidedSetup } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString('base64')}`);
const settle = () => new Promise(resolve => setTimeout(resolve, 5));
const response = value => ({ ok: true, status: 200, json: async () => value });
const guide = { id: 'offline-selected-answer', version: 5, status: 'question', stage: 'purpose',
  completed_stages: [], answers: [], brief: { facts: {}, requirements: [] }, capabilities: { available: true },
  messages: [{ id: 'job', role: 'user', text: 'Help us keep track of household plans.' }],
  question: { id: 'context', stage: 'purpose', title: 'Which context should it remember?', multiple: false, allow_text: true,
    options: [{ id: 'personal', label: 'Personal context', description: 'Each person has their own history.' },
      { id: 'shared', label: 'Shared context', description: 'A shared household record.' }] } };

async function mounted(input, verify, { state = guide, onPost } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const calls = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true, requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    fetch: async (url, options = {}) => {
      const call = { url, method: options.method || 'GET', body: options.body && JSON.parse(options.body) };
      calls.push(call);
      if (call.method === 'POST') {
        assert.equal(url, `/api/guide/${state.id}/respond`, 'Only a deliberate composer send may post to this offline guide.');
        assert.ok(onPost, 'Selection and removal must not send anything.');
        return onPost(call);
      }
      if (url === `/api/guide/${state.id}`) return response(structuredClone(state));
      if (url === '/api/guide/list') return response({ setups: [] });
      if (url.includes('/timeline?')) return response({ available: true, frames: [], bindings: [], exhausted: true });
      throw new Error(`Unexpected offline request: ${call.method} ${url}`);
    } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  window.localStorage.setItem(GUIDE_KEY, state.id);
  window.localStorage.setItem(inputKey(state), JSON.stringify({ key: inputKey(state), selected: [], draft: '', mode: 'answer', ...input }));
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(document.getElementById('root'));
  const editor = () => document.querySelector('#setup-message');
  const button = label => {
    const node = [...document.querySelectorAll('button')].find(item => item.getAttribute('aria-label') === label || item.textContent.trim() === label);
    assert.ok(node, `Missing actual choice/composer action: ${label}`); return node;
  };
  const click = node => act(async () => { node.click(); await settle(); });
  const choose = id => click([...document.querySelectorAll('.setup-choice')].find(node => node.querySelector('strong')?.textContent === state.question.options.find(option => option.id === id).label));
  const edit = text => act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(editor(), text);
    editor().dispatchEvent(new dom.window.Event('input', { bubbles: true }));
    editor().dispatchEvent(new dom.window.Event('change', { bubbles: true }));
    await settle();
  });
  try {
    await act(async () => { root.render(React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null,
      onBlueprint() {}, config: { architect: { available: true, status: 'ready' } } })); await settle(); });
    await act(settle);
    await verify({ document, window, calls, button, click, choose, editor, edit,
      saved: () => JSON.parse(window.localStorage.getItem(inputKey(state))) });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test('selected answer labels retain selection order and removal preserves the exact draft owner and combination without sending', async () => {
  const draft = '  Keep private and shared information separate.\nAsk before sharing.  ';
  await mounted({ draft, combine_options: true }, async ({ document, calls, button, click, choose, editor, saved }) => {
    await choose('shared'); await choose('personal');
    const labels = () => [...document.querySelectorAll('.setup-selected-answer button')].map(node => node.getAttribute('aria-label'));
    assert.deepEqual(labels(), ['Remove Shared context from answer', 'Remove Personal context from answer']);
    assert.match(editor().placeholder, /optional/i);
    assert.equal(button('Send answer').disabled, false);
    assert.equal(Boolean(document.querySelector('.setup-choice-actions .setup-action')), false, 'The question itself cannot submit an answer.');
    await click(button('Remove Shared context from answer'));
    assert.deepEqual(labels(), ['Remove Personal context from answer']);
    assert.deepEqual(saved(), { key: inputKey(guide), selected: ['personal'], draft, mode: 'answer', combine_options: true });
    assert.equal(editor().value, draft);
    assert.equal(button('Send answer').disabled, true, 'An explicit combination still needs at least two choices.');
    assert.deepEqual(calls.filter(call => call.method === 'POST'), []);
  });
});

test('one composer send submits exact selected order, optional context and explicit combination', async () => {
  const text = 'Keep each person private, with a shared household record.';
  await mounted({ draft: text }, async ({ document, calls, button, click, choose }) => {
    await click(button('More than one fits')); await choose('shared'); await choose('personal');
    assert.equal(Boolean(document.querySelector('.setup-choice-actions .setup-action')), false);
    assert.match(document.querySelector('.setup-choice-caption').textContent, /Send answer below/);
    await click(button('Send answer'));
    const posts = calls.filter(call => call.method === 'POST');
    assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { version: guide.version, action: 'answer', selected: ['shared', 'personal'], text, combine_options: true });
  }, { onPost: call => response({ ...structuredClone(guide), version: guide.version + 1,
    operation: { status: 'completed', request_id: 'b'.repeat(64), version: guide.version, request: call.body } }) });
});

test('explicit chat hides held answer labels and sends only the chat while preserving those selections', async () => {
  const text = 'Please explain the privacy tradeoff before I decide.';
  await mounted({ selected: ['shared', 'personal'], combine_options: true, draft: text }, async ({ document, calls, button, click, saved }) => {
    await click(button('Ask or suggest a change'));
    assert.equal(document.querySelector('.setup-selected-answer') === null, true);
    await click(button('Send message'));
    const posts = calls.filter(call => call.method === 'POST');
    assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { version: guide.version, action: 'chat', selected: [], text });
    assert.deepEqual(saved().selected, ['shared', 'personal']);
    assert.equal(saved().combine_options, true);
  }, { onPost: call => response({ ...structuredClone(guide), version: guide.version + 1,
    operation: { status: 'completed', request_id: 'c'.repeat(64), version: guide.version, request: call.body } }) });
});

test('a late admitted answer keeps a newer explicit chat draft through a changed question', async () => {
  let resolveBody, entered = false;
  const body = new Promise(resolve => { resolveBody = resolve; });
  const nextText = '  Before the next answer, explain who sees the shared record.\nKeep this note.  ';
  try {
    await mounted({ selected: ['personal'], draft: 'Begin with private context.' }, async ({ document, calls, button, click, editor, edit, window }) => {
      await click(button('Send answer')); assert.equal(entered, true);
      await click(button('Ask or suggest a change')); await edit(nextText);
      assert.equal(editor().value, nextText);
      await act(async () => { resolveBody(); await settle(); });
      assert.equal(editor().value, nextText);
      assert.equal(button('Ask or suggest a change').getAttribute('aria-pressed'), 'true');
      assert.equal(document.querySelector('.setup-selected-answer') === null, true);
      const saved = JSON.parse(window.localStorage.getItem(inputKey({ ...guide, question: nextQuestion })));
      assert.equal(saved.draft, nextText); assert.equal(saved.mode, 'chat');
      assert.deepEqual(calls.filter(call => call.method === 'POST').map(call => call.body),
        [{ version: guide.version, action: 'answer', selected: ['personal'], text: 'Begin with private context.' }]);
    }, { onPost: call => ({ ok: true, status: 200, json: async () => { entered = true; await body;
      return { ...structuredClone(guide), version: guide.version + 1, question: nextQuestion,
        operation: { status: 'completed', request_id: 'd'.repeat(64), version: guide.version, request: call.body } }; } }) });
  } finally { resolveBody(); }
});
const nextQuestion = { ...guide.question, id: 'interfaces', title: 'How would you use it?',
  options: [{ id: 'conversation', label: 'A conversation', description: 'Ask when you need it.' }] };

test('an allowed free-text answer remains valid without selecting a card', async () => {
  const text = 'Something else: one private research project.';
  await mounted({ draft: text }, async ({ document, calls, button, click }) => {
    assert.equal(document.querySelector('.setup-selected-answer') === null, true);
    assert.equal(button('Send answer').disabled, false);
    await click(button('Send answer'));
    assert.deepEqual(calls.filter(call => call.method === 'POST').map(call => call.body),
      [{ version: guide.version, action: 'answer', selected: [], text }]);
  }, { onPost: call => response({ ...structuredClone(guide), version: guide.version + 1,
    operation: { status: 'completed', request_id: 'e'.repeat(64), version: guide.version, request: call.body } }) });
});
