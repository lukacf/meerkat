import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';

const web = process.env.FORGE_GUIDE_PACKAGE_WEB || fileURLToPath(new URL('.', import.meta.url));
const selectedSource = process.env.FORGE_GUIDE_PACKAGE_SOURCE || join(web, 'guided.jsx');
const require = createRequire(join(web, 'package.json'));
const React = require('react'), { act } = React;
const { build } = require('esbuild');
const { JSDOM } = createRequire(resolve(web, '../../console/package.json'))('jsdom');
const { inputKey } = await import(pathToFileURL(join(web, 'wizard-state.mjs')).href);
const { GUIDE_KEY } = await import(pathToFileURL(join(web, 'guided-state.mjs')).href);
const source = await readFile(selectedSource, 'utf8');
console.log('Mounted guide source:', selectedSource, createHash('sha256').update(source).digest('hex'));
const directory = await mkdtemp(join(tmpdir(), 'forge-guide-package-deadline-'));
const modulePath = join(directory, 'guided.mjs');
after(() => rm(directory, { recursive: true, force: true }));
const built = await build({
  stdin: { contents: source, sourcefile: selectedSource, resolveDir: web, loader: 'jsx' },
  bundle: true, write: false, platform: 'node', format: 'esm', jsx: 'automatic',
  outfile: modulePath, sourcemap: 'linked', sourcesContent: true, logLevel: 'silent',
  loader: { '.css': 'empty' }, nodePaths: [join(web, 'node_modules')],
  alias: { '@console-core': resolve(web, '../../packages/console-core/src') },
  plugins: [{ name: 'shared-real-react', setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    // Drawing is inert. Actual guide controls, Console transcript and parser remain mounted.
    builder.onLoad({ filter: /proposal-simulation\.jsx$/ }, () => ({
      contents: 'export const ProposalSimulation = () => null;', loader: 'js',
    }));
  } }],
});
for (const output of built.outputFiles) {
  await writeFile(output.path, output.contents);
  if (output.path.endsWith('.map') && process.env.FORGE_GUIDE_PACKAGE_MAP)
    await writeFile(process.env.FORGE_GUIDE_PACKAGE_MAP, output.contents);
  if (output.path.endsWith('.mjs') && process.env.FORGE_GUIDE_PACKAGE_BUNDLE)
    await writeFile(process.env.FORGE_GUIDE_PACKAGE_BUNDLE, output.contents);
}
const { GuidedSetup } = await import(pathToFileURL(modulePath).href);

