import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act } from 'react';
import { GUIDE_KEY } from './guided-state.mjs';
import { inputKey, manualKey } from './wizard-state.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const realSetTimeout = globalThis.setTimeout;
const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('guided.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent',
  jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'real-guide-inert-drawing', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /conversation-transcript\.[tj]sx$/ }, () => ({
      contents: 'export const ConversationTranscript = () => null;', loader: 'js',
    }));
    // Keep the actual guide and plan renderer. Only generated drawing is inert.
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: `import React from 'react';
        export function ProposalSimulation(props) {
          const [expanded,setExpanded]=React.useState(false);
          if (!props.plan) return React.createElement('section', {'className': 'ps-empty'}, 'Your mob takes shape here');
          return React.createElement('div', {'data-proposal-snapshot': JSON.stringify(props.plan)},
            React.createElement('button', {'aria-expanded':expanded,onClick:()=>{
              if(!expanded)props.onExpand?.();setExpanded(!expanded);
            }}, expanded?'Close fixture expanded view':'Expand fixture preview'));
        }`, loader: 'js',
    }));
  } }],
});
const { GuidedSetup } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
const id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const planFor = name => ({
  schema_version: 2, project: { name, slug: 'owned-proposal', description: 'A supplied job.' },
  provider: 'openai', model: 'gpt-6.1-sol', target: 'mac',
  agent_classes: [{ id: 'lead', name: 'Context owner', description: 'Own the supplied context.',
    instructions: 'Use the supplied evidence.', instances: 1, members: [], provider: 'openai',
    model: 'gpt-6.1-sol', tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
  topology: { mode: 'manual', entrypoint: 'lead:main', edges: [] },
  connections: [], flows: [], knowledge: { mode: 'none' }, advanced: { port: 4110 },
});
const internalStarter = planFor('Atlas Operations');
internalStarter.agent_classes[0].instances = 4;
internalStarter.flows = [{ id: 'briefing', name: 'Research briefing', description: 'Internal sample workflow.', steps: [] }];
const freshGuide = () => ({
  id, version: 1, status: 'question', stage: 'purpose', completed_stages: [], answers: [], messages: [],
  blueprint: null, proposal: null, brief: { facts: {}, requirements: [] },
  readiness: { ready: false, decision_blockers: [] }, capabilities: { available: false },
  question: { id: 'purpose', stage: 'purpose', title: 'What should your system do?',
    description: 'Describe one concrete job.', multiple: false, allow_text: true, options: [] },
});

async function mounted(initial, verify, { manual, available = false, providerRead, respondRead, retainedValues, denyStorage = false,
  narrow = false, savedGuides = [], timelineRead, runAnimationFrames = false, holdAnimationFrames = false, guideRead, component = GuidedSetup, setupDom } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const values = retainedValues || new Map(initial.id ? [[GUIDE_KEY, initial.id]] : []);
  if (manual) values.set(manualKey(initial.id), JSON.stringify(manual));
  const storage = { getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key),
    key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
  const intervals = new Map(), calls = [], publications = [], advanced = [], animationFrames = new Map();
  let nextTimer = 0, server = structuredClone(initial);
  const savedStates = new Map(savedGuides.map(guide => [guide.id, structuredClone(guide)]));
  if (initial.id) savedStates.set(initial.id, structuredClone(initial));
  const mediaListeners = new Set();
  const media = { matches: narrow, addEventListener: (_, listener) => mediaListeners.add(listener),
    removeEventListener: (_, listener) => mediaListeners.delete(listener) };
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true, localStorage: denyStorage ? { getItem() { throw new Error("Storage denied"); }, setItem() { throw new Error("Storage denied"); }, removeItem() { throw new Error("Storage denied"); } } : storage,
    requestAnimationFrame: holdAnimationFrames
      ? callback => { const key = ++nextTimer; animationFrames.set(key, callback); return key; }
      : runAnimationFrames
      ? callback => realSetTimeout(() => callback(Date.now()), 0)
      : dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: holdAnimationFrames ? key => animationFrames.delete(key)
      : runAnimationFrames ? globalThis.clearTimeout : dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: query => query === '(max-width: 760px)' ? media
      : { matches: false, addEventListener() {}, removeEventListener() {} },
    setInterval: (callback, delay) => { const key = ++nextTimer; intervals.set(key, { callback, delay }); return key; },
    clearInterval: key => intervals.delete(key),
    fetch: async (url, options = {}) => {
      const method = options.method || 'GET';
      const call = { url, method, body: options.body && JSON.parse(options.body) };
      calls.push(call);
      const providerRequest = providerRead && ['/api/provider-settings', '/api/architect/status', '/api/architect/start'].includes(url);
      assert.ok(providerRequest || method === 'GET' || method === 'POST' &&
        (url === `/api/guide/${id}/respond` || initial.id === null && url === '/api/guide/start'),
        `The fixture must not admit unrelated work: ${method} ${url}`);
      let body;
      if (providerRequest) body = await providerRead(call);
      else if (url === '/api/guide/new') body = structuredClone(server);
      else if (/^\/api\/guide\/[^/]+$/.test(url) &&
        (url === `/api/guide/${server.id}` || savedStates.has(url.slice('/api/guide/'.length)))) {
        const requested = url.slice('/api/guide/'.length);
        if (server.id) savedStates.set(server.id, structuredClone(server));
        server = structuredClone(savedStates.get(requested));
        body = guideRead ? await guideRead(call, structuredClone(server)) : structuredClone(server);
      }
      else if (url === '/api/guide/start') {
        assert.equal(server.id, null, 'Only the first explicit anonymous submission creates a saved guide.');
        assert.deepEqual(Object.keys(call.body), ['start_request_id']);
        assert.match(call.body.start_request_id, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
          'Anonymous creation keeps the existing retained request correlation.');
        server = { ...server, id, version: 1 };
        body = structuredClone(server);
      }
      else if (url === '/api/guide/list') body = { setups: savedGuides.map(guide => ({
        id: guide.id, name: guide.name || guide.proposal?.project.name || guide.blueprint?.project.name || guide.id,
        status: savedStates.get(guide.id)?.status || guide.status })) };
      else if (/^\/api\/guide\/[^/]+\/timeline(?:\?|$)/.test(url))
        body = timelineRead ? await timelineRead(call) : { frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true };
      else if (url === '/api/provider-settings') body = { runtime_active: false, work_active: false };
      else if (url === '/api/architect/status') body = { available, status: available ? 'ready' : 'unavailable' };
      else if (url === `/api/guide/${id}/respond`) {
        const request = JSON.parse(options.body);
        server = respondRead ? await respondRead(server, request) : { ...server, operation: { status: 'running', version: server.version,
          request_id: 'a'.repeat(64), request }, live_design: { guide_id: id, guide_version: server.version,
          request_id: 'a'.repeat(64), blueprint: planFor('My observing crew'),
          brief: { facts: { outcome: { summary: 'Plan our next observing session.', status: 'confirmed',
            source_message_ids: ['first-job'] } }, requirements: [] } } };
        body = structuredClone(server);
      } else body = { httpError: 404, detail: 'No existing fixture operation.' };
      return { ok: !body.httpError, status: body.httpError || 200, json: async () => body };
    },
  };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  setupDom?.(dom);
  const { createRoot } = await import(pathToFileURL(require.resolve('react-dom/client')));
  const root = createRoot(dom.window.document.getElementById('root'));
  const before = structuredClone(internalStarter);
  try {
    await act(async () => {
      root.render(React.createElement(component, { active: true, bp: internalStarter, Icon: () => null,
        onBlueprint: value => publications.push(value),
        onAdvanced: (blueprint, editorId) => advanced.push({ blueprint, editorId }),
        config: { architect: { available } } }));
      await settle();
    });
    await act(async () => settle());
    await verify({ document: dom.window.document, calls, publications, advanced, values,
      current: () => server, save: value => { server = value; },
      flushAnimationFrames: async () => {
        await act(async () => {
          const queued = [...animationFrames.values()]; animationFrames.clear();
          for (const callback of queued) callback(Date.now());
          await settle();
        });
      },
      setNarrow: async value => {
        await act(async () => { media.matches = value; for (const listener of mediaListeners) listener(); await settle(); });
      },
      reload: async (guideId = server.id) => {
        await act(async () => {
          const toggle = [...dom.window.document.querySelectorAll('.setup-history-control > button')]
            .find(button => button.textContent.includes('Your setups'));
          assert.ok(toggle, 'Reload uses the existing saved setup selector.');
          toggle.click(); await settle();
        });
        await act(async () => {
          const index = savedGuides.findIndex(guide => guide.id === guideId);
          assert.ok(index >= 0, 'The fixture exposes the selected saved setup.');
          const choice = [...dom.window.document.querySelectorAll('.setup-history-menu .setup-history-conversation')][index];
          assert.ok(choice, 'The selected setup has its ordinary history control.');
          choice.click(); await settle();
        });
      },
      poll: async () => {
        const timer = [...intervals.values()].find(item => item.delay === 2500);
        assert.ok(timer, 'A running guide has its ordinary saved-state poll.');
        await act(async () => { await timer.callback(); await settle(); });
      },
      pollProvider: async () => {
        const timer = [...intervals.values()].find(item => item.delay === 4000);
        assert.ok(timer, 'Provider startup retains its ordinary status observation.');
        await act(async () => { await timer.callback(); await settle(); });
      } });
    assert.deepEqual(internalStarter, before, 'The internal editor scaffold stays intact.');
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

test('fresh mounted guide never presents the non-null workbench starter as a user proposal', async () => {
  await mounted(freshGuide(), async ({ document, calls, publications }) => {
    const pane = document.querySelector('.setup-plan-pane');
    assert.ok(pane.querySelector('.ep-empty-facts'));
    assert.doesNotMatch(pane.textContent, /Atlas Operations|Research briefing|4 agents/);
    assert.equal(!!pane.querySelector('.ep-stats'), false);
    assert.equal(!!pane.querySelector('[data-proposal-snapshot]'), false);
    assert.match(document.body.textContent, /Provider setup needed/);
    assert.deepEqual(publications, [internalStarter], 'The non-null internal editor draft is still published.');
    assert.ok(calls.every(call => call.method === 'GET'), 'Opening the empty guide admits no work.');
    assert.ok(calls.every(call => !call.url.startsWith('/api/visualization')), 'No starter animation is requested.');
  });
});

test('first actual staged proposal appears immediately and survives its saved publication', async () => {
  const guide = freshGuide(); guide.capabilities.available = true;
  await mounted(guide, async ({ document, current, save, poll }) => {
    assert.ok(document.querySelector('.ep-empty-facts'));
    const textarea = document.querySelector('#setup-message');
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value')
        .set.call(textarea, 'Plan an observing session for our club.');
      textarea.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
    });
    const send = document.querySelector('#setup-send');
    assert.equal(send.disabled, false);
    await act(async () => { send.click(); await settle(); });
    const snapshot = () => JSON.parse(document.querySelector('[data-proposal-snapshot]').dataset.proposalSnapshot);
    assert.equal(snapshot().project.name, 'My observing crew');
    assert.match(document.querySelector('.setup-plan-pane').textContent, /Checked draft preview/);
    const running = current();
    save({ ...running, version: 2, proposal: running.live_design.blueprint, brief: running.live_design.brief,
      operation: { ...running.operation, status: 'completed' } });
    await poll();
    assert.equal(snapshot().project.name, 'My observing crew');
    assert.doesNotMatch(document.querySelector('.setup-plan-pane').textContent, /Atlas Operations|Checked draft preview/);
  }, { available: true });
});

test('fresh provider-unavailable guide can deliberately open the internal starter in the advanced editor', async () => {
  await mounted(freshGuide(), async ({ document, calls, advanced }) => {
    const pane = document.querySelector('.setup-plan-pane');
    const button = [...pane.querySelectorAll('button')]
      .find(item => item.textContent.includes('Advanced editor'));
    assert.ok(button, 'Manual setup remains available before a proposal exists.');
    assert.equal(button.disabled, false);
    const before = structuredClone(calls);
    await act(async () => { button.click(); await settle(); });
    assert.equal(advanced.length, 1);
    assert.deepEqual(advanced[0].blueprint, internalStarter);
    assert.match(advanced[0].editorId, /^[a-f0-9-]{36}$/);
    assert.deepEqual(calls, before, 'Opening the editor admits no guide or animation work.');
    assert.doesNotMatch(pane.textContent, /Atlas Operations|Research briefing|4 agents/);
    assert.equal(!!pane.querySelector('[data-proposal-snapshot]'), false);
  });
});

test('fresh welcome mobile Advanced editor preserves an unsent idea without creating a proposal or guide turn', async () => {
  const idea = 'Help our neighbourhood share tools without losing track of who has them.';
  const anonymous = { ...freshGuide(), id: null, version: 0, messages: [{ role: 'assistant',
    text: "Let's start with something useful you want to get done.", stage: 'purpose', action: 'start' }] };
  await mounted(anonymous, async ({ document, calls, advanced, values, current }) => {
    assert.ok(document.querySelector('.setup-welcome'));
    const textarea = document.querySelector('#setup-message');
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value')
        .set.call(textarea, idea);
      textarea.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
      await settle();
    });
    const editor = document.querySelector('.setup-composer-area button.forge-welcome-mobile-editor');
    assert.ok(editor, 'The mobile welcome has its own route to the existing Advanced editor.');
    assert.equal(editor.textContent.trim(), 'Advanced editor');
    assert.equal(editor.disabled, false, 'Manual editing does not require a configured provider.');
    assert.equal(JSON.parse(values.get(inputKey(anonymous))).draft, idea);
    const requests = structuredClone(calls), savedDrafts = new Map(values);
    await act(async () => { editor.click(); await settle(); });
    assert.equal(advanced.length, 1);
    assert.deepEqual(advanced[0].blueprint, internalStarter, 'The editor receives the existing scaffold, not an invented proposal.');
    assert.match(advanced[0].editorId, /^[a-f0-9-]{36}$/);
    assert.equal(document.querySelector('#setup-message').value, idea);
    assert.deepEqual(values, savedDrafts, 'Opening the editor preserves the exact unsent draft bytes.');
    assert.deepEqual(current(), anonymous);
    assert.equal(document.querySelector('[data-proposal-snapshot]') === null, true);
    assert.equal(document.querySelector('.setup-provider') === null, true);
    assert.deepEqual(calls, requests, 'Opening the editor sends no guide, provider or animation request.');
    assert.ok(calls.every(call => call.method === 'GET'));
  });
});

