import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile, access } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { build } from 'esbuild';
import { createConsoleSetupPort, mountConsoleSetup, visibleText } from '../launchkit/templates_v2/console_setup.mjs';

const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
// Exercise the actual acquired public UI closure, not a checkout or port imitation.
// This in-memory bundle neither imports a native Session nor publishes assets.
const toolkitSource = `${(process.env.FORGE_TOOLKIT_UI_SOURCE ?? new URL('../launchkit/templates_v2/toolkit-ui/', import.meta.url).pathname).replace(/\/$/, '')}/`;
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

focused('actual public Toolkit view mounts beside native root and uses fixed authenticated routes', async () => {
  const f = fixture(), view = f.mount(); await flush();
  assert.equal(f.dom.window.document.querySelector('#root').textContent, 'Actual native Console belongs here');
  assert.ok(f.container.querySelector('.toolkit-conversation'));
  assert.equal(f.container.querySelector('h1').textContent, 'Set up your app');
  assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
  assert.deepEqual(f.requests.find(call => call.path === '/initialize').body, {});
  assert.ok(f.requests.every(call => call.credentials === 'same-origin' && call.redirect === 'error'));
  view.dispose(); f.dom.window.close();
});

focused('same-login tabs cannot associate another send with a message dropped before admission', async () => {
  const a = fixture(), b = fixture(), shared = { scope: 'same-login-child', state: idle() }, admitted = new Map();
  for (const f of [a, b]) {
    f.host.scope = shared.scope;
    for (const path of ['/initialize', '/state']) f.overrides.set(path, () => response({ status: 'ready', state: { scope: shared.scope, kind: 'state', state: shared.state }, capabilities: {} }));
    f.overrides.set('/send/*', call => {
      const association = decodeURIComponent(call.path.slice('/send/'.length));
      return response(admitted.get(association) ?? { association, status: 'unknown' });
    });
  }
  const av = a.mount(), bv = b.mount(); await flush();
  a.overrides.set('/send', async () => { throw new Error('POST never entered the host'); });
  const draft = 'The same words are not send identity';
  const ai = a.container.querySelector('textarea'), bi = b.container.querySelector('textarea');
  ai.value = draft; a.submit('.toolkit-conversation form'); await flush();
  b.overrides.set('/send', call => {
    const receipt = { association: call.body.association, request_id: 'h:tab-b', status: 'pending' };
    admitted.set(call.body.association, receipt); shared.state = busy('h:tab-b');
    return response(receipt, 202);
  });
  bi.value = draft; b.submit('.toolkit-conversation form'); await flush();
  for (const f of [a, b]) f.state(shared.state);
  await flush();
  shared.state = completed('h:tab-b');
  admitted.set(b.requests.find(call => call.path === '/send').body.association, {
    ...admitted.get(b.requests.find(call => call.path === '/send').body.association), status: 'completed',
    result: { scope: shared.scope, send_request_id: 'h:tab-b', completion: 'completed', state: shared.state },
  });
  for (const f of [a, b]) f.state(shared.state);
  await flush(); await a.port.refresh();
  assert.equal(ai.value, draft, 'B completion cannot clear A unaccepted draft');
  assert.equal(a.port.outcomeUncertain, true); assert.equal(a.port.canSend, false);
  assert.equal(bi.value, ''); assert.equal(b.port.canSend, true);
  for (const f of [a, b]) assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
  assert.equal(a.requests.filter(call => call.path === '/initialize').length, 1);
  av.dispose(); bv.dispose(); a.dom.window.close(); b.dom.window.close();
});

focused('a direct send receipt for another browser association cannot accept or clear the draft', async () => {
  const f = fixture(), view = f.mount(); await flush();
  f.overrides.set('/send', call => response({ association: `${call.body.association}wrong`, request_id: 'h:other-tab', status: 'pending' }, 202));
  const input = f.container.querySelector('textarea'); input.value = 'Keep my original draft';
  f.submit('.toolkit-conversation form'); await flush();
  f.state(completed('h:other-tab')); await flush(); await f.port.refresh();
  assert.equal(input.value, 'Keep my original draft');
  assert.equal(f.port.outcomeUncertain, true); assert.equal(f.port.canSend, false);
  assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
  assert.equal(f.requests.some(call => call.path.startsWith('/result/')), false);
  view.dispose(); f.dom.window.close();
});

focused('real conversation streams exact matching text and retains confirmed result if history fails', async () => {
  const f = fixture(), view = f.mount(); await flush();
  f.overrides.set('/send', call => { f.host.state = busy('h:one'); return response({ association: call.body.association, request_id: 'h:one', status: 'pending' }, 202); });
  const input = f.container.querySelector('textarea'); input.value = '  Connect my tools\n';
  f.submit('.toolkit-conversation form'); await flush();
  const sent = f.requests.find(call => call.path === '/send').body;
  assert.equal(sent.text, '  Connect my tools\n'); assert.match(sent.association, /^b:[0-9a-f]{32}$/);
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'conversation', send_request_id: 'h:other', event: { kind: 'text_delta', text: 'FOREIGN' } } });
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'conversation', send_request_id: 'h:one', event: { kind: 'text_delta', text: 'Exact response' } } });
  assert.match(f.container.textContent, /Exact response/); assert.doesNotMatch(f.container.textContent, /FOREIGN/);
  f.overrides.set('/history', () => response({ error: { code: 'unavailable' } }, 503));
  f.state(completed('h:one')); await flush();
  assert.match(f.container.querySelector('.forge-setup__completion').textContent, /Response completed/);
  assert.match(f.container.textContent, /Saved conversation is unavailable. Current status and confirmed results are retained/);
  assert.equal(f.button('Send message').disabled, true);
  assert.equal(input.value, '');
  view.dispose(); f.dom.window.close();
});

focused('lost entered send reply joins only its exact retained association without replay across remount', async () => {
  const f = fixture(); let view = f.mount(); await flush();
  let association;
  f.overrides.set('/send', async call => { association = call.body.association; throw new Error('entered original send, HTTP response lost'); });
  f.container.querySelector('textarea').value = 'one original message'; f.submit('.toolkit-conversation form'); await flush();
  assert.equal(f.port.outcomeUncertain, true); assert.equal(f.port.canSend, false);
  await f.port.refresh(); assert.equal(f.port.outcomeUncertain, true);
  view.dispose(); assert.equal(f.sources.every(source => source.closed), true);
  view = f.mount(); await flush(); assert.equal(f.port.canSend, false);
  f.state(busy('h:retained')); await flush();
  assert.equal(f.port.outcomeUncertain, true, 'Shared state is not proof that the original POST entered');
  f.overrides.set('/send/*', call => {
    assert.equal(decodeURIComponent(call.path.slice('/send/'.length)), association);
    return response({ association, request_id: 'h:retained', status: 'pending' });
  });
  await f.port.refresh(); await flush();
  assert.equal(f.port.outcomeUncertain, false); assert.equal(f.port.canSend, false);
  f.state(completed('h:retained')); await flush();
  assert.equal(f.port.canSend, true);
  assert.match(f.container.textContent, /Last confirmed result: Response completed/);
  assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
  assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
  view.dispose(); f.dom.window.close();
});