async function manualExportRecovery(kind) {
  const checkedApplication = kind === 'checked-application';
  const plan = name => ({ schema_version: 2, target: 'mac', provider: 'openai', model: 'gpt-6.1-sol',
    project: { name, slug: name.toLowerCase().replaceAll(' ', '-'), description: 'Prepare a useful workshop handover.' },
    agent_classes: [{ id: 'lead', name: 'Workshop owner', description: 'Keep the supplied workshop context.',
      instructions: 'Use the supplied evidence.', instances: 1, members: [], provider: 'openai', model: 'gpt-6.1-sol',
      tools: { memory: true, workspace: false, notes: 'none', mcp: [] } }],
    topology: { mode: 'manual', entrypoint: 'lead:main', edges: [] }, connections: [], flows: [],
    knowledge: { mode: 'none' }, advanced: { port: 4110 } });
  const guide = (id, name) => ({ id, version: 8, status: 'ready', stage: 'ready', completed_stages: [], answers: [],
    blueprint: plan(name), proposal: null, question: null, operation: null,
    brief: { facts: { outcome: { summary: 'Prepare workshop handovers.', status: 'confirmed', source_message_ids: ['real-user'] } }, requirements: [] },
    messages: [{ id: 'real-user', role: 'user', text: 'Prepare workshop handovers for our coordinators.' }],
    readiness: { ready: true, decision_blockers: [] }, capabilities: { available: true } });
  const a = guide('manual-export-a', 'Workshop A'), b = guide('manual-export-b', 'Workshop B');
  const build = owner => ({ guide_id: owner.id, guide_version: owner.version, current: true, status: 'completed',
    build_id: `accepted-${owner.id}`, source_digest: (owner.id === a.id ? 'a' : 'b').repeat(64),
    package_sha256: (owner.id === a.id ? 'c' : 'd').repeat(64), ready_to_package: true,
    source_verified: true, worker_stop_confirmed: true, cleanup_pending: false,
    acceptance_current: true, acceptance_trusted: true,
    acceptance: { issuer: 'mobkit-launch-acceptance-host', source_digest: (owner.id === a.id ? 'a' : 'b').repeat(64),
      status: 'pass', tests_run: 1, requirements: [], counterexamples: [], summary: 'Inert accepted application source.' },
    files: [], checks: [], progress: [] });
  const packageUrl = checkedApplication
    ? `/api/guide/${a.id}/implementation/package?build_id=${build(a).build_id}` : '/api/blueprint/package';
  const validationUrl = '/api/blueprint/validate';
  const validation = { valid: true, package_availability: { available: true }, files: [], checks: [] };
  const finalContent = checkedApplication ? 'exact checked Workshop A package' : 'exact Workshop A native package';
  const noteA = '  Ask about the next workshop step.\nKeep my exact context \u00e5\u{1f9ed}.  ';
  const noteB = '  This observing idea belongs only to B.  ';
  const inputA = { key: inputKey(a), draft: noteA, selected: [], mode: 'chat' };
  const inputB = { key: inputKey(b), draft: noteB, selected: [], mode: 'chat' };
  const values = new Map([[GUIDE_KEY, a.id], ...[inputA, inputB].map(input => [input.key, JSON.stringify(input)])]);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key), key: index => [...values.keys()][index] ?? null, get length() { return values.size; } };
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/', pretendToBeVisual: true });
  const document = dom.window.document, calls = [], downloads = [], urls = new Map(), revoked = new Set();
  const deadlines = new Map(), held = [];
  const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
  const settle = () => new Promise(resolve => realSetTimeout(resolve, 10));
  let packageReads = 0, validationReads = 0, root;
  function hold(stage, options) {
    let resolve;
    const pending = new Promise(done => { resolve = done; });
    const read = { resolve, started: stage === 'headers', signal: options.signal, stage }; held.push(read);
    if (stage === 'headers') return pending;
    return { ok: true, status: 200, [stage === 'validation' ? 'json' : 'blob']: async () => {
      read.started = true; return pending;
    } };
  }
  const replacements = { window: dom.window, document, navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window), IS_REACT_ACT_ENVIRONMENT: true,
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    localStorage: storage, sessionStorage: dom.window.sessionStorage,
    // Accelerate only the export-read deadline. Actual metadata/polling timers keep their delays.
    setTimeout(callback, delay, ...args) {
      if (delay !== 60000) return realSetTimeout(callback, delay, ...args);
      const token = {}; deadlines.set(token, () => callback(...args)); return token;
    },
    clearTimeout(token) { if (!deadlines.delete(token)) realClearTimeout(token); },
    async fetch(url, options = {}) {
      const method = options.method || 'GET'; calls.push({ url, method });
      if (method !== 'GET') {
        assert.equal(checkedApplication, false, 'Checked application recovery remains GET-only');
        assert.equal(method, 'POST');
        assert.ok([validationUrl, packageUrl].includes(url), 'Only deliberate pure validation/export POSTs are allowed');
        assert.deepEqual(JSON.parse(options.body), a.blueprint, 'Both export reads use the exact selected native plan');
        assert.equal(options.headers?.['Content-Type'], 'application/json');
        if (url === validationUrl) {
          validationReads++;
          if (kind === 'native-validation' && validationReads <= 2) return hold('validation', options);
          assert.ok(validationReads <= 3, 'Validation is not automatically retried');
          return { ok: true, status: 200, json: async () => structuredClone(validation) };
        }
      }
      if (url === packageUrl) {
        assert.equal(method, checkedApplication ? 'GET' : 'POST', 'Native export remains an exact-plan POST and checked application remains GET-only');
        packageReads++;
        if (kind !== 'native-validation' && packageReads <= 2) return hold(kind === 'native-headers' ? 'headers' : 'body', options);
        assert.equal(packageReads, kind === 'native-validation' ? 1 : 3, 'Only explicit retries may obtain another ZIP');
        return { ok: true, status: 200, blob: async () => new Blob([finalContent]) };
      }
      let result;
      if (url === `/api/guide/${a.id}`) result = structuredClone(a);
      else if (url === `/api/guide/${b.id}`) result = structuredClone(b);
      else if (url === '/api/guide/list') result = { setups: [a, b].map(owner => ({ id: owner.id, name: owner.blueprint.project.name, status: owner.status })) };
      else if (url === '/api/architect/status') result = { available: true, status: 'ready', busy: false, model: 'gpt-6.1-sol' };
      else if (url === '/api/capabilities') result = { capabilities: {} };
      else if (url === '/api/checker-setup') result = { status: 'ready', can_prepare: false, detail: 'Inert ready checker' };
      else {
        for (const owner of [a, b]) {
          const base = `/api/guide/${owner.id}`;
          if (url === `${base}/implementation`) result = build(owner);
          else if (url.startsWith(`${base}/implementation/timeline?`)) result = { build_id: build(owner).build_id, frames: [], next_cursor: null, has_more: false };
          else if (url.startsWith(`${base}/timeline?`)) result = { available: true, frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true };
          else if (url === `${base}/gcp-deployment`) result = null;
        }
      }
      if (result !== undefined) return { ok: true, status: 200, json: async () => result };
      // Exact inert observation refusals for source paths with no installed/trial artifact.
      assert.match(url, /^\/api\/guide\/manual-export-[ab]\/(?:deployment(?:\/options\?.*)?|remote-deployment|rehearsal)$/,
        `Unexpected observation: ${url}`);
      return { ok: false, status: 404, json: async () => ({ detail: 'No inert installed or trial artifact.' }) };
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const createUrl = URL.createObjectURL, revokeUrl = URL.revokeObjectURL;
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  URL.createObjectURL = blob => { const url = `blob:manual-export-${urls.size}`; urls.set(url, blob); return url; };
  URL.revokeObjectURL = url => revoked.add(url);
  dom.window.HTMLAnchorElement.prototype.click = function() { downloads.push({ filename: this.download, blob: urls.get(this.href) }); };
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  const composer = () => document.querySelector('#setup-message');
  const send = () => document.querySelector('#setup-send');
  const manual = () => document.querySelector(checkedApplication ? '.setup-manual-export' : '.setup-team-package');
  const download = () => [...manual().querySelectorAll('button')].find(button =>
    (checkedApplication ? /Download checked application|Preparing package/ : /Download agent team package|Preparing package/).test(button.textContent));
  const named = label => {
    const node = [...document.querySelectorAll('button')].find(button => button.textContent.trim() === label || button.querySelector('span')?.textContent === label);
    assert.ok(node, `Missing actual guide control: ${label}`); return node;
  };
  const click = node => act(async () => { node.click(); await settle(); });
  async function observeRead(index) {
    // Observe only the fake read already requested by the user's click, after existing fingerprints settle.
    for (let tries = 0; tries < 30 && !held[index]?.started; tries++) await act(settle);
    assert.equal(held[index]?.started, true, 'The explicit click must reach its selected export read');
  }
  async function finish(read, content) {
    assert.equal(read.started, true);
    const result = read.stage === 'validation' ? structuredClone(validation)
      : read.stage === 'headers' ? { ok: true, status: 200, blob: async () => new Blob([content]) } : new Blob([content]);
    await act(async () => { read.resolve(result); await settle(); });
    await act(settle);
  }
  try {
    const { createRoot } = require('react-dom/client');
    root = createRoot(document.getElementById('root'));
    await act(async () => { root.render(React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null,
      onBlueprint() {}, onSave() {}, config: { architect: { available: true, status: 'ready' } } })); await settle(); });
    await act(settle);
    assert.ok(manual(), 'The current ready setup has the actual selected export section');
    manual().open = true;
    assert.equal(download().disabled, false); assert.equal(send().disabled, false);
    assert.equal(composer().value, noteA);
    const originalA = structuredClone(a), originalB = structuredClone(b);
    await click(download()); await observeRead(0);
    assert.equal(packageReads, kind === 'native-validation' ? 0 : 1); assert.equal(held[0].started, true);
    const firstReads = { validationReads, packageReads };
    assert.equal(download().disabled, true); assert.equal(send().disabled, true);
    assert.equal(composer().disabled, true, 'The selected export excludes edits until its owned read settles');
    // Every fake transport ignores AbortSignal, so the real helper must release the UI itself.
    await act(async () => { for (const callback of [...deadlines.values()]) callback(); await settle(); });
    await act(settle);
    assert.equal(download().disabled, false, 'A held pure export read must release the actual retry button');
    assert.equal(send().disabled, false, 'A metadata-only export must release the guide composer after the read deadline');
    assert.equal(composer().disabled, false, 'The original editor becomes editable after the deadline');
    assert.match(document.querySelector('.setup-error')?.textContent || '', /taking too long.*Try again/);
    assert.equal(held[0].signal?.aborted, true); assert.equal(deadlines.size, 0);
    assert.deepEqual({ validationReads, packageReads }, firstReads, 'Timeout does not retry or export automatically'); assert.equal(downloads.length, 0);
    assert.equal(composer().value, noteA); assert.equal(JSON.parse(values.get(inputA.key)).draft, noteA);
    await finish(held[0], 'obsolete timed-out ZIP');
    assert.equal(downloads.length, 0);
    assert.deepEqual({ validationReads, packageReads }, firstReads, 'A late validation cannot start export and late bytes cannot retry');

    await click(download()); await observeRead(1);
    assert.equal(packageReads, kind === 'native-validation' ? 0 : 2); assert.equal(held[1].started, true);
    const departedReads = { validationReads, packageReads };
    await click(named('Your setups')); await click(named('Workshop B')); await act(settle);
    assert.equal(values.get(GUIDE_KEY), b.id); assert.equal(composer().value, noteB);
    await finish(held[1], 'late ZIP belonging to departed Workshop A');
    assert.equal(downloads.length, 0, 'A ZIP for the departed owner is never downloaded into B');
    assert.deepEqual({ validationReads, packageReads }, departedReads, 'A departed validation does not advance to package export');
    assert.equal(deadlines.size, 0); assert.equal(composer().value, noteB); assert.equal(composer().disabled, false);
    assert.equal(JSON.parse(values.get(inputA.key)).draft, noteA); assert.equal(JSON.parse(values.get(inputB.key)).draft, noteB);
    await click(named('Your setups')); await click(named('Workshop A')); await act(settle);
    manual().open = true;
    assert.equal(composer().value, noteA); assert.equal(download().disabled, false);
    await click(download());
    for (let tries = 0; tries < 30 && downloads.length === 0; tries++) await act(settle);
    assert.equal(packageReads, kind === 'native-validation' ? 1 : 3); assert.equal(downloads.length, 1);
    assert.equal(downloads[0].filename, checkedApplication ? 'workshop-a-application.zip' : 'workshop-a.zip');
    assert.equal(await downloads[0].blob.text(), finalContent);
    assert.equal(send().disabled, false); assert.equal(composer().value, noteA); assert.equal(composer().disabled, false); assert.equal(deadlines.size, 0);
    assert.deepEqual(a, originalA); assert.deepEqual(b, originalB);
    assert.ok(calls.every(call => call.method === 'GET' || !checkedApplication && call.method === 'POST' &&
      [validationUrl, packageUrl].includes(call.url)), 'No guide, build or installation command is admitted');
    assert.equal(validationReads, checkedApplication ? 0 : 3, 'Only three deliberate clicks validate the native plan');
    assert.equal(calls.filter(call => call.url === packageUrl).length, kind === 'native-validation' ? 1 : 3);
  } finally {
    if (root) await act(async () => root.unmount());
    URL.createObjectURL = createUrl; URL.revokeObjectURL = revokeUrl;
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test('manual checked-application export recovers a held ZIP body without losing the guide draft or downloading a departed owner',
  () => manualExportRecovery('checked-application'));
for (const [kind, stage] of [['native-validation', 'validation JSON'], ['native-headers', 'package response headers'], ['native-body', 'ZIP body']]) {
  test(`native-team export recovers held ${stage} without losing its draft, replaying work or downloading a departed owner`,
    () => manualExportRecovery(kind));
}