test('saved proposals and genuine pending manual overrides retain their visible designs', async () => {
  const saved = { ...freshGuide(), proposal: planFor('Saved astronomy team') };
  await mounted(saved, async ({ document }) => {
    assert.match(document.querySelector('.setup-plan-pane').textContent, /Saved astronomy team/);
    assert.equal(!!document.querySelector('.ep-empty-facts'), false);
  });
  const manual = { blueprint: planFor('My exact manual team'),
    changes: ['Project name changed'], text: 'Review my precise changes.' };
  await mounted(saved, async ({ document, values }) => {
    const pane = document.querySelector('.setup-plan-pane');
    assert.match(pane.textContent, /My exact manual team/);
    assert.match(pane.textContent, /Your changes are waiting for guide review/);
    assert.equal(JSON.parse(pane.querySelector('[data-proposal-snapshot]').dataset.proposalSnapshot).project.name,
      'My exact manual team');
    assert.deepEqual(JSON.parse(values.get(manualKey(id))), manual);
  }, { manual });
});

const unconfiguredProviders = () => ({
  providers: ['openai', 'anthropic', 'gemini'].map(provider => ({ provider, configured: false,
    source: null, editable: true })), effective_provider: null, runtime_active: false,
  work_active: false, load_error: null,
});
const buttonNamed = (parent, name) => {
  const button = [...parent.querySelectorAll('button')].find(item => item.textContent === name ||
    item.closest('.forge-starting-conversation__choices') && item.querySelector('strong')?.textContent === name ||
    name === 'Close' && item.getAttribute('aria-label') === 'Close provider settings');
  assert.ok(button, `Expected ${name}`);
  return button;
};

test('opening context crosses the provider handoff without sending or rewriting the draft', async () => {
  await mounted(freshGuide(), async ({ document, calls }) => {
    await act(async () => {
      document.querySelector('.forge-starting-conversation__invitation button[aria-expanded]').click(); await settle();
    });
    await act(async () => {
      buttonNamed(document.querySelector('.forge-starting-conversation'), 'A team').click(); await settle();
    });
    await act(async () => {
      buttonNamed(document.querySelector('.forge-starting-conversation'), 'Keeping track of things').click(); await settle();
    });
    assert.equal(document.querySelector('#setup-message').value, '');
    const context = document.querySelector('.forge-starting-context').textContent;
    await act(async () => { document.querySelector('#setup-send').click(); await settle(); });
    const summary = document.querySelector('.setup-provider-starting-context');
    assert.ok(summary, 'The actual guide passes its context to provider setup.');
    assert.match(summary.textContent, /A team.*Keeping track of things/s);
    assert.match(summary.textContent, /Not sent/);
    assert.equal(document.querySelector('.setup-provider-idea') === null, true);
    await act(async () => { buttonNamed(document.querySelector('.setup-provider'), 'Back to my idea').click(); await settle(); });
    assert.equal(document.querySelector('#setup-message').value, '');
    assert.equal(document.querySelector('.forge-starting-context').textContent, context);
    assert.ok(calls.every(call => call.method === 'GET'), 'Connecting navigation never submits the context.');
  }, { providerRead: async call => call.url === '/api/provider-settings' ? unconfiguredProviders()
    : { available: false, status: 'unavailable' } });
});

test('explicit provider onboarding retains save and startup handoffs in the mounted guide', async () => {
  let configured = false, runtime = { available: false, status: 'unavailable' }, finishStart;
  const starting = new Promise(resolve => { finishStart = resolve; });
  const providerRead = async call => {
    if (call.url === '/api/provider-settings') {
      if (call.method === 'PUT') {
        assert.deepEqual(call.body, { provider: 'openai', api_key: 'synthetic-first-use-key' });
        configured = true;
        runtime = { available: true, status: 'stopped' };
      } else assert.equal(call.method, 'GET');
      const settings = unconfiguredProviders();
      return configured ? { ...settings, effective_provider: 'openai', runtime_active: runtime.status !== 'stopped',
        providers: settings.providers.map(row => row.provider === 'openai'
          ? { ...row, configured: true, source: 'saved' } : row) } : settings;
    }
    if (call.url === '/api/architect/status') {
      assert.equal(call.method, 'GET');
      return structuredClone(runtime);
    }
    assert.equal(call.url, '/api/architect/start');
    assert.equal(call.method, 'POST');
    runtime = { available: true, status: 'starting' };
    return starting;
  };
  await mounted(freshGuide(), async ({ document, calls, pollProvider }) => {
    const pane = () => document.querySelector('.setup-provider');
    assert.equal(pane() === null, true, 'The initial guide lets the user describe an idea before provider setup.');
    const headerProvider = document.querySelector('#setup-header-menu button');
    assert.match(headerProvider.textContent, /Guide settings/);
    await act(async () => { headerProvider.click(); await settle(); });
    assert.ok(pane(), 'The explicit header action opens provider onboarding.');
    const original = pane();
    const input = pane().querySelector('input');
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLInputElement.prototype, 'value')
        .set.call(input, 'synthetic-first-use-key');
      input.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
      input.dispatchEvent(new document.defaultView.Event('change', { bubbles: true }));
    });
    await act(async () => { buttonNamed(pane(), 'Save locally').click(); await settle(); });
    assert.equal(pane() === original, true, 'Saved credentials do not retire the mounted onboarding owner.');
    assert.match(pane().textContent, /Saved privately on the computer running Forge/);
    assert.equal(pane().querySelector('input').value, '');
    assert.equal(buttonNamed(pane(), 'Start guide').disabled, false);
    assert.equal(calls.filter(call => call.method === 'POST').length, 0, 'Saving does not start native work.');

    await act(async () => { buttonNamed(pane(), 'Start guide').click(); await settle(); });
    assert.equal(buttonNamed(pane(), 'Back to my idea').disabled, true);
    await pollProvider();
    assert.equal(pane() === original, true, 'An available but starting observation preserves the startup owner.');
    assert.ok(buttonNamed(pane(), 'Starting guide...'));
    await act(async () => { headerProvider.click(); await settle(); });
    assert.equal(pane() === original, true, 'The header action cannot retire an owned startup operation.');
    assert.equal(buttonNamed(pane(), 'Back to my idea').disabled, true);
    assert.equal(calls.filter(call => call.url === '/api/architect/start').length, 1);
    runtime = { available: true, status: 'ready' };
    await act(async () => { finishStart(structuredClone(runtime)); await settle(); });
    await act(async () => settle());
    assert.equal(pane() === original, true, 'The confirmed-ready message remains available until explicit close.');
    assert.match(pane().textContent, /Setup team ready/);
    assert.equal(buttonNamed(pane(), 'Back to my idea').disabled, false);
    assert.equal(calls.filter(call => call.url === '/api/architect/start').length, 1);
    assert.ok(document.querySelector('.ep-empty-facts'));
    assert.equal(document.querySelector('[data-proposal-snapshot]') === null, true);
    assert.ok(calls.every(call => !call.url.startsWith('/api/visualization') && !call.url.endsWith('/respond')));
    await act(async () => { buttonNamed(pane(), 'Back to my idea').click(); await settle(); });
    assert.equal(pane() === null, true, 'Explicit close retires the completed onboarding.');
  }, { providerRead });
});

test('explicit provider onboarding honors close while the initial guide remains unconfigured', async () => {
  await mounted(freshGuide(), async ({ document, calls }) => {
    assert.equal(document.querySelector('.setup-provider') === null, true);
    const headerProvider = document.querySelector('#setup-header-menu button');
    assert.match(headerProvider.textContent, /Guide settings/);
    await act(async () => { headerProvider.click(); await settle(); });
    const pane = document.querySelector('.setup-provider');
    assert.ok(pane);
    await act(async () => { buttonNamed(pane, 'Back to my idea').click(); await settle(); });
    assert.equal(document.querySelector('.setup-provider') === null, true);
    const input = document.querySelector('#setup-message');
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value')
        .set.call(input, 'My unfinished description');
      input.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
      await settle();
    });
    assert.equal(input.value, 'My unfinished description');
    assert.equal(document.querySelector('.setup-provider') === null, true, 'Later renders do not reopen explicitly dismissed onboarding.');
    assert.match(document.body.textContent, /Provider setup needed/);
    assert.ok(document.querySelector('.ep-empty-facts'));
    assert.ok(calls.every(call => call.method === 'GET'), 'Closing and drafting admit no work.');
  });
});

test('initial guide connection retains the idea through explicit provider setup and requires a new explicit send', async () => {
  const idea = 'Help our neighbourhood share gardening tools.';
  const key = 'synthetic-welcome-provider-key';
  let configured = false, runtime = { available: false, status: 'unavailable' };
  const providerRead = async call => {
    if (call.url === '/api/provider-settings') {
      if (call.method === 'PUT') {
        assert.deepEqual(call.body, { provider: 'openai', api_key: key });
        configured = true;
        runtime = { available: true, status: 'stopped' };
      } else assert.equal(call.method, 'GET');
      const settings = unconfiguredProviders();
      return configured ? { ...settings, effective_provider: 'openai', runtime_active: runtime.status === 'ready',
        providers: settings.providers.map(row => row.provider === 'openai'
          ? { ...row, configured: true, source: 'saved' } : row) } : settings;
    }
    if (call.url === '/api/architect/status') {
      assert.equal(call.method, 'GET');
      return structuredClone(runtime);
    }
    assert.equal(call.url, '/api/architect/start');
    assert.equal(call.method, 'POST');
    assert.equal(configured, true);
    runtime = { available: true, status: 'ready' };
    return structuredClone(runtime);
  };
  const anonymous = { ...freshGuide(), id: null, version: 0, messages: [{ role: 'assistant',
    text: "Let's start with something useful you want to get done.", stage: 'purpose', action: 'start' }] };
  await mounted(anonymous, async ({ document, calls, values }) => {
    const guidePosts = () => calls.filter(call => call.method === 'POST' && call.url.startsWith('/api/guide/'));
    const pane = () => document.querySelector('.setup-provider');
    const message = () => document.querySelector('#setup-message');
    assert.ok(document.querySelector('.setup-welcome'));
    assert.ok(document.querySelector('.setup-welcome-intro h1, .setup-welcome-intro h2'));
    assert.equal(pane() === null, true, 'The technical provider form does not precede the initial idea.');
    assert.equal(calls.some(call => call.url === '/api/provider-settings'), false);
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value')
        .set.call(message(), idea);
      message().dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
      await settle();
    });
    await act(async () => { document.querySelector('.forge-example-disclosure').open = true; });
    const example = [...document.querySelectorAll('.setup-starting-points button')]
      .find(button => button.textContent.startsWith('Stay on top of my projects'));
    assert.ok(example, 'The existing starting-point preview remains available.');
    await act(async () => { example.click(); await settle(); });
    assert.equal(message().value, idea, 'An example does not replace an already written idea.');
    assert.equal(pane() === null, true);
    assert.equal(guidePosts().length, 0, 'Typing and previewing an example send no model turn.');
    const primary = document.querySelector('#setup-send');
    assert.ok(primary.classList.contains('setup-welcome-continue'));
    assert.equal(primary.textContent.trim(), 'Connect your guide');
    assert.equal(primary.disabled, false);
    assert.match(document.querySelector('.forge-welcome-shortcuts').textContent,
      /Enter to connect your guide.*Shift \+ Enter for a new line/);
    await act(async () => { primary.click(); primary.click(); await settle(); });
    assert.ok(pane());
    assert.ok(document.querySelector('.setup-welcome-provider'));
    assert.equal(document.querySelector('#setup-send').disabled, true,
      'The retained idea cannot be submitted while its first provider form is open.');
    assert.equal(message().value, idea);
    assert.equal(guidePosts().length, 0, 'The first action opens settings instead of admitting a guide turn.');
    assert.equal(calls.filter(call => call.url === '/api/provider-settings' && call.method === 'GET').length, 1,
      'Repeated captured guide connection clicks do not remount the provider or duplicate its initial observation.');
    const providerInput = pane().querySelector('input[type="password"]');
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLInputElement.prototype, 'value')
        .set.call(providerInput, key);
      providerInput.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
      providerInput.dispatchEvent(new document.defaultView.Event('change', { bubbles: true }));
    });
    await act(async () => { buttonNamed(pane(), 'Save locally').click(); await settle(); });
    assert.match(pane().textContent, /saving does not check provider access/);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.equal(message().value, idea);
    assert.equal(guidePosts().length, 0, 'Save does not send the retained idea.');
    assert.equal(calls.some(call => call.url === '/api/architect/start'), false);
    await act(async () => { buttonNamed(pane(), 'Start guide').click(); await settle(); });
    assert.match(pane().textContent, /Setup team ready/);
    assert.equal(document.querySelector('#setup-send').disabled, true,
      'Ready provider status does not enable a competing send before settings are explicitly closed.');
    assert.equal(message().value, idea);
    assert.equal(guidePosts().length, 0, 'Start does not send the retained idea.');
    await act(async () => { buttonNamed(pane(), 'Back to my idea').click(); await settle(); });
    assert.equal(pane() === null, true);
    assert.equal(message().value, idea);
    assert.equal(document.querySelector('#setup-send').disabled, false);
    assert.equal(document.querySelector('#setup-send').textContent.trim(), 'Send to guide');
    assert.equal(guidePosts().length, 0, 'Closing confirmed settings does not submit a turn.');
    assert.ok([...values.values()].some(value => value.includes(idea)), 'The anonymous idea remains in its existing draft store.');
    assert.ok([...values.values()].every(value => !value.includes(key)), 'Provider secrets never enter guide draft storage.');
    await act(async () => { document.querySelector('#setup-send').click(); await settle(); });
    assert.deepEqual(guidePosts().map(call => call.url), ['/api/guide/start', `/api/guide/${id}/respond`]);
    assert.deepEqual(guidePosts()[1].body, { version: 1, action: 'answer', selected: [], text: idea });
    assert.equal(calls.filter(call => call.url === '/api/architect/start').length, 1);
    assert.equal(document.querySelector('.setup-welcome') === null, true, 'An admitted running guide leaves the initial welcome.');
  }, { providerRead });
});