focused('generic unavailable send response is uncertain, while exact pre-admission busy refusal permits correction', async () => {
  for (const [code, status, uncertain] of [['unavailable', 503, true], ['busy', 409, false]]) {
    const f = fixture(), view = f.mount(); await flush();
    f.overrides.set('/send', () => response({ error: { code } }, status));
    f.container.querySelector('textarea').value = 'original'; f.submit('.toolkit-conversation form'); await flush();
    assert.equal(f.port.outcomeUncertain, uncertain); assert.equal(f.port.canSend, !uncertain);
    assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
    view.dispose(); f.dom.window.close();
  }
});

focused('cancel ACK does not report stopped; exact original cancelled completion does', async () => {
  const f = fixture(), view = f.mount(); await flush();
  f.state(busy('h:cancel')); await flush();
  let finished = false;
  const outcome = f.port.stopObservedRun('view reason').then(value => { finished = true; return value; }); await flush();
  assert.equal(finished, false); assert.match(f.container.textContent, /Waiting for confirmed cancellation/);
  assert.deepEqual(f.requests.find(call => call.path === '/cancel').body, { send_request_id: 'h:cancel' });
  f.state(completed('h:cancel', 'cancelled')); await flush();
  assert.equal(await outcome, 'stopped'); assert.match(f.container.textContent, /Agent run cancelled/);
  assert.equal(f.requests.filter(call => call.path === '/cancel').length, 1);
  view.dispose(); f.dom.window.close();
});

focused('private prompt only, exact offered recovery choice, More only when offered', async () => {
  const f = fixture(), view = f.mount(); await flush();
  const body = { kind: 'forge_recovery', choices: [{ choice_id: 'offered:one', application_id: 'Reading', change_id: 'saved-change', origin: 'http://127.0.0.1:4510' }], more: false, skipped_entries: 0 };
  f.emit({ type: 'prompt', id: 'p:one', scope: f.host.scope, prompt: body }, false);
  assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
  f.prompt(body); assert.ok(f.button('Reading')); assert.equal(f.button('More saved setups') === undefined, true);
  f.button('Reading').click(); await flush();
  assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:one', answer: { kind: 'forge_recovery', choice: { kind: 'existing', choice_id: 'offered:one' } } });
  assert.match(f.container.textContent, /no effect is confirmed yet/);
  assert.equal(f.button('Reading').disabled, true);
  view.dispose(); f.dom.window.close();
});

focused('offered origin is inert and forwarded exactly, not an editable or navigable URL', async () => {
  const f = fixture(), view = f.mount(); await flush();
  const origin = '<img src=x onerror=alert(1)>\u202Eaddress';
  f.prompt({ kind: 'forge_origin', permitted_origin: origin });
  assert.equal(f.container.querySelector('img') === null, true); assert.equal(f.container.querySelector('a') === null, true);
  assert.match(f.container.textContent, /\[U\+202E\]/); assert.equal(f.container.querySelector('.forge-setup__protected input') === null, true);
  f.button('Continue with this Forge').click(); await flush();
  assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:one', answer: { kind: 'forge_origin', origin } });
  view.dispose(); f.dom.window.close();
});

focused('password preserves exact spaces and Unicode, clears immediately, never enters chat or storage', async () => {
  const f = fixture(), view = f.mount(); await flush();
  const held = deferred(); f.overrides.set('/answer', () => held.promise);
  f.prompt({ kind: 'forge_password', origin: 'http://127.0.0.1:4510', mode: 'initial', min_chars: 12, max_chars: 4096 });
  const password = '  private-test-🌌-secret  ';
  const input = f.container.querySelector('input[type=password]'); input.value = password;
  f.submit('.forge-setup__password-form');
  assert.equal(input.value, ''); await flush();
  assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:one', answer: { kind: 'forge_password', password } });
  assert.equal(f.requests.some(call => call.path === '/send'), false);
  assert.equal(f.dom.window.localStorage.length, 0); assert.equal(f.dom.window.sessionStorage.length, 0);
  assert.doesNotMatch(f.container.textContent, /private-test/);
  held.resolve(response({ status: 'forwarded' }, 202)); await flush();
  view.dispose(); f.dom.window.close();
});

focused('complete proposal and warnings remain selectable and inert with visible direction/control characters', async () => {
  const f = fixture(), view = f.mount(); await flush();
  const body = { configuration: { name: '<script>private()</script>\u202E\u007F', full: 'x'.repeat(9000) }, warnings: ['Commit starts reconciliation and can admit queued messages.', 'LAST COMPLETE WARNING'] };
  f.prompt({ kind: 'approval', approval_id: 'native-private-approval', summary: 'Review your connection', body, allowed_decisions: ['deny'], expires_at: null });
  const proposal = f.container.querySelector('pre');
  assert.equal(proposal.textContent, visibleText(JSON.stringify(body, null, 2)));
  assert.match(proposal.textContent, /LAST COMPLETE WARNING/); assert.match(proposal.textContent, /\[U\+202E\]\[U\+007F\]/);
  assert.equal(proposal.tabIndex, 0); assert.equal(f.container.querySelector('script') === null, true);
  assert.equal(f.button('Approve this exact change') === undefined, true);
  f.button('Do not approve').click(); await flush();
  assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:one', answer: { kind: 'approval', decision: 'deny' } });
  view.dispose(); f.dom.window.close();
});

focused('closing only detaches observation; same live prompt remount has no retained password draft or resubmission', async () => {
  const f = fixture(); let view = f.mount(); await flush();
  const prompt = { kind: 'forge_password', origin: 'http://127.0.0.1:4510', mode: 'reauthenticate', min_chars: 12, max_chars: 4096 };
  f.prompt(prompt, 'p:retained'); const old = f.container.querySelector('input'); old.value = 'unsubmitted-secret';
  view.dispose(); assert.equal(old.value, ''); assert.equal(f.sources.every(source => source.closed), true);
  assert.equal(f.requests.some(call => ['/answer', '/cancel'].includes(call.path)), false);
  view = f.mount(); await flush(); f.prompt(prompt, 'p:retained');
  assert.equal(f.container.querySelector('input').value, '');
  assert.ok(f.container.querySelector('input[type=password]'));
  assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
  view.dispose(); f.dom.window.close();
});

