import { after, test } from "node:test";
import assert from "node:assert/strict";
import React, { act } from "react";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const require = createRequire(import.meta.url);
// Use a file-backed bundle with its linked source map so failure stacks do not
// contain the complete base64 bundle. The component and React remain real.
const bundleDirectory = await mkdtemp(join(tmpdir(), 'mobkit-provider-first-use-'));
const providerModulePath = join(bundleDirectory, 'provider-setup.mjs');
after(() => rm(bundleDirectory, { recursive: true, force: true }));
const built = await build({ entryPoints: [fileURLToPath(new URL('provider-setup.jsx', import.meta.url))],
  bundle: true, write: false, platform: 'node', format: 'esm', loader: { '.css': 'empty' },
  outfile: providerModulePath, sourcemap: 'linked', sourcesContent: true, logLevel: 'silent',
  plugins: [{ name: 'real-react', setup(b) { b.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, a => ({
    path: pathToFileURL(require.resolve(a.path)).href, external: true })); } }] }).catch(async error => {
      await rm(bundleDirectory, { recursive: true, force: true });
      throw error;
    });
try {
  for (const output of built.outputFiles) await writeFile(output.path, output.contents);
} catch (error) {
  await rm(bundleDirectory, { recursive: true, force: true });
  throw error;
}
const { ProviderSetup } = await import(pathToFileURL(providerModulePath).href);
const fresh = () => ({ providers: ['openai', 'anthropic', 'gemini'].map(provider => ({ provider, configured: false,
  source: null, editable: true })), effective_provider: null, runtime_active: false, work_active: false, load_error: null });
const saved = () => ({ ...fresh(), effective_provider: 'openai', providers: fresh().providers.map(p =>
  p.provider === 'openai' ? { ...p, configured: true, source: 'saved' } : p) });

async function mount(read, check, { strictMode = false } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost/' });
  const names = ['window','document','navigator','HTMLElement','IS_REACT_ACT_ENVIRONMENT','fetch','localStorage','sessionStorage'];
  const old = new Map(names.map(n => [n, Object.getOwnPropertyDescriptor(globalThis,n)]));
  const calls = [], runtime = [];
  for (const n of ['window','document','navigator','HTMLElement']) Object.defineProperty(globalThis,n,{configurable:true,value:dom.window[n] || dom.window});
  Object.defineProperty(globalThis,'IS_REACT_ACT_ENVIRONMENT',{configurable:true,value:true});
  for (const n of ['localStorage','sessionStorage']) Object.defineProperty(globalThis,n,{configurable:true,get(){throw new Error('Credential form cannot use browser storage');}});
  Object.defineProperty(globalThis,'fetch',{configurable:true,value:async(url,options={}) => {
    const call = {url,method:options.method || 'GET',body:options.body && JSON.parse(options.body)};
    calls.push(call);
    const result = await read(call);
    if (result instanceof Error) throw result;
    return { ok:true, json:async()=>result };
  }});
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root'));
  let retired = false;
  const unmount = async () => {
    if (retired) return;
    retired = true;
    await act(async () => root.unmount());
  };
  const tick = async() => act(async()=>new Promise(r=>setTimeout(r,5)));
  const click = async label => { const button=[...dom.window.document.querySelectorAll('button')].find(b=>b.textContent===label);
    assert.ok(button, label); await act(async()=>button.click()); await tick(); return button; };
  const enter = async key => { const input=dom.window.document.querySelector('input'); assert.ok(input);
    await act(async()=>{ Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype,'value').set.call(input,key);
      input.dispatchEvent(new dom.window.Event('input',{bubbles:true})); input.dispatchEvent(new dom.window.Event('change',{bubbles:true})); }); };
  try {
    const component = React.createElement(ProviderSetup,{onRuntime:r=>runtime.push(r),onClose:()=>{}});
    await act(async()=>root.render(strictMode ? React.createElement(React.StrictMode,null,component) : component));
    await tick(); await check({ document:dom.window.document,calls,runtime,click,enter,tick,unmount });
  } finally {
    await unmount(); dom.window.close();
    for(const [n,d] of old) { if(d) Object.defineProperty(globalThis,n,d); else delete globalThis[n]; }
  }
}