test('optional starting context stays separate from writing and is consumed only by the explicit guide send', async () => {
  const anonymous = { ...freshGuide(), id: null, version: 0, capabilities: { available: true }, messages: [] };
  await mounted(anonymous, async ({ document, calls, values }) => {
    const helper = () => document.querySelector('.forge-starting-help');
    const textarea = () => document.querySelector('#setup-message');
    const writes = () => calls.filter(call => call.method !== 'GET');
    await clickStarting(document, 'Answer two questions');
    assert.equal(helper().dataset.open, 'true');
    await clickStarting(document, 'A team');
    assert.equal(textarea().value, '', 'The audience answer stays out of the writing.');
    assert.deepEqual(writes(), []);
    await clickStarting(document, 'Repetitive work');
    assert.equal(textarea().value, '', 'No stock prose is inserted into the composer.');
    assert.equal(helper().dataset.open, 'false');
    assert.equal(document.activeElement === document.querySelector('#setup-send'), true, 'A choices-only start exposes Send without opening the native editor.');
    assert.match(document.querySelector('.forge-starting-context__sentence').textContent, /For a team, help with repetitive work\./);
    const saved = JSON.parse(values.get(inputKey(anonymous)));
    assert.equal(saved.draft, '');
    assert.deepEqual(saved.starting_help, { version: 2, audience_id: 'team', kind_id: 'repetitive' });
    assert.deepEqual(writes(), [], 'Local choices never send guide work or settings.');
    assert.equal(document.querySelector('#setup-send').disabled, false, 'A novice can start with only the two choices.');
    assert.equal(document.querySelector('#setup-send').textContent.trim(), 'Send to guide');
    assert.equal(document.querySelector('#setup-after-send p').textContent, 'Your two answers stay here until you send.');
    await act(async () => { document.querySelector('#setup-send').click(); await settle(); });
    assert.deepEqual(writes().map(call => call.url), ['/api/guide/start', `/api/guide/${id}/respond`]);
    assert.deepEqual(writes()[1].body, { version: 1, action: 'answer', selected: [], text: 'For a team, I would like help with repetitive work.' });
    assert.equal(textarea().value, '');
    assert.equal(JSON.parse(values.get(inputKey({ ...anonymous, id, version: 1 }))).starting_help, undefined);
  }, { available: true, runAnimationFrames: true });
});

test('first-minute mobile choices-only completion focuses Send while explicit Write keeps the saved context', async () => {
  const anonymous = { ...freshGuide(), id: null, version: 0, capabilities: { available: true }, messages: [] };
  await mounted(anonymous, async ({ document, calls, values }) => {
    const editor = document.querySelector('#setup-message');
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'My household'); await clickStarting(document, 'Keeping track of things');
    const send = document.querySelector('#setup-send'), helper = document.querySelector('.forge-starting-conversation');
    const saved = JSON.parse(values.get(inputKey(anonymous)));
    assert.equal(document.activeElement === send, true, 'Completing mobile choices avoids opening the editor keyboard.');
    assert.equal(send.disabled, false); assert.equal(send.textContent.trim(), 'Send to guide');
    assert.equal(editor.value, ''); assert.equal(document.querySelectorAll('textarea').length, 1);
    assert.equal(helper.dataset.open, 'false'); assert.equal(helper.dataset.route, 'choose');
    assert.equal(startingContextButton(document, 'Your two answers').getAttribute('aria-pressed'), 'true');
    assert.equal(document.getElementById('setup-idea-shortcuts') === null, true, 'Mobile retains ordinary multiline writing rather than desktop Enter guidance.');
    assert.deepEqual(saved.starting_help, { version: 2, audience_id: 'household', kind_id: 'organizing' });
    await clickStarting(document, 'In my own words');
    assert.equal(document.activeElement === editor, true, 'Only explicit Write focuses the native editor after a choices-only start.');
    assert.equal(helper.dataset.route, 'write'); assert.equal(helper.dataset.open, 'false');
    assert.equal(document.querySelector('#setup-message') === editor, true);
    assert.deepEqual(JSON.parse(values.get(inputKey(anonymous))), saved, 'Changing route preserves the exact draft and committed choices.');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), [], 'Completion and explicit Write never send a turn or start runtime work.');
  }, { available: true, narrow: true, runAnimationFrames: true });
});

test('the native editor and optional thinking helper never replace or send the exact idea', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  const written = '  My actual idea\nKeep <script>literal</script> and every space.  \t';
  await mounted(guide, async ({ document, calls, values }) => {
    const helper = document.querySelector('.forge-starting-conversation');
    const editor = document.querySelector('#setup-message');
    const invitation = helper.querySelector('.forge-starting-conversation__invitation');
    assert.equal(invitation.getAttribute('role'), 'group', 'The starting routes use an ordinary group rather than tabs.');
    assert.equal(invitation.querySelector('[role="tab"]') === null, true);
    assert.equal(invitation.querySelectorAll('button').length, 2, 'Both routes remain available beside the one native editor.');
    const writeRoute = startingContextButton(document, 'In my own words');
    const chooseRoute = startingContextButton(document, 'Answer two questions');
    assert.equal(writeRoute.getAttribute('aria-pressed'), 'true');
    assert.equal(writeRoute.getAttribute('aria-controls'), 'setup-message');
    assert.equal(chooseRoute.getAttribute('aria-pressed'), 'false');
    assert.match(invitation.textContent, /Write.*In my own words.*Describe a task or a rough idea.*Choose.*Answer two questions.*Pick who it helps and what kind of work/s);
    assert.equal(helper.querySelector('.forge-starting-conversation__lead').textContent, 'Two ways to begin. Switch any time - your words are kept.');
    await typeStarting(document, ' \n\t ');
    await clickStarting(document, 'Answer two questions');
    assert.equal(helper.querySelector('.forge-starting-conversation__draft') === null, true, 'Whitespace-only writing has no draft excerpt.');
    assert.equal(document.querySelector('#setup-message') === editor, true);
    assert.equal(writeRoute.getAttribute('aria-pressed'), 'false');
    assert.equal(chooseRoute.getAttribute('aria-pressed'), 'true');
    assert.equal(writeRoute.closest('[hidden]') === null, true, 'Writing remains available while choices are open.');
    await clickStarting(document, 'In my own words');
    assert.equal(helper.dataset.open, 'false');
    assert.equal(writeRoute.getAttribute('aria-pressed'), 'true');
    assert.equal(chooseRoute.getAttribute('aria-pressed'), 'false');
    assert.equal(document.activeElement === editor, true, 'The Write route returns focus through the guarded native editor handoff.');
    assert.equal(document.querySelector('#setup-message') === editor, true, 'Switching routes keeps the original native editor mounted.');
    await typeStarting(document, written);
    editor.focus();
    assert.equal(document.activeElement === editor, true, 'Writing deliberately returns focus to the existing editor.');
    assert.equal(document.querySelectorAll('textarea').length, 1, 'The opening has one user-owned editor.');
    assert.equal(editor.value, written);
    assert.equal(document.querySelector('.forge-starting-conversation') === helper, true);
    assert.equal(helper.dataset.open, 'false', 'Writing does not open or answer the context questions.');
    assert.equal(JSON.parse(values.get(inputKey(guide))).starting_help, undefined);
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);

    const think = startingContextButton(document, 'Answer two questions');
    assert.equal(think.getAttribute('aria-expanded'), 'false');
    const contentId = think.getAttribute('aria-controls');
    assert.equal(document.getElementById(contentId).hidden, true);
    await clickStarting(document, 'Answer two questions');
    assert.equal(document.querySelector('.forge-starting-conversation') === helper, true, 'Thinking opens the same mounted helper.');
    assert.equal(helper.dataset.open, 'true');
    assert.equal(document.getElementById(contentId).hidden, false, 'The invitation opens the controlled questions.');
    const excerpt = helper.querySelector('.forge-starting-conversation__draft');
    assert.equal(excerpt.querySelector('span').textContent, 'Your idea - not sent');
    assert.equal(excerpt.querySelector('p').textContent, written, 'The excerpt renders every draft character as literal React text.');
    assert.equal(excerpt.querySelector('button, input, textarea, [contenteditable], script') === null, true, 'The excerpt is read-only and creates no extra editor or executable markup.');
    assert.equal(document.querySelector('#setup-message') === editor, true);
    assert.equal(helper.querySelector('h3').textContent, 'Who is it for?');
    assert.equal(document.activeElement === helper.querySelector('h3'), true);
    assert.equal(startingContextButton(document, 'A team').getAttribute('aria-pressed'), 'false');
    await clickStarting(document, 'A team');
    assert.equal(helper.querySelector('h3').textContent, 'What would help most?');
    await clickStarting(document, 'Answer two questions');
    assert.equal(helper.querySelector('h3').textContent, 'What would help most?', 'Selecting the already open Choose route does not reset staged answers.');
    assert.equal(invitation.querySelectorAll('button').length, 2);
    assert.equal(helper.querySelector('.forge-starting-conversation__draft') === excerpt, true, 'Moving between questions retains the same read-only excerpt.');
    assert.equal(excerpt.querySelector('p').textContent, written);
    assert.equal(document.querySelector('#setup-message') === editor, true);
    assert.equal(editor.value, written, 'The first choice never inserts context prose.');
    assert.equal(JSON.parse(values.get(inputKey(guide))).starting_help, undefined);
    await clickStarting(document, 'Repetitive work');
    const context = document.querySelector('.forge-starting-context');
    assert.equal(context.querySelector('.forge-starting-context__heading').textContent, 'Starting pointNot sent',
      'The committed choices visibly remain unsent context.');
    assert.equal(document.querySelector('.forge-starting-conversation') === helper, true);
    assert.equal(document.querySelector('#setup-message') === editor, true);
    assert.equal(document.querySelectorAll('textarea').length, 1);
    assert.equal(editor.value, written);
    assert.equal(document.activeElement === editor, true);
    const saved = JSON.parse(values.get(inputKey(guide)));
    assert.equal(saved.draft, written);
    assert.deepEqual(saved.starting_help, { version: 2, audience_id: 'team', kind_id: 'repetitive' });
    assert.equal(helper.querySelector('.forge-starting-conversation__draft') === null, true, 'Closing choices removes the excerpt when the editor returns.');
    assert.equal(helper.querySelector('.forge-starting-conversation__invitation') === invitation, true, 'Applied context retains both mounted route controls.');
    assert.equal(writeRoute.getAttribute('aria-pressed'), 'false');
    assert.equal(chooseRoute.getAttribute('aria-pressed'), 'true');
    assert.equal(chooseRoute.closest('[hidden]') === null, true);
    assert.equal(chooseRoute.querySelector('strong').textContent, 'Your two answers');
    assert.equal(chooseRoute.querySelector('small').textContent, 'A team / Repetitive work');
    await clickStarting(document, 'Your two answers');
    assert.equal(helper.dataset.open, 'false', 'The answered Choose route keeps the committed review visible.');
    const field = startingContextButton(document, 'Change who this helps: A team'); field.focus();
    await clickStarting(document, 'Change who this helps: A team');
    assert.equal(startingContextButton(document, 'A team').getAttribute('aria-pressed'), 'true', 'The individual review field opens its applied answer for editing.');
    assert.equal(helper.querySelector('.forge-starting-conversation__draft p').textContent, written);
    await clickStarting(document, 'In my own words');
    assert.equal(helper.querySelector('.forge-starting-conversation__draft') === null, true);
    assert.equal(document.querySelector('#setup-message') === editor, true, 'Cancelling a choice edit keeps the same native textarea.');
    assert.equal(editor.value, written);
    assert.equal(document.activeElement === editor, true);
    assert.deepEqual(JSON.parse(values.get(inputKey(guide))), saved, 'The read-only excerpt and cancelled choice edit do not change saved writing or context.');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), [], 'Neither starting path sends the idea or changes provider settings.');
  }, { available: true, runAnimationFrames: true });
});

test('saved understanding and pending work do not become a fresh welcome merely because no plan exists', async t => {
  const cases = [
    ['brief fact', guide => { guide.brief.facts = { outcome: { summary: 'Track our next observing session.',
      status: 'confirmed', source_message_ids: ['original-user'] } }; }],
    ['requirement', guide => { guide.brief.requirements = [{ outcome: 'Preserve the requested owner approval.', disposition: 'open' }]; }],
    ['warning', guide => { guide.warnings = ['A saved integration is still unverified.']; }],
    ['feature gap', guide => { guide.feature_gaps = [{ outcome: 'Retain an existing connector requirement.', disposition: 'handoff' }]; }],
    ['answer', guide => { guide.answers = [{ stage: 'purpose', text: 'Help our existing crew.', selected: [] }]; }],
    ['completed stage', guide => { guide.completed_stages = ['purpose']; }],
    ['user message', guide => { guide.messages = [{ id: 'retained-user', role: 'user', text: 'My original idea is already saved.' }]; }],
    ['offered choice', guide => { guide.question.options = [{ id: 'continue', label: 'Keep the saved scope', description: 'Retain the existing decision.' }]; }],
    ['running operation', guide => { guide.operation = { status: 'running', version: guide.version,
      request_id: 'c'.repeat(64), request: { version: guide.version, action: 'answer', selected: [], text: 'My saved answer.' } }; }],
    ['failed operation', guide => { guide.operation = { status: 'failed', version: guide.version,
      request_id: 'd'.repeat(64), detail: 'The saved response could not finish.',
      request: { version: guide.version, action: 'answer', selected: [], text: 'My failed saved answer.' } }; }],
    ['interrupted operation', guide => { guide.operation = { status: 'interrupted', version: guide.version,
      request_id: 'e'.repeat(64), detail: 'The saved response was interrupted.',
      request: { version: guide.version, action: 'answer', selected: [], text: 'My interrupted saved answer.' } }; }],
    ['manual adjustment', () => {}, { blueprint: planFor('Retained manual work'), changes: ['Project name changed'],
      text: 'Review these retained changes.' }],
  ];
  for (const [name, change, manual] of cases) await t.test(name, async () => {
    const guide = freshGuide();
    change(guide);
    await mounted(guide, async ({ document, calls, current, values }) => {
      assert.equal(document.querySelector('.setup-welcome') === null, true, `${name} remains in the continuing guide.`);
      const send = document.querySelector('.setup-welcome-continue');
      if (guide.operation?.status === 'running') {
        assert.ok(send, 'Retained first-turn work keeps its disabled waiting control.');
        assert.equal(send.disabled, true);
        assert.match(send.textContent, /Waiting for guide/);
      } else assert.equal(send === null, true);
      assert.ok(document.querySelector('.setup-plan-pane'), 'The existing plan and understanding surface stays mounted.');
      assert.deepEqual(current(), guide, 'Rendering welcome boundaries does not rewrite the saved setup.');
      if (['failed', 'interrupted'].includes(guide.operation?.status)) {
        const failure = document.querySelector(`[data-operation-status="${guide.operation.status}"]`);
        assert.ok(failure, 'The original saved operation retains its recovery surface.');
        assert.match(failure.textContent, new RegExp(guide.operation.request.text));
        assert.equal(buttonNamed(failure, 'Retry saved answer').disabled, false,
          'Returning to failed or interrupted work retains deliberate retry.');
      }
      if (manual) assert.deepEqual(JSON.parse(values.get(manualKey(id))), manual);
      assert.ok(calls.every(call => call.method === 'GET'), 'Returning context admits no turn or native action.');
    }, { manual });
  });
});