focused('lost protected answer never resubmits or restores enabled controls on same-ID remount', async () => {
  const f = fixture(); let view = f.mount(); await flush();
  const prompt = { kind: 'forge_origin', permitted_origin: 'http://127.0.0.1:4510' };
  f.overrides.set('/answer', () => { throw new Error('lost answer response'); });
  f.prompt(prompt); f.button('Continue with this Forge').click(); await flush();
  assert.match(f.container.textContent, /answer outcome is unconfirmed/); assert.equal(f.button('Continue with this Forge').disabled, true);
  view.dispose(); view = f.mount(); await flush(); f.prompt(prompt);
  assert.equal(f.button('Continue with this Forge').disabled, true);
  assert.equal(f.requests.filter(call => call.path === '/answer').length, 1);
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'prompt_closed', prompt_id: 'p:one', disposition: 'answered' } });
  assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
  view.dispose(); f.dom.window.close();
});

focused('logout failure response clears all private controls and refuses late callbacks', async () => {
  const f = fixture(), view = f.mount(); await flush();
  f.prompt({ kind: 'forge_password', origin: 'http://127.0.0.1:4510', mode: 'initial', min_chars: 12, max_chars: 4096 });
  const input = f.container.querySelector('input'); input.value = 'held-unsubmitted-secret';
  const oldStream = f.sources.findLast(source => source.url.endsWith('/private'));
  f.overrides.set('/state', () => response({ error: { code: 'invalid_binding' } }, 401));
  await f.port.refresh(); await flush();
  assert.equal(input.value, ''); assert.equal(f.container.querySelector('input') === null, true);
  assert.match(f.container.textContent, /Sign in to the Console again/);
  oldStream.emit({ type: 'prompt', id: 'p:late', scope: f.host.scope, prompt: { kind: 'forge_origin', permitted_origin: 'OLD-PRIVATE' } });
  assert.doesNotMatch(f.container.textContent, /OLD-PRIVATE/);
  view.dispose(); f.dom.window.close();
});

focused('late history state cannot overwrite a newer exact completion', async () => {
  const f = fixture(), view = f.mount(); await flush();
  const held = deferred(); f.overrides.set('/history', () => held.promise);
  const reading = f.port.history(); f.state(completed('h:new', 'cancelled'));
  held.resolve(response({ history: f.host.history, state: idle() })); await reading;
  assert.match(f.container.querySelector('.forge-setup__completion').textContent, /Agent run cancelled/);
  view.dispose(); f.dom.window.close();
});

focused('new login scope replaces view and clears pending private controls rather than reusing them', async () => {
  const f = fixture(), view = f.mount(); await flush();
  f.prompt({ kind: 'forge_password', origin: 'http://127.0.0.1:4510', mode: 'initial', min_chars: 12, max_chars: 4096 });
  const input = f.container.querySelector('input'); input.value = 'old-login-private-draft';
  f.host.scope = 'replacement-host-scope'; f.state(idle()); await flush();
  assert.equal(input.value, ''); assert.equal(f.container.querySelector('input') === null, true);
  assert.equal(f.port.scope, 'replacement-host-scope');
  f.emit({ type: 'prompt', id: 'p:old', scope: 'private-host-scope', prompt: { kind: 'forge_origin', permitted_origin: 'WRONG' } }, true);
  assert.doesNotMatch(f.container.textContent, /WRONG/);
  view.dispose(); f.dom.window.close();
});

focused('single private observation carries ordered text and refuses detached callbacks after remount', async () => {
  const f = fixture(); let view = f.mount(); await flush();
  f.overrides.set('/send', call => { f.host.state = busy('h:ordered'); return response({ association: call.body.association, request_id: 'h:ordered', status: 'pending' }, 202); });
  f.container.querySelector('textarea').value = 'One request'; f.submit('.toolkit-conversation form'); await flush();
  const item = { type: 'event', event: { scope: f.host.scope, kind: 'conversation', send_request_id: 'h:ordered', event: { kind: 'text_delta', text: 'Once only' } } };
  f.emit(item, false); f.emit(item, true);
  assert.equal(f.container.querySelector('[aria-label="Agent response in progress"]').textContent, 'Once only');
  f.state(completed('h:ordered')); await flush();
  const oldStream = f.sources.findLast(source => !source.closed);
  view.dispose(); view = f.mount(); await flush();
  oldStream.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: busy('h:ordered') } });
  assert.equal(f.port.canSend, true); assert.match(f.container.textContent, /Last confirmed result: Response completed/);
  view.dispose(); f.dom.window.close();
});

