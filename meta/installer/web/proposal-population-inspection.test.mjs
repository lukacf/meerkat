import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createRequire } from 'node:module';
import React, { act } from 'react';
import { fingerprint } from './state.mjs';
import { createSimulation } from './simulation-state.mjs';

const {JSDOM} = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
function populationPlan(prefix = 'old', count = 305) {
  const contexts = ['alpha', 'beta'].map(id => ({id, name: `${prefix} ${id} contexts`, description: 'Independent continuing context',
    instructions: 'Maintain the named context.', instances: count,
    members: Array.from({length: count}, (_, index) => ({key: `case_${index + 1}`, name: `${prefix} ${id} case ${index + 1}`, instructions: 'Own this case.'})),
    tools: {memory: true, notes: 'read'}}));
  return {project: {name: 'Large configured population', slug: 'population-inspector'},
    agent_classes: [{id: 'lead', name: `${prefix} coordinator`, description: 'Shared outcome', instructions: 'Coordinate the existing owners.', instances: 1, tools: {memory: true}}, ...contexts],
    topology: {mode: 'hub', entrypoint: 'lead:main', edges: []},
    flows: [{id: 'summary', name: 'Context summary', steps: [{id: 'prepare', agent: 'lead:main', prompt: 'Prepare a summary.', depends_on: []}]}],
    connections: [], knowledge: {mode: 'none'}};
}

async function mountedPopulation(verify) {
  const require = createRequire(import.meta.url), instances = [], posts = [], timers = new Map();
  const dom = new JSDOM('<div id="root"></div>', {url: 'http://localhost/', pretendToBeVisual: true});
  let timerId = 0, receiptId = 0, latestPlan = populationPlan(), latestSetup = 'population-guide';
  const replacements = {window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    sessionStorage: dom.window.sessionStorage, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    __populationAnimationInstances: instances, matchMedia: () => ({matches: false, addEventListener() {}, removeEventListener() {}}),
    ResizeObserver: class {observe() {} disconnect() {}},
    setTimeout: (callback, delay) => {const id = ++timerId; timers.set(id, {callback, delay}); return id;},
    clearTimeout: id => timers.delete(id),
    fetch: async (target, options = {}) => {
      if(target.startsWith('/api/visualization-intents/')){
        const body=JSON.parse(options.body);return {ok:true,json:async()=>({...body,status:target.endsWith('/release')?'released':'live',
          intent_token:`00000000-0000-4000-8000-${String(body.intent_revision).padStart(12,'0')}`,lease_seconds:90})};
      }
      if (options.method !== 'POST') return {ok: false, status: 404, json: async () => ({})};
      assert.equal(target, '/api/visualizations');
      const body = JSON.parse(options.body); posts.push(body);
      const [blueprint_hash, model_hash, request_fingerprint] = await Promise.all([fingerprint(body.blueprint), fingerprint(body.model),
        fingerprint({blueprint: body.blueprint, model: body.model, ...(body.feedback ? {feedback: body.feedback} : {})})]);
      const id = String(++receiptId).padStart(64, '0'), code = `return inertProgram${receiptId};`;
      const code_hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(code)))].map(value => value.toString(16).padStart(2, '0')).join('');
      const native_admission = {interaction_id: `offline-${receiptId}`, cursor: 'console:1'};
      const revision_binding = {version: 'prior-program-v1', setup_id: body.setup_id, stream_id: body.stream_id, context_hash: 'a'.repeat(64)};
      const publication_receipt = {authority: 'publish_visualization', request_key: 'e'.repeat(64), blueprint_hash, model_hash,
        reference_hash: 'd'.repeat(64), code_hash, revision_binding};
      return {ok: true, json: async () => ({id, status: 'completed', source_generation: receiptId, native_admission, revision_binding,
        setup_id: body.setup_id, stream_id: body.stream_id, request_fingerprint, blueprint_hash, model_hash, native_request_key: 'e'.repeat(64),
        artifact: {key: 'e'.repeat(64), contract: 'freeform-animation-v3', blueprint_hash, model_hash, reference_hash: 'd'.repeat(64),
          code_hash, code, native_admission, revision_binding, publication_receipt}})};
    }};
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, {configurable: true, value});
  const result = await build({entryPoints: [fileURLToPath(new URL('proposal-simulation.jsx', import.meta.url))], bundle: true, write: false,
    format: 'esm', platform: 'node', logLevel: 'silent', loader: {'.css': 'empty'}, nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
    alias: {'@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url))},
    plugins: [{name: 'inert-population-preview', setup(builder) {
      builder.onResolve({filter: /^react(?:\/.*)?$/}, args => ({path: pathToFileURL(require.resolve(args.path)).href, external: true}));
      builder.onResolve({filter: /(?:freeform-animation|mob-sim-renderer)\.mjs$/}, args => ({path: args.path, namespace: 'inert-preview'}));
      builder.onLoad({filter: /.*/, namespace: 'inert-preview'}, args => ({contents: args.path.includes('mob-sim-renderer')
        ? 'export class MobSim {setScene(){}seek(){}destroy(){}}'
        : `export function animationFailureDetail(){throw Error('Unexpected renderer failure in the population fixture');}
          export class FreeformAnimation {
          constructor(layer,options){this.layer=layer;this.options=options;this.alive=true;this.duration=30;this.time=0;
            this.frame=document.createElement('iframe');layer.append(this.frame);
            globalThis.__populationAnimationInstances.push(this);}
          seek(time){this.time=time;this.options.onFrame?.(time,30);}
          destroy(){this.alive=false;}
          first(){this.options.onReady({duration:30,chapters:[]});this.options.onCaption({title:'Inert preview',description:'Offline presentation fixture'});this.options.onFirstFrame();}
        }`, loader: 'js'}));
    }}]});
  const {ProposalSimulation} = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
  const {createRoot} = await import('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root'));
  const settle = callback => act(async () => {await callback?.(); await new Promise(resolve => setImmediate(resolve));});
  const mount = (plan = latestPlan, setup = latestSetup) => settle(() => {
    latestPlan = plan; latestSetup = setup;
    root.render(React.createElement(ProposalSimulation, {plan, setupId: setup}));
  });
  const button = name => [...dom.window.document.querySelectorAll('button')].find(item => item.textContent === name || item.getAttribute('aria-label') === name);
  const click = name => settle(() => {const target = typeof name === 'string' ? button(name) : name; assert.ok(target, `Existing ${name} button`); target.click();});
  const results = () => [...dom.window.document.querySelectorAll('.ps-member-results button')];
  const search = value => settle(() => {
    const input = dom.window.document.querySelector('input[aria-label="Find an identity"]'); assert.ok(input);
    input.focus();
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set.call(input, value);
    input.dispatchEvent(new dom.window.Event('input', {bubbles: true}));
  });
  const generate = () => settle(async () => {
    const entry = [...timers].find(([, timer]) => timer.delay === 650); assert.ok(entry, 'A current authoring debounce exists');
    timers.delete(entry[0]); await entry[1].callback();
  });
  try {
    await mount();
    await verify({dom, instances, posts, mount, settle, click, button, search, results, generate,
      text: () => dom.window.document.body.textContent, count: () => dom.window.document.querySelector('.proposal-simulation').dataset.nodeCount});
  } finally {
    await settle(() => root.unmount()); dom.window.close();
    assert.ok(instances.every(instance => !instance.alive), 'Unmount releases all inert authored owners');
    for (const [key, descriptor] of previous) {if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];}
  }
}