for (const narrow of [false, true]) test(`first response retains the circular illustration and gathered context until a real proposal replaces it (${narrow ? 'phone' : 'desktop'})`, async () => {
  const guide = freshGuide(); guide.capabilities.available = true;
  let admitFirstResponse;
  const firstResponse = new Promise(resolve => { admitFirstResponse = resolve; });
  await mounted(guide, async ({ document, current, save, poll, calls }) => {
    const pane = document.querySelector('.setup-plan-pane');
    assert.ok(pane.querySelector('.forge-welcome-portrait'));
    const motion = () => [...pane.querySelectorAll('button')].find(button => /^(Pause|Resume) motion$/.test(button.textContent));
    assert.ok(pane.querySelector('animateTransform'), 'The accepted circular illustration starts animated.');
    await act(async () => motion().click());
    assert.equal(motion().getAttribute('aria-pressed'), 'true');
    assert.equal(pane.querySelector('animateTransform, animate') === null, true, 'Pausing removes indefinite animation from the desktop illustration.');
    assert.equal(document.querySelector('.forge-welcome-mobile-art animateTransform, .forge-welcome-mobile-art animate') === null, true,
      'The phone miniature shares the same motion preference.');
    assert.equal(document.querySelector('.forge-welcome-dialog animateTransform, .forge-welcome-dialog animate') === null, true,
      'The phone illustration sheet shares the same motion preference.');
    const textarea = document.querySelector('#setup-message');
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value')
        .set.call(textarea, 'Help our neighbourhood share gardening tools.');
      textarea.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
    });
    await act(async () => { document.querySelector('#setup-send').click(); await settle(); });
    const shell = document.querySelector('.setup-shell');
    assert.ok(shell.classList.contains('setup-first-turn'), 'The original pending send retains its presentation canvas.');
    assert.equal(shell.classList.contains('setup-welcome'), false, 'Pending activity does not inherit welcome-only hiding rules.');
    assert.ok(document.querySelector('.setup-native-chat'), 'The real native activity surface remains mounted during admission.');
    assert.equal(document.querySelector('.setup-guide-wait [role="status"]').textContent, 'Sending to your guide');
    assert.equal(document.querySelector('#setup-message').disabled, false, 'The existing editable composer remains usable.');
    assert.equal(document.querySelector('.setup-current-question') === null, true, 'Only the obsolete first question is suppressed.');
    await act(async () => { admitFirstResponse(); await settle(); });
    assert.ok(shell.classList.contains('setup-first-turn'), 'The actual saved running receipt retains the same presentation canvas.');
    assert.match(document.querySelector('.setup-native-chat .setup-native-phase').textContent, /Working through your agent design/);
    assert.equal(document.querySelector('.setup-guide-wait [role="status"]').textContent, 'Saved - your guide is working');
    assert.equal(document.querySelector('.setup-guide-wait small') === null, true, 'The confirmed first request has one waiting status.');
    assert.equal(document.querySelector('#setup-message').value, '', 'The sent idea clears from the owned composer.');
    assert.equal(textarea.placeholder, 'Write your next message - you can send it after the reply.');
    assert.equal(document.querySelector('#setup-send').textContent.trim(), 'Waiting for guide');
    assert.match(document.querySelector('.forge-next-draft-hint').textContent, /Your next draft stays here/);
    assert.match(document.querySelector('.setup-header .forge-welcome-orientation [aria-current="step"]').textContent, /Idea/);
    assert.equal(!!document.querySelector('.forge-mobile-journey'), narrow);
    if (narrow) assert.match(document.querySelector('.forge-mobile-journey [aria-current="step"]').textContent, /Idea/);
    assert.equal(document.querySelector('#setup-send').disabled, true);
    assert.equal(document.querySelector('.setup-composer-mode') === null, true, 'The obsolete opening question offers no competing message modes.');
    const nextDraft = 'Please keep a separate note for the next reply.';
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value')
        .set.call(textarea, nextDraft);
      textarea.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
    });
    assert.equal(document.querySelector('#setup-send').disabled, true, 'A next draft never joins the running request.');
    assert.equal(motion().getAttribute('aria-pressed'), 'true', 'Sending and the next draft never restart a paused illustration.');
    assert.equal(pane.querySelector('animateTransform, animate') === null, true);
    await act(async () => motion().click());
    assert.equal(motion().getAttribute('aria-pressed'), 'false');
    assert.ok(pane.querySelector('animateTransform'), 'Motion resumes only after an explicit action.');
    assert.ok(pane.querySelector('.forge-welcome-portrait'), 'Native guide activity does not blank the right pane.');
    assert.ok(pane.querySelector('.ep-illustrated'));
    assert.equal(pane.querySelector('.forge-portrait-caption small').textContent, 'Preview illustration. Your proposal appears as we talk.');
    assert.equal(pane.querySelector('[data-proposal-snapshot]') === null, true);
    assert.equal(pane.querySelector('.ep-stats') === null, true, 'The illustration invents no agents or progress.');
    assert.equal(calls.filter(call => call.url === `/api/guide/${id}/respond`).length, 1);
    const running = current();
    save({ ...running, version: 2, operation: { ...running.operation, status: 'completed' },
      brief: { facts: { outcome: { summary: 'Share gardening tools with neighbours.', status: 'confirmed' } }, requirements: [
        { id: 'shared-loans', outcome: 'Keep a shared record of tool loans.', disposition: 'handoff' },
        { id: 'purchases', outcome: 'Organising new purchases can wait.', disposition: 'deferred' },
      ] },
      question: { ...running.question, title: 'Who will use this?', options: [{ id: 'small-group', label: 'A small group', description: 'Neighbours sharing tools.' }] } });
    await poll();
    assert.equal(shell.classList.contains('setup-first-turn'), false, 'Published understanding leaves the first-turn presentation.');
    assert.equal(textarea.value, nextDraft, 'The guide reply leaves the next draft exactly as written.');
    assert.equal(document.querySelector('#setup-send').disabled, false);
    assert.notEqual(document.querySelector('#setup-send').textContent.trim(), 'Waiting for guide');
    assert.equal(document.querySelector('.forge-next-draft-hint') === null, true);
    assert.match(document.querySelector('.setup-header .forge-welcome-orientation [aria-current="step"]').textContent, /Shape/);
    if (narrow) assert.match(document.querySelector('.forge-mobile-journey [aria-current="step"]').textContent, /Shape/);
    assert.equal(calls.filter(call => call.url === `/api/guide/${id}/respond`).length, 1, 'The completed reply never sends the next draft automatically.');
    assert.ok(pane.querySelector('.forge-welcome-portrait'), 'Gathered context stays alongside the illustration until a real proposal exists.');
    assert.ok(pane.querySelector('.ep-illustrated'));
    assert.match(pane.textContent, /Share gardening tools with neighbours/);
    assert.match(pane.querySelector('.ep-requirements').textContent, /Keep a shared record of tool loans/);
    assert.match(pane.querySelector('.ep-scope-decisions').textContent, /Your scope decisions.*Organising new purchases can wait/s);
    await act(async () => {
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value')
        .set.call(textarea, 'A small group of neighbours.');
      textarea.dispatchEvent(new document.defaultView.Event('input', { bubbles: true }));
    });
    await act(async () => { document.querySelector('#setup-send').click(); await settle(); });
    save({ ...current(), version: 3, proposal: planFor('Neighbourhood tool library') });
    await poll();
    assert.equal(pane.querySelector('.forge-welcome-portrait') === null, true);
    assert.equal(JSON.parse(pane.querySelector('[data-proposal-snapshot]').dataset.proposalSnapshot).project.name,
      'Neighbourhood tool library');
  }, { narrow, available: true, respondRead: async (server, request) => {
    if (server.version === 1) await firstResponse;
    return { ...server, operation: { status: 'running', version: server.version, request_id: 'a'.repeat(64), request } };
  } });
});

test('opening the desktop proposal selects its retained mobile pane while closing preserves preview and unsent guide draft',async()=>{
  const guide={...freshGuide(),proposal:planFor('My repair cafe'),status:'review',question:null};
  await mounted(guide,async({document,calls,publications,current})=>{
    const shell=document.querySelector('.setup-shell'),preview=document.querySelector('[data-proposal-snapshot]');
    assert.ok(shell.classList.contains('setup-mobile-chat'),'Desktop starts with the latent Conversation selection');
    const draft=document.querySelector('#setup-message');
    await act(async()=>{
      Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype,'value').set.call(draft,'  Keep this thought unsent\nfor later  ');
      draft.dispatchEvent(new document.defaultView.Event('input',{bubbles:true}));
    });
    const writes=calls.filter(call=>call.method!=='GET'),publicationCount=publications.length,source=JSON.stringify(current());
    const expand=preview.querySelector('button');await act(async()=>{expand.click();await settle();});
    assert.ok(shell.classList.contains('setup-mobile-plan'),'The existing responsive CSS must keep the expanded descendant visible on a later narrow viewport');
    assert.equal(document.querySelector('.setup-mobile-tabs button[aria-pressed="true"]').textContent.trim(),'Your team 1');
    assert.equal(expand.getAttribute('aria-expanded'),'true');
    assert.equal(document.querySelector('[data-proposal-snapshot]') === preview, true,'Selecting the pane does not remount its preview');
    await act(async()=>{expand.click();await settle();});
    assert.equal(expand.getAttribute('aria-expanded'),'false');assert.ok(shell.classList.contains('setup-mobile-plan'));
    assert.equal(document.querySelector('[data-proposal-snapshot]') === preview, true);
    assert.equal(document.querySelector('#setup-message') === draft, true);assert.equal(draft.value,'  Keep this thought unsent\nfor later  ');
    assert.deepEqual(calls.filter(call=>call.method!=='GET'),writes,'Opening and closing admit no work; pending read-only observations may settle');assert.equal(publications.length,publicationCount);
    assert.equal(JSON.stringify(current()),source,'Presentation selection does not change saved guide state');
  });
});


function startingContextButton(document, label) {
  const found = [...document.querySelectorAll('.forge-starting-help button, .forge-starting-context button')].find(node => {
    const name = node.getAttribute('aria-label') || node.querySelector('strong')?.textContent || node.textContent;
    return name.trim().replace(/\s*\u2192$/, '') === label && !node.closest('[hidden]');
  });
  assert.ok(found, `Missing starting context action: ${label}`); return found;
}
async function clickStarting(document, label) {
  await act(async () => { startingContextButton(document, label).click(); await settle(); await settle(); });
}
async function typeStarting(document, value) {
  await act(async () => {
    const editor = document.querySelector('#setup-message');
    Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value').set.call(editor, value);
    editor.dispatchEvent(new document.defaultView.Event('input', { bubbles: true })); await settle();
  });
}