focused('expired or undisplayable proposal cannot be approved and Escape closes only the view', async () => {
  const f = fixture();
  const trigger = f.dom.window.document.createElement('button'); trigger.textContent = 'Open setup'; f.dom.window.document.body.append(trigger); trigger.focus();
  const view = f.mount(); await flush();
  f.prompt({ kind: 'approval', approval_id: 'private', summary: 'Expired change', body: { warnings: ['All evidence is retained'] }, allowed_decisions: ['approve', 'deny'], expires_at: '2000-01-01T00:00:00Z' });
  assert.equal(f.button('Approve this exact change').disabled, true); assert.equal(f.button('Do not approve').disabled, true);
  f.button('Close').focus();
  f.button('Close').dispatchEvent(new f.dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  assert.equal(f.container.children.length, 0); assert.equal(f.dom.window.document.activeElement === trigger, true);
  assert.equal(f.requests.some(call => ['/cancel', '/answer'].includes(call.path)), false);
  view.dispose(); f.dom.window.close();
});

focused('offline lane binds exact acquired Toolkit UI input bytes without claiming installed browser resources', async () => {
  const expected = {"index.js": "114daaab72d18df9b362af9fac1e0c35e4fc5a6e15d5ce52d59098eab907047b", "setup-model.js": "40aad313183a0455ac1e5048ec7a7300aace2e9a5a68d418e8ab6c69c31079f8", "setup-conversation.js": "5c5737d383116002ead5f5c6d6352fd6069adc2de375f1f9e6946570b8a163a6", "setup-markdown.js": "5bb95eba3e26c6585cff2ecf61f18f5f14ef1e8d21d62e813535254ec573c203", "style.css": "f0ac4e309ba203551eb4e3d6aaaa9295b8ce3e08e6e0b0da8da28d3cae628eec", "vendor/marked/marked.esm.js": "db9e8a2f07a084f68a5e40e258b69755af894f9df4ec47e06e7f9a309584871f", "vendor/entities/decode.js": "4ee2c7d8aa8381caf18140561e41305d27d79964ac502ae330944874a1f9ccb8"};
  for (const [file, hash] of Object.entries(expected)) assert.equal(createHash('sha256').update(await readFile(`${toolkitSource}${file}`)).digest('hex'), hash);
});

focused('pending initialization attaches and answers original origin/password prompts before ready', async () => {
  const f = fixture(); let answered = 0;
  f.host.state = { ...idle(), available: false, can_send: false };
  f.overrides.set('/initialize', () => response({ status: 'starting' }, 202));
  f.overrides.set('/state', () => answered === 2 ? response({ status: 'ready', state: { scope: f.host.scope, kind: 'state', state: idle() }, capabilities: {} }) : response({ status: 'starting' }, 202));
  f.overrides.set('/answer', () => { answered += 1; if (answered === 2) f.host.state = idle(); return response({ status: 'forwarded' }, 202); });
  const view = f.mount(); await flush();
  assert.equal(f.sources.filter(source => !source.closed).length, 1, 'One private observation attaches during initialization');
  const earlyPrivate = f.sources.findLast(source => source.url.endsWith('/private'));
  earlyPrivate.readyState = 2; earlyPrivate.onerror(); await flush();
  assert.equal(earlyPrivate.closed, true);
  assert.equal(f.sources.filter(source => !source.closed).length, 1, 'Early unavailable503 attachment must be replaced by observation only');
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: f.host.state } }, true);
  f.prompt({ kind: 'forge_origin', permitted_origin: 'http://127.0.0.1:4510' }, 'p:startup-origin');
  assert.equal(f.port.available, false); assert.equal(f.port.canSend, false);
  assert.ok(f.button('Continue with this Forge'));
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: f.host.state } });
  assert.ok(f.button('Continue with this Forge'), 'Pre-initialization unavailable projection must not retire the prompt');
  f.button('Continue with this Forge').click(); await flush();
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'prompt_closed', prompt_id: 'p:startup-origin', disposition: 'answered' } });
  f.prompt({ kind: 'forge_password', origin: 'http://127.0.0.1:4510', mode: 'initial', min_chars: 12, max_chars: 4096 }, 'p:startup-password');
  f.container.querySelector('input').value = '  fixture-only-password  '; f.submit('.forge-setup__password-form'); await flush();
  assert.equal(answered, 2);
  assert.deepEqual(f.requests.filter(call => call.path === '/answer').map(call => call.body), [
    { prompt_id: 'p:startup-origin', answer: { kind: 'forge_origin', origin: 'http://127.0.0.1:4510' } },
    { prompt_id: 'p:startup-password', answer: { kind: 'forge_password', password: '  fixture-only-password  ' } },
  ]);
  await f.port.refresh(); await flush();
  assert.equal(f.port.available, true); assert.ok(f.container.querySelector('.toolkit-conversation'));
  assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
  assert.equal(f.requests.some(call => call.path === '/send'), false);
  view.dispose(); f.dom.window.close();
});

focused('closed prompt IDs suppress delayed private frames before first display and after answered remount', async () => {
  const f = fixture(); let view = f.mount(); await flush();
  const body = { kind: 'forge_origin', permitted_origin: 'http://127.0.0.1:4510' };
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'prompt_closed', prompt_id: 'p:never-display', disposition: 'answered' } });
  f.prompt(body, 'p:never-display');
  assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
  f.prompt(body, 'p:answered'); f.button('Continue with this Forge').click(); await flush();
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'prompt_closed', prompt_id: 'p:answered', disposition: 'decision_recorded' } });
  f.prompt(body, 'p:answered');
  assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
  view.dispose(); view = f.mount(); await flush(); f.prompt(body, 'p:answered');
  assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
  assert.equal(f.requests.filter(call => call.path === '/answer').length, 1);
  view.dispose(); f.dom.window.close();
});

focused('non-success exact completions retain truthful result without public committed-response success copy', async () => {
  for (const completion of ['callback_pending', 'callback_batch_pending', 'cancelled', 'abandoned', 'abandoned_with_error', 'completed_with_finalization_failure', 'runtime_terminated']) {
    const f = fixture(), view = f.mount(); await flush();
    f.overrides.set('/send', call => { f.host.state = busy('h:terminal'); return response({ association: call.body.association, request_id: 'h:terminal', status: 'pending' }, 202); });
    const input = f.container.querySelector('textarea'); input.value = 'Original terminal test'; f.submit('.toolkit-conversation form'); await flush();
    f.state(completed('h:terminal', completion)); await flush();
    assert.doesNotMatch(f.container.textContent, /Response received\. Complete/);
    assert.doesNotMatch(f.container.querySelector('.forge-setup__status-text').textContent, /unconfirmed/);
    assert.equal(input.value, 'Original terminal test');
    assert.ok(f.container.querySelector('.forge-setup__completion').textContent.startsWith('Last confirmed result:'));
    assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
    view.dispose(); f.dom.window.close();
  }
});

focused('explicit unavailable event fences private controls even if later same-scope GET returns a snapshot', async () => {
  const f = fixture(), view = f.mount(); await flush();
  f.prompt({ kind: 'forge_origin', permitted_origin: 'http://127.0.0.1:4510' });
  f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'unavailable', reason: 'failed' } });
  assert.equal(f.port.available, false); assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
  const count = f.sources.length;
  await f.port.refresh(); await flush();
  assert.equal(f.port.available, false); assert.equal(f.sources.length, count);
  assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
  view.dispose(); f.dom.window.close();
});