test('the mounted inspector browses all 611 identities, announces exact ranges and inspects the final member', async () => {
  await mountedPopulation(async ui => {
    await ui.click('Explore identities');
    assert.equal(ui.button('Previous identities').disabled, true);
    const seen = [];
    for (let page = 0; page < 11; page++) {
      assert.match(ui.text(), new RegExp(`Showing ${page * 60 + 1}-${Math.min((page + 1) * 60, 611)} of 611 matching identities`));
      assert.match(ui.text(), new RegExp(`Page ${page + 1} of 11`));
      seen.push(...ui.results().map(item => item.querySelector('code').textContent));
      if (page < 10) {
        const container = ui.dom.window.document.querySelector('.ps-member-results'); container.scrollTop = 200;
        await ui.click('Next identities');
        assert.equal(ui.dom.window.document.activeElement === ui.results()[0], true, 'Paging moves keyboard focus to the new result page');
        assert.equal(container.scrollTop, 0);
      }
    }
    assert.equal(ui.results().length, 11);
    assert.equal(ui.button('Next identities').disabled, true);
    assert.equal(new Set(seen).size, 611);
    assert.deepEqual(seen, createSimulation(populationPlan()).nodes.filter(node => node.kind === 'agent').map(node => node.id));
    const status = ui.dom.window.document.querySelector('.ps-inspector [role="status"]');
    assert.equal(status.getAttribute('aria-live'), 'polite');
    assert.equal(status.getAttribute('aria-atomic'), 'true');
    await ui.click(ui.results().at(-1));
    const inspector = ui.dom.window.document.querySelector('.ps-inspector');
    assert.equal(inspector.querySelector('h4').textContent, 'old beta case 305');
    assert.equal(inspector.querySelector('details code').textContent, 'beta:case_305');
    assert.equal(inspector.querySelectorAll('.ps-relationships li').length, 2, 'Final member retains its exact peer and notes grant');
    assert.equal(ui.dom.window.document.activeElement === inspector, true);
    await ui.click('Close inspector');
    assert.equal(ui.dom.window.document.activeElement === ui.button('Explore identities'), true, 'A removed result button has a visible close-focus fallback');
  });
});

