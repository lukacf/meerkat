import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import React, { act } from 'react';
import { deploymentExperiences } from './deployment-panel.mjs';
import { createInstalledUseDraftStore, installedUseScopeKey, installedUseUrl } from './deployment-use.mjs';

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const bundleDirectory = await mkdtemp(join(tmpdir(), 'mobkit-local-application-unicode-'));
const modulePath = join(bundleDirectory, 'deployment-use.mjs');
after(() => rm(bundleDirectory, { recursive: true, force: true }));

// Build only when the reviewed test module is executed. The real mount, use
// view, editable task and source validators remain intact. Externalizing both
// React and ReactDOM preserves the harness's shared module instances. A file
// and linked source map keep a failure stack from printing a complete bundle.
const bundled = await build({
  entryPoints: [fileURLToPath(new URL('deployment-use.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node',
  outfile: modulePath, sourcemap: 'linked', sourcesContent: true,
  logLevel: 'silent', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  plugins: [{ name: 'shared-real-react', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
}).catch(async error => {
  await rm(bundleDirectory, { recursive: true, force: true });
  throw error;
});
try {
  for (const output of bundled.outputFiles) await writeFile(output.path, output.contents);
} catch (error) {
  await rm(bundleDirectory, { recursive: true, force: true });
  throw error;
}

const guide = { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', version: 7 };
const sourceDigest = 'a'.repeat(64), packageDigest = 'b'.repeat(64);
const entryId = 'cafe-workspace', entryLabel = 'Café workspace';
const installed = {
  guide_id: guide.id, guide_version: guide.version, deployment_id: 'owned-cafe-install',
  build_id: 'accepted-cafe-build', source_digest: sourceDigest, package_sha256: packageDigest,
  current: true, status: 'running', process_live: true, health: 'ready',
  console_url: 'http://127.0.0.1:18880/console',
  bootstrap: { complete: true, ready: true, counts: { broken: 0 } },
};
const checked = {
  guide_id: guide.id, guide_version: guide.version, build_id: installed.build_id,
  current: true, source_digest: sourceDigest, source_verified: true, status: 'completed',
};
const idea = {
  label: 'Review my observing notes',
  request: 'Review my café notes.\nExplain one useful next step.',
  description: 'Start with a useful summary of my own notes.',
};
const metadata = {
  scope: 'verified-installed-source', application_outcomes_verified: false,
  deployment_id: installed.deployment_id, build_id: installed.build_id,
  source_digest: sourceDigest, package_sha256: packageDigest,
  guide_version: guide.version, current_guide_version: guide.version,
  version: guide.version, current: true, status: 'running',
  project: { name: 'Café observing notes' },
  entrypoint: { identity: 'lead:main', name: 'Observing lead' },
  native_runtime_available: true, console_url: installed.console_url,
  continuation: { build_id: checked.build_id, source_digest: sourceDigest,
    available: true, requires_acknowledgement: false },
  first_use: { scope: 'authored-first-use-suggestions', metadata_sha256: 'c'.repeat(64),
    entry_id: entryId, tasks: [idea], detail: null },
};
const declared = (path, url = `http://127.0.0.1:18881${path}`) => ({
  deployment_id: installed.deployment_id, build_id: installed.build_id,
  package_sha256: packageDigest, source_digest: sourceDigest, metadata_sha256: 'd'.repeat(64),
  scope: 'declared-source', verified: false,
  entries: [{ id: entryId, label: entryLabel, path, port: 18881, url,
    status: 'declared', verified: false }],
});
const editedTask = '  Compare café notes with 觀測 🌌.\nKeep my exact context and spacing.  ';
const expectedUrl = 'http://127.0.0.1:18881/caf%C3%A9';

async function mountedUse(experiences, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost/', pretendToBeVisual: true });
  const calls = [], unexpected = [];
  let currentMetadata = metadata;
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (target, options = {}) => {
      const call = { target: String(target), method: options.method || 'GET' };
      calls.push(call);
      if (call.target !== installedUseUrl(guide.id) || call.method !== 'GET' || options.body != null) {
        unexpected.push(call);
        throw new Error(`Unexpected offline request: ${call.method} ${call.target}`);
      }
      return { ok: true, json: async () => structuredClone(currentMetadata) };
    },
  };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  const { InstalledApplicationUseMount } = await import(pathToFileURL(modulePath).href);
  const { createRoot } = await import('react-dom/client');
  const root = createRoot(dom.window.document.getElementById('root'));
  const admissions = [];
  let props = {
    state: guide, receipt: installed, build: checked, blocked: false,
    experiences, experienceLoading: false, experienceIssue: null,
    onRepairReceipt: value => admissions.push(value), onReloadExperiences: () => {}, Icon: () => null,
  };
  const render = async changes => {
    props = { ...props, ...changes };
    await act(async () => root.render(React.createElement(InstalledApplicationUseMount, props)));
  };
  const taskField = () => dom.window.document.querySelector('.deployment-use-field textarea');
  const edit = async task => {
    const field = taskField();
    assert.ok(field, 'The real installed task editor is mounted');
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(field, task);
      field.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
    });
  };
  const chooseIdea = async () => {
    const button = dom.window.document.querySelector('.deployment-use-ideas button');
    assert.ok(button, 'The authored task choice is mounted');
    await act(async () => button.click());
  };
  const draft = () => createInstalledUseDraftStore(dom.window.sessionStorage)
    .read(installedUseScopeKey(installed, guide));
  const withdrawHealth = async () => {
    currentMetadata = { ...metadata, native_runtime_available: false, console_url: null };
    await render({ receipt: { ...installed, health: 'unavailable', console_url: null } });
  };
  try {
    await render({});
    await verify({ document: dom.window.document, calls, admissions, chooseIdea, edit, taskField, draft, withdrawHealth });
    assert.deepEqual(unexpected, [], 'The fixture allows only the exact installed-source GET');
    assert.ok(calls.length > 0);
    assert.ok(calls.every(call => call.method === 'GET' && call.target === installedUseUrl(guide.id)));
    assert.deepEqual(admissions, [], 'Trying or editing a task never admits a build');
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

const appLink = document => document.querySelector('.deployment-use-interfaces a');
const taskLink = document => [...document.querySelectorAll('a')]
  .find(link => link.textContent === 'Continue with this task');
const taskPayload = link => JSON.parse(decodeURIComponent(new URL(link.href).hash.slice('#mobkit-task='.length)));

for (const [name, path] of [['raw Unicode', '/café'], ['already escaped equivalent', '/caf%C3%A9']]) {
  test(`local installed application ${name} preserves its exact entry and editable task with one URL encoding`, async () => {
    const experiences = declared(path);
    const projected = deploymentExperiences(experiences, installed, guide.id);
    assert.equal(projected.issue, null);
    assert.equal(projected.entries.length, 1);
    assert.equal(projected.entries[0].id, entryId);
    assert.equal(projected.entries[0].path, path, 'URL normalization does not replace declared entry authority');
    assert.equal(projected.entries[0].url, expectedUrl);
    await mountedUse(experiences, async ({ document, chooseIdea, edit, taskField, draft, withdrawHealth, calls }) => {
      assert.equal(appLink(document)?.getAttribute('href'), expectedUrl);
      assert.equal(appLink(document)?.textContent.trim(), `Open ${entryLabel}`);
      assert.equal(taskLink(document) === undefined, true, 'An empty draft is not handed off');
      await chooseIdea();
      assert.equal(taskField().value, idea.request);
      assert.deepEqual(taskPayload(taskLink(document)), { version: 1, task: idea.request });
      await edit(editedTask);
      const handoff = taskLink(document);
      assert.ok(handoff, 'The exact authored entry accepts the current human draft');
      assert.equal(handoff.getAttribute('aria-label'), `Continue with this task in ${entryLabel}`);
      assert.equal(handoff.href.split('#')[0], expectedUrl);
      assert.equal(new URL(handoff.href).pathname, '/caf%C3%A9');
      assert.deepEqual(taskPayload(handoff), { version: 1, task: editedTask });
      assert.equal(handoff.getAttribute('referrerpolicy'), 'no-referrer');
      assert.equal(handoff.rel, 'noopener noreferrer');
      assert.equal(handoff.target, '_blank');
      assert.equal(taskField().value, editedTask);
      assert.deepEqual(draft(), { task: editedTask, outcome: '', improvement: '' });
      const readsBeforeWithdrawal = calls.length;
      await withdrawHealth();
      assert.ok(calls.length > readsBeforeWithdrawal, 'Native health loss rechecks installed-source details');
      assert.equal(appLink(document) === null, true, 'Unavailable native health withdraws the ordinary application link');
      assert.equal(taskLink(document) === undefined, true, 'Unavailable native health withdraws the task handoff');
      assert.equal(taskField().value, editedTask);
      assert.deepEqual(draft(), { task: editedTask, outcome: '', improvement: '' });
    });
  });
}

test('local installed application refuses a different declared path while retaining the editable task', async () => {
  const experiences = declared('/café', 'http://127.0.0.1:18881/caf%C3%A9/other');
  const projected = deploymentExperiences(experiences, installed, guide.id);
  assert.deepEqual(projected.entries, []);
  assert.ok(projected.issue, 'Unicode normalization cannot admit a different path');
  await mountedUse(experiences, async ({ document, chooseIdea, edit, taskField, draft }) => {
    assert.equal(appLink(document) === null, true);
    assert.equal(taskLink(document) === undefined, true);
    assert.match(document.body.textContent, /Retry UI links/);
    await chooseIdea();
    assert.equal(taskField().value, idea.request);
    await edit(editedTask);
    assert.equal(appLink(document) === null, true);
    assert.equal(taskLink(document) === undefined, true);
    assert.equal(taskField().value, editedTask);
    assert.deepEqual(draft(), { task: editedTask, outcome: '', improvement: '' });
  });
});