focused('late unavailable snapshot after initialization recovers by GET observation without another startup', async () => {
  const f = fixture(), view = f.mount(); await flush();
  try {
    assert.equal(f.port.available, true); assert.equal(f.timers.size, 0);
    const held = deferred();
    f.overrides.set('/state', () => held.promise);
    const earlierRead = f.port.refresh();
    f.state({ ...idle(), available: false, can_send: false });
    assert.equal(f.port.snapshot.connection, 'not-ready');
    assert.equal(f.port.available, false); assert.equal(f.port.canSend, false);
    assert.equal(f.container.querySelector('.forge-setup__status-text').textContent, 'Opening the setup conversation...');
    assert.equal(f.timers.size, 1, 'A not-ready snapshot must retain GET observation after initialization');
    held.resolve(response({ status: 'ready', state: { scope: f.host.scope, kind: 'state', state: idle() }, capabilities: {} }));
    await earlierRead; await flush();
    assert.equal(f.port.available, false, 'The earlier HTTP response remains fenced by the newer delivered snapshot');
    f.overrides.delete('/state'); f.host.state = idle();
    const stateReads = f.requests.filter(call => call.path === '/state').length;
    const [timer, observe] = f.timers.entries().next().value; f.timers.delete(timer); observe(); await flush();
    assert.equal(f.requests.filter(call => call.path === '/state').length, stateReads + 1);
    assert.equal(f.port.available, true); assert.ok(f.container.querySelector('.toolkit-conversation'));
    assert.equal(f.container.querySelector('.forge-setup__status-text').textContent, 'Ready when you are');
    assert.equal(f.timers.size, 0); assert.equal(f.port.scope, f.host.scope);
    assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
    assert.equal(f.requests.some(call => ['/send', '/answer', '/cancel', '/close'].includes(call.path)), false);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('explicit terminal unavailable stops not-ready observation and cannot be revived by a same-scope read', async () => {
  for (const reason of ['retired', 'expired', 'closing', 'failed']) {
    const f = fixture(), view = f.mount(); await flush();
    try {
      f.state({ ...idle(), available: false, can_send: false });
      assert.equal(f.timers.size, 1);
      f.prompt({ kind: 'forge_origin', permitted_origin: 'http://127.0.0.1:4510' });
      assert.ok(f.button('Continue with this Forge'));
      f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'unavailable', reason } });
      assert.equal(f.timers.size, 0); assert.equal(f.sources.every(source => source.closed), true);
      assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
      f.host.state = idle(); await f.port.refresh(); await flush();
      assert.equal(f.port.available, false); assert.equal(f.port.canSend, false);
      assert.equal(f.timers.size, 0); assert.equal(f.sources.every(source => source.closed), true);
      assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /Setup is unavailable/);
      assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
      assert.equal(f.requests.some(call => ['/send', '/answer', '/cancel', '/close'].includes(call.path)), false);
    } finally { view.dispose(); f.dom.window.close(); }
  }
});

focused('authentication retirement stops not-ready observation instead of reopening the setup child', async () => {
  for (const status of [401, 403]) {
    const f = fixture(), view = f.mount(); await flush();
    try {
      f.state({ ...idle(), available: false, can_send: false });
      assert.equal(f.timers.size, 1);
      f.overrides.set('/state', () => response({ error: { code: 'invalid_binding' } }, status));
      const [timer, observe] = f.timers.entries().next().value; f.timers.delete(timer); observe(); await flush();
      assert.equal(f.port.snapshot.connection, 'sign-in'); assert.equal(f.port.available, false);
      assert.equal(f.timers.size, 0); assert.equal(f.sources.every(source => source.closed), true);
      assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /Sign in to the Console again/);
      f.overrides.delete('/state'); f.host.state = idle(); await f.port.refresh(); await flush();
      assert.equal(f.port.available, false); assert.equal(f.timers.size, 0);
      assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
      assert.equal(f.requests.some(call => ['/send', '/answer', '/cancel', '/close'].includes(call.path)), false);
    } finally { view.dispose(); f.dom.window.close(); }
  }
});

focused('early observation errors during setup opening retain nonterminal status and GET-only recovery', async () => {
  const f = fixture();
  f.host.state = { ...idle(), available: false, can_send: false };
  f.overrides.set('/initialize', () => response({ status: 'starting' }, 202));
  f.overrides.set('/state', () => response({ status: 'starting' }, 202));
  const view = f.mount(); await flush();
  try {
    f.state(f.host.state);
    assert.equal(f.container.querySelector('.forge-setup__status-text').textContent, 'Opening the setup conversation...');
    f.overrides.set('/state', () => response({ error: { code: 'unavailable' } }, 503));
    const earlyPrivate = f.sources.findLast(source => !source.closed && source.url.endsWith('/private'));
    earlyPrivate.readyState = 2; earlyPrivate.onerror();
    assert.equal(f.port.snapshot.connection, 'reconnecting');
    assert.equal(f.port.available, false); assert.equal(f.port.canSend, false);
    assert.equal(f.container.querySelector('.forge-setup__status-text').textContent, 'Opening the setup conversation...');
    await flush();
    assert.equal(f.container.querySelector('.forge-setup__status-text').textContent, 'Opening the setup conversation...');
    assert.equal(f.timers.size, 1, 'Temporary stream and GET failures keep observing the original startup');

    f.overrides.delete('/state'); f.host.state = idle();
    const beforeRead = f.requests.filter(call => call.path === '/state').length;
    f.button('Check current status').click(); await flush();
    assert.equal(f.requests.filter(call => call.path === '/state').length, beforeRead + 1);
    assert.equal(f.port.available, true); assert.equal(f.port.scope, f.host.scope);
    assert.equal(f.container.querySelector('.forge-setup__status-text').textContent, 'Ready when you are');

    const held = deferred(); f.overrides.set('/state', () => held.promise);
    const readyPrivate = f.sources.findLast(source => !source.closed && source.url.endsWith('/private'));
    readyPrivate.onerror();
    assert.equal(f.container.querySelector('.forge-setup__status-text').textContent, 'Connection interrupted. Current work is retained by the host.');
    held.resolve(response({ status: 'ready', state: { scope: f.host.scope, kind: 'state', state: idle() }, capabilities: {} }));
    await flush(); f.overrides.delete('/state');

    f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'unavailable', reason: 'failed' } });
    assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /Setup is unavailable/);
    assert.equal(f.timers.size, 0); assert.equal(f.sources.every(source => source.closed), true);
    f.button('Check current status').click(); await flush();
    assert.equal(f.port.available, false); assert.equal(f.port.canSend, false);
    assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /Setup is unavailable/);
    assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
    assert.deepEqual(f.requests.filter(call => call.method !== 'GET').map(call => call.path), ['/initialize']);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('held association completion resolves its original send without rewinding newer shared activity', async () => {
  const f = fixture(), detach = f.port.attachView(); await flush();
  let association, finished = false;
  f.overrides.set('/send', async call => { association = call.body.association; throw new Error('original entered, response lost'); });
  const original = f.port.send('One retained original').then(() => { finished = true; });
  await flush(); assert.equal(f.port.outcomeUncertain, true);
  const held = deferred();
  f.overrides.set('/send/*', call => { assert.equal(decodeURIComponent(call.path.slice('/send/'.length)), association); return held.promise; });
  const observation = f.port.refresh(); await flush();
  f.state(busy('h:newer-tab')); await flush();
  assert.equal(finished, false);
  held.resolve(response({ association, request_id: 'h:original', status: 'completed',
    result: { scope: f.host.scope, send_request_id: 'h:original', completion: 'completed', state: completed('h:original') },
  }));
  await observation; await original;
  assert.equal(finished, true); assert.equal(f.port.outcomeUncertain, false);
  assert.equal(f.port.snapshot.state.active_send?.send_request_id, 'h:newer-tab', 'Exact original completion must not rewind the newer public state');
  assert.equal(f.port.canSend, false); assert.equal(f.port.canStop, true);
  assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
  assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
  detach(); f.dom.window.close();
});

