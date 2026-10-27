import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile, access } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
const repository = process.env.FORGE_PENDING_TEST_REPO_ROOT ?? new URL('../../', import.meta.url).pathname;
const sourceRoot = process.env.FORGE_PENDING_SOURCE_ROOT ?? new URL('../launchkit/templates_v2/', import.meta.url).pathname;
const { build } = createRequire(pathToFileURL(`${repository}/installer/web/package.json`))('esbuild');
const { createConsoleSetupPort, mountConsoleSetup } = await import(pathToFileURL(`${sourceRoot}/console_setup.mjs`));

const { JSDOM } = createRequire(pathToFileURL(`${repository}/console/package.json`))('jsdom');
// Exercise the actual acquired public UI closure, not a checkout or port imitation.
// This in-memory bundle neither imports a native Session nor publishes assets.
const toolkitSource = `${`${sourceRoot}/toolkit-ui/`.replace(/\/$/, '')}/`;
const toolkitPresent = await access(`${toolkitSource}index.js`).then(() => true, () => false);
const bundled = toolkitPresent ? await build({ entryPoints: [`${toolkitSource}index.js`], bundle: true, write: false, format: 'esm', platform: 'browser', logLevel: 'silent' }) : null;
const { mountSetupConversation, SetupConversationError } = bundled ? await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].contents).toString('base64')}`) : {};
const focused = (name, run) => test(name, { skip: toolkitPresent ? false : 'Acquired Toolkit public UI closure unavailable; set FORGE_TOOLKIT_UI_SOURCE to its qualified directory' }, run);
const flush = async () => { for (let index = 0; index < 15; index++) await new Promise(resolve => setImmediate(resolve)); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const copy = value => structuredClone(value);
const idle = () => ({ available: true, can_send: true, can_stop: false, outcome_uncertain: false, active_send: null, last_completion: null });
const busy = id => ({ ...idle(), can_send: false, can_stop: true, active_send: { send_request_id: id, cancellation_requested: false } });
const completed = (id, completion = 'completed') => ({ ...idle(), last_completion: { send_request_id: id, completion } });
const response = (body, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => copy(body) });

const providerTarget = () => ({ model: 'model-example', provider: 'provider-example', realm: 'selected-realm', binding: 'selected-binding', profile: 'selected-profile' });
function serviceFixture() {
  const f = fixture();
  f.serviceTarget = { initialize_request_id: 'h:provider-start' };
  const binding = () => ({ type: 'binding', protocol_version: 3, target: copy(f.serviceTarget) });
  for (const path of ['/initialize', '/state']) f.overrides.set(path, () => response({ status: 'starting', binding: binding() }, 202));
  f.bind = target => { f.serviceTarget = copy(target); f.emit(binding()); };
  f.state = value => { f.host.state = value; f.emit({ type: 'event', event: { target: copy(f.serviceTarget), kind: 'state', state: value } }); };
  f.closed = (id, disposition = 'answered', target = f.serviceTarget) => f.emit({ type: 'event', event: { target: copy(target), kind: 'prompt_closed', prompt_id: id, disposition } });
  f.provider = (prompt, id = 'p:provider', target = f.serviceTarget) => f.emit({ type: 'prompt', protocol_version: 3, target: copy(target), id, prompt });
  f.ready = async value => {
    f.bind({ scope: f.host.scope }); f.host.state = value;
    f.overrides.set('/state', () => response({ status: 'ready', binding: binding(), state: { target: copy(f.serviceTarget), kind: 'state', state: f.host.state }, capabilities: {} }));
    f.state(value); await f.port.refresh(); await flush();
  };
  return f;
}

function fixture() {
  const dom = new JSDOM('<main id="root"><p>Actual native Console belongs here</p></main><div id="setup"></div>', { url: 'http://127.0.0.1:4510/console' });
  const requests = [], sources = [], overrides = new Map(), timers = new Map();
  let timer = 0;
  const host = { scope: 'private-host-scope', state: idle(), history: { messages: [], offset: 0, total: 0, hasMore: false } };
  class Events {
    constructor(url) { this.url = url; this.closed = false; sources.push(this); }
    close() { this.closed = true; }
    emit(item) { this.onmessage?.({ data: JSON.stringify(item) }); }
  }
  const fetcher = async (url, options) => {
    const path = url.slice('/console/forge/connect'.length);
    const call = { path, method: options.method, ...(options.body ? { body: JSON.parse(options.body) } : {}), credentials: options.credentials, redirect: options.redirect };
    requests.push(call);
    const handler = overrides.get(path);
    if (handler) return handler(call);
    if (path.startsWith('/send/')) return (overrides.get('/send/*') ?? (() => response({ association: decodeURIComponent(path.slice('/send/'.length)), status: 'unknown' })))(call);
    if (path === '/initialize' || path === '/state') return response({ status: 'ready', state: { scope: host.scope, kind: 'state', state: host.state }, capabilities: {} });
    if (path === '/history' || path.startsWith('/history?')) return response({ history: host.history, state: host.state });
    if (path.startsWith('/result/')) return response({ request_id: decodeURIComponent(path.split('/').at(-1)), status: 'pending' });
    if (path === '/answer') return response({ status: 'forwarded' }, 202);
    if (path === '/cancel') return response({ scope: host.scope, send_request_id: call.body.send_request_id, status: 'cancellation_requested' });
    throw new Error(`Unexpected offline route ${path}`);
  };
  const port = createConsoleSetupPort({ fetch: fetcher, EventSource: Events, SetupConversationError, schedule: callback => { timers.set(++timer, callback); return timer; }, unschedule: id => timers.delete(id) });
  const container = dom.window.document.querySelector('#setup');
  const mount = () => mountConsoleSetup(container, { port, mountSetupConversation });
  const emit = (item, privateChannel = true) => sources.findLast(source => !source.closed && source.url.endsWith(privateChannel ? '/private' : '/public'))?.emit(item);
  const state = value => { host.state = value; emit({ type: 'event', event: { scope: host.scope, kind: 'state', state: value } }); };
  const prompt = (body, id = 'p:one') => emit({ type: 'prompt', id, scope: host.scope, prompt: body }, true);
  const buttons = () => [...container.querySelectorAll('button')];
  const button = label => buttons().find(node => node.textContent === label || node.textContent.startsWith(label));
  const submit = selector => container.querySelector(selector).dispatchEvent(new dom.window.Event('submit', { bubbles: true, cancelable: true }));
  return { dom, host, port, requests, sources, overrides, timers, container, mount, emit, state, prompt, button, submit };
}


// New controls use the existing host fixture and actual acquired UI in memory.
// Ordinary scoped protocol4 state has the same web envelope as ordinary1;
// service3 alone supplies its retained target/binding envelope.
const pendingCompletion = id => ({ ...completed(id), can_send: false, private_notice_pending: true });
const innerStatus = f => f.container.querySelector('.toolkit-conversation > p[role="status"]');
const historyReads = f => f.requests.filter(call => call.path === '/history' || call.path.startsWith('/history?')).length;

focused('private-pending correction: newly completed private wait refreshes retained history once', async () => {
  for (const service of [false, true]) {
    const f = service ? serviceFixture() : fixture(), view = f.mount();
    try {
      await flush(); if (service) await f.ready({ ...idle(), private_notice_pending: false });
      f.state({ ...busy('h:another-view'), private_notice_pending: false }); await flush();
      const before = historyReads(f);
      f.host.history = { messages: [{ role: 'assistant', text: 'Completed elsewhere and retained while the private decision waits.' }], offset: 0, total: 1, hasMore: false };
      f.state(pendingCompletion('h:another-view')); await flush();
      assert.equal(historyReads(f), before + 1, 'A completed run must refresh the mounted history even while sending remains paused');
      assert.match(f.container.querySelector('[aria-label="Saved conversation"]').textContent, /Completed elsewhere and retained/);
      for (let index = 0; index < 3; index++) f.state(pendingCompletion('h:another-view'));
      await flush(); assert.equal(historyReads(f), before + 1, 'Repeated exact completion must not repeat the history read');
      f.state({ ...busy('h:next'), last_completion: completed('h:another-view').last_completion, private_notice_pending: false });
      f.state({ ...busy('h:next'), last_completion: completed('h:another-view').last_completion, private_notice_pending: true });
      await flush(); assert.equal(historyReads(f), before + 1, 'A private notice cannot reinterpret the preceding completion as new');
      assert.equal(f.requests.some(call => ['/send', '/cancel', '/answer'].includes(call.path)), false);
      view.dispose();
      const reopened = f.mount(); await flush();
      assert.equal(historyReads(f), before + 2, 'A new view loads history once rather than replaying the old completion refresh');
      reopened.dispose();
    } finally { view.dispose(); f.dom.window.close(); }
  }
  // A sending view keeps its own post-send history read. Host refresh invokes
  // the acquired view's existing pending guard rather than duplicating it.
  const f = fixture(), view = f.mount();
  try {
    await flush(); const before = historyReads(f), held = deferred();
    f.overrides.set('/send', () => held.promise);
    f.container.querySelector('textarea').value = 'One actual browser-owned send';
    f.submit('.toolkit-conversation form'); await flush();
    const request = f.requests.find(call => call.path === '/send');
    held.resolve(response({ association: request.body.association, request_id: 'h:owned', status: 'pending' }, 202)); await flush();
    f.host.history = { messages: [{ role: 'assistant', text: 'One own completed response.' }], offset: 0, total: 1, hasMore: false };
    f.state(pendingCompletion('h:owned')); await flush();
    assert.equal(historyReads(f), before + 1, 'The pending view owns one post-send read');
    assert.match(f.container.querySelector('[aria-label="Saved conversation"]').textContent, /One own completed response/);
    assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
    assert.equal(f.requests.some(call => ['/cancel', '/answer'].includes(call.path)), false);
  } finally { view.dispose(); f.dom.window.close(); }
});


focused('paused archive integration: native private wait keeps neutral copy and one retained-history refresh', async () => {
  const paused = 'Sending is paused. Check the protected setup panel or current host status.';
  for (const service of [false, true]) {
    // This ordinary scoped envelope is a protocol4-shaped host specimen, not
    // evidence that an ordinary native protocol1 producer accepts the flag.
    const f = service ? serviceFixture() : fixture(), view = f.mount();
    try {
      await flush();
      if (service) await f.ready({ ...idle(), private_notice_pending: false });
      const initialPosts = f.requests.filter(call => call.method === 'POST').length;
      assert.equal(f.port.canSend, true);
      f.state({ ...busy('h:paused-archive'), private_notice_pending: false }); await flush();
      assert.equal(innerStatus(f).textContent, paused);
      assert.equal(f.port.canSend, false);
      assert.equal(f.port.canStop, true);
      assert.equal(f.button('Send message').disabled, true);
      assert.equal(f.button('Stop this agent run').disabled, false);

      const before = historyReads(f);
      f.host.history = { messages: [{ role: 'assistant', text: 'The completed response remains readable while the private notice waits.' }], offset: 0, total: 1, hasMore: false };
      f.state(pendingCompletion('h:paused-archive')); await flush();
      assert.equal(historyReads(f), before + 1, 'The existing host refreshes this new exact completion once with the incoming UI');
      assert.match(f.container.querySelector('[aria-label="Saved conversation"]').textContent, /completed response remains readable/);
      assert.equal(innerStatus(f).textContent, paused);
      assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /Waiting for a private decision/);
      assert.equal(f.port.snapshot.state.private_notice_pending, true);
      assert.equal(f.port.snapshot.state.active_send, null);
      assert.equal(f.port.canSend, false);
      assert.equal(f.port.canStop, false);
      assert.equal(f.button('Send message').disabled, true);
      assert.equal(f.button('Stop this agent run').disabled, true);
      assert.equal(f.container.querySelector('textarea').readOnly, false);

      f.state(pendingCompletion('h:paused-archive'));
      f.state(pendingCompletion('h:paused-archive')); await flush();
      assert.equal(historyReads(f), before + 1, 'Repeated retained completion does not duplicate the host history read');
      // Service3 requires its real Boolean flag. Only the ordinary specimen
      // exercises absence; neither case infers sending authority from it.
      for (const flag of service ? [false] : [false, undefined]) {
        const value = { ...completed('h:paused-archive'), can_send: false,
          ...(flag === undefined ? {} : { private_notice_pending: flag }) };
        f.state(value); await flush();
        assert.equal(innerStatus(f).textContent, paused);
        assert.doesNotMatch(innerStatus(f).textContent, /another message is running|agent is still responding/i);
        assert.equal(f.port.canSend, false);
        assert.equal(f.port.canStop, false);
        assert.equal(historyReads(f), before + 1);
      }

      f.state({ ...completed('h:paused-archive'), can_send: false, outcome_uncertain: true, private_notice_pending: false }); await flush();
      assert.equal(f.port.outcomeUncertain, true);
      assert.match(innerStatus(f).textContent, /requires host reconciliation/);
      assert.equal(f.container.querySelector('textarea').readOnly, true);
      assert.equal(f.button('Send message').disabled, true);
      assert.equal(f.button('Stop this agent run').disabled, true);
      f.emit({ type: 'event', event: { ...(service ? { target: copy(f.serviceTarget) } : { scope: f.host.scope }), kind: 'unavailable' } }); await flush();
      assert.equal(f.port.available, false);
      assert.equal(f.port.snapshot.state, null);
      assert.equal(innerStatus(f) === null, true);
      assert.equal(f.requests.filter(call => call.method === 'POST').length, initialPosts, 'Passive status, completion and retirement must not send, cancel, answer or initialize again');
      assert.equal(f.requests.some(call => ['/send', '/cancel', '/answer'].includes(call.path)), false);
    } finally { view.dispose(); f.dom.window.close(); }
  }
});

focused('latest history correction: long conversation restores newest replies and preserves explicit paging', async () => {
  for (const service of [false, true]) {
    const f = service ? serviceFixture() : fixture();
    const messages = Array.from({ length: 70 }, (_, index) => ({
      role: index % 2 ? 'assistant' : 'user', text: `Saved entry ${index}`,
    }));
    // Native Connect uses the most recent 50 rows when offset is omitted.
    // Supplying an offset keeps its ordinary absolute-page meaning.
    const history = call => {
      const requested = new URL(`http://fixture.invalid${call.path}`).searchParams.get('offset');
      const offset = requested === null ? Math.max(0, messages.length - 50) : Number(requested);
      return response({ history: { messages: messages.slice(offset, offset + 50), offset,
        total: messages.length, hasMore: offset + 50 < messages.length }, state: f.host.state });
    };
    for (const path of ['/history', '/history?offset=0', '/history?offset=50']) f.overrides.set(path, history);
    let view = f.mount();
    const saved = () => f.container.querySelector('ol[aria-label="Saved conversation"]');
    try {
      await flush();
      if (service) await f.ready({ ...idle(), private_notice_pending: false });
      assert.match(saved().textContent, /Saved entry 69/, 'Opening a long conversation must restore its newest saved reply');
      assert.doesNotMatch(saved().textContent, /Saved entry 0(?:You|Setup agent|Copy message)/);
      assert.equal(f.button('Earlier messages').disabled, false);
      assert.equal(f.button('Later messages').disabled, true);

      f.button('Earlier messages').click(); await flush();
      assert.match(saved().textContent, /Saved entry 0/);
      assert.doesNotMatch(saved().textContent, /Saved entry 69/);
      assert.equal(f.requests.at(-1).path, '/history?offset=0');
      f.button('Later messages').click(); await flush();
      assert.match(saved().textContent, /Saved entry 69/);
      assert.equal(f.requests.at(-1).path, '/history?offset=50');

      f.button('Refresh history').click(); await flush();
      assert.match(saved().textContent, /Saved entry 20/);
      assert.match(saved().textContent, /Saved entry 69/);
      assert.equal(f.requests.at(-1).path, '/history');
      view.dispose(); view = f.mount(); await flush();
      assert.match(saved().textContent, /Saved entry 69/, 'Reopening the drawer must retain the newest page');
      assert.deepEqual(f.requests.filter(call => call.method === 'POST').map(call => call.path), ['/initialize'],
        'Mounting, paging, refreshing and reopening must not submit work again');

      f.overrides.set('/send', call => {
        f.host.state = { ...busy('h:latest-reply'), ...(service ? { private_notice_pending: false } : {}) };
        messages.push({ role: 'user', text: call.body.text });
        return response({ association: call.body.association, request_id: 'h:latest-reply', status: 'pending' }, 202);
      });
      const input = f.container.querySelector('textarea'); input.value = 'Continue from the latest reply';
      f.submit('.toolkit-conversation form'); await flush();
      const event = { kind: 'conversation', send_request_id: 'h:latest-reply',
        event: { kind: 'text_complete', text: 'Newest reply after the long conversation', messageId: 'latest-message' },
        ...(service ? { target: copy(f.serviceTarget) } : { scope: f.host.scope }) };
      f.emit({ type: 'event', event });
      assert.match(f.container.querySelector('[aria-label="Agent response in progress"]').textContent,
        /Newest reply after the long conversation/);
      messages.push({ role: 'assistant', text: 'Newest reply after the long conversation' });
      f.state({ ...completed('h:latest-reply'), ...(service ? { private_notice_pending: false } : {}) });
      await flush();
      assert.match(saved().textContent, /Continue from the latest reply/);
      assert.match(saved().textContent, /Newest reply after the long conversation/,
        'The post-send read must not replace the newest reply with the oldest page');
      assert.equal(f.container.querySelector('[aria-label="Agent response in progress"]').textContent, '');
      assert.equal(input.value, '');
      assert.equal(f.requests.at(-1).path, '/history');
      assert.deepEqual(f.requests.filter(call => call.method === 'POST').map(call => call.path), ['/initialize', '/send']);
      assert.ok(f.requests.filter(call => call.path === '/history' || call.path.startsWith('/history?'))
        .every(call => call.method === 'GET'));
    } finally { view.dispose(); f.dom.window.close(); }
  }
});