test('starting context field edit: answered review stays closed while individual revisions preserve the other field', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  const written = '  Keep my exact idea.\nChanging context must not rewrite it.  \t';
  await mounted(guide, async ({ document, calls, values }) => {
    const stored = () => JSON.parse(values.get(inputKey(guide))).starting_help;
    const helper = () => document.querySelector('.forge-starting-conversation');
    const editor = document.querySelector('#setup-message');
    await typeStarting(document, written);
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Repetitive work');
    const context = document.querySelector('.forge-starting-context');
    assert.equal(context.getAttribute('aria-label'), 'Starting point, not sent');
    assert.equal(context.querySelector('.forge-starting-context__sentence').textContent, 'For a team, help with repetitive work.');
    assert.equal(context.querySelector('.forge-starting-context__hint').textContent, 'Change either part. The other stays as it is.');
    assert.equal(startingContextButton(document, 'Change who this helps: A team').textContent, 'For a team');
    assert.equal(startingContextButton(document, 'Change the kind of work: Repetitive work').textContent, 'repetitive work');
    const audienceTrigger = startingContextButton(document, 'Change who this helps: A team'); audienceTrigger.focus();
    await clickStarting(document, 'Change who this helps: A team');
    assert.equal(helper().querySelector('.forge-starting-conversation__progress').textContent, 'Change who this helps');
    assert.match(helper().querySelector('.forge-starting-conversation__answered').textContent, /Keeping Repetitive work/);
    assert.equal(helper().querySelector('.forge-starting-conversation__progress-marks') === null, true, 'A single-field edit does not promise two questions.');
    await clickStarting(document, 'Cancel change');
    assert.deepEqual(stored(), { version: 2, audience_id: 'team', kind_id: 'repetitive' }, 'Cancelling before a field choice keeps both applied answers.');
    assert.equal(document.activeElement === audienceTrigger, true, 'Cancelling returns to the owned review field instead of opening the editor.');
    await clickStarting(document, 'Change who this helps: A team'); await clickStarting(document, 'My household');
    assert.equal(helper().dataset.open, 'false', 'One audience selection completes the audience-only edit.');
    assert.deepEqual(stored(), { version: 2, audience_id: 'household', kind_id: 'repetitive' });
    assert.equal(context.querySelector('.forge-starting-context__sentence').textContent, 'For my household, help with repetitive work.');
    assert.equal(document.activeElement === audienceTrigger, true, 'An audience-only commit returns to its original review field.');
    assert.equal(editor.value, written);
    assert.match(helper().querySelector('[role="status"]').textContent, /Starting point updated/);
    await clickStarting(document, 'Your two answers');
    assert.equal(helper().dataset.open, 'false', 'The answered route is review, not a restart of both questions.');
    assert.deepEqual(stored(), { version: 2, audience_id: 'household', kind_id: 'repetitive' });
    await clickStarting(document, 'Change who this helps: My household'); await clickStarting(document, 'Just me');
    assert.deepEqual(stored(), { version: 2, audience_id: 'personal', kind_id: 'repetitive' }, 'Changing the audience preserves the committed kind.');
    await clickStarting(document, 'Change the kind of work: Repetitive work'); await clickStarting(document, 'Figuring something out');
    assert.deepEqual(stored(), { version: 2, audience_id: 'personal', kind_id: 'understanding' }, 'Both fields can be revised through their own review controls.');
    const kindTrigger = startingContextButton(document, 'Change the kind of work: Figuring something out');
    await clickStarting(document, 'Change the kind of work: Figuring something out');
    assert.equal(helper().querySelector('.forge-starting-conversation__progress').textContent, 'Change the kind of work');
    await clickStarting(document, 'Keeping track of things');
    assert.deepEqual(stored(), { version: 2, audience_id: 'personal', kind_id: 'organizing' }, 'A kind edit preserves the applied audience.');
    assert.equal(helper().dataset.open, 'false');
    assert.equal(document.activeElement === kindTrigger, true);
    assert.equal(editor.value, written);
    await clickStarting(document, 'Change who this helps: Just me'); await clickStarting(document, "I'm still exploring");
    await clickStarting(document, 'Change the kind of work: Keeping track of things'); await clickStarting(document, 'Still exploring');
    assert.deepEqual(stored(), { version: 2, audience_id: 'exploring', kind_id: 'exploring' });
    assert.equal(context.querySelector('.forge-starting-context__sentence').textContent, "For someone I'm still figuring out, help with finding a starting point.",
      'Exploratory selections remain exploratory in the visible sentence.');
    assert.equal(editor.value, written);
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), [], 'Local field edits never admit a request.');
  }, { available: true, runAnimationFrames: true });
});

for (const scenario of ['ordinary handoff', 'focus elsewhere', 'selected guide changed'])
  test(`starting context field edit focus custody: ${scenario}`, async () => {
    const guide = { ...freshGuide(), capabilities: { available: true }, name: 'Original field edit' };
    const other = { ...freshGuide(), id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', capabilities: { available: true }, name: 'Other field edit' };
    const written = '  Keep this field edit private.\nNo text replacement.  ';
    await mounted(guide, async ({ document, calls, values, flushAnimationFrames, reload }) => {
      await typeStarting(document, written);
      await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Repetitive work');
      await flushAnimationFrames();
      const editor = document.querySelector('#setup-message');
      const editTrigger = startingContextButton(document, 'Change who this helps: A team');
      await clickStarting(document, 'Change who this helps: A team');
      const chosen = startingContextButton(document, 'Many people'); chosen.focus();
      await clickStarting(document, 'Many people');
      assert.deepEqual(JSON.parse(values.get(inputKey(guide))).starting_help, { version: 2, audience_id: 'many', kind_id: 'repetitive' });
      assert.equal(editor.value, written);
      assert.equal(document.querySelector('.forge-starting-conversation').dataset.open, 'false');
      let retainedFocus;
      if (scenario === 'focus elsewhere') {
        retainedFocus = [...document.querySelectorAll('button')].find(button => button.textContent.trim().startsWith('Guide settings'));
        assert.ok(retainedFocus); retainedFocus.focus();
      } else if (scenario === 'selected guide changed') {
        await reload(other.id); retainedFocus = document.activeElement;
      }
      await flushAnimationFrames();
      assert.equal(document.activeElement === (scenario === 'ordinary handoff' ? editTrigger : retainedFocus), true,
        'An audience-only commit restores its captured review trigger only while deferred focus custody remains owned.');
      assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
    }, { available: true, holdAnimationFrames: true, savedGuides: [guide, other] });
  });

test('starting context reload: exact choices restore independently and revision leaves all writing unchanged', async () => {
  const anonymous = { ...freshGuide(), id: null, version: 0, capabilities: { available: true }, messages: [] };
  const written = '  My own project\nKeep these words <script>literal</script>.  \n\nTrack decisions for Tuesday.  \t';
  let retained;
  await mounted(anonymous, async ({ document, values, calls }) => {
    await typeStarting(document, written);
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Keeping track of things');
    assert.equal(document.querySelector('#setup-message').value, written);
    const saved = JSON.parse(values.get(inputKey(anonymous)));
    assert.equal(saved.draft, written);
    assert.deepEqual(saved.starting_help, { version: 2, audience_id: 'team', kind_id: 'organizing' });
    retained = values;
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, runAnimationFrames: true });
  await mounted(anonymous, async ({ document, values, calls }) => {
    const editor = document.querySelector('#setup-message');
    assert.equal(editor.value, written);
    const audienceTrigger = startingContextButton(document, 'Change who this helps: A team');
    await clickStarting(document, 'Change who this helps: A team');
    assert.equal(startingContextButton(document, 'A team').getAttribute('aria-pressed'), 'true');
    await clickStarting(document, 'Many people');
    assert.deepEqual(JSON.parse(values.get(inputKey(anonymous))).starting_help, { version: 2, audience_id: 'many', kind_id: 'organizing' },
      'An audience-only edit keeps the previously chosen kind of work.');
    assert.equal(document.querySelector('.forge-starting-conversation').dataset.open, 'false');
    assert.equal(document.activeElement === audienceTrigger, true);
    const kindTrigger = startingContextButton(document, 'Change the kind of work: Keeping track of things');
    await clickStarting(document, 'Change the kind of work: Keeping track of things'); await clickStarting(document, 'Repetitive work');
    assert.equal(editor.value, written);
    assert.equal(JSON.parse(values.get(inputKey(anonymous))).draft, written);
    assert.deepEqual(JSON.parse(values.get(inputKey(anonymous))).starting_help, { version: 2, audience_id: 'many', kind_id: 'repetitive' });
    assert.equal(document.activeElement === kindTrigger, true);
    assert.equal(document.querySelectorAll('textarea').length, 1);
    assert.equal(document.querySelector('script') === null, true);
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, retainedValues: retained, runAnimationFrames: true });
});

test('starting context: cancel restores applied choices and removal never changes user writing', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  const written = 'Keep this wording exactly.  \n';
  await mounted(guide, async ({ document, calls, values }) => {
    await typeStarting(document, written);
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Keeping track of things');
    const example = document.querySelector('.forge-starting-example');
    assert.ok(example, 'The selected context offers adjacent inspiration.');
    assert.equal(example.id, 'setup-message-starting-example');
    assert.equal(example.querySelector('.forge-starting-example__label').textContent, 'For example');
    assert.match(example.textContent, /Keep track of decisions and deadlines for a project/);
    assert.equal(example.querySelector('button, input, textarea, a') === null, true, 'Inspiration has no insertion or submission controls.');
    assert.equal(document.querySelector('#setup-message').value, written, 'The example never enters the user-owned draft.');
    const editTrigger = startingContextButton(document, 'Change who this helps: A team'); editTrigger.focus();
    await clickStarting(document, 'Change who this helps: A team');
    await act(async () => { document.querySelector('.forge-starting-conversation').dispatchEvent(new document.defaultView.KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await settle(); });
    assert.match(document.querySelector('.forge-starting-context__sentence').textContent, /For a team, help with keeping track/);
    assert.equal(document.activeElement === editTrigger, true, 'Escape returns to the same committed field without applying a replacement answer.');
    assert.equal(document.querySelector('.forge-starting-example') === example, true, 'Cancelling a staged edit retains the applied inspiration.');
    assert.match(example.textContent, /Keep track of decisions and deadlines for a project/);
    assert.equal(document.querySelector('#setup-message').value, written);
    await clickStarting(document, 'Change the kind of work: Keeping track of things'); await clickStarting(document, 'Repetitive work');
    assert.equal(document.querySelector('.forge-starting-example') === example, true, 'Changing context updates the retained example in place.');
    assert.match(example.textContent, /Turn scattered notes into a clear weekly update/);
    assert.equal(document.querySelector('#setup-message').value, written, 'Updated inspiration leaves exact user writing untouched.');
    await clickStarting(document, 'Remove starting point');
    assert.equal(document.querySelector('.forge-starting-context') === null, true, '.forge-starting-context stays absent');
    assert.equal(document.querySelector('.forge-starting-example') === null, true, 'Removing the context removes its inspiration.');
    assert.equal(document.querySelector('#setup-message').value, written);
    assert.equal(JSON.parse(values.get(inputKey(guide))).starting_help, undefined);
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, runAnimationFrames: true });
});

test('starting context: handled chip edits do not reopen after returning to the same saved setup', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true }, name: 'My unsent idea' };
  await mounted(guide, async ({ document, calls, reload }) => {
    await typeStarting(document, 'Keep my writing.');
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Keeping track of things');
    await clickStarting(document, 'Change who this helps: A team'); await clickStarting(document, 'Cancel change');
    await reload();
    assert.equal(document.querySelector('.forge-starting-conversation').dataset.open, 'false', 'A cancelled edit stays closed on return.');
    assert.equal(document.querySelector('#setup-message').value, 'Keep my writing.');
    await clickStarting(document, 'Change the kind of work: Keeping track of things'); await clickStarting(document, 'Repetitive work');
    await reload();
    assert.equal(document.querySelector('.forge-starting-conversation').dataset.open, 'false', 'A completed edit stays closed on return.');
    assert.match(document.querySelector('.forge-starting-context__sentence').textContent, /For a team, help with repetitive work\./);
    assert.equal(document.querySelector('#setup-message').value, 'Keep my writing.');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, savedGuides: [guide], runAnimationFrames: true });
});

test('starting context: removing an applied starting point also dismisses its unfinished edit', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  await mounted(guide, async ({ document, calls, values }) => {
    await typeStarting(document, 'My actual task stays here.');
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Keeping track of things');
    await clickStarting(document, 'Change who this helps: A team');
    await clickStarting(document, 'Remove starting point');
    assert.equal(document.querySelector('.forge-starting-conversation').dataset.open, 'false', 'Removal dismisses the unfinished edit.');
    assert.equal(document.querySelector('.forge-starting-context') === null, true, '.forge-starting-context stays absent');
    assert.equal(document.querySelector('#setup-message').value, 'My actual task stays here.');
    assert.equal(document.activeElement?.id, 'setup-message');
    assert.equal(JSON.parse(values.get(inputKey(guide))).starting_help, undefined);
    await clickStarting(document, 'Answer two questions');
    assert.equal(startingContextButton(document, 'Many people').getAttribute('aria-pressed'), 'false', 'Removed choices do not reappear in a new helper.');
    await clickStarting(document, 'Back to my words');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, runAnimationFrames: true });
});

test('starting context reload: a changed question restores writing without borrowing earlier separate choices', async () => {
  const anonymous = { ...freshGuide(), id: null, version: 0, capabilities: { available: true }, messages: [] };
  const written = '  My own project.  \nMy actual task.';
  const hint = { version: 2, audience_id: 'team', kind_id: 'organizing' };
  const values = new Map([[inputKey(anonymous), JSON.stringify({ key: inputKey(anonymous), draft: written, selected: [], mode: 'answer', starting_help: hint })],
    ['mobkit-setup-input-v2:new:latest', JSON.stringify({ key: inputKey(anonymous), draft: written, selected: [], mode: 'answer' })]]);
  const changed = { ...anonymous, question: { ...anonymous.question, description: 'A changed decision owns its own composer.' } };
  await mounted(changed, async ({ document, calls, values }) => {
    assert.equal(document.querySelector('#setup-message').value, written);
    assert.equal(document.querySelector('.forge-starting-context') === null, true, '.forge-starting-context stays absent');
    assert.equal(JSON.parse(values.get(inputKey(changed))).starting_help, undefined);
    await clickStarting(document, 'Answer two questions');
    assert.equal(startingContextButton(document, 'A team').getAttribute('aria-pressed'), 'false');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, retainedValues: values });
});

test('starting context reload: denied storage keeps separate choices only in the open page', async () => {
  const anonymous = { ...freshGuide(), id: null, version: 0, capabilities: { available: true }, messages: [] };
  const written = '  Keep this private idea.  ';
  await mounted(anonymous, async ({ document, calls, values }) => {
    await typeStarting(document, written);
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Keeping track of things');
    await clickStarting(document, 'Change the kind of work: Keeping track of things'); await clickStarting(document, 'Repetitive work');
    assert.equal(document.querySelector('#setup-message').value, written);
    assert.match(document.querySelector('.forge-starting-context__sentence').textContent, /For a team, help with repetitive work\./);
    assert.match(document.body.textContent, /Browser recovery is unavailable/);
    assert.equal(values.size, 0);
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, denyStorage: true, runAnimationFrames: true });
});

test('starting context: an older saved prose handoff remains editable under its existing contract', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  const inserted = 'For a team, I would like help organizing information and commitments.';
  const written = inserted + '\nMy own task.';
  const values = new Map([[GUIDE_KEY, guide.id], [inputKey(guide), JSON.stringify({ key: inputKey(guide), draft: written, selected: [], mode: 'answer',
    starting_help: { audience_id: 'team', kind_id: 'organizing', prefix: '', inserted } })]]);
  await mounted(guide, async ({ document, calls }) => {
    assert.equal(document.querySelector('#setup-message').value, written);
    assert.ok(document.querySelector('.forge-starting-help__handoff'));
    await clickStarting(document, 'Change choices'); await clickStarting(document, 'Many people'); await clickStarting(document, 'Repetitive work');
    assert.equal(document.querySelector('#setup-message').value, 'For many people, I would like help with repetitive work.\nMy own task.');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  }, { available: true, retainedValues: values, runAnimationFrames: true });
});