focused('one authenticated setup stream per tab carries ordered state conversation and private prompts', async () => {
  const a = fixture(), b = fixture(), av = a.mount(), bv = b.mount();
  try {
    await flush();
    for (const f of [a, b]) {
      assert.deepEqual(f.sources.filter(source => !source.closed).map(source => source.url), ['/console/forge/connect/events/private'], 'The existing private observer already carries the public projection');
      f.overrides.set('/send', call => response({ association: call.body.association, request_id: 'h:single', status: 'pending' }, 202));
      const input = f.container.querySelector('textarea'); input.value = 'One actual original';
      f.submit('.toolkit-conversation form'); await flush();
      f.host.state = busy('h:single');
      f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: f.host.state } }, true);
      assert.equal(f.port.canStop, true);
      f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'conversation', send_request_id: 'h:foreign', event: { kind: 'text_delta', text: 'FOREIGN' } } }, true);
      f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'conversation', send_request_id: 'h:single', event: { kind: 'text_delta', text: 'Exact private-stream projection' } } }, true);
      assert.match(f.container.textContent, /Exact private-stream projection/); assert.doesNotMatch(f.container.textContent, /FOREIGN/);
      f.prompt({ kind: 'forge_origin', permitted_origin: 'http://127.0.0.1:4510' }, 'p:single');
      assert.ok(f.button('Continue with this Forge'));
      f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'prompt_closed', prompt_id: 'p:single', disposition: 'answered' } }, true);
      assert.equal(f.port.protectedPrompt, null);
      f.host.state = completed('h:single');
      f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: f.host.state } }, true); await flush();
      assert.equal(input.value, ''); assert.equal(f.port.canSend, true);
      assert.equal(f.sources.filter(source => !source.closed).length, 1);
      assert.equal(f.requests.filter(call => call.path === '/send').length, 1);
      assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
    }
  } finally { av.dispose(); bv.dispose(); a.dom.window.close(); b.dom.window.close(); }
});

focused('v3 initialization offers only exact protected provider targets before a native scope exists', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush();
    f.provider({ kind: 'provider_target', choices: [{ choice_id: 'native-choice', target: providerTarget(), status: 'Native configured target' }] });
    assert.ok(f.button('model-example'), 'Pre-session private provider choice must be reachable');
    assert.equal(f.port.scope, ''); assert.equal(f.port.available, false); assert.equal(f.port.canSend, false);
    assert.equal(f.container.querySelector('.toolkit-conversation') === null, true);
    assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /private provider setup/i);
    assert.equal(f.button('Cancel this setup interaction') === undefined, true);
    f.button('model-example').click(); await flush();
    assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:provider', answer: { kind: 'provider_target', choice_id: 'native-choice' } });
    assert.equal(f.requests.some(call => call.path === '/send'), false);
    assert.equal(f.sources.filter(source => !source.closed).length, 1);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 API key uses exact UTF8 limits and clears the private input before forwarding', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush(); f.provider({ kind: 'provider_api_key', target: providerTarget(), max_bytes: 8192 });
    const input = f.container.querySelector('input[type=password]');
    assert.ok(input, 'API key requires its own protected control');
    assert.equal(input.autocomplete, 'off');
    for (const invalid of [' ', 'x\u0001', 'x\u0085', 'é'.repeat(4097)]) {
      input.value = invalid; f.submit('.forge-setup__password-form'); await flush();
      assert.equal(f.requests.some(call => call.path === '/answer'), false);
      assert.doesNotMatch(f.container.textContent, /ééé|U\+0001|U\+0085/);
    }
    const exact = ` ${'é'.repeat(4095)} `;
    input.value = exact; f.submit('.forge-setup__password-form');
    assert.equal(input.value, '', 'Secret input clears synchronously before the HTTP await');
    await flush();
    assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:provider', answer: { kind: 'provider_api_key', key: exact } });
    assert.equal(f.requests.some(call => call.path === '/send'), false);
    assert.doesNotMatch(f.container.textContent, /ééé/);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 completed response with a pending private notice is not shown as a model responding', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush(); await f.ready({ ...completed('h:finished'), can_send: false, private_notice_pending: true });
    assert.equal(f.port.snapshot.state.private_notice_pending, true);
    assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /private decision/i);
    assert.doesNotMatch(f.container.querySelector('.forge-setup__status-text').textContent, /responding|Opening/);
    assert.match(f.container.querySelector('.forge-setup__completion').textContent, /Response completed/);
    assert.equal(f.port.canSend, false);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 provider actions and answers cannot inject unoffered targets or authority', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush(); f.provider({ kind: 'provider_action', target: providerTarget(), actions: ['use', 'back'] });
    assert.ok(f.button('Use this connection')); assert.ok(f.button('Choose another connection'));
    assert.equal(f.button('Enter an API key privately') === undefined, true); assert.equal(f.button('Sign in to this provider') === undefined, true);
    for (const answer of [{ kind: 'provider_action', action: 'sign_in' }, { kind: 'provider_action', action: 'use', realm: 'injected' }, { kind: 'cancel' }]) {
      assert.equal(await f.port.answer('p:provider', answer), false);
    }
    assert.equal(f.requests.some(call => call.path === '/answer'), false);
    f.button('Use this connection').click(); await flush();
    assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:provider', answer: { kind: 'provider_action', action: 'use' } });
    assert.equal(await f.port.answer('p:provider', { kind: 'provider_action', action: 'use' }), false);
    assert.equal(f.requests.filter(call => call.path === '/answer').length, 1);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 browser and device sign-in show exact inert information and only auth cancellation', async () => {
  for (const prompt of [
    { kind: 'provider_browser', target: providerTarget(), authorization_url: 'https://provider.invalid/auth?exact=<tag>\u202E' },
    { kind: 'provider_device', target: providerTarget(), verification_uri: 'https://provider.invalid/device', user_code: 'AB<CD>\u2066' },
  ]) {
    const f = serviceFixture(), view = f.mount();
    try {
      await flush(); f.provider(prompt);
      const exact = prompt.authorization_url ?? prompt.verification_uri;
      assert.ok([...f.container.querySelectorAll('code')].some(node => node.textContent === visibleText(exact)));
      if (prompt.user_code) assert.ok([...f.container.querySelectorAll('code')].some(node => node.textContent === visibleText(prompt.user_code)));
      assert.equal(f.container.querySelector('a') === null, true);
      assert.equal(f.container.querySelector('tag') === null, true);
      assert.equal(f.button('I have signed in') === undefined, true);
      assert.equal(await f.port.answer('p:provider', { kind: prompt.kind, success: true }), false);
      f.button('Cancel provider sign-in').click(); await flush();
      assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:provider', answer: { kind: 'cancel' } });
      assert.equal(f.port.available, false); assert.equal(f.requests.some(call => call.path === '/send' || call.path === '/cancel'), false);
    } finally { view.dispose(); f.dom.window.close(); }
  }
});

