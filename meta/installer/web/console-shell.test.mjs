import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { build } from 'esbuild';
import { mountConsoleShell } from '../launchkit/templates_v2/console_shell.mjs';
import { mountConsoleSetup, createConsoleSetupPort } from '../launchkit/templates_v2/console_setup.mjs';

const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const nativeHTML = await readFile(new URL('../tests/fixtures/console_shell/native-console.html', import.meta.url), 'utf8');
const toolkit = new URL('../launchkit/templates_v2/toolkit-ui/index.js', import.meta.url);
const bundled = await build({ entryPoints: [toolkit.pathname], bundle: true, write: false, format: 'esm', platform: 'browser', logLevel: 'silent' });
const { mountSetupConversation } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].contents).toString('base64')}`);
const flush = async () => { for (let i = 0; i < 15; i++) await new Promise(resolve => setImmediate(resolve)); };
function fixture(narrow = false, width = null, setup = true) {
  const dom = new JSDOM(nativeHTML, { url: 'http://127.0.0.1:4510/console' });
  const doc = dom.window.document, native = doc.querySelector('#root'), calls = [], streams = [], mediaListeners = new Set();
  const media = { matches: narrow, addEventListener: (_, listener) => mediaListeners.add(listener), removeEventListener: (_, listener) => mediaListeners.delete(listener) };
  const mediaChange = value => { media.matches = value; for (const listener of mediaListeners) listener(); };
  let widthLimit = null;
  const resizeWidth = value => { assert.ok(Number.isFinite(widthLimit)); mediaChange(value <= widthLimit); };
  const state = { available: true, can_send: true, can_stop: false, outcome_uncertain: false, active_send: null, last_completion: null };
  class Events {
    constructor(url) { this.url = url; this.closed = false; streams.push(this); }
    close() { this.closed = true; }
    emit(item) { this.onmessage?.({ data: JSON.stringify(item) }); }
  }
  const port = createConsoleSetupPort({ fetch: async (url, options) => {
    calls.push({ url, body: options.body && JSON.parse(options.body), method: options.method });
    const value = url.endsWith('/history') || url.includes('/history?') ? { history: { messages: [], offset: 0, total: 0, hasMore: false }, state }
      : { status: 'ready', state: { scope: 'fixture-private-scope', kind: 'state', state }, capabilities: {} };
    return { ok: true, status: 200, json: async () => value };
  }, EventSource: Events, schedule: () => 1, unschedule: () => {} });
  const shell = mountConsoleShell({ document: doc, matchMedia: query => {
    const boundary = /^\(max-width: ([0-9]+)px\)$/.exec(query); assert.ok(boundary);
    widthLimit = Number(boundary[1]); if (width !== null) media.matches = width <= widthLimit;
    return media;
  }, ...(setup === true ? { mountConsoleSetup, mountSetupConversation } : setup || {}), setupOptions: { port } });
  const open = doc.querySelector('.forge-console-shell__open'), drawer = doc.querySelector('.forge-console-shell__drawer');
  return { dom, doc, native, calls, streams, media, mediaListeners, mediaChange, resizeWidth, shell, open, drawer };
}

test('base authenticated shell shows ordinary sign-out without setup modules or requests', () => {
  const f = fixture(false, null, false);
  try {
    assert.equal(f.open === null, true); assert.equal(f.drawer === null, true);
    assert.equal(f.doc.querySelector('.forge-console-shell__hint') === null, true);
    const form = f.doc.querySelector('.forge-console-shell__toolbar form');
    assert.equal(form.method, 'post'); assert.equal(form.getAttribute('action'), '/logout');
    const button = form.querySelector('button');
    assert.equal(button.textContent, 'Sign out'); assert.equal(button.type, 'submit');
    assert.equal(form.elements.length, 1); assert.deepEqual([...new f.dom.window.FormData(form)], []);
    let observed = false;
    form.addEventListener('submit', event => { observed = !event.defaultPrevented && event.submitter === button; event.preventDefault(); });
    button.click(); assert.equal(observed, true);
    f.shell.open(); assert.equal(f.calls.length, 0); assert.equal(f.streams.length, 0);
    assert.equal(f.doc.querySelector('#root') === f.native, true);
    assert.equal(f.doc.querySelector('.forge-console-shell__native').hasAttribute('inert'), false);
    f.shell.dispose(); assert.equal(f.native.parentNode === f.doc.body, true);
  } finally { f.shell.dispose(); f.dom.window.close(); }
});

test('optional setup import failure leaves host sign-out and native Console usable', async () => {
  let loads = 0;
  const f = fixture(false, null, { loadSetup: async () => { loads++; throw new Error('Unavailable optional module fixture'); } });
  try {
    const form = f.doc.querySelector('.forge-console-shell__logout');
    assert.ok(form); assert.equal(loads, 0);
    f.open.focus(); f.open.click(); await flush();
    assert.equal(loads, 1); assert.match(f.drawer.textContent, /Setup could not be displayed/);
    assert.equal(f.doc.querySelector('.forge-console-shell__native').hasAttribute('inert'), false);
    f.drawer.querySelector('button').click();
    assert.equal(f.drawer.hidden, true); assert.equal(f.doc.activeElement === f.open, true);
    assert.equal(form.isConnected, true); assert.equal(form.querySelector('button').disabled, false);
    assert.equal(f.calls.length, 0); assert.equal(f.streams.length, 0);
  } finally { f.shell.dispose(); f.dom.window.close(); }
});

test('closing a loading setup cannot mount a late view or start its host requests', async () => {
  const pending = [];
  const f = fixture(true, null, { loadSetup: () => new Promise(resolve => pending.push(resolve)) });
  let mounts = 0;
  const selected = { mountSetupConversation, mountConsoleSetup: (...args) => { mounts++; return mountConsoleSetup(...args); } };
  try {
    f.open.focus(); f.open.click(); assert.equal(pending.length, 1);
    f.drawer.querySelector('button').click();
    assert.equal(f.drawer.hidden, true); assert.equal(f.doc.activeElement === f.open, true);
    f.open.click(); assert.equal(pending.length, 2);
    pending[0](selected); await flush();
    assert.equal(mounts, 0); assert.equal(f.calls.length, 0);
    pending[1](selected); await flush();
    assert.equal(mounts, 1); assert.ok(f.drawer.querySelector('.toolkit-conversation'));
    assert.equal(f.calls.filter(call => call.url.endsWith('/initialize')).length, 1);
    assert.equal(f.doc.querySelector('#root') === f.native, true);
    f.shell.close(); assert.equal(f.streams.every(stream => stream.closed), true);
    assert.equal(f.doc.querySelector('.forge-console-shell__logout').isConnected, true);
  } finally { f.shell.dispose(); f.dom.window.close(); }
});

test('disposing during optional module loading prevents a late setup mount', async () => {
  let resolve, mounts = 0;
  const f = fixture(false, null, { loadSetup: () => new Promise(done => { resolve = done; }) });
  try {
    f.open.click(); f.shell.dispose();
    resolve({ mountSetupConversation, mountConsoleSetup: () => { mounts++; throw new Error('Late mount must not run'); } });
    await flush(); assert.equal(mounts, 0); assert.equal(f.calls.length, 0);
    assert.equal(f.native.parentNode === f.doc.body, true); assert.equal(f.doc.querySelector('.forge-console-shell') === null, true);
  } finally { f.shell.dispose(); f.dom.window.close(); }
});

test('captured native root and IIFE are retained; closed shell does not initialize Toolkit', () => {
  const f = fixture();
  assert.equal(createHash('sha256').update(nativeHTML).digest('hex'), '6ba5c7a5f6f1786a7fcc2e01060b5adc5cc4a561e016c0238ccdbe77d65d4a21');
  assert.equal(f.doc.querySelector('#root') === f.native, true);
  assert.equal(f.doc.querySelector('script').getAttribute('src'), '/console/assets/console-app.js?v=250bc27b79ec');
  assert.equal(f.drawer.hidden, true); assert.equal(f.calls.length, 0);
  assert.equal(f.doc.querySelector('.forge-console-shell__native').getAttribute('inert'), null);
  f.shell.dispose(); assert.equal(f.native.parentNode === f.doc.body, true); f.dom.window.close();
});

test('wide sibling mounts actual Toolkit view without inerting or changing native controls', async () => {
  const f = fixture();
  assert.equal(f.open.textContent, 'Set up your app');
  const nativeButton = f.doc.createElement('button'); nativeButton.textContent = 'Existing native action'; f.native.append(nativeButton);
  f.open.focus(); f.open.click(); await flush();
  assert.equal(f.drawer.getAttribute('role'), 'complementary'); assert.equal(f.drawer.hasAttribute('aria-modal'), false);
  assert.equal(f.doc.querySelector('.forge-console-shell__native').hasAttribute('inert'), false);
  assert.ok(f.drawer.querySelector('.toolkit-conversation')); assert.equal(f.native.contains(f.drawer), false);
  assert.equal(f.drawer.querySelector('h1').textContent, 'Set up your app');
  assert.ok(f.drawer.querySelector('.forge-setup__help').textContent.includes('options this app supports'));
  nativeButton.focus(); assert.equal(f.doc.activeElement === nativeButton, true);
  assert.equal(f.calls.filter(call => call.url.endsWith('/initialize')).length, 1);
  f.shell.close(); assert.equal(f.doc.activeElement === f.open, true); assert.equal(f.streams.every(stream => stream.closed), true);
  f.shell.open(); await flush(); assert.equal(f.calls.filter(call => call.url.endsWith('/initialize')).length, 1);
  assert.equal(f.calls.some(call => ['/cancel', '/close', '/answer', '/send'].some(path => call.url.endsWith(path))), false);
  f.shell.dispose(); f.dom.window.close();
});

test('small modal traps keyboard and background, Escape restores focus and owned attributes', async () => {
  const f = fixture(true);
  const outside = f.doc.createElement('section'); outside.setAttribute('aria-hidden', 'false'); outside.setAttribute('inert', 'preexisting'); f.doc.body.append(outside);
  f.open.focus(); f.open.click(); await flush();
  assert.equal(f.drawer.getAttribute('role'), 'dialog'); assert.equal(f.drawer.getAttribute('aria-modal'), 'true');
  assert.equal(f.doc.querySelector('.forge-console-shell__native').hasAttribute('inert'), true);
  assert.equal(f.doc.querySelector('.forge-console-shell__toolbar').hasAttribute('inert'), true);
  const controls = [...f.drawer.querySelectorAll('button:not(:disabled),textarea:not(:disabled)')];
  controls.at(-1).focus(); controls.at(-1).dispatchEvent(new f.dom.window.KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }));
  assert.equal(f.doc.activeElement === controls[0], true);
  controls[0].dispatchEvent(new f.dom.window.KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true, cancelable: true }));
  assert.equal(f.doc.activeElement === controls.at(-1), true);
  f.open.focus(); assert.equal(f.drawer.contains(f.doc.activeElement), true);
  f.doc.activeElement.dispatchEvent(new f.dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  assert.equal(f.drawer.hidden, true); assert.equal(f.doc.activeElement === f.open, true);
  assert.equal(f.doc.querySelector('.forge-console-shell__native').hasAttribute('inert'), false);
  assert.equal(outside.getAttribute('inert'), 'preexisting'); assert.equal(outside.getAttribute('aria-hidden'), 'false');
  assert.equal(f.calls.some(call => call.url.endsWith('/cancel')), false);
  f.shell.dispose(); f.dom.window.close();
});

test('resize changes geometry without remounting private view or restarting host work', async () => {
  const f = fixture(); f.open.click(); await flush();
  const view = f.drawer.querySelector('.forge-setup');
  f.mediaChange(true); assert.equal(f.drawer.getAttribute('aria-modal'), 'true');
  f.mediaChange(false); assert.equal(f.drawer.hasAttribute('aria-modal'), false);
  assert.equal(f.drawer.querySelector('.forge-setup') === view, true);
  assert.equal(f.calls.filter(call => call.url.endsWith('/initialize')).length, 1);
  f.shell.dispose(); assert.equal(f.mediaListeners.size, 0); assert.equal(f.native.parentNode === f.doc.body, true); f.dom.window.close();
});

test('closing modal clears private password presentation, preserving native DOM on disposal', async () => {
  const f = fixture(true); f.open.click(); await flush();
  f.streams.find(source => source.url.endsWith('/private')).emit({ type: 'prompt', id: 'p:private', scope: 'fixture-private-scope', prompt: { kind: 'forge_password', origin: 'http://127.0.0.1:4510', mode: 'initial', min_chars: 12, max_chars: 4096 } });
  const password = f.drawer.querySelector('input'); password.value = 'never-submitted-private-fixture';
  f.shell.dispose(); assert.equal(password.value, ''); assert.equal(f.doc.querySelector('input') === null, true);
  assert.equal(f.native.parentNode === f.doc.body, true); assert.equal(f.doc.body.classList.contains('forge-console-shell-page'), false);
  assert.equal(f.calls.some(call => call.url.endsWith('/answer') || call.url.endsWith('/cancel')), false);
  f.dom.window.close();
});

test('a second shell mount reuses presentation rather than duplicating native root or toolbar', () => {
  const f = fixture();
  assert.equal(mountConsoleShell({ document: f.doc, mountConsoleSetup, mountSetupConversation }), f.shell);
  assert.equal(f.doc.querySelectorAll('#root').length, 1); assert.equal(f.doc.querySelectorAll('.forge-console-shell__toolbar').length, 1);
  f.shell.dispose(); f.shell.dispose(); f.dom.window.close();
});

test('mounted sign-out submits through host logout as a plain POST form', () => {
  const f = fixture();
  try {
    const form = f.doc.querySelector('.forge-console-shell__toolbar form');
    assert.ok(form, 'The authenticated Console needs a visible host sign-out form.');
    assert.equal(form.method, 'post'); assert.equal(form.getAttribute('action'), '/logout');
    assert.equal(form.action, 'http://127.0.0.1:4510/logout'); assert.equal(form.target, '');
    const button = form.querySelector('button');
    assert.equal(button.type, 'submit'); assert.equal(button.textContent, 'Sign out'); assert.equal(button.disabled, false);
    assert.equal(form.elements.length, 1); assert.deepEqual([...new f.dom.window.FormData(form)], []);
    let submission = null;
    form.addEventListener('submit', event => {
      submission = { submitter: event.submitter, defaultPrevented: event.defaultPrevented };
      // This fixture suppresses navigation only after observing native form submission.
      event.preventDefault();
    }, { once: true });
    button.click();
    assert.deepEqual(submission, { submitter: button, defaultPrevented: false });
    assert.equal(f.calls.length, 0); assert.equal(f.streams.length, 0);
    f.shell.dispose(); assert.equal(form.isConnected, false); assert.equal(f.native.parentNode === f.doc.body, true);
  } finally { f.shell.dispose(); f.dom.window.close(); }
});

test('1439px uses a modal and 1440px restores native space without restarting setup', async () => {
  const f = fixture(false, 1439);
  try {
    const nativeArea = f.doc.querySelector('.forge-console-shell__native');
    const toolbar = f.doc.querySelector('.forge-console-shell__toolbar');
    const nativeButton = f.doc.createElement('button'); nativeButton.textContent = 'Existing native action'; f.native.append(nativeButton);
    f.open.focus(); f.open.click(); await flush();
    assert.equal(f.drawer.getAttribute('role'), 'dialog'); assert.equal(f.drawer.getAttribute('aria-modal'), 'true');
    assert.equal(nativeArea.hasAttribute('inert'), true); assert.equal(toolbar.hasAttribute('inert'), true);
    const view = f.drawer.querySelector('.forge-setup'); assert.ok(view);
    f.resizeWidth(1440);
    assert.equal(f.drawer.getAttribute('role'), 'complementary'); assert.equal(f.drawer.hasAttribute('aria-modal'), false);
    assert.equal(nativeArea.hasAttribute('inert'), false); assert.equal(toolbar.hasAttribute('inert'), false);
    nativeButton.focus(); assert.equal(f.doc.activeElement === nativeButton, true);
    assert.equal(f.drawer.querySelector('.forge-setup') === view, true); assert.equal(f.doc.querySelector('#root') === f.native, true);
    f.resizeWidth(1439);
    assert.equal(f.drawer.getAttribute('aria-modal'), 'true'); assert.equal(nativeArea.hasAttribute('inert'), true);
    nativeButton.focus(); assert.equal(f.drawer.contains(f.doc.activeElement), true);
    assert.equal(f.drawer.querySelector('.forge-setup') === view, true);
    assert.equal(f.calls.filter(call => call.url.endsWith('/initialize')).length, 1);
    f.shell.close(); assert.equal(f.doc.activeElement === f.open, true);
    assert.equal(nativeArea.hasAttribute('inert'), false); assert.equal(toolbar.hasAttribute('inert'), false);
    assert.equal(f.calls.some(call => ['/cancel', '/close', '/answer', '/send'].some(path => call.url.endsWith(path))), false);
  } finally { f.shell.dispose(); f.dom.window.close(); }
});