test('fresh provider form privately saves once, clears key and starts only on explicit click', async()=>{
  let configured=false;
  await mount(c=>c.url==='/api/provider-settings' ? c.method==='PUT' ? (configured=true,saved()) : configured?saved():fresh()
    : c.url==='/api/architect/status'?{status:'stopped',available:true}:c.url==='/api/architect/start'?{status:'ready',available:true}:assert.fail(c.url),
    async({document,calls,click,enter})=>{
      assert.match(document.body.textContent,/gpt-6.1-sol/);
      await enter('synthetic-secret-key'); await click('Save locally');
      const write=calls.find(c=>c.method==='PUT'); assert.deepEqual(write.body,{provider:'openai',api_key:'synthetic-secret-key'});
      assert.equal(document.querySelector('input').value,''); assert.doesNotMatch(document.body.textContent,/synthetic-secret-key/);
      assert.equal(calls.filter(c=>c.url==='/api/architect/start').length,0);
      await click('Start guide'); assert.equal(calls.filter(c=>c.url==='/api/architect/start').length,1);
      assert.match(document.body.textContent,/provider access is checked when it runs/);
    });
});

test('environment configuration exposes no input and cannot be replaced', async()=>{
  await mount(()=>({...saved(),providers:saved().providers.map(p=>p.provider==='openai'?{...p,source:'environment'}:p)}),
    async({document,calls})=>{ assert.equal(document.querySelector('input') === null,true); assert.match(document.body.textContent,/server environment/);
      assert.ok(calls.every(c=>c.method==='GET')); });
});

test('lost save response latches until GET readback and never displays secret error', async()=>{
  let configured=false;
  await mount(c=>c.url==='/api/provider-settings'? c.method==='PUT'?(configured=true,new Error('synthetic-secret-key')):configured?saved():fresh():{status:'stopped'},
    async({document,calls,click,enter})=>{ await enter('synthetic-secret-key'); await click('Save locally');
      assert.doesNotMatch(document.body.textContent,/synthetic-secret-key/); assert.equal(document.querySelector('input').value,'');
      await enter('replacement-fixture'); const button=[...document.querySelectorAll('button')].find(b=>b.textContent==='Save locally');
      assert.equal(button.disabled,true); assert.equal(calls.filter(c=>c.method==='PUT').length,1);
      await click('Check saved settings'); assert.ok(calls.filter(c=>c.url==='/api/provider-settings'&&c.method==='GET').length===2);
      assert.equal(document.querySelector('input').value,'');
    });
});

test('a confirmed save remains confirmed when the following status read fails', async()=>{
  await mount(c=>c.url==='/api/provider-settings'?c.method==='PUT'?saved():fresh():new Error('offline'),
    async({document,calls,click,enter})=>{
      await enter('synthetic-secret-key'); await click('Save locally');
      assert.match(document.body.textContent,/The key was saved/);
      assert.doesNotMatch(document.body.textContent,/Saving was not confirmed/);
      assert.equal(document.querySelector('input').value,'');
      assert.equal(calls.filter(c=>c.method==='PUT').length,1);
    });
});

test('lost startup response requires readback before allowing another native action', async()=>{
  let started=false;
  await mount(c=>c.url==='/api/provider-settings'?{...saved(),runtime_active:started}:
    c.url==='/api/architect/start'?(started=true,new Error('lost response')):{status:started?'ready':'stopped',available:true},
    async({document,calls,click})=>{
      await click('Start guide');
      const start=[...document.querySelectorAll('button')].find(b=>b.textContent==='Start guide');
      assert.equal(start.disabled,true);
      assert.equal(calls.filter(c=>c.method==='POST').length,1);
      await click('Check saved settings');
      assert.ok(document.querySelectorAll('button').length);
      assert.equal(calls.filter(c=>c.method==='POST').length,1);
      assert.ok([...document.querySelectorAll('button')].find(b=>b.textContent==='Stop setup team'));
    });
});

const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};
const buttonFor = (document, label) => {
  const button = [...document.querySelectorAll('button')].find(item => item.textContent === label);
  assert.ok(button, label);
  return button;
};
const complete = async (waiting, value, tick) => {
  await act(async () => { waiting.resolve(value); });
  await tick();
};

