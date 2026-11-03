import { test, after } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { dirname, join, relative } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";

// Mount actual installation rendering with fixture-only HTTP observations.
// The overlay permits the same controls to expose the retained pre-fix source.
const web = process.env.FORGE_WEB_SOURCE || dirname(fileURLToPath(import.meta.url));
const overlay = process.env.FORGE_WEB_OVERLAY;
const require = createRequire(join(web, "package.json"));
const { build } = require("esbuild");
const React = require("react"), { act } = React;
const { JSDOM } = createRequire(join(web, "../../console/package.json"))("jsdom");
const bundled = await build({
  stdin: { contents: "export {DeploymentPanel} from './deployment-panel.jsx'; export {canonicalJson} from './state.mjs';",
    resolveDir: web, sourcefile: "pending-install-guidance-entry.mjs" },
  bundle: true, write: false, platform: "node", format: "esm", jsx: "automatic", logLevel: "silent",
  loader: { ".css": "empty" }, nodePaths: [join(web, "node_modules")],
  plugins: [{ name: "inert-installation-guidance", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    if (overlay) builder.onLoad({ filter: /\.jsx$/ }, async args => {
      const name = relative(web, args.path);
      if (name.startsWith("..")) return;
      const replacement = join(overlay, name);
      try { await access(replacement); } catch { return; }
      return { contents: await readFile(replacement, "utf8"), resolveDir: dirname(args.path), loader: "jsx" };
    });
  } }],
});
const temporary = await mkdtemp(join(tmpdir(), "forge-pending-install-guidance-"));
const entry = join(temporary, "entry.mjs");
await writeFile(entry, bundled.outputFiles[0].text);
after(() => rm(temporary, { recursive: true, force: true }));
const { DeploymentPanel, canonicalJson } = await import(pathToFileURL(entry).href);
const Icon = () => null;
const reason = "This saved design selects connection preparation that the current installer cannot package yet. Keep the design for later, or discuss an explicitly revised plan with your guide.";

function fixture(pending = true) {
  const plan = { schema_version: 2, project: { name: "Workshop", slug: "workshop" }, target: "linux",
    connections: [], agent_classes: [], flows: [], knowledge: { mode: "none" }, advanced: { port: 4110 } };
  if (pending) plan.acquisition = { helper_identity: "research:main", helper_tools: "preparation" };
  const state = { id: "pending-installation-guide", version: 4, status: "ready", blueprint: plan, proposal: null,
    question: null, readiness: { ready: true, context_ready: true }, brief: {} };
  state.package_availability = { guide_id: state.id, guide_version: state.version, source: "blueprint",
    blueprint_hash: createHash("sha256").update(canonicalJson(plan)).digest("hex"), available: !pending, reason: pending ? reason : null };
  return { state, plan };
}

function installedReceipt(state, kind) {
  return { guide_id: state.id, guide_version: 3, deployment_id: "retained-installation", status: "stopped",
    destination: "/owned/retained-workshop", detail: "The existing installation is stopped; its records are retained.",
    execution_mode: "interactive", process_live: false, package_sha256: "a".repeat(64), source_digest: "b".repeat(64),
    ...(kind === "native-team" ? { source_kind: kind, source_id: "c".repeat(64), build_id: null } : { build_id: "retained-build" }) };
}

