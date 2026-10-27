import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";

// Mount the production deployment panel and answer its destination options
// read after 40 s, as a small host does right after a build. Only the read
// deadlines are held; no server, installation or package action runs.
test("the destination card loads when the options read answers after 40 s", async () => {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const state = { id: "00000000-0000-4000-8000-000000000040", version: 8,
    proposal: { project: { name: "Synthetic app", slug: "synthetic-app" }, target: "mac" } };
  const checked = { guide_id: state.id, guide_version: state.version, build_id: "synthetic-build",
    status: "completed", current: true, ready_to_package: true, acceptance_current: true,
    acceptance_trusted: true, acceptance: { status: "pass" }, source_digest: "a".repeat(64) };
  const base = `/api/guide/${state.id}`;
  const optionsUrl = `${base}/deployment/options?build_id=${checked.build_id}`;
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  const deadlines = new Map(), optionsReads = [], unexpected = [];
  let answer = null;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage, localStorage: dom.window.localStorage,
    IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    setTimeout(callback, delay, ...args) {
      if (delay !== 30000 && delay !== 120000) return originalSetTimeout(callback, delay, ...args);
      const token = { syntheticDeadline: true, delay };
      deadlines.set(token, () => callback(...args));
      return token;
    },
    clearTimeout(token) {
      if (!deadlines.delete(token)) originalClearTimeout(token);
    },
    fetch: async (target, options = {}) => {
      if (target === optionsUrl && (options.method || "GET") === "GET") {
        optionsReads.push(target);
        return new Promise(resolve => { answer = resolve; });
      }
      if (target === `${base}/deployment` || target === `${base}/remote-deployment`)
        return { ok: false, status: 404, json: async () => ({ detail: "No saved installation" }) };
      if (target === `${base}/gcp-deployment`) return { ok: true, json: async () => null };
      unexpected.push(target);
      return { ok: false, status: 404, json: async () => ({ detail: "Synthetic transport has no such read." }) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollIntoView = () => {};
  const { createRoot } = await import("react-dom/client");
  const built = await build({ entryPoints: [fileURLToPath(new URL("deployment-panel.jsx", import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] });
  const { DeploymentPanel } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  const root = createRoot(dom.window.document.getElementById("root"));
  const document = dom.window.document;
  const card = () => [...document.querySelectorAll("button")].some((node) => node.textContent.includes("Download accepted Mac package"));
  try {
    await act(async () => root.render(React.createElement(DeploymentPanel, { state, plan: state.proposal, build: checked,
      designBusy: false, pendingManual: false, Icon: () => null })));
    assert.deepEqual(optionsReads, [optionsUrl], "One destination options read is pending.");
    assert.deepEqual([...deadlines.keys()].map((token) => token.delay), [120000],
      "The destination options read uses its explicit 120 s deadline, not the ordinary 30 s read deadline.");
    // 40 s pass: every held deadline at or under 40 s fires.
    await act(async () => {
      for (const [token, fire] of [...deadlines]) if (token.delay <= 40000) { deadlines.delete(token); fire(); }
    });
    await act(async () => answer({ ok: true, json: async () => ({ build_id: checked.build_id, guide_version: state.version,
      current: true, target: "mac", host_target: "linux", supported: false,
      handoff: { mode: "remote-package", status: "not-connected", package_url: `${base}/implementation/package?build_id=${checked.build_id}`,
        steps: [], limitations: ["Synthetic package; no installation has run."] } }) }));
    assert.doesNotMatch(document.body.textContent, /taking too long/);
    assert.equal(card(), true, "The 40 s answer loads the destination card.");
    assert.equal(deadlines.size, 0, "The settled read clears its deadline.");
    assert.deepEqual(unexpected, []);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});