test('failed initial settings read admits one captured retry and reaches explicit first use', async () => {
  const settingsRead = deferred(), statusRead = deferred();
  let reads = 0, configured = false, statuses = 0;
  await mount(call => {
    if (call.url === '/api/provider-settings' && call.method === 'GET') {
      reads += 1;
      if (reads === 1) return new Error('private-initial-failure');
      if (reads === 2) return settingsRead.promise;
      return configured ? saved() : fresh();
    }
    if (call.url === '/api/provider-settings' && call.method === 'PUT') {
      configured = true;
      return saved();
    }
    if (call.url === '/api/architect/status' && call.method === 'GET')
      return ++statuses === 1 ? statusRead.promise : { status: 'stopped', available: true };
    if (call.url === '/api/architect/start' && call.method === 'POST') return { status: 'ready', available: true };
    assert.fail(`${call.method} ${call.url}`);
  }, async ({ document, calls, runtime, click, enter, tick }) => {
    assert.match(document.body.textContent, /Provider settings could not be read/);
    assert.doesNotMatch(document.body.textContent, /private-initial-failure/);
    const retry = buttonFor(document, 'Check settings');
    await act(async () => { retry.click(); retry.click(); });
    await tick();
    assert.equal(reads, 2, 'captured retry clicks admit one settings read');
    assert.equal(document.querySelector('[role="alert"]') === null, true);
    assert.match(document.body.textContent, /Reading local settings/);
    assert.equal(buttonFor(document, 'Close').disabled, true);

    await complete(settingsRead, fresh(), tick);
    assert.equal(document.querySelector('input').value, '');
    assert.equal(document.querySelector('input').disabled, true);
    assert.equal(buttonFor(document, 'Check saved settings').disabled, true);
    assert.equal(statuses, 1);
    assert.equal(runtime.length, 0);
    await complete(statusRead, { status: 'stopped', available: true }, tick);
    assert.equal(document.querySelector('input').disabled, false);
    assert.equal(buttonFor(document, 'Close').disabled, false);
    assert.deepEqual(runtime, [{ status: 'stopped', available: true }]);

    await enter('synthetic-first-use-key');
    await click('Save locally');
    assert.equal(document.querySelector('input').value, '');
    assert.doesNotMatch(document.body.textContent, /synthetic-first-use-key/);
    assert.equal(calls.filter(call => call.method === 'PUT').length, 1);
    assert.equal(calls.filter(call => call.method === 'POST').length, 0);
    assert.equal(buttonFor(document, 'Start guide').disabled, false);
    await click('Start guide');
    assert.equal(calls.filter(call => call.method === 'POST').length, 1);
    assert.equal(runtime.at(-1).status, 'ready');
    assert.match(document.body.textContent, /Setup team ready/);
  });
});

for (const failure of [false, true]) {
  test(`StrictMode older initial ${failure ? 'failure' : 'success'} cannot replace a confirmed save`, async () => {
    const first = deferred(), second = deferred();
    let reads = 0;
    await mount(call => {
      if (call.url === '/api/provider-settings' && call.method === 'GET') {
        reads += 1;
        assert.ok(reads <= 2, 'only the two actual StrictMode initial reads are expected');
        return reads === 1 ? first.promise : second.promise;
      }
      if (call.url === '/api/provider-settings' && call.method === 'PUT') return saved();
      if (call.url === '/api/architect/status' && call.method === 'GET') return { status: 'stopped', available: true };
      assert.fail(`${call.method} ${call.url}`);
    }, async ({ document, calls, click, enter, tick }) => {
      assert.equal(reads, 2, 'StrictMode starts overlapping effect reads');
      await complete(second, fresh(), tick);
      await enter('synthetic-confirmed-key');
      await click('Save locally');
      assert.match(document.body.textContent, /Saved privately on the computer running Forge/);
      assert.equal(buttonFor(document, 'Start guide').disabled, false);
      await complete(first, failure ? new Error('private-old-observation') : fresh(), tick);
      assert.match(document.body.textContent, /Saved privately on the computer running Forge/);
      assert.match(document.body.textContent, /OpenAI is currently used/);
      assert.equal(buttonFor(document, 'Start guide').disabled, false);
      assert.equal(document.querySelector('[role="alert"]') === null, true);
      assert.equal(document.querySelector('input').value, '');
      assert.doesNotMatch(document.body.textContent, /private-old-observation|synthetic-confirmed-key/);
      assert.equal(calls.filter(call => call.method === 'PUT').length, 1);
      assert.equal(calls.filter(call => call.method === 'POST').length, 0);
    }, { strictMode: true });
  });
}