async function mounted({ state, plan }, sourceKind, verify, { receipt = null, build: selectedBuild = null } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], previous = new Map();
  const values = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true, requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window), fetch: async (url, options = {}) => {
      calls.push({ url, method: options.method || "GET" });
      assert.equal(options.method || "GET", "GET", "No installation, provider or runtime action is allowed.");
      let data = null, status = 200;
      const local = `/api/guide/${state.id}/deployment`;
      if (url === local) { data = receipt; status = receipt ? 200 : 404; }
      else if (url === `${local}/use`) data = null;
      else if (url.startsWith(`${local}/options?`)) {
        data = { current: true, guide_version: state.version, current_guide_version: state.version,
          target: "linux", host_target: "linux", supported: true, fields: [], console_port: 4110,
          default_destination: "/owned/new-workshop", default_execution_mode: "interactive", knowledge_mode: "none",
          execution_modes: [{ id: "interactive", available: true }],
          ...(sourceKind === "native-team" ? { source_kind: "native-team", source_id: "d".repeat(64),
            source_digest: "e".repeat(64), build_id: null } : { build_id: selectedBuild?.build_id, source_digest: selectedBuild?.source_digest }) };
      } else if (url === `/api/guide/${state.id}/gcp-deployment`) data = null;
      else if (url === `/api/guide/${state.id}/remote-deployment`) status = 404;
      else assert.fail(`No live HTTP fallback: ${url}`);
      return { ok: status === 200, status, json: async () => status === 404 ? { detail: "No saved fixture installation" } : data };
    } };
  for (const [key, value] of Object.entries(values)) {
    previous.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
    Object.defineProperty(globalThis, key, { configurable: true, value });
  }
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  const { createRoot } = await import(pathToFileURL(require.resolve("react-dom/client")).href);
  const root = createRoot(dom.window.document.getElementById("root"));
  try {
    await act(async () => root.render(React.createElement(DeploymentPanel, { state, plan, build: selectedBuild,
      initialSourceKind: sourceKind, Icon })));
    await act(async () => new Promise(resolve => setTimeout(resolve, 20)));
    await verify(dom.window.document, calls);
    assert.equal(calls.filter(call => call.method !== "GET").length, 0);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, value] of previous) {
      if (value) Object.defineProperty(globalThis, key, value);
      else delete globalThis[key];
    }
  }
}

function guidance(document) {
  const panel = document.querySelector('section[aria-label="Installation and operation"]');
  assert.ok(panel, "The Linux installation area remains reachable.");
  const paragraph = panel.querySelector("h2 + p");
  assert.ok(paragraph, "The actual installation guidance is rendered.");
  return paragraph.textContent;
}

for (const kind of ["native-team", "application"]) test(`pending accepted Linux ${kind} explains the package limitation instead of an impossible action`, async () => {
  const selected = fixture();
  await mounted(selected, kind, async (document, calls) => {
    assert.equal(guidance(document), reason);
    assert.ok(!document.body.textContent.includes("Accept your ready plan with Use plan"));
    assert.ok(!document.body.textContent.includes("You can still download a proposed package above"));
    assert.ok(!document.body.textContent.includes("Build or improve it first"));
    assert.ok(![...document.querySelectorAll("button")].some(button => button.textContent === "Install on this computer"));
    assert.ok(!calls.some(call => call.url.includes("/options?")), "Pending source does not prepare an installation.");
  });
});

for (const kind of ["native-team", "application"]) test(`pending design preserves the saved ${kind} installation detail and Start control`, async () => {
  const selected = fixture(), receipt = installedReceipt(selected.state, kind);
  await mounted(selected, kind, async document => {
    assert.equal(guidance(document), receipt.detail);
    assert.ok(!guidance(document).includes(reason));
    const label = kind === "native-team" ? "Start installed agent team" : "Start installed application";
    const button = [...document.querySelectorAll("button")].find(item => item.textContent === label);
    assert.ok(button && !button.disabled, "Saved stopped source retains its own operation path.");
    assert.ok(document.body.textContent.includes(receipt.destination));
  }, { receipt });
});

test("ready Linux agent team keeps its normal install guidance and available preparation", async () => {
  const selected = fixture(false);
  await mounted(selected, "native-team", async document => {
    assert.equal(guidance(document), "Install the agent team for Workshop into a folder you own, then use its Console. No custom application build is needed.");
    const button = [...document.querySelectorAll("button")].find(item => item.textContent === "Install on this computer");
    assert.ok(button && !button.disabled);
  });
});

test("current accepted application keeps its existing install path despite pending new design packages", async () => {
  const selected = fixture();
  const accepted = { guide_id: selected.state.id, guide_version: selected.state.version, current: true,
    build_id: "accepted-current-build", source_digest: "f".repeat(64), status: "completed", ready_to_package: true,
    acceptance_current: true, acceptance_trusted: true, acceptance: { status: "pass" } };
  await mounted(selected, "application", async document => {
    assert.equal(guidance(document), "Install the accepted application for Workshop into a folder you own. After startup, open the application from this page.");
    const button = [...document.querySelectorAll("button")].find(item => item.textContent === "Install on this computer");
    assert.ok(button && !button.disabled);
  }, { build: accepted });
});
