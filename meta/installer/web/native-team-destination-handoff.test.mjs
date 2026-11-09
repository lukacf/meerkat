import { test, after } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";
import { mkdtemp, rm, writeFile } from "node:fs/promises";

const web = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(web, "package.json"));
const { build } = require("esbuild");
const React = require("react"), { act } = React;
const { renderToStaticMarkup } = require("react-dom/server");
const { JSDOM } = createRequire(join(web, "../../console/package.json"))("jsdom");
const bundled = await build({
  stdin: { contents: "export {DeploymentPanel} from './deployment-panel.jsx'; export {NativeTeamPackage} from './guided.jsx'; export {setupProgress} from './setup-progress.mjs'; export {deploymentHandoff, deploymentCanInstall} from './deployment-panel.mjs';",
    resolveDir: web, sourcefile: "native-team-destination-test.mjs" },
  bundle: true, write: false, platform: "node", format: "esm", jsx: "automatic", logLevel: "silent",
  loader: { ".css": "empty" }, nodePaths: [join(web, "node_modules")],
  alias: { "@console-core": join(web, "../../packages/console-core/src") },
  plugins: [{ name: "actual-react-for-native-destination", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }],
});
const temporary = await mkdtemp(join(tmpdir(), "forge-native-destination-"));
const entry = join(temporary, "entry.mjs");
await writeFile(entry, bundled.outputFiles[0].text);
after(() => rm(temporary, { recursive: true, force: true }));
const { DeploymentPanel, NativeTeamPackage, setupProgress, deploymentHandoff, deploymentCanInstall } = await import(pathToFileURL(entry).href);
const Icon = () => null;

function fixture(target = "linux") {
  const plan = { schema_version: 2, project: { name: "Study team", slug: "study-team" }, target,
    connections: [], agent_classes: [], flows: [], knowledge: { mode: "none" }, advanced: { port: 4110 } };
  const state = { id: "native-destination-guide", version: 4, status: "ready", blueprint: plan,
    proposal: null, question: null, readiness: { ready: true, context_ready: true }, brief: {} };
  const handoff = { mode: target === "mac" ? "local" : "remote-package", status: "not-connected",
    package_url: `/api/guide/${state.id}/deployment/package?source_kind=native-team&version=4`,
    steps: [{ title: "Transfer the package to the destination", where: "destination computer", commands: ["bash install.sh"],
      description: "Extract into a new empty folder on the selected Linux computer." }],
    limitations: ["Forge has not connected to or verified a remote deployment."], operations: [] };
  const options = { current: true, guide_version: state.version, current_guide_version: state.version,
    source_kind: "native-team", source_id: "a".repeat(64), source_digest: "b".repeat(64), build_id: null,
    target, host_target: "mac", supported: target === "mac", handoff, fields: [], console_port: 4110,
    default_destination: "/owned/new-study-team", default_execution_mode: "interactive", knowledge_mode: "none",
    execution_modes: [{ id: "interactive", available: target === "mac" }] };
  return { state, plan, options };
}

async function mounted(selected, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], downloads = [], blobs = [], previous = new Map();
  let state = structuredClone(selected.state), options = structuredClone(selected.options), packageRead = () => ({ fixture: true });
  const local = `/api/guide/${state.id}/deployment`;
  const globals = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: async (url, settings = {}) => {
      const method = settings.method || "GET";
      calls.push({ url, method });
      assert.equal(method, "GET", "No install, native, service, build or provider operation is allowed.");
      let data, status = 200;
      if (url === local || url === `${local}/use`) data = null;
      else if (url.startsWith(`${local}/options?`)) data = structuredClone(options);
      else if (url === options.handoff.package_url) return { ok: true, status: 200, blob: packageRead };
      else if (url === `/api/guide/${state.id}/gcp-deployment`) data = null;
      else if (url === `/api/guide/${state.id}/remote-deployment`) status = 404;
      else assert.fail(`No live HTTP fallback: ${url}`);
      return { ok: status === 200, status, json: async () => status === 404 ? { detail: "No saved fixture installation" } : data };
    } };
  for (const [key, value] of Object.entries(globals)) {
    previous.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
    Object.defineProperty(globalThis, key, { configurable: true, value });
  }
  for (const key of ["createObjectURL", "revokeObjectURL"]) {
    previous.set(`URL.${key}`, Object.getOwnPropertyDescriptor(URL, key));
    Object.defineProperty(URL, key, { configurable: true, value: key === "createObjectURL"
      ? value => { blobs.push(value); return "blob:fixture-download"; } : () => {} });
  }
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLAnchorElement.prototype.click = function() { downloads.push({ href: this.href, filename: this.download }); };
  const { createRoot } = await import(pathToFileURL(require.resolve("react-dom/client")).href);
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(DeploymentPanel,
    { state, plan: state.blueprint, build: null, initialSourceKind: "native-team", Icon })));
  const button = label => [...dom.window.document.querySelectorAll("button")].find(node => node.textContent.trim() === label);
  try {
    await render();
    await act(async () => new Promise(resolve => setTimeout(resolve, 20)));
    await verify({ document: dom.window.document, button, calls, downloads, blobs,
      setPackageRead(read) { packageRead = read; },
      async revise(next) { state = structuredClone(next); options = { ...options, guide_version: state.version, current_guide_version: state.version }; await render(); } });
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, value] of previous) {
      const target = key.startsWith("URL.") ? URL : globalThis, name = key.replace(/^URL\./, "");
      if (value) Object.defineProperty(target, name, value); else delete target[name];
    }
  }
}