const proposalNoticeName = 'Your team - proposal updated';
function noticeGuide({ guideId = id, proposal = null, blueprint = null, name = 'Proposal guide' } = {}) {
  return { ...freshGuide(), id: guideId, name, status: 'refining', proposal, blueprint,
    messages: [{ id: 'supplied-job', role: 'user', text: 'Help our group keep track of shared equipment.' }],
    brief: { facts: { outcome: { summary: 'Track shared equipment.', status: 'confirmed', source_message_ids: ['supplied-job'] } }, requirements: [] } };
}
function teamTab(document) {
  const button = [...document.querySelectorAll('.setup-mobile-tabs > button')]
    .find(node => node.textContent.trim().startsWith('Your team'));
  assert.ok(button, 'The proposal notice belongs to the existing Your team tab.');
  assert.equal(document.querySelectorAll('.setup-mobile-tabs > button').length, 2, 'No extra navigation control is introduced.');
  return button;
}
function assertProposalNotice(document, expected, detail) {
  const button = teamTab(document), dot = button.querySelector('.setup-team-update-dot');
  if (expected) {
    assert.equal(button.getAttribute('aria-label'), proposalNoticeName, detail);
    assert.ok(dot, 'The existing tab carries a quiet visual update cue.');
    assert.equal(dot.getAttribute('aria-hidden'), 'true', 'The dot is decorative; the tab supplies its accessible meaning.');
  } else {
    assert.notEqual(button.getAttribute('aria-label'), proposalNoticeName, detail);
    assert.equal(dot === null, true, detail);
  }
}
async function selectNoticePane(document, pane) {
  await act(async () => {
    const button = pane === 'plan' ? teamTab(document)
      : [...document.querySelectorAll('.setup-mobile-tabs > button')].find(node => node.textContent.trim() === 'Conversation');
    assert.ok(button); button.click(); await settle();
  });
}
async function writeNoticeDraft(document, text) {
  await act(async () => {
    const editor = document.querySelector('#setup-message');
    Object.getOwnPropertyDescriptor(document.defaultView.HTMLTextAreaElement.prototype, 'value').set.call(editor, text);
    editor.dispatchEvent(new document.defaultView.Event('input', { bubbles: true })); await settle();
  });
}

for (const field of ['proposal', 'blueprint']) test(`proposal update cue: the first saved ${field} appears after this guide's empty baseline`, async () => {
  const guide = noticeGuide();
  await mounted(guide, async ({ document, current, save, reload, calls }) => {
    assertProposalNotice(document, false, 'Neither the internal starter nor an empty saved guide is a publication.');
    save({ ...current(), version: 2, [field]: planFor('Shared equipment team') });
    await reload();
    assertProposalNotice(document, true, `The first saved ${field} announces a real proposal while Conversation is selected.`);
    assert.equal(document.querySelector('.setup-shell').classList.contains('setup-mobile-chat'), true,
      'Publishing a proposal does not force the user away from Conversation.');
    assert.ok(calls.every(call => call.method === 'GET'), 'The notice and readback submit no guide work.');
  }, { narrow: true, savedGuides: [guide] });
});

test('proposal update cue: starter, live design, draft edits and operation-only changes stay silent', async () => {
  const guide = noticeGuide();
  await mounted(guide, async ({ document, current, save, reload, calls }) => {
    assertProposalNotice(document, false, 'A supplied internal workbench starter does not announce a user proposal.');
    const draft = 'Keep the equipment notes private to our group.';
    await writeNoticeDraft(document, draft);
    assertProposalNotice(document, false, 'Writing a draft is not a saved proposal publication.');
    const requestId = 'a'.repeat(64);
    save({ ...current(), version: 2,
      operation: { status: 'running', version: 2, request_id: requestId,
        request: { version: 2, action: 'answer', selected: [], text: 'The supplied equipment job.' } },
      live_design: { guide_id: id, guide_version: 2, request_id: requestId,
        blueprint: planFor('Still being considered'), brief: current().brief } });
    await reload();
    assert.ok(document.querySelector('[data-proposal-snapshot]'), 'The real staged preview is present, so this exclusion is exercised.');
    assertProposalNotice(document, false, 'The provisional live design does not announce a saved proposal.');
    save({ ...current(), version: 3, operation: { ...current().operation, status: 'completed' }, live_design: null });
    await reload();
    assertProposalNotice(document, false, 'Version and operation completion alone cannot create a proposal cue.');
    assert.equal(document.querySelector('#setup-message').value, draft, 'Passive readback preserves the unsent draft.');
    save({ ...current(), version: 4, proposal: planFor('Saved equipment team') });
    await reload();
    assertProposalNotice(document, true, 'Only the later saved proposal produces the cue.');
    assert.ok(calls.every(call => call.method === 'GET'));
  }, { narrow: true, savedGuides: [guide] });
});

test('proposal update cue: viewing clears it, unchanged polling stays quiet and a same-count revision relights it', async () => {
  const first = planFor('Shared equipment team');
  const guide = noticeGuide({ proposal: first });
  await mounted(guide, async ({ document, current, save, reload, poll, calls }) => {
    assertProposalNotice(document, false, 'An existing saved proposal is this guide selection baseline.');
    const revised = structuredClone(first);
    revised.agent_classes[0].instructions += ' Retain lending history and return dates.';
    save({ ...current(), version: 2, proposal: revised,
      operation: { status: 'running', version: 1, request_id: 'a'.repeat(64),
        request: { version: 1, action: 'answer', selected: [], text: 'Add lending history.' } } });
    await reload();
    assertProposalNotice(document, true, 'A responsibility revision matters even when the agent count is unchanged.');
    assert.equal(teamTab(document).querySelector('span:not(.setup-team-update-dot)').textContent, '1');
    await selectNoticePane(document, 'plan');
    assertProposalNotice(document, false, 'Opening the actual Your team pane acknowledges the update.');
    await selectNoticePane(document, 'chat');
    save(structuredClone(current())); await poll();
    assertProposalNotice(document, false, 'Polling the same saved content cannot relight an acknowledged publication.');
    save({ ...current(), version: 3, operation: { ...current().operation, detail: 'Working on the next answer.' } });
    await poll();
    assertProposalNotice(document, false, 'New operation metadata and a higher version still do not revise the proposal.');
    const next = structuredClone(revised);
    next.agent_classes[0].description = 'Own group lending context and due dates.';
    save({ ...current(), version: 4, proposal: next }); await poll();
    assertProposalNotice(document, true, 'Another real same-count content revision becomes unread again.');
    assert.equal(teamTab(document).querySelector('span:not(.setup-team-update-dot)').textContent, '1');
    assert.ok(calls.every(call => call.method === 'GET'));
  }, { narrow: true, savedGuides: [guide] });
});

test('proposal update cue: switching guides establishes each selected baseline without borrowing another cue', async () => {
  const otherId = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
  const guide = noticeGuide({ proposal: planFor('Equipment team'), name: 'Equipment setup' });
  const other = noticeGuide({ guideId: otherId, proposal: planFor('Garden team'), name: 'Garden setup' });
  await mounted(guide, async ({ document, current, save, reload, calls }) => {
    assertProposalNotice(document, false);
    const revised = structuredClone(current().proposal); revised.project.description = 'Lending and repair history.';
    save({ ...current(), version: 2, proposal: revised }); await reload();
    assertProposalNotice(document, true, 'The first guide owns its own unread publication.');
    await reload(otherId);
    assert.equal(current().id, otherId);
    assertProposalNotice(document, false, 'The other guide starts at its own saved proposal baseline.');
    const gardenRevision = structuredClone(current().proposal); gardenRevision.agent_classes[0].instructions += ' Keep seasonal context.';
    save({ ...current(), version: 2, proposal: gardenRevision }); await reload();
    assertProposalNotice(document, true, 'The selected second guide can announce its own content revision.');
    await reload(id);
    assert.equal(current().id, id);
    assertProposalNotice(document, false, 'Returning establishes the selected saved baseline rather than inheriting the previous guide cue.');
    assert.ok(calls.every(call => call.method === 'GET'), 'Switching and noticing remain read-only.');
  }, { narrow: true, savedGuides: [guide, other] });
});

test('proposal update cue: desktop and the visible team pane already see publications before returning to Conversation', async () => {
  const guide = noticeGuide({ proposal: planFor('Shared equipment team') });
  await mounted(guide, async ({ document, current, save, reload, setNarrow, calls }) => {
    const desktopRevision = structuredClone(current().proposal);
    desktopRevision.agent_classes[0].instructions += ' Keep maintenance history.';
    save({ ...current(), version: 2, proposal: desktopRevision }); await reload();
    assertProposalNotice(document, false, 'A desktop publication is already visible beside the conversation.');
    await setNarrow(true);
    assertProposalNotice(document, false, 'Narrowing the viewport does not turn a viewed desktop proposal into unread work.');
    await selectNoticePane(document, 'plan');
    const viewedRevision = structuredClone(current().proposal);
    viewedRevision.agent_classes[0].description = 'Group equipment and maintenance context owner.';
    save({ ...current(), version: 3, proposal: viewedRevision }); await reload();
    assertProposalNotice(document, false, 'Publication while Your team is visible is already seen.');
    await selectNoticePane(document, 'chat');
    assertProposalNotice(document, false, 'Returning to Conversation keeps the viewed publication acknowledged.');
    const unseenRevision = structuredClone(current().proposal);
    unseenRevision.project.description = 'Shared equipment with repair and lending history.';
    save({ ...current(), version: 4, proposal: unseenRevision }); await reload();
    assertProposalNotice(document, true, 'Only the subsequent narrow Conversation publication needs a cue.');
    assert.ok(calls.every(call => call.method === 'GET'));
  }, { savedGuides: [guide] });
});

test('first proposal recovery remains visible in the mobile team pane and returns to conversation without resending', async () => {
  const guide = freshGuide();
  guide.capabilities.available = true;
  guide.operation = { status: 'running', request_id: 'b'.repeat(64), version: 1,
    started_at: '2026-10-10T09:00:00Z' };
  await mounted(guide, async ({ document, calls }) => {
    await selectNoticePane(document, 'plan');
    const pane = document.querySelector('.setup-plan-pane');
    const status = pane.querySelector('.ep-first-proposal-status');
    assert.ok(status, 'An interrupted first request has feedback in the visible team pane.');
    assert.match(status.textContent, /Live updates interrupted/);
    assert.doesNotMatch(status.textContent, /Your guide is working/);
    assert.ok(pane.querySelector('.forge-welcome-portrait'), 'Recovery preserves the circular illustration.');
    assert.equal(pane.querySelector('[data-proposal-snapshot]') === null, true, 'Recovery invents no proposal.');
    const before = calls.length;
    const returnButton = status.querySelector('button');
    assert.match(returnButton?.textContent || '', /Return to conversation/);
    await act(async () => { returnButton.click(); await settle(); });
    await act(async () => new Promise(resolve => realSetTimeout(resolve, 25)));
    assert.ok(document.querySelector('.setup-shell').classList.contains('setup-mobile-chat'));
    assert.ok(document.activeElement === document.querySelector('.setup-conversation'), 'Focus follows the mobile view change.');
    assert.match(document.querySelector('.setup-native-chat').textContent, /Live updates interrupted/);
    assert.ok(calls.slice(before).every(call => call.method === 'GET'), 'Returning to Conversation admits no work.');
    assert.ok(calls.every(call => call.method === 'GET'), 'The retained first request is never resent.');
  }, { narrow: true, available: true, runAnimationFrames: true,
    timelineRead: () => ({ httpError: 503, detail: 'Fixture observation interruption' }) });
});

test('first proposal recovery is retained during setup selection and a delayed return cannot focus another setup', async () => {
  const guide = freshGuide();
  guide.capabilities.available = true;
  guide.operation = { status: 'running', request_id: 'c'.repeat(64), version: 1,
    started_at: '2026-10-10T09:00:00Z' };
  const other = { ...freshGuide(), id: 'dddddddd-dddd-4ddd-8ddd-dddddddddddd', name: 'Another saved idea' };
  let releaseSelection, selectionEntered;
  const heldSelection = new Promise(resolve => { releaseSelection = resolve; });
  const entered = new Promise(resolve => { selectionEntered = resolve; });
  await mounted(guide, async ({ document, calls, reload, flushAnimationFrames, current }) => {
    await flushAnimationFrames();
    await selectNoticePane(document, 'plan');
    await flushAnimationFrames();
    const recovery = () => document.querySelector('.ep-first-proposal-status');
    assert.match(recovery().textContent, /Live updates interrupted/);
    await act(async () => { recovery().querySelector('button').click(); await settle(); });
    await reload(other.id);
    await entered;
    await act(async () => settle());
    assert.match(recovery().textContent, /Live updates interrupted/,
      'The retained guide keeps its interrupted observation while the next setup is loading.');
    assert.doesNotMatch(recovery().textContent, /Your guide is working/);
    const focusBefore = document.activeElement;
    await flushAnimationFrames();
    assert.ok(document.activeElement === focusBefore, 'The delayed return cannot take focus during selection.');
    await act(async () => { releaseSelection(); await settle(); });
    assert.equal(current().id, other.id);
    await flushAnimationFrames();
    assert.ok(document.activeElement !== document.querySelector('.setup-conversation'),
      'The old return never transfers focus to the new guide.');
    assert.ok(calls.every(call => call.method === 'GET'), 'Navigation admits no guide work.');
  }, { narrow: true, available: true, holdAnimationFrames: true, savedGuides: [guide, other],
    guideRead: async (call, saved) => {
      if (call.url.endsWith(other.id)) { selectionEntered(); await heldSelection; }
      return saved;
    },
    timelineRead: () => ({ httpError: 503, detail: 'Fixture observation interruption' }) });
});

