import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { JSDOM, ResourceLoader } from 'jsdom';
import { mountConnectorSignIn } from '../examples/connector-sign-in.mjs';

// Exercise the actual Python projection with the public UI validator/renderer.
// The child imports Toolkit only and uses no native client or protected surface.
const pythonSource = fileURLToPath(new URL('../../python/src', import.meta.url));
const fixtureScript = fileURLToPath(new URL('./fixtures/connector-projection.py', import.meta.url));
const fixtures = JSON.parse(execFileSync(process.env.TOOLKIT_TEST_PYTHON ?? 'python3', [fixtureScript], {
  env: { PATH: process.env.PATH ?? '/usr/bin:/bin', PYTHONPATH: pythonSource }, encoding: 'utf8',
}));
const tick = () => new Promise(resolve => setImmediate(resolve));

function mount(t, snapshot, dispatch) {
  const network = [];
  class DenyResources extends ResourceLoader {
    fetch(url) { network.push(url); throw new Error('Unexpected fixture resource'); }
  }
  const dom = new JSDOM('<!doctype html><main></main>', { resources: new DenyResources() });
  dom.window.open = () => { network.push('open'); throw new Error('Unexpected navigation'); };
  dom.window.fetch = () => { network.push('fetch'); throw new Error('Unexpected network'); };
  const container = dom.window.document.querySelector('main');
  const view = mountConnectorSignIn({ container, snapshot, dispatch });
  t.after(() => { view.dispose(); dom.window.close(); });
  return { container, view, network };
}

const button = container => [...container.querySelectorAll('button')]
  .find(node => node.textContent === 'Review sign-in');

test('Python host projection passes public UI validation with exact reference-only dispatch', async t => {
  const snapshot = structuredClone(fixtures.enabled), calls = [];
  const ui = mount(t, snapshot, async request => { calls.push(request); });
  assert.match(ui.container.textContent, /Account chosen during sign-in/);
  assert.match(ui.container.textContent, /Waiting for sign-in/);
  assert.equal(ui.container.querySelector('iframe,a,input,form'), null);
  button(ui.container).click();
  await tick();
  assert.deepEqual(calls, [{ scope: snapshot.scope, viewRevision: snapshot.revision,
    control: snapshot.tasks[0].userActions[0].controls[0] }]);
  assert.deepEqual(ui.network, []);
  assert.equal(ui.container.querySelector('iframe,a,input,form'), null);
  assert.match(ui.container.textContent, /Waiting for sign-in/);
  assert.doesNotMatch(JSON.stringify(calls), /authorize_url|redirect_uri|state|code/);
});

test('missing protected host presenter gives no sign-in control', t => {
  const ui = mount(t, structuredClone(fixtures.disabled), async () => assert.fail('No control offered'));
  assert.equal(button(ui.container), undefined);
  assert.match(ui.container.textContent, /not supplied a protected sign-in presenter/);
  assert.deepEqual(ui.network, []);
});

test('Known account uses the existing renderer caption exactly once', t => {
  const ui = mount(t, structuredClone(fixtures.known), async () => {});
  const text = ui.container.textContent;
  assert.match(text, /Requested account: known-subject/);
  assert.doesNotMatch(text, /Requested account: Requested account:/);
  assert.deepEqual(ui.network, []);
});

test('disposed panel ignores late completion without cancelling or restarting dispatch', async t => {
  const calls = [];
  let resolve;
  const pending = new Promise(done => { resolve = done; });
  const ui = mount(t, structuredClone(fixtures.enabled), request => { calls.push(request); return pending; });
  const old = button(ui.container);
  old.click();
  old.click();
  assert.equal(calls.length, 1);
  ui.view.dispose();
  old.click();
  resolve({ kind: 'presentation_returned' });
  await tick();
  assert.equal(calls.length, 1);
  assert.equal(ui.container.childElementCount, 0);
  assert.deepEqual(ui.network, []);
});

test('host dispatch rejection stays uncertain and does not reveal its exception or retry', async t => {
  let calls = 0;
  const ui = mount(t, structuredClone(fixtures.enabled), async () => {
    calls++;
    throw new Error('FIXTURE-PRIVATE-CALLBACK-URL');
  });
  button(ui.container).click();
  await tick();
  button(ui.container).click();
  assert.equal(calls, 1);
  assert.doesNotMatch(ui.container.textContent, /FIXTURE-PRIVATE/);
  assert.match(ui.container.textContent, /Waiting for sign-in/);
  assert.deepEqual(ui.network, []);
});