for (const failure of [false, true]) {
  test(`StrictMode older initial ${failure ? 'failure' : 'success'} cannot replace a newer explicit readback`, async () => {
    const first = deferred(), second = deferred();
    let reads = 0;
    await mount(call => {
      if (call.url === '/api/provider-settings' && call.method === 'GET') {
        reads += 1;
        if (reads === 1) return first.promise;
        if (reads === 2) return second.promise;
        assert.equal(reads, 3, 'one explicit retry follows the current failed initial read');
        return saved();
      }
      if (call.url === '/api/architect/status' && call.method === 'GET') return { status: 'stopped', available: true };
      assert.fail(`${call.method} ${call.url}`);
    }, async ({ document, calls, runtime, click, tick }) => {
      assert.equal(reads, 2);
      await complete(second, new Error('current-initial-failure'), tick);
      assert.match(document.body.textContent, /Provider settings could not be read/);
      await click('Check settings');
      assert.equal(reads, 3);
      assert.equal(buttonFor(document, 'Start guide').disabled, false);
      assert.deepEqual(runtime, [{ status: 'stopped', available: true }]);
      await complete(first, failure ? new Error('private-superseded-failure') : fresh(), tick);
      assert.match(document.body.textContent, /OpenAI is currently used/);
      assert.equal(buttonFor(document, 'Start guide').disabled, false);
      assert.equal(document.querySelector('[role="alert"]') === null, true);
      assert.doesNotMatch(document.body.textContent, /private-superseded-failure|current-initial-failure/);
      assert.equal(runtime.length, 1);
      assert.ok(calls.every(call => call.method === 'GET'));
    }, { strictMode: true });
  });
}

test('settings readback alone cannot clear the unknown startup receipt latch', async () => {
  let statuses = 0;
  await mount(call => {
    if (call.url === '/api/provider-settings' && call.method === 'GET') return saved();
    if (call.url === '/api/architect/start' && call.method === 'POST') return new Error('private-lost-start');
    if (call.url === '/api/architect/status' && call.method === 'GET')
      return ++statuses === 1 ? new Error('private-status-failure') : { status: 'stopped', available: true };
    assert.fail(`${call.method} ${call.url}`);
  }, async ({ document, calls, runtime, click }) => {
    await click('Start guide');
    assert.equal(buttonFor(document, 'Start guide').disabled, true);
    assert.match(document.body.textContent, /team may already have received the request/);
    await click('Check saved settings');
    assert.equal(buttonFor(document, 'Start guide').disabled, true);
    assert.match(document.body.textContent, /Current settings or team status could not be read/);
    assert.match(document.body.textContent, /team may already have received the request/);
    assert.equal(runtime.length, 0);
    assert.equal(calls.filter(call => call.method === 'POST').length, 1);
    assert.doesNotMatch(document.body.textContent, /private-lost-start|private-status-failure/);
    await click('Check saved settings');
    assert.equal(buttonFor(document, 'Start guide').disabled, false);
    assert.doesNotMatch(document.body.textContent, /team may already have received the request/);
    assert.deepEqual(runtime, [{ status: 'stopped', available: true }]);
    assert.equal(calls.filter(call => call.method === 'POST').length, 1, 'readback never resends the lost action');
    assert.equal(calls.filter(call => call.method === 'PUT').length, 0);
  });
});

for (const failure of [false, true]) {
  test(`retired initial retry ignores late settings ${failure ? 'failure' : 'success'} without a runtime read`, async () => {
    const readback = deferred();
    let reads = 0;
    await mount(call => {
      assert.equal(call.url, '/api/provider-settings');
      assert.equal(call.method, 'GET');
      reads += 1;
      assert.ok(reads <= 2);
      return reads === 1 ? new Error('initial-offline') : readback.promise;
    }, async ({ document, calls, runtime, click, tick, unmount }) => {
      await click('Check settings');
      assert.equal(reads, 2);
      assert.equal(buttonFor(document, 'Close').disabled, true);
      await unmount();
      await complete(readback, failure ? new Error('private-late-failure') : saved(), tick);
      assert.equal(document.getElementById('root').childElementCount, 0);
      assert.equal(calls.length, 2, 'retired readback cannot begin a follow-up runtime read');
      assert.equal(runtime.length, 0);
      assert.ok(calls.every(call => call.method === 'GET'));
    });
  });
}


test('selected model host exposes its configured model without a key field or credential writes', async()=>{
  const host = { ...fresh(), effective_provider:'anthropic', effective_model:'claude-sonnet-5-5',
    locked_provider:'anthropic', providers:fresh().providers.map(p=>p.provider==='anthropic'
      ? {...p,configured:true,source:'native_host_config',editable:false}:p) };
  await mount(c=>c.url==='/api/provider-settings'?host:{status:'stopped',available:true},
    async({document,calls})=>{
      assert.equal(document.querySelector('input') === null,true);
      assert.match(document.body.textContent,/claude-sonnet-5-5/);
      assert.match(document.body.textContent,/model configured by this Forge host/);
      assert.doesNotMatch(document.body.textContent,/authentication verified/i);
      assert.ok(calls.every(c=>c.method==='GET'));
    });
});