test('proposal update cue: viewing a pending manual design cannot acknowledge different newly saved content', async () => {
  const guide = noticeGuide({ proposal: planFor('Saved equipment team') });
  const manual = { blueprint: planFor('My pending manual team'),
    changes: ['Project name changed'], text: 'Review this pending design.' };
  await mounted(guide, async ({ document, current, save, reload, values, calls }) => {
    const visible = () => JSON.parse(document.querySelector('[data-proposal-snapshot]').dataset.proposalSnapshot);
    assertProposalNotice(document, false, 'The initial selected saved proposal is a silent baseline.');
    assert.equal(visible().project.name, 'My pending manual team');
    await selectNoticePane(document, 'plan');
    const savedRevision = structuredClone(current().proposal);
    savedRevision.agent_classes[0].instructions += ' Preserve lending history.';
    save({ ...current(), version: 2, proposal: savedRevision }); await reload();
    assert.equal(visible().project.name, 'My pending manual team', 'The visible pane still shows the pending manual design.');
    assertProposalNotice(document, true, 'The cue remains pending while the open pane displays a different manual design.');
    await selectNoticePane(document, 'chat');
    assertProposalNotice(document, true, 'Seeing a different pending design did not acknowledge the newly saved proposal.');
    assert.deepEqual(JSON.parse(values.get(manualKey(id))), manual, 'Noticing saved content does not discard pending edits.');
    await selectNoticePane(document, 'plan');
    await act(async () => { buttonNamed(document, 'Discard pending changes').click(); await settle(); });
    assert.equal(visible().project.name, 'Saved equipment team', 'The explicit discard reveals the actual saved proposal.');
    assert.equal(visible().agent_classes[0].instructions, savedRevision.agent_classes[0].instructions);
    await selectNoticePane(document, 'chat');
    assertProposalNotice(document, false, 'Only viewing the matching saved content acknowledges this update.');
    assert.ok(calls.every(call => call.method === 'GET'));
  }, { narrow: true, manual, savedGuides: [guide] });
});


test('first-minute hierarchy offers both starting routes above the editor and keeps the concrete prompt after typing', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  await mounted(guide, async ({ document, calls }) => {
    const helper = document.querySelector('.forge-starting-conversation');
    const editor = document.querySelector('#setup-message');
    assert.ok(helper.compareDocumentPosition(editor) & document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING,
      'Both starting routes precede the one native idea editor.');
    await typeStarting(document, 'Keep my exact project words.');
    await clickStarting(document, 'Answer two questions');
    await clickStarting(document, 'A team');
    await clickStarting(document, 'Repetitive work');
    const label = document.querySelector('.forge-welcome-input-heading label');
    assert.equal(label.htmlFor, 'setup-message');
    const context = document.querySelector('.forge-starting-context');
    const example = document.querySelector('.forge-starting-example');
    const follows = document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING;
    assert.ok(context.compareDocumentPosition(label) & follows, 'Editable context precedes the concrete writing question.');
    assert.ok(label.compareDocumentPosition(example) & follows, 'The quiet example follows its task question.');
    assert.ok(example.compareDocumentPosition(editor) & follows, 'Inspiration stays adjacent to the user-owned editor.');
    assert.equal(label.textContent, "What's one task you keep doing by hand?");
    assert.equal(document.querySelector('.forge-welcome-input-heading small').textContent, 'Optional');
    assert.equal(editor.value, 'Keep my exact project words.');
    await typeStarting(document, 'Keep my exact project words. More detail.');
    assert.equal(label.textContent, "What's one task you keep doing by hand?", 'Typing never removes the visible task prompt.');
    assert.equal(document.querySelector('#setup-after-send').textContent, 'Nothing is sent until you choose Send.');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), [], 'Context and writing remain unsent.');
  }, { available: true, runAnimationFrames: true });
});

test('disconnected first-minute explains connection before questions without sending the idea', async () => {
  await mounted(freshGuide(), async ({ document, calls }) => {
    await typeStarting(document, 'My unsent goal.');
    assert.equal(document.querySelector('#setup-send').textContent.trim(), 'Connect your guide');
    assert.match(document.querySelector('#setup-after-send').textContent, /First, connect your guide.*Nothing is sent yet/);
    assert.doesNotMatch(document.querySelector('#setup-after-send').textContent, /Your guide asks a few questions/);
    assert.equal(document.querySelector('#setup-message').value, 'My unsent goal.');
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), []);
  });
});

test('first-minute editor descriptions follow context, provider visibility and mobile without dangling guidance', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  await mounted(guide, async ({ document, calls, setNarrow }) => {
    const editor = document.querySelector('#setup-message');
    const descriptions = expected => {
      const ids = (editor.getAttribute('aria-describedby') || '').split(/\s+/).filter(Boolean);
      assert.deepEqual(ids, expected);
      return ids.map(id => {
        const node = document.getElementById(id);
        assert.ok(node, `The editor description ${id} exists in the current mounted state.`);
        assert.equal(document.querySelectorAll(`[id="${id}"]`).length, 1, 'Description IDs are unique.');
        return node.textContent;
      }).join(' ');
    };
    assert.match(descriptions(['setup-after-send', 'setup-idea-shortcuts']), /Nothing is sent until.*Enter to send.*Shift \+ Enter/);
    await clickStarting(document, 'Answer two questions'); await clickStarting(document, 'A team'); await clickStarting(document, 'Keeping track of things');
    assert.match(descriptions(['setup-idea-optional', 'setup-message-starting-example', 'setup-after-send', 'setup-idea-shortcuts']),
      /Optional.*Keep track of decisions and deadlines.*Your two answers stay here until you send/s);
    assert.equal(document.activeElement === document.querySelector('#setup-send'), true);
    assert.equal(editor.placeholder, 'For example: Keep track of decisions and deadlines for a project.');
    await clickStarting(document, 'Change the kind of work: Keeping track of things'); await clickStarting(document, 'Repetitive work');
    assert.match(descriptions(['setup-idea-optional', 'setup-message-starting-example', 'setup-after-send', 'setup-idea-shortcuts']), /Turn scattered notes into a clear weekly update/);
    const written = '  My actual work stays unchanged.\nNot sent.  ';
    await typeStarting(document, written);
    const settings = [...document.querySelectorAll('button')].find(button => button.textContent.trim().startsWith('Guide settings'));
    assert.ok(settings);
    await act(async () => { settings.click(); await settle(); });
    assert.ok(document.querySelector('.setup-provider'));
    descriptions([]);
    assert.equal(document.querySelector('#setup-message') === editor, true);
    assert.equal(editor.value, written);
    await act(async () => { buttonNamed(document.querySelector('.setup-provider'), 'Back to my idea').click(); await settle(); await settle(); });
    assert.match(descriptions(['setup-idea-optional', 'setup-message-starting-example', 'setup-after-send', 'setup-idea-shortcuts']), /Optional.*Turn scattered notes.*Nothing is sent until/s);
    assert.equal(editor.value, written);
    assert.equal(document.activeElement === editor, true);
    await clickStarting(document, 'Remove starting point');
    descriptions(['setup-after-send', 'setup-idea-shortcuts']);
    assert.equal(document.querySelector('.forge-starting-example') === null, true);
    await setNarrow(true);
    descriptions(['setup-after-send']);
    assert.equal(document.getElementById('setup-idea-shortcuts') === null, true, 'Mobile Return does not inherit desktop shortcut guidance.');
    assert.equal(editor.value, written);
    assert.deepEqual(calls.filter(call => call.method !== 'GET'), [], 'Description and provider navigation never submit or start work.');
  }, { available: true, runAnimationFrames: true });
});

test('first-minute editor descriptions clear after explicit submission enters the guide conversation', async () => {
  const guide = { ...freshGuide(), capabilities: { available: true } };
  await mounted(guide, async ({ document, calls }) => {
    await typeStarting(document, 'Shape a useful research team for my next project.');
    assert.ok(document.querySelector('#setup-message').getAttribute('aria-describedby'));
    await act(async () => { document.querySelector('#setup-send').click(); await settle(); });
    assert.equal(document.querySelector('#setup-message').getAttribute('aria-describedby'), null,
      'A continuing conversation does not reference opening-only guidance.');
    assert.equal(document.getElementById('setup-after-send') === null, true);
    assert.equal(calls.filter(call => call.method !== 'GET').length, 1, 'Only the explicit guide Send admits a turn.');
  }, { available: true, runAnimationFrames: true });
});


for (const scenario of ['ordinary handoff', 'hidden choice body fallback', 'focus elsewhere', 'new draft', 'selected guide changed'])
  test(`starting context focus custody: ${scenario}`, async () => {
    const guide = { ...freshGuide(), capabilities: { available: true }, name: 'Original unsent idea' };
    const other = { ...freshGuide(), id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', capabilities: { available: true }, name: 'Other unsent idea' };
    const written = '  Keep my exact unsent wording.\nDo not substitute a context draft.  ';
    await mounted(guide, async ({ document, calls, values, flushAnimationFrames, reload }) => {
      const editor = document.querySelector('#setup-message');
      await typeStarting(document, written);
      await clickStarting(document, 'Answer two questions');
      await clickStarting(document, 'A team');
      const chosen = startingContextButton(document, 'Repetitive work');
      chosen.focus();
      await clickStarting(document, 'Repetitive work');
      assert.equal(editor.value, written, 'Choice commitment leaves every original draft character intact.');
      assert.deepEqual(JSON.parse(values.get(inputKey(guide))).starting_help,
        { version: 2, audience_id: 'team', kind_id: 'repetitive' }, 'Exercise the real version2 context commit.');
      let retainedFocus;
      if (scenario === 'hidden choice body fallback') {
        chosen.blur();
        assert.equal(document.activeElement === document.body, true, 'The browser may drop focus when the chosen helper control becomes hidden.');
      } else if (scenario === 'focus elsewhere') {
        retainedFocus = [...document.querySelectorAll('button')].find(button => button.textContent.trim().startsWith('Guide settings'));
        assert.ok(retainedFocus, 'Another real keyboard action remains available.');
        retainedFocus.focus();
        assert.equal(document.activeElement === retainedFocus, true);
      } else if (scenario === 'new draft') {
        await typeStarting(document, written + ' Newer user writing.');
        retainedFocus = [...document.querySelectorAll('button')].find(button => button.textContent.trim().startsWith('Guide settings'));
        retainedFocus.focus();
      } else if (scenario === 'selected guide changed') {
        await reload(other.id);
        retainedFocus = document.activeElement;
      }
      await flushAnimationFrames();
      if (scenario === 'ordinary handoff' || scenario === 'hidden choice body fallback') assert.equal(document.activeElement === editor, true,
        'The accepted context transfers focus to the original composer after commit.');
      else assert.ok(document.activeElement === retainedFocus,
        'An old deferred context handoff must not take focus after the user moves elsewhere.');
      if (scenario !== 'selected guide changed') assert.equal(editor.value,
        scenario === 'new draft' ? written + ' Newer user writing.' : written);
      assert.deepEqual(calls.filter(call => call.method !== 'GET'), [], 'Context/focus navigation never submits or starts runtime work.');
    }, { available: true, holdAnimationFrames: true, savedGuides: [guide, other] });
  });

for (const scenario of ['ordinary handoff', 'hidden cancel body fallback', 'focus elsewhere', 'new draft', 'selected guide changed'])
  test(`starting context cancellation focus custody: ${scenario}`, async () => {
    const guide = { ...freshGuide(), capabilities: { available: true }, name: 'Original cancelled idea' };
    const other = { ...freshGuide(), id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', capabilities: { available: true }, name: 'Other unsent idea' };
    const written = '  Keep my cancelled idea exactly.\nNo choice should replace these words.  \t';
    await mounted(guide, async ({ document, calls, values, flushAnimationFrames, reload }) => {
      const editor = document.querySelector('#setup-message');
      await typeStarting(document, written);
      await clickStarting(document, 'Answer two questions');
      await clickStarting(document, 'A team');
      const helper = document.querySelector('.forge-starting-conversation');
      assert.equal(helper.dataset.open, 'true');
      assert.equal(helper.querySelector('h3').textContent, 'What would help most?');
      const cancel = startingContextButton(document, 'Back to my words');
      cancel.focus();
      await clickStarting(document, 'Back to my words');
      assert.equal(helper.dataset.open, 'false', 'Cancellation commits the closed helper before the focus frame.');
      assert.equal(cancel.closest('[hidden]')?.hidden, true, 'The old Back action is now inside the hidden question region.');
      assert.equal(document.querySelector('#setup-message') === editor, true, 'The exact native editor remains mounted through cancellation.');
      assert.equal(document.querySelectorAll('textarea').length, 1);
      assert.equal(editor.value, written);
      assert.equal(JSON.parse(values.get(inputKey(guide))).starting_help, undefined,
        'A partly answered helper does not commit separate context when cancelled.');
      assert.equal(document.activeElement === cancel, true, 'Returning to the editor waits until the queued visibility handoff.');

      let retainedFocus;
      if (scenario === 'hidden cancel body fallback') {
        cancel.blur();
        assert.equal(document.activeElement === document.body, true, 'Closing the questions may drop browser focus to body.');
      } else if (scenario === 'focus elsewhere') {
        retainedFocus = [...document.querySelectorAll('button')].find(button => button.textContent.trim().startsWith('Guide settings'));
        assert.ok(retainedFocus, 'Another real keyboard action remains available.');
        retainedFocus.focus();
        assert.equal(document.activeElement === retainedFocus, true);
      } else if (scenario === 'new draft') {
        cancel.blur();
        await typeStarting(document, written + ' Newer user writing.');
        retainedFocus = document.body;
        assert.equal(document.activeElement === retainedFocus, true,
          'Exercise the changed-draft guard even when the hidden-control body fallback would otherwise permit focus.');
      } else if (scenario === 'selected guide changed') {
        await reload(other.id);
        retainedFocus = document.activeElement;
        assert.equal(document.querySelector('.forge-starting-conversation')?.dataset.open, 'false');
      }

      await flushAnimationFrames();
      if (scenario === 'ordinary handoff' || scenario === 'hidden cancel body fallback') assert.equal(document.activeElement === editor, true,
        'The closed helper transfers focus to its original editor after the visibility commit.');
      else assert.equal(document.activeElement === retainedFocus, true,
        'A deferred cancellation must not reclaim focus after newer navigation, writing or a selected-guide change.');
      if (scenario !== 'selected guide changed') {
        assert.equal(document.querySelector('#setup-message') === editor, true);
        assert.equal(editor.value, scenario === 'new draft' ? written + ' Newer user writing.' : written);
        assert.equal(JSON.parse(values.get(inputKey(guide))).draft, editor.value);
      } else {
        assert.equal(JSON.parse(values.get(inputKey(guide))).draft, written, 'Switching setups retains the original exact cancelled draft.');
        assert.equal(document.querySelector('#setup-message').value, '', 'The destination setup does not borrow the original writing.');
      }
      assert.deepEqual(calls.filter(call => call.method !== 'GET'), [], 'Cancellation and focus navigation never send a turn or start runtime work.');
    }, { available: true, holdAnimationFrames: true, savedGuides: [guide, other] });
  });

// Real native transcript coverage for the mobile response handoff. Existing
// proposal tests retain their inert drawing/transcript bundle by default.
const nativeResponseBundle = await build({
  entryPoints: [fileURLToPath(new URL('guided.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent',
  jsx: 'automatic', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) },
  plugins: [{ name: 'real-guide-response-native-transcript', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: `import React from 'react';
        export function ProposalSimulation(props) {
          return props.plan ? React.createElement('div', {'data-proposal-snapshot': JSON.stringify(props.plan)})
            : React.createElement('section', {className: 'ps-empty'}, 'Your mob takes shape here');
        }`, loader: 'js',
    }));
  } }],
});
const { GuidedSetup: NativeResponseGuidedSetup } = await import(`data:text/javascript;base64,${Buffer.from(nativeResponseBundle.outputFiles[0].text).toString('base64')}`);
const responseReply = 'We can keep shared lending history. Who will use this system?';
const responseNextDraft = '  Keep my next note private. Å\nDo not send this yet.  ';
const responseStartedAt = '2026-10-10T10:00:00Z';
const responsePublishedAt = '2026-10-10T10:00:20Z';
function responseGuide({ status = 'running', guideId = id } = {}) {
  return { ...freshGuide(), id: guideId, name: 'Tool lending idea', capabilities: { available: true },
    operation: { status, request_id: '8'.repeat(64), version: 1, started_at: responseStartedAt,
      request: { version: 1, action: 'chat', text: 'Help neighbours share equipment.', selected: [] } } };
}
function responsePublication(running, changes = {}) {
  return { ...running, version: 2,
    operation: { ...running.operation, status: 'completed' },
    messages: [{ id: 'response-user', role: 'user', action: 'chat', text: 'Help neighbours share equipment.', created_at: responseStartedAt },
      { id: 'response-assistant', role: 'assistant', action: 'chat', text: responseReply, created_at: responsePublishedAt }],
    brief: { facts: { outcome: { summary: 'Keep a shared lending record.', status: 'confirmed', source_message_ids: ['response-user'] } }, requirements: [] },
    stage: 'audience', question: { id: 'audience', stage: 'audience', title: 'Who will use this?',
      description: 'Choose the group whose context the agents should retain.', multiple: false, allow_text: true,
      options: [{ id: 'neighbours', label: 'A small group of neighbours', description: 'A shared lending history for our group.' }] },
    ...changes };
}
function visibleMobileResponse(document) {
  const band = document.querySelector('.setup-shell > .setup-mobile-response');
  return band?.querySelector('button.setup-new-response') || null;
}
function responseGeometry(dom) {
  const proto = dom.window.HTMLElement.prototype;
  const hiddenConversation = () => globalThis.matchMedia('(max-width: 760px)').matches &&
    dom.window.document.querySelector('.setup-shell')?.classList.contains('setup-mobile-plan');
  Object.defineProperty(proto, 'clientHeight', { configurable: true,
    get() { return this.classList.contains('setup-chat-scroll') && !hiddenConversation() ? 380 : 0; } });
  Object.defineProperty(proto, 'clientWidth', { configurable: true,
    get() { return this.classList.contains('setup-chat-scroll') && !hiddenConversation() ? 390 : 0; } });
  Object.defineProperty(proto, 'scrollHeight', { configurable: true, get() { return 3500; } });
  Object.defineProperty(proto, 'scrollTop', { configurable: true,
    get() { return this.ownedScrollTop ?? (this.classList.contains('setup-chat-scroll') ? 1100 : 0); },
    set(value) { this.ownedScrollTop = value; } });
  proto.getBoundingClientRect = function() {
    const scroll = dom.window.document.querySelector('.setup-chat-scroll'), offset = scroll?.scrollTop || 0;
    if (this.closest('.setup-conversation') && hiddenConversation()) return { top: 0, height: 0, bottom: 0 };
    if (this.classList.contains('setup-chat-scroll')) return { top: 100, height: 380, bottom: 480 };
    if (this.classList.contains('cc-message--assistant')) return { top: 160 - offset, height: 900, bottom: 1060 - offset };
    if (this.classList.contains('setup-current-question')) return { top: 1200 - offset, height: 440, bottom: 1640 - offset };
    if (this.classList.contains('setup-choice')) return { top: 1330 - offset, height: 100, bottom: 1430 - offset };
    return { top: 0, height: 0, bottom: 0 };
  };
  proto.scrollTo = function(options) { this.scrollTop = options.top; };
}
const nativeResponseOptions = { narrow: true, available: true, component: NativeResponseGuidedSetup,
  setupDom: responseGeometry, holdAnimationFrames: true };

