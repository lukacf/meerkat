import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';

const repository = process.env.FORGE_STARTUP_TEST_REPO_ROOT ?? new URL('../../', import.meta.url).pathname;
const hostSource = process.env.FORGE_STARTUP_HOST_SOURCE ?? `${repository}/installer/launchkit/templates_v2/console_setup.mjs`;
const toolkitSource = `${repository}/installer/launchkit/templates_v2/toolkit-ui`;
const { build } = createRequire(pathToFileURL(`${repository}/installer/web/package.json`))('esbuild');
const { JSDOM } = createRequire(pathToFileURL(`${repository}/console/package.json`))('jsdom');
const { createConsoleSetupPort, mountConsoleSetup } = await import(pathToFileURL(hostSource));
// The existing acquired view is bundled in memory only. No native host or
// alternate presenter is constructed, and no browser storage is used.
const bundled = await build({ entryPoints: [`${toolkitSource}/index.js`], bundle: true, write: false, format: 'esm', platform: 'browser', logLevel: 'silent' });
const { mountSetupConversation, SetupConversationError } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].contents).toString('base64')}`);
const flush = async () => { for (let index = 0; index < 15; index++) await new Promise(resolve => setImmediate(resolve)); };
const deferred = () => { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; };
const idle = () => ({ available: true, can_send: true, can_stop: false, outcome_uncertain: false, active_send: null, last_completion: null });
const response = (body, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => structuredClone(body) });
const refusal = code => response({ error: { code } }, code === 'unsupported_presenter' ? 501 : 503);

function fixture() {
  const dom = new JSDOM('<main>Existing Console</main><div id="setup"></div>', { url: 'http://127.0.0.1:4510/console' });
  for (const key of ['localStorage', 'sessionStorage']) Object.defineProperty(dom.window, key, { get() { throw new Error('Browser storage is forbidden'); } });
  const requests = [], sources = [], overrides = new Map(), timers = new Map();
  let timer = 0;
  const host = { scope: 'startup-host-scope', state: idle() };
  const ready = () => response({ status: 'ready', state: { scope: host.scope, kind: 'state', state: host.state }, capabilities: {} });
  class Events {
    constructor(url) { this.url = url; this.closed = false; sources.push(this); }
    close() { this.closed = true; }
    emit(item) { this.onmessage?.({ data: JSON.stringify(item) }); }
  }
  const port = createConsoleSetupPort({
    fetch: async (url, options) => {
      const path = url.slice('/console/forge/connect'.length);
      const call = { path, method: options.method, credentials: options.credentials, redirect: options.redirect, ...(options.body ? { body: JSON.parse(options.body) } : {}) };
      requests.push(call);
      if (overrides.has(path)) return overrides.get(path)(call);
      if (path === '/initialize' || path === '/state') return ready();
      if (path === '/history') return response({ history: { messages: [], offset: 0, total: 0, hasMore: false }, state: host.state });
      throw new Error(`Unexpected offline route ${path}`);
    }, EventSource: Events, SetupConversationError,
    schedule: callback => { timers.set(++timer, callback); return timer; }, unschedule: id => timers.delete(id),
  });
  const container = dom.window.document.querySelector('#setup');
  const mount = () => mountConsoleSetup(container, { port, mountSetupConversation });
  const emit = item => sources.findLast(source => !source.closed)?.emit(item);
  const check = () => [...container.querySelectorAll('button')].find(node => node.textContent === 'Check current status').click();
  const status = () => container.querySelector('.forge-setup__status-text').textContent;
  const posts = () => requests.filter(call => call.method === 'POST');
  const stateReads = () => requests.filter(call => call.path === '/state').length;
  const assertOneInitialize = () => assert.deepEqual(posts(), [{ path: '/initialize', method: 'POST', credentials: 'same-origin', redirect: 'error', body: {} }]);
  return { dom, host, port, requests, sources, overrides, timers, container, mount, emit, check, status, posts, stateReads, assertOneInitialize, ready };
}

// These are host-shaped inert responses, not native initialization evidence.
// Every mutation is logged by the strict offline transport.
test('explicit startup refusal is readable, stops passive observation and recovers only through a valid current GET', async () => {
  const reasons = [
    ['native_auth_required', /needs native authentication.*Signing in to the Console does not provide this authentication.*Forge/],
    ['required_context_missing', /missing required application context.*Forge/],
    ['unsupported_presenter', /runtime does not support the setup conversation.*Forge/],
  ];
  for (const path of ['/initialize', '/state']) for (const [code, text] of reasons) {
    const f = fixture();
    if (path === '/initialize') {
      f.overrides.set('/initialize', () => refusal(code));
      f.overrides.set('/state', () => refusal(code));
    }
    else {
      f.overrides.set('/initialize', () => response({ status: 'starting' }, 202));
      f.overrides.set('/state', () => refusal(code));
    }
    let view = f.mount();
    try {
      await flush();
      assert.match(f.status(), text, `${path} must show the exact host refusal rather than perpetual opening`);
      assert.match(f.container.querySelector('.forge-setup__footer').textContent, /Checking status does not restart the agent/);
      assert.equal(f.port.snapshot.startupRefusal, code);
      assert.equal(f.port.available, false);
      assert.equal(f.container.querySelector('.toolkit-conversation') === null, true);
      assert.equal(f.timers.size, 0, 'A refused original initialization must not schedule passive GETs');
      assert.equal(f.sources.filter(source => !source.closed).length, 0, 'Private observation must stop for the refused original initialization');
      assert.doesNotMatch(f.container.querySelector('.forge-setup__footer').textContent, /leaves the setup agent running/);
      f.assertOneInitialize();
      if (path === '/initialize') assert.equal(f.stateReads(), 0, 'An explicit initialization refusal must not start a passive state read');
      const before = f.stateReads();
      f.check(); await flush();
      assert.equal(f.stateReads(), before + 1, 'The existing status control must remain GET-only');
      assert.equal(f.port.snapshot.startupRefusal, code);
      assert.equal(f.timers.size, 0);
      f.assertOneInitialize();
      // Neither generic failure nor malformed or still-starting observation
      // clears a retained exact startup refusal.
      f.overrides.set('/state', () => refusal('unavailable')); f.check(); await flush();
      assert.equal(f.port.snapshot.startupRefusal, code);
      f.overrides.set('/state', () => response({ status: 'ready', state: { scope: f.host.scope, kind: 'state', state: {} } })); f.check(); await flush();
      assert.equal(f.port.snapshot.startupRefusal, code);
      f.overrides.set('/state', () => response({ status: 'starting' }, 202)); f.check(); await flush();
      assert.equal(f.port.snapshot.startupRefusal, code);
      assert.match(f.status(), text);
      const oldSources = [...f.sources];
      view.dispose(); view = f.mount(); await flush();
      assert.equal(f.port.snapshot.startupRefusal, code);
      f.assertOneInitialize();
      for (const source of oldSources) source.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: idle() } });
      await flush(); assert.equal(f.port.snapshot.startupRefusal, code, 'Closed observers cannot clear the presentation refusal');
      f.overrides.delete('/state'); f.check(); await flush();
      assert.equal(f.port.snapshot.startupRefusal, null);
      assert.equal(f.port.available, true);
      assert.equal(f.port.canSend, true);
      assert.equal(f.port.canStop, false);
      assert.equal(f.status(), 'Ready when you are');
      assert.ok(f.container.querySelector('.toolkit-conversation'), 'A valid current host state remounts the actual acquired conversation');
      assert.equal(f.sources.filter(source => !source.closed).length, 1);
      assert.equal(f.timers.size, 0);
      // This error name on a history route must remain a history read failure,
      // never reinterpret an initialized application as a refused startup.
      f.overrides.set('/history', () => refusal(code));
      await assert.rejects(f.port.history(), error => error.code === 'history_unavailable');
      assert.equal(f.port.snapshot.startupRefusal, null);
      assert.equal(f.port.available, true);
      f.assertOneInitialize();
    } finally { view.dispose(); f.dom.window.close(); }
  }
});

test('unknown and network startup failures remain transient and pre-session provider prompts remain recoverable', async () => {
  for (const initial of [() => { throw new Error('Offline specimen'); }, () => refusal('unavailable'), () => refusal('future_unknown_code')]) {
    const f = fixture(), target = { initialize_request_id: 'h:provider-start' };
    const binding = () => ({ type: 'binding', protocol_version: 3, target: structuredClone(target) });
    f.overrides.set('/initialize', initial);
    f.overrides.set('/state', () => refusal('unavailable'));
    const view = f.mount();
    try {
      await flush();
      assert.match(f.status(), /Opening the setup conversation/);
      assert.equal(f.port.snapshot.startupRefusal ?? null, null);
      assert.equal(f.timers.size, 1);
      assert.equal(f.sources.filter(source => !source.closed).length, 1);
      f.emit(binding());
      f.emit({ type: 'prompt', protocol_version: 3, target: structuredClone(target), id: 'p:provider-start', prompt: {
        kind: 'provider_action', target: { model: 'inert-model', provider: 'inert-provider', realm: 'inert-realm', binding: 'inert-binding' }, actions: ['enter_key'],
      } }); await flush();
      assert.equal(f.status(), 'Complete the private provider setup to continue.');
      assert.match(f.container.querySelector('[aria-label="Protected setup decision"]').textContent, /Connect this provider/);
      assert.ok([...f.container.querySelectorAll('button')].some(node => node.textContent === 'Enter an API key privately' && !node.disabled));
      target.scope = f.host.scope; delete target.initialize_request_id;
      f.overrides.set('/state', () => response({ status: 'ready', binding: binding(), state: { target: structuredClone(target), kind: 'state', state: { ...idle(), private_notice_pending: false } } }));
      // Run the already scheduled read rather than initialize the host again.
      const [id, tick] = f.timers.entries().next().value; f.timers.delete(id); tick(); await flush();
      assert.equal(f.port.available, true);
      assert.equal(f.status(), 'Ready when you are');
      assert.equal(f.timers.size, 0);
      f.assertOneInitialize();
      assert.equal(f.requests.some(call => ['/answer', '/send', '/cancel'].includes(call.path)), false);
    } finally { view.dispose(); f.dom.window.close(); }
  }
});

test('late refusal from an older startup read cannot replace a newer bound observation', async () => {
  const f = fixture(), held = deferred();
  f.overrides.set('/initialize', () => response({ status: 'starting' }, 202));
  f.overrides.set('/state', () => held.promise);
  const view = f.mount();
  try {
    await flush(); assert.equal(f.stateReads(), 1);
    f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: idle() } }); await flush();
    assert.equal(f.port.available, true);
    held.resolve(refusal('native_auth_required')); await flush();
    assert.equal(f.port.snapshot.startupRefusal ?? null, null);
    assert.equal(f.port.available, true);
    assert.equal(f.status(), 'Ready when you are', 'A stale refused read cannot change a newer current observation');
    f.overrides.delete('/state'); f.check(); await flush();
    assert.equal(f.port.canSend, true);
    f.assertOneInitialize();
    assert.equal(f.requests.some(call => ['/answer', '/send', '/cancel'].includes(call.path)), false);
  } finally { view.dispose(); f.dom.window.close(); }
});


test('generic result failure after a successful state read retains initialized reconnecting recovery', async () => {
  const f = fixture(), view = f.mount(), id = 'h:original-send';
  const resultPath = `/result/${encodeURIComponent(id)}`;
  try {
    await flush();
    f.overrides.set('/send', call => {
      f.host.state = { ...idle(), can_send: false, can_stop: true,
        active_send: { send_request_id: id, cancellation_requested: false } };
      return response({ association: call.body.association, request_id: id, status: 'pending' }, 202);
    });
    f.overrides.set(resultPath, () => refusal('unavailable'));
    // One inert original send follows the real port admission path. The GET
    // state observation then succeeds before the original result read fails.
    void f.port.send('Offline original request').catch(() => {}); await flush();
    assert.equal(f.requests.filter(call => call.path === resultPath).length, 1);
    assert.equal(f.port.snapshot.connection, 'reconnecting', 'A generic result failure must retain the pre-existing initialized recovery state');
    assert.equal(f.port.snapshot.startupRefusal, null);
    assert.equal(f.port.available, true);
    assert.equal(f.port.canStop, true);
    assert.equal(f.timers.size, 1);
    const posts = f.posts().map(call => call.path);
    assert.deepEqual(posts, ['/initialize', '/send']);
    f.overrides.set(resultPath, () => response({ request_id: id, status: 'pending' }));
    f.check(); await flush();
    assert.equal(f.port.snapshot.connection, 'ready');
    assert.equal(f.port.snapshot.startupRefusal, null);
    assert.deepEqual(f.posts().map(call => call.path), posts, 'Reading the original result cannot repeat either mutation');
    assert.equal(f.requests.some(call => ['/answer', '/cancel'].includes(call.path)), false);
  } finally { view.dispose(); f.dom.window.close(); }
});