test('search and population switches reset pages, expose exact counts and keep no-match feedback usable', async () => {
  await mountedPopulation(async ui => {
    await ui.click('Explore identities');
    await ui.click('Next identities'); await ui.click('Next identities');
    await ui.search('beta case');
    assert.match(ui.text(), /Showing 1-60 of 305 matching identities/);
    assert.match(ui.text(), /Page 1 of 6/);
    assert.equal(ui.button('Previous identities').disabled, true);
    for (let page = 0; page < 5; page++) await ui.click('Next identities');
    assert.match(ui.text(), /Showing 301-305 of 305 matching identities/);
    assert.equal(ui.results().at(-1).querySelector('code').textContent, 'beta:case_305');
    await ui.search('No matching context');
    assert.match(ui.text(), /No matching identities\./);
    assert.equal(ui.results().length, 0);
    assert.equal(ui.dom.window.document.querySelector('nav[aria-label="Identity result pages"]') === null, true);
    assert.equal(ui.dom.window.document.activeElement.getAttribute('aria-label'), 'Find an identity');
    await ui.search('beta case');
    assert.match(ui.text(), /Showing 1-60 of 305 matching identities/, 'Returning to a prior search must not restore its old final page');
    await ui.search('BETA:CASE_305');
    assert.equal(ui.results().length, 1);
    assert.match(ui.text(), /Showing 1-1 of 1 matching identities/);
    await ui.search('');
    assert.match(ui.text(), /Showing 1-60 of 611 matching identities/);
    await ui.click('Next identities');
    await ui.click('Inspect old alpha contexts, 305 identities');
    assert.match(ui.text(), /305 configured identities/);
    assert.match(ui.text(), /Showing 1-60 of 305 matching identities/);
    assert.ok(ui.results().every(item => item.querySelector('code').textContent.startsWith('alpha:')));
    for (let page = 0; page < 5; page++) await ui.click('Next identities');
    assert.equal(ui.results().length, 5);
    assert.equal(ui.results().at(-1).querySelector('code').textContent, 'alpha:case_305');
    await ui.click('Inspect old beta contexts, 305 identities');
    assert.match(ui.text(), /Showing 1-60 of 305 matching identities/);
    assert.equal(ui.results()[0].querySelector('code').textContent, 'beta:case_1');
  });
});

test('equivalent parent rerenders preserve paging while actual source, view and guide changes reset it', async () => {
  await mountedPopulation(async ui => {
    await ui.click('Explore identities'); await ui.click('Next identities'); await ui.click('Next identities');
    const first = ui.results()[0].querySelector('code').textContent;
    await ui.mount(structuredClone(populationPlan()));
    assert.match(ui.text(), /Showing 121-180 of 611 matching identities/);
    assert.equal(ui.results()[0].querySelector('code').textContent, first);
    await ui.mount(populationPlan('replacement', 32));
    assert.equal(ui.dom.window.document.querySelector('.ps-inspector') === null, true);
    await ui.click('Explore identities');
    assert.match(ui.text(), /Showing 1-60 of 65 matching identities/);
    await ui.click('Next identities');
    assert.match(ui.text(), /Showing 61-65 of 65 matching identities/);
    await ui.settle(() => {
      const select = ui.dom.window.document.querySelector('select[aria-label="View to simulate"]');
      select.value = 'summary'; select.dispatchEvent(new ui.dom.window.Event('change', {bubbles: true}));
    });
    assert.equal(ui.dom.window.document.querySelector('.ps-inspector') === null, true);
    await ui.click('Explore identities');
    assert.match(ui.text(), /Showing 1-60 of 65 matching identities/);
    await ui.click('Next identities');
    await ui.mount(undefined, 'another-guide');
    assert.equal(ui.dom.window.document.querySelector('.ps-inspector') === null, true);
    await ui.click('Explore identities');
    assert.match(ui.text(), /Showing 1-60 of 65 matching identities/);
  });
});

test('updating a custom scene keeps inspection on its displayed historical model until the new first frame', async () => {
  await mountedPopulation(async ui => {
    await ui.generate(); await ui.settle(() => ui.instances[0].first());
    await ui.mount(populationPlan('new', 32));
    assert.equal(ui.count(), '611');
    assert.match(ui.text(), /updating\.\.\./);
    await ui.click('Explore identities');
    for (let page = 0; page < 10; page++) await ui.click('Next identities');
    assert.match(ui.text(), /Showing 601-611 of 611 matching identities/);
    assert.equal(ui.results().at(-1).querySelector('strong').textContent, 'old beta case 305');
    await ui.mount(structuredClone(populationPlan('new', 32)));
    assert.match(ui.text(), /Showing 601-611 of 611 matching identities/);
    await ui.generate();
    assert.equal(ui.count(), '611');
    assert.match(ui.text(), /Showing 601-611 of 611 matching identities/);
    await ui.settle(() => ui.instances.at(-1).first());
    assert.equal(ui.count(), '65');
    assert.match(ui.text(), /Showing 1-60 of 65 matching identities/);
    assert.equal(ui.results()[0].querySelector('strong').textContent, 'new coordinator');
    assert.ok(ui.results().every(item => !item.querySelector('strong').textContent.startsWith('old ')));
    assert.equal(ui.dom.window.document.activeElement === ui.results()[0], true, 'Replacing a removed historical result preserves a usable keyboard target');
  });
});