test("a native manual destination handoff requires the exact current source and native package URL", () => {
  const { state, options } = fixture();
  assert.equal(deploymentHandoff(options, null, state), options.handoff);
  assert.equal(deploymentCanInstall(null, state, options), false);
  for (const change of [{ current: false }, { guide_version: 3 }, { current_guide_version: 3 }, { source_id: "invalid" },
    { source_digest: "invalid" }, { build_id: "application-build" }, { target: "mac" }, { host_target: "linux" },
    { supported: true }, { supported: undefined },
    { handoff: { ...options.handoff, status: "ready" } },
    { handoff: { ...options.handoff, package_url: "/api/guide/other/deployment/package?source_kind=native-team&version=4" } },
    { handoff: { ...options.handoff, package_url: options.handoff.package_url.replace("version=4", "version=3") } }])
    assert.equal(deploymentHandoff({ ...options, ...change }, null, state), null);
});

test("Linux agent team on Mac shows its manual destination steps and downloads the team without private admission", async () => {
  const selected = fixture(), before = structuredClone(selected);
  await mounted(selected, async ({ document, button, calls, downloads, blobs }) => {
    assert.match(document.querySelector("h2 + p").textContent, /targets your Linux computer.*This computer runs macOS/);
    assert.match(document.body.textContent, /no custom application build is needed/);
    assert.ok(document.querySelector(".remote-manual-deployment"));
    assert.match(document.querySelector(".remote-manual-deployment").textContent, /Install this agent team on your Linux computer.*bash install.sh/);
    assert.equal(button("Install on this computer") === undefined, true);
    assert.equal(document.querySelector(".deployment-form") === null, true);
    assert.equal(button("Check and connect") === undefined, true);
    await act(async () => button("Download agent team package").click());
    assert.deepEqual(downloads, [{ href: "blob:fixture-download", filename: "study-team-agent-team.zip" }]);
    assert.equal(blobs.length, 1);
    assert.equal(calls.filter(call => call.url === selected.options.handoff.package_url).length, 1);
    assert.deepEqual(selected, before);
  });
});

test("a matching Mac agent team still opens the original private setup instead of manual handoff", async () => {
  await mounted(fixture("mac"), async ({ document, button, calls }) => {
    assert.equal(document.querySelector(".remote-manual-deployment") === null, true);
    assert.equal(button("Download agent team package") === undefined, true);
    assert.ok(button("Install on this computer"));
    await act(async () => button("Install on this computer").click());
    assert.ok(document.querySelector(".deployment-form"));
    assert.match(document.querySelector(".deployment-form h3").textContent, /Set up your installation/);
    assert.ok(calls.every(call => call.method === "GET"));
  });
});

test("a pending native download cannot publish after the selected plan changes", async () => {
  const selected = fixture();
  await mounted(selected, async ({ button, downloads, blobs, setPackageRead, revise }) => {
    let finish;
    setPackageRead(() => new Promise(resolve => { finish = resolve; }));
    await act(async () => button("Download agent team package").click());
    assert.equal(typeof finish, "function");
    await revise({ ...selected.state, version: 5, brief: { purpose: "Changed study" } });
    await act(async () => finish({ fixture: true }));
    assert.deepEqual(downloads, []);
    assert.deepEqual(blobs, []);
  });
});

test("switching away from the native source and back cannot publish its earlier pending download", async () => {
  await mounted(fixture(), async ({ document, downloads, blobs, button, setPackageRead }) => {
    let finish;
    setPackageRead(() => new Promise(resolve => { finish = resolve; }));
    await act(async () => button("Download agent team package").click());
    await act(async () => document.querySelector('input[name="deployment-source"][value="application"]').click());
    await act(async () => document.querySelector('input[name="deployment-source"][value="native-team"]').click());
    await act(async () => finish({ fixture: true }));
    assert.deepEqual(downloads, []);
    assert.deepEqual(blobs, []);
    assert.ok(document.querySelector('input[name="deployment-source"][value="native-team"]').checked);
  });
});

test("the preflight entry promises the selected destination without assuming the current computer", () => {
  const { state } = fixture(), before = structuredClone(state);
  const progress = setupProgress(state);
  assert.match(progress.detail, /Check the selected destination/);
  assert.ok(!progress.detail.includes("on this computer"));
  assert.equal(progress.actions[0].label, "Install agent team");
  const html = renderToStaticMarkup(React.createElement(NativeTeamPackage, { state, Icon, onPackage: () => {}, onInstall: () => {} }));
  assert.match(html, /Check your destination and private setup/);
  assert.ok(!html.includes("Install here with private setup"));
  assert.match(html, /Download agent team package/);
  assert.deepEqual(state, before);
});