test('mobile response handoff: a saved context question is visible from Your team and opens its exact native reply', async () => {
  const guide = responseGuide();
  await mounted(guide, async ({ document, calls, current, save, poll, flushAnimationFrames }) => {
    await flushAnimationFrames();
    await writeNoticeDraft(document, responseNextDraft);
    await selectNoticePane(document, 'plan');
    await flushAnimationFrames();
    assert.equal(visibleMobileResponse(document) === null, true, 'A running request is not a saved response publication.');
    save(responsePublication(current())); await poll(); await flushAnimationFrames();
    const shell = document.querySelector('.setup-shell');
    assert.ok(shell.classList.contains('setup-mobile-plan'), 'The real plan pane stays selected while context is published.');
    const band = document.querySelector('.setup-shell > .setup-mobile-response');
    const notice = visibleMobileResponse(document);
    assert.ok(notice, 'The actual saved response must be discoverable outside hidden Conversation.');
    assert.equal(band.getAttribute('aria-label'), 'Guide response available');
    assert.equal(band.closest('.setup-conversation') === null, true, 'The control belongs to the visible outer mobile flow.');
    assert.equal(document.querySelectorAll('.setup-mobile-tabs > button').length, 2);
    assert.equal(document.querySelector('.setup-plan-pane [data-proposal-snapshot]') === null, true);
    assert.ok(document.querySelector('.setup-plan-pane .forge-welcome-portrait'), 'Context gathering preserves the circular preview.');
    const conversation = document.querySelector('.setup-conversation'), scroll = document.querySelector('.setup-chat-scroll');
    assert.equal(scroll.clientHeight, 0, 'The selected plan has the ordinary hidden Conversation state.');
    assert.equal(document.querySelector('#setup-message').value, responseNextDraft);
    await act(async () => { notice.click(); await settle(); });
    await flushAnimationFrames();
    assert.ok(shell.classList.contains('setup-mobile-chat'));
    assert.ok(document.activeElement === conversation, 'Focus follows the deliberately opened native conversation.');
    assert.equal(scroll.clientHeight, 380, 'The response is revealed in a nonzero conversation viewport.');
    const assistant = scroll.querySelector('.cc-conversation-turn:last-child .cc-message--assistant');
    assert.ok(assistant, 'The actual native Console transcript supplies the response target.');
    assert.match(assistant.textContent, /We can keep shared lending history/);
    assert.equal(assistant.getBoundingClientRect().top, 116, 'Explicit reveal opens the saved reply, not the choices or transcript footer.');
    assert.match(document.querySelector('.setup-current-question').textContent, /Who will use this/);
    assert.equal(document.querySelector('.setup-new-response') === null, true, 'Only viewing that exact reply consumes the notice.');
    assert.equal(document.querySelector('#setup-message').value, responseNextDraft);
    assert.ok(calls.every(call => call.method === 'GET'), 'Opening, publication and navigation submit no guide or provider work.');
  }, nativeResponseOptions);
});

test('mobile response handoff: a delayed original reveal cannot open a newly selected saved setup', async () => {
  const guide = responseGuide(), other = { ...freshGuide(), id: 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee', name: 'Another retained idea' };
  await mounted(guide, async ({ document, calls, current, save, poll, reload, flushAnimationFrames }) => {
    await flushAnimationFrames();
    await writeNoticeDraft(document, responseNextDraft);
    await selectNoticePane(document, 'plan'); await flushAnimationFrames();
    save(responsePublication(current())); await poll(); await flushAnimationFrames();
    const notice = visibleMobileResponse(document);
    assert.ok(notice, 'The original notice is valid before the delayed action is queued.');
    await act(async () => { notice.click(); await settle(); });
    await reload(other.id);
    await selectNoticePane(document, 'plan');
    const shell = document.querySelector('.setup-shell'), focusBefore = document.activeElement;
    const scroll = document.querySelector('.setup-chat-scroll'), scrollBefore = scroll.scrollTop;
    await flushAnimationFrames();
    assert.equal(current().id, other.id);
    assert.ok(shell.classList.contains('setup-mobile-plan'), 'The old response cannot select another guide\'s Conversation.');
    assert.ok(document.activeElement === focusBefore, 'The old frame cannot focus the newly selected guide.');
    assert.equal(scroll.scrollTop, scrollBefore, 'The old frame cannot scroll another guide\'s transcript.');
    assert.equal(visibleMobileResponse(document) === null, true, 'The other guide has no saved response to borrow.');
    await reload(guide.id);
    assert.equal(document.querySelector('#setup-message').value, responseNextDraft, 'The original unsent draft remains owned by its saved setup.');
    assert.ok(calls.every(call => call.method === 'GET'));
  }, { ...nativeResponseOptions, savedGuides: [guide, other] });
});

test('mobile response handoff: desktop does not add a mobile response band for a real saved publication', async () => {
  const guide = responseGuide();
  await mounted(guide, async ({ document, calls, current, save, poll, flushAnimationFrames }) => {
    await flushAnimationFrames();
    save(responsePublication(current())); await poll(); await flushAnimationFrames();
    assert.equal(document.querySelector('.setup-shell > .setup-mobile-response') === null, true);
    assert.ok(document.querySelector('.setup-chat-scroll').clientHeight > 0, 'Desktop keeps the actual native transcript available.');
    assert.match(document.querySelector('.setup-native-chat').textContent, /We can keep shared lending history/);
    assert.ok(calls.every(call => call.method === 'GET'));
  }, { ...nativeResponseOptions, narrow: false });
});

for (const [label, corrupt] of [
  ['running activity', state => ({ ...state, operation: { ...state.operation, status: 'running' } })],
  ['completion without a saved assistant', state => ({ ...state, messages: state.messages.slice(0, 1) })],
  ['reply with a different request action', state => ({ ...state, messages: state.messages.map(message => message.role === 'assistant' ? { ...message, action: 'answer' } : message) })],
  ['reply timestamp before the original request', state => ({ ...state, messages: state.messages.map(message => message.role === 'assistant' ? { ...message, created_at: '2026-10-10T09:59:59Z' } : message) })],
  ['version unchanged from the original request', state => ({ ...state, version: 1 })],
]) test(`mobile response handoff: ${label} cannot create a visible response action`, async () => {
  const guide = corrupt(responsePublication(responseGuide()));
  await mounted(guide, async ({ document, calls, flushAnimationFrames }) => {
    await flushAnimationFrames();
    await selectNoticePane(document, 'plan'); await flushAnimationFrames();
    assert.ok(document.querySelector('.setup-shell').classList.contains('setup-mobile-plan'));
    assert.equal(visibleMobileResponse(document) === null, true);
    assert.ok(calls.every(call => call.method === 'GET'));
  }, nativeResponseOptions);
});

test('mobile response handoff: moving focus to another control releases a deferred explicit reveal', async () => {
  const guide = responseGuide();
  await mounted(guide, async ({ document, calls, current, save, poll, flushAnimationFrames }) => {
    await flushAnimationFrames();
    await writeNoticeDraft(document, responseNextDraft);
    await selectNoticePane(document, 'plan'); await flushAnimationFrames();
    save(responsePublication(current())); await poll(); await flushAnimationFrames();
    const notice = visibleMobileResponse(document);
    assert.ok(notice);
    await act(async () => { notice.focus(); notice.click(); await settle(); });
    const editor = document.querySelector('#setup-message');
    await act(async () => { editor.focus(); await settle(); });
    await flushAnimationFrames();
    assert.ok(document.activeElement === editor, 'A deliberately focused editor must not be replaced by the earlier response action.');
    assert.ok(document.querySelector('.setup-shell').classList.contains('setup-mobile-chat'));
    assert.equal(editor.value, responseNextDraft);
    assert.ok(calls.every(call => call.method === 'GET'));
  }, nativeResponseOptions);
});

test('mobile response handoff: choosing Your team again releases a deferred explicit conversation reveal', async () => {
  const guide = responseGuide();
  await mounted(guide, async ({ document, calls, current, save, poll, flushAnimationFrames }) => {
    await flushAnimationFrames();
    await writeNoticeDraft(document, responseNextDraft);
    await selectNoticePane(document, 'plan'); await flushAnimationFrames();
    save(responsePublication(current())); await poll(); await flushAnimationFrames();
    const notice = visibleMobileResponse(document);
    assert.ok(notice);
    await act(async () => { notice.focus(); notice.click(); await settle(); });
    const planControl = teamTab(document);
    await act(async () => { planControl.focus(); planControl.click(); await settle(); });
    await flushAnimationFrames();
    assert.ok(document.querySelector('.setup-shell').classList.contains('setup-mobile-plan'), 'The later chosen pane stays selected.');
    assert.ok(document.activeElement === planControl, 'The earlier response action cannot focus a hidden conversation.');
    assert.equal(document.querySelector('.setup-chat-scroll').clientHeight, 0);
    assert.equal(document.querySelector('#setup-message').value, responseNextDraft);
    assert.ok(calls.every(call => call.method === 'GET'));
  }, nativeResponseOptions);
});
