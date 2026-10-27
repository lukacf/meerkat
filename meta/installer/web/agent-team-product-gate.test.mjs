import { test, after } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";
import { mkdtemp, rm, writeFile } from "node:fs/promises";

// The generic agent team gate: when Forge's capabilities report
// agent_team.available === false, for whatever reason they give, the UI offers
// no agent team. No entry reports a refusal today; the gate stays so that the
// UI keeps honouring one when it is reported.
const web = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(web, "package.json"));
const { build } = require("esbuild");
const React = require("react"), { act } = React;
const { JSDOM } = createRequire(join(web, "../../console/package.json"))("jsdom");
const bundled = await build({
  stdin: { contents: "export {DeploymentPanel} from './deployment-panel.jsx'; export {guideStatusText} from './guided.jsx'; export {setupProgress} from './setup-progress.mjs';",
    resolveDir: web, sourcefile: "agent-team-product-gate-test.mjs" },
  bundle: true, write: false, platform: "node", format: "esm", jsx: "automatic", logLevel: "silent",
  loader: { ".css": "empty" }, nodePaths: [join(web, "node_modules")],
  alias: { "@console-core": join(web, "../../packages/console-core/src") },
  plugins: [{ name: "actual-react-for-agent-team-gate", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }],
});
const temporary = await mkdtemp(join(tmpdir(), "forge-agent-team-gate-"));
const entry = join(temporary, "entry.mjs");
await writeFile(entry, bundled.outputFiles[0].text);
after(() => rm(temporary, { recursive: true, force: true }));
const { DeploymentPanel, guideStatusText, setupProgress } = await import(pathToFileURL(entry).href);
const Icon = () => null;

function acceptedPlan() {
  const plan = { schema_version: 2, project: { name: "Study team", slug: "study-team" }, target: "mac",
    connections: [], agent_classes: [], flows: [], knowledge: { mode: "none" }, advanced: { port: 4110 } };
  const state = { id: "agent-team-gate-guide", version: 4, status: "ready", blueprint: plan,
    proposal: null, question: null, readiness: { ready: true, context_ready: true }, brief: {} };
  const options = { current: true, guide_version: 4, current_guide_version: 4, source_kind: "native-team",
    source_id: "a".repeat(64), source_digest: "b".repeat(64), build_id: null, target: "mac", host_target: "mac",
    supported: true, handoff: null, fields: [], console_port: 4110, default_destination: "/owned/study-team",
    default_execution_mode: "interactive", knowledge_mode: "none", execution_modes: [{ id: "interactive", available: true }] };
  return { state, options };
}

async function mounted(props, verify) {
  const { state, options } = acceptedPlan();
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], previous = new Map();
  const local = `/api/guide/${state.id}/deployment`;
  const globals = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: async (url, settings = {}) => {
      calls.push(url);
      assert.equal(settings.method || "GET", "GET", "No install or package operation is allowed.");
      let data = null, status = 200;
      if (url.startsWith(`${local}/options?`)) data = structuredClone(options);
      else if (url === `/api/guide/${state.id}/remote-deployment`) status = 404;
      else if (![local, `${local}/use`, `/api/guide/${state.id}/gcp-deployment`].includes(url))
        assert.fail(`No live HTTP fallback: ${url}`);
      return { ok: status === 200, status, json: async () => status === 404 ? { detail: "No fixture installation" } : data };
    } };
  for (const [key, value] of Object.entries(globals)) {
    previous.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
    Object.defineProperty(globalThis, key, { configurable: true, value });
  }
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  const { createRoot } = await import(pathToFileURL(require.resolve("react-dom/client")).href);
  const root = createRoot(dom.window.document.getElementById("root"));
  try {
    await act(async () => root.render(React.createElement(DeploymentPanel,
      { state, plan: state.blueprint, build: null, initialSourceKind: "native-team", Icon, ...props })));
    await act(async () => new Promise(resolve => setTimeout(resolve, 20)));
    await verify({ document: dom.window.document, calls });
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, value] of previous) {
      if (value) Object.defineProperty(globalThis, key, value); else delete globalThis[key];
    }
  }
}

const agentTeamChoice = document => document.querySelector('input[name="deployment-source"][value="native-team"]');
const nativeOptionsRead = calls => calls.some(url => url.includes("/options?") && url.includes("source_kind=native-team"));

test("the source entry's deployment panel offers the agent team choice", async () => {
  await mounted({}, async ({ document, calls }) => {
    assert.ok(agentTeamChoice(document), "the agent team choice is offered");
    assert.ok(nativeOptionsRead(calls), "the agent team's setup options are read");
  });
});

test("a reported agent team refusal hides the agent team choice and never reads its options", async () => {
  await mounted({ agentTeamAvailable: false }, async ({ document, calls }) => {
    assert.equal(agentTeamChoice(document) === null, true, "no agent team choice");
    assert.equal(document.querySelector("fieldset.deployment-run-mode") === null, true, "no source chooser");
    assert.doesNotMatch(document.body.textContent, /agent team/i);
    assert.equal(nativeOptionsRead(calls), false, "no agent team options are requested");
  });
});

test("with an agent team refusal reported, setup progress and status lead to the builder, not the agent team", () => {
  const { state } = acceptedPlan();
  const offered = setupProgress(state, {}, {});
  assert.equal(offered.title, "Your agent team is ready to install");
  const gated = setupProgress(state, {}, { agentTeamAvailable: false });
  assert.notEqual(gated?.title, "Your agent team is ready to install");
  assert.notEqual(gated?.title, "Your agent team package is available");
  assert.ok(!(gated?.actions || []).some(action => action.sourceKind === "native-team" || action.target === "package"),
    "no agent team install or package action");
  assert.equal(guideStatusText({ state, ready: true }), "Plan accepted. Download your agent team, or build a custom application.");
  assert.equal(guideStatusText({ state, ready: true, agentTeamAvailable: false }), "Plan accepted. Build your application to install it.");
});
