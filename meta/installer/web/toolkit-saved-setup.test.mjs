import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act } from 'react';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const bundled = await build({ entryPoints: [fileURLToPath(new URL('toolkit-saved-setup.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent', jsx: 'automatic', loader: { '.css': 'empty' },
  plugins: [{ name: 'shared-react', setup(builder) { builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
    path: pathToFileURL(require.resolve(args.path)).href, external: true })); } }] });
const { ToolkitSavedSetup } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
const base = '/api/guide/current-guide/toolkit-saved';
const rows = [
  { request_id: '1'.repeat(32), request_sha256: 'a'.repeat(64), family: 'services', profile: 'gmail' },
  { request_id: '2'.repeat(32), request_sha256: 'b'.repeat(64), family: 'services', profile: 'slack' },
];
const host = { target: 'macos-arm64', families: ['services'] };
const idle = { available: true, status: 'selected', host, manifest_sha256: 'c'.repeat(64), requests: rows };
const prior = row => ({ selection: row, host, host_fingerprint: 'd'.repeat(64), manifest_sha256: idle.manifest_sha256,
  request_text: JSON.stringify({ selection: { profile: row.profile, values: { context: 'full historical contents' },
    operations: ['historical_operation'] } }) });
const completed = row => ({ ...idle, status: 'review_required', selection: { ...row, version: 7 }, current_operations: [],
  review: { status: 'review_required', family: row.family, profile: row.profile,
    values: { user_id: 'synthetic@example.invalid' }, prior_operations: ['historical_operation'], unapplied: [],
    help: { operations: [{ name: 'new_current_operation' }] } } });
const pause = () => new Promise(resolve => setTimeout(resolve, 5));

async function mount(initial, handle, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost/' });
  const calls = [], discussed = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true, fetch: async (url, options = {}) => {
      const body = options.body && JSON.parse(options.body);
      calls.push({ url, method: options.method || 'GET', body });
      const value = options.method ? await handle(url, body) : initial;
      return { ok: true, json: async () => value };
    } };
  const descriptors = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(document.getElementById('root'));
  const render = version => React.createElement(ToolkitSavedSetup, { guideId: 'current-guide', version,
    disabled: false, onDiscuss: text => discussed.push(text), onClose() {} });
  try {
    await act(async () => { root.render(render(7)); await pause(); });
    await verify({ calls, discussed, root, render, click: async label => {
      const button = [...document.querySelectorAll('button')].find(node => node.textContent === label);
      assert.ok(button, label);
      await act(async () => { button.click(); await pause(); });
    } });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of descriptors) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test('full selected contents and explicit audience check precede exact current-host review', async () => {
  await mount(idle, (url, body) => url.endsWith('/preview') ? prior(rows[0]) : completed(rows[0]),
    async ({ calls, discussed, click }) => {
      await click('gmailservicesRequest 11111111');
      assert.match(document.querySelector('pre').textContent, /full historical contents/);
      const reviewButton = [...document.querySelectorAll('button')].find(node => node.textContent === 'Review current settings');
      assert.equal(reviewButton.disabled, true);
      await act(async () => { reviewButton.click(); await pause(); });
      assert.equal(calls.filter(call => call.url.endsWith('/review')).length, 0);
      await act(async () => { document.querySelector('input[type=checkbox]').click(); await pause(); });
      await click('Review current settings');
      assert.deepEqual(calls.at(-1), { url: `${base}/review`, method: 'POST', body: { version: 7,
        manifest_sha256: 'c'.repeat(64), request_id: '1'.repeat(32), request_sha256: 'a'.repeat(64),
        host_fingerprint: 'd'.repeat(64), audience_approved: true } });
      assert.match(document.querySelector('.setup-saved-tools-current').textContent, /No current operations selected/);
      assert.match(document.querySelector('.setup-saved-tools-current').textContent, /has not installed a connector/);
      assert.equal(document.querySelectorAll('input[type=checkbox]').length, 0);
      await click('Discuss how this fits my mob');
      assert.match(discussed[0], /Historical operations are not current selections/);
      assert.match(discussed[0], /Do not install, authenticate, activate tools/);
      assert.match(discussed[0], new RegExp('a'.repeat(64)));
      assert.equal(calls.some(call => /respond|install|check/.test(call.url)), false);
    });
});

test('selecting a second request removes the first completed review and renews audience choice', async () => {
  await mount(completed(rows[0]), () => prior(rows[1]), async ({ click, discussed }) => {
    assert.match(document.querySelector('h3').textContent, /gmail/);
    await click('slackservicesRequest 22222222');
    assert.equal(document.querySelector('.setup-saved-tools-current') === null, true);
    assert.match(document.querySelector('pre').textContent, /slack/);
    assert.equal(document.querySelector('input[type=checkbox]').checked, false);
    assert.equal([...document.querySelectorAll('button')].find(node => node.textContent === 'Review current settings').disabled, true);
    assert.equal([...document.querySelectorAll('button')].some(node => node.textContent === 'Discuss how this fits my mob'), false);
    assert.deepEqual(discussed, []);
  });
});

test('changed guide version disables admission without submitting the unsent conversation', async () => {
  await mount(idle, () => prior(rows[0]), async ({ click, calls, root, render }) => {
    await click('gmailservicesRequest 11111111');
    await act(async () => { document.querySelector('input[type=checkbox]').click(); await pause(); });
    await act(async () => { root.render(render(8)); await pause(); });
    assert.match(document.querySelector('section').textContent, /Your setup changed/);
    await click('Review current settings');
    assert.equal(calls.some(call => call.url.endsWith('/review')), false);
    assert.equal(calls.some(call => call.url.endsWith('/respond')), false);
  });
});