focused('v3 retained provider key prompt remount loses secret bytes and never repeats uncertain answers', async () => {
  const f = serviceFixture(); let view = f.mount();
  try {
    await flush(); const prompt = { kind: 'provider_api_key', target: providerTarget(), max_bytes: 8192 };
    f.provider(prompt); const detached = f.container.querySelector('input'); detached.value = 'unsubmitted fixture key';
    const oldStream = f.sources.findLast(source => !source.closed);
    view.dispose(); assert.equal(detached.value, '');
    view = f.mount(); await flush(); f.provider(prompt);
    assert.equal(f.container.querySelector('input').value, '');
    f.overrides.set('/answer', async () => { throw new Error('Synthetic lost answer response'); });
    f.container.querySelector('input').value = ' exact fixture key '; f.submit('.forge-setup__password-form'); await flush();
    assert.equal(f.port.answerState, 'uncertain');
    view.dispose(); view = f.mount(); await flush(); f.provider(prompt);
    assert.equal(f.container.querySelector('input').value, ''); assert.equal(f.button('Submit API key privately').disabled, true);
    oldStream.emit({ type: 'prompt', protocol_version: 3, target: { initialize_request_id: 'h:foreign' }, id: 'p:foreign', prompt });
    assert.equal(f.port.protectedPrompt.id, 'p:provider');
    assert.equal(f.requests.filter(call => call.path === '/answer').length, 1); assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
    assert.equal(f.sources.filter(source => !source.closed).length, 1);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 exact target transition does not fabricate scope and fences stale prompts and bindings', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush(); const prompt = { kind: 'provider_action', target: providerTarget(), actions: ['use'] };
    f.provider(prompt, 'p:wrong', { initialize_request_id: 'h:foreign' }); assert.equal(f.port.protectedPrompt, null);
    f.provider(prompt, 'p:first'); assert.equal(f.port.scope, '');
    const held = deferred(); f.overrides.set('/answer', () => held.promise);
    f.button('Use this connection').click(); await flush();
    f.closed('p:first');
    await f.ready({ ...idle(), private_notice_pending: false });
    assert.equal(f.port.scope, f.host.scope); assert.equal(f.port.available, true);
    f.provider(prompt, 'p:late', { initialize_request_id: 'h:provider-start' }); assert.equal(f.port.protectedPrompt, null);
    f.emit({ type: 'binding', protocol_version: 3, target: { initialize_request_id: 'h:provider-start' } });
    assert.equal(f.port.scope, f.host.scope); assert.equal(f.port.available, true);
    f.provider(prompt, 'p:first');
    assert.equal(f.port.protectedPrompt, null, 'A closed initializing prompt cannot be renamed into the native scope');
    held.resolve(response({ status: 'forwarded' }, 202)); await flush();
    f.provider(prompt, 'p:scoped'); assert.equal(f.port.answerState, 'idle');
    f.closed('p:scoped');
    f.provider(prompt, 'p:scoped'); assert.equal(f.port.protectedPrompt, null);
    f.bind({ scope: 'replacement' }); assert.equal(f.port.available, false); assert.equal(f.port.protectedPrompt, null);
    assert.match(f.container.querySelector('.forge-setup__status-text').textContent, /replaced/);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 held HTTP state cannot rewind a newer private binding or native activity', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush(); const held = deferred(); f.overrides.set('/state', () => held.promise);
    const reading = f.port.refresh(); await flush();
    f.bind({ scope: f.host.scope }); f.state({ ...busy('h:newer'), private_notice_pending: false });
    held.resolve(response({ status: 'starting', binding: { type: 'binding', protocol_version: 3, target: { initialize_request_id: 'h:provider-start' } } }, 202));
    await reading;
    assert.equal(f.port.snapshot.binding.scope, f.host.scope);
    assert.equal(f.port.snapshot.state.active_send.send_request_id, 'h:newer');
    assert.equal(f.port.available, true); assert.equal(f.port.canStop, true);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 approval is complete inert data and forwards no approval or target identifiers', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush();
    f.provider({ kind: 'approval', approval_id: 'owner-approval', summary: 'Exact <change>\u202E', body: { proposed: '<script>bad</script>', warnings: ['All retained warnings'] }, allowed_decisions: ['approve', 'deny'], expires_at: null });
    assert.match(f.container.querySelector('pre').textContent, /All retained warnings/);
    assert.equal(f.container.querySelector('script') === null, true);
    assert.equal(f.button('Cancel this setup interaction') === undefined, true);
    assert.equal(await f.port.answer('p:provider', { kind: 'approval', decision: 'approve', approval_id: 'owner-approval' }), false);
    f.button('Do not approve').click(); await flush();
    assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:provider', answer: { kind: 'approval', decision: 'deny' } });
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v1 defaults refuse unnegotiated provider prompts without changing the legacy conversation', async () => {
  const f = fixture(), view = f.mount();
  try {
    await flush(); f.prompt({ kind: 'provider_api_key', target: providerTarget(), max_bytes: 8192 });
    assert.equal(f.container.querySelector('input') === null, true);
    assert.equal(f.port.available, true); assert.equal(f.port.scope, f.host.scope);
    assert.ok(f.container.querySelector('.toolkit-conversation'));
    assert.equal(f.requests.some(call => call.path === '/answer'), false);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 target events retain strict pre-session state and close the exact native auth wait without success inference', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush();
    const initializing = { available: false, can_send: false, can_stop: false, outcome_uncertain: false, active_send: null, last_completion: null, private_notice_pending: false };
    f.state(initializing);
    assert.deepEqual(f.port.snapshot.state, initializing);
    assert.equal(f.port.scope, ''); assert.equal(f.port.canSend, false); assert.equal(f.port.canStop, false);
    const prompt = { kind: 'provider_browser', target: providerTarget(), authorization_url: 'https://provider.invalid/sign-in' };
    f.provider(prompt, 'p:auth'); assert.ok(f.button('Cancel provider sign-in'));
    f.closed('p:auth', 'owner_completed', { initialize_request_id: 'h:foreign' }); assert.equal(f.port.protectedPrompt.id, 'p:auth');
    f.closed('p:auth', 'owner_completed'); assert.equal(f.port.protectedPrompt, null);
    f.provider(prompt, 'p:auth'); assert.equal(f.port.protectedPrompt, null);
    assert.equal(f.port.available, false); assert.equal(f.port.snapshot.state.last_completion, null);
    assert.doesNotMatch(f.container.querySelector('.forge-setup__status-text').textContent, /success|responding/i);
    f.provider({ kind: 'provider_api_key', target: providerTarget(), max_bytes: 8192 }, 'p:key');
    f.container.querySelector('input').value = 'unsent fixture secret';
    f.emit({ type: 'event', event: { target: copy(f.serviceTarget), kind: 'unavailable', reason: 'failed' } });
    assert.equal(f.port.protectedPrompt, null); assert.equal(f.container.querySelector('input') === null, true);
    f.provider(prompt, 'p:late'); assert.equal(f.port.protectedPrompt, null);
    assert.equal(f.requests.some(call => ['/answer', '/send', '/cancel'].includes(call.path)), false);
    assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 target events reject scope aliases mixed targets and false initializing readiness', async () => {
  const f = serviceFixture(), view = f.mount();
  try {
    await flush();
    for (const event of [
      { target: copy(f.serviceTarget), kind: 'state', state: { ...idle(), private_notice_pending: false } },
      { target: copy(f.serviceTarget), kind: 'state', state: { available: false, can_send: false, can_stop: false, outcome_uncertain: false, active_send: null, last_completion: null, private_notice_pending: true } },
      { target: { ...f.serviceTarget, scope: f.host.scope }, kind: 'state', state: { ...idle(), private_notice_pending: false } },
    ]) { f.emit({ type: 'event', event }); assert.equal(f.port.snapshot.state, null); }
    await f.ready({ ...idle(), private_notice_pending: false });
    assert.equal(f.port.available, true);
    const state = copy(f.port.snapshot.state);
    f.emit({ type: 'event', event: { scope: f.host.scope, kind: 'state', state: { ...busy('h:alias'), private_notice_pending: false } } });
    f.emit({ type: 'event', event: { target: { scope: 'foreign' }, kind: 'state', state: { ...busy('h:foreign'), private_notice_pending: false } } });
    assert.deepEqual(f.port.snapshot.state, state);
    f.provider({ kind: 'provider_action', target: providerTarget(), actions: ['use'] }, 'p:action');
    f.closed('p:action', 'owner_completed'); assert.equal(f.port.protectedPrompt.id, 'p:action', 'Owner completion only closes informational auth waits');
    f.closed('p:action', 'answered'); assert.equal(f.port.protectedPrompt, null);
  } finally { view.dispose(); f.dom.window.close(); }
});

focused('v3 pre-session authentication retirement fences an entered auth cancellation through late replies and remount', async () => {
  for (const status of [401, 403]) {
    const f = serviceFixture(); let view = f.mount();
    const held = deferred();
    try {
      await flush();
      const target = copy(f.serviceTarget);
      const prompt = { kind: 'provider_browser', target: providerTarget(), authorization_url: 'https://provider.invalid/retained-private-sign-in' };
      f.provider(prompt, 'p:auth-retiring');
      const oldStream = f.sources.findLast(source => !source.closed);
      f.overrides.set('/answer', () => held.promise);
      f.button('Cancel provider sign-in').click(); await flush();
      assert.equal(f.port.scope, ''); assert.equal(f.port.answerState, 'sending');
      assert.deepEqual(f.requests.find(call => call.path === '/answer').body, { prompt_id: 'p:auth-retiring', answer: { kind: 'cancel' } });
      assert.match(f.container.textContent, /Submitting your protected answer/);
      f.overrides.set('/state', () => response({ error: { code: 'invalid_binding' } }, status));
      await f.port.refresh(); await flush();
      assert.equal(f.port.snapshot.connection, 'sign-in');
      assert.equal(f.port.protectedPrompt, null); assert.equal(f.port.available, false);
      assert.equal(f.port.canSend, false); assert.equal(f.port.canStop, false);
      assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
      assert.doesNotMatch(f.container.textContent, /retained-private-sign-in|Submitting your protected answer|cancelled|success/i);
      assert.match(f.container.textContent, /Sign in to the Console again/);
      assert.equal(f.sources.every(source => source.closed), true); assert.equal(f.timers.size, 0);
      held.resolve(response({ status: 'forwarded' }, 202)); await flush();
      oldStream.emit({ type: 'prompt', protocol_version: 3, target, id: 'p:auth-retiring', prompt });
      oldStream.emit({ type: 'event', event: { target, kind: 'prompt_closed', prompt_id: 'p:auth-retiring', disposition: 'cancelled' } });
      assert.equal(f.port.snapshot.connection, 'sign-in'); assert.equal(f.port.protectedPrompt, null);
      view.dispose(); view = f.mount(); await flush();
      oldStream.emit({ type: 'binding', protocol_version: 3, target });
      oldStream.emit({ type: 'prompt', protocol_version: 3, target, id: 'p:auth-retiring', prompt });
      assert.equal(f.port.snapshot.connection, 'sign-in'); assert.equal(f.port.protectedPrompt, null);
      assert.equal(f.container.querySelector('.forge-setup__protected').hidden, true);
      assert.doesNotMatch(f.container.textContent, /retained-private-sign-in|cancelled|success/i);
      assert.equal(f.requests.filter(call => call.path === '/initialize').length, 1);
      assert.equal(f.requests.filter(call => call.path === '/answer').length, 1);
      assert.equal(f.requests.some(call => ['/send', '/cancel', '/close'].includes(call.path)), false);
      assert.equal(f.sources.length, 1); assert.equal(f.timers.size, 0);
    } finally { held.resolve(response({ status: 'forwarded' }, 202)); view.dispose(); f.dom.window.close(); }
  }
});
