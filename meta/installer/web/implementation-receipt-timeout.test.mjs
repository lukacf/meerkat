import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile, writeFile } from "node:fs/promises";
import { resolve, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const web = resolve(process.env.FORGE_RECEIPT_TIMEOUT_WEB || dirname(fileURLToPath(import.meta.url)));
const require = createRequire(resolve(web, "package.json"));
const { build } = require("esbuild");
const React = require("react");
const { act } = React;
const { JSDOM } = createRequire(resolve(web, "../../console/package.json"))("jsdom");

async function loadPanel() {
  const replacement = process.env.FORGE_RECEIPT_TIMEOUT_SOURCE;
  const requestSource = process.env.FORGE_RECEIPT_TIMEOUT_REQUEST_SOURCE;
  const built = await build({
    absWorkingDir: web,
    entryPoints: [resolve(web, "implementation-panel.jsx")],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
    sourcemap: "external", sourcesContent: true, outfile: "receipt-timeout-panel.mjs",
    loader: { ".css": "empty" }, nodePaths: [resolve(web, "node_modules")],
    alias: { "@console-core": resolve(web, "../../packages/console-core/src") },
    plugins: [{ name: "owned-receipt-source", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
      builder.onLoad({ filter: /(?:implementation-panel\.jsx|state\.mjs)$/ }, async args => {
        const source = args.path === resolve(web, "implementation-panel.jsx") ? replacement
          : args.path === resolve(web, "state.mjs") ? requestSource : null;
        if (!source) return;
        return { contents: await readFile(source, "utf8"), loader: args.path.endsWith(".jsx") ? "jsx" : "js",
          resolveDir: web };
      });
    } }],
  });
  const javascript = built.outputFiles.find(file => file.path.endsWith(".mjs"));
  const map = built.outputFiles.find(file => file.path.endsWith(".map"));
  if (process.env.FORGE_RECEIPT_TIMEOUT_BUNDLE) {
    await writeFile(process.env.FORGE_RECEIPT_TIMEOUT_BUNDLE, javascript.contents);
    await writeFile(`${process.env.FORGE_RECEIPT_TIMEOUT_BUNDLE}.map`, map.contents);
  }
  return import(`data:text/javascript;base64,${Buffer.from(javascript.contents).toString("base64")}`);
}

test("a stalled build receipt body releases status recovery and preserves the unsent builder note", async () => {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const state = { id: "receipt-recovery-guide", version: 14,
    proposal: { project: { name: "Night plans" }, target: "mac" } };
  const receiptUrl = `/api/guide/${state.id}/implementation`;
  const noteKey = `mobkit-implementation-feedback-v1:${state.id}`;
  const note = "  Keep my forecast comparison\n\u00e5  ";
  const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
  const deadlines = new Map();
  const calls = [];
  let receiptReads = 0, firstSignal = null, firstBodyStarted = false;
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage,
    IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    // Only the explicit 30 second read deadline is advanced. Poll/UI timers keep their real timing.
    setTimeout: (callback, delay, ...args) => {
      if (delay !== 30000) return realSetTimeout(callback, delay, ...args);
      const token = {};
      deadlines.set(token, () => callback(...args));
      return token;
    },
    clearTimeout: token => {
      if (deadlines.has(token)) deadlines.delete(token);
      else realClearTimeout(token);
    },
    fetch: async (target, options = {}) => {
      calls.push({ target, method: options.method || "GET" });
      assert.notEqual(options.method, "POST", "status recovery must never dispatch a build");
      if (target === "/api/checker-setup") return { ok: true, json: async () => ({
        status: "ready", can_prepare: false, detail: "Inert ready checker",
      }) };
      assert.equal(target, receiptUrl, "only the selected guide's receipt may be requested");
      receiptReads += 1;
      if (receiptReads <= 2) {
        if (receiptReads === 1) firstSignal = options.signal;
        return { ok: true, json: () => {
          firstBodyStarted = true;
          // Headers arrived; the response body remains pending independently of AbortSignal.
          return new Promise(() => {});
        } };
      }
      return { ok: false, status: 404, json: async () => ({ detail: "No saved build" }) };
    },
  };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import(pathToFileURL(require.resolve("react-dom/client")).href);
  const { ImplementationPanel } = await loadPanel();
  const root = createRoot(dom.window.document.getElementById("root"));
  const button = label => [...dom.window.document.querySelectorAll("button")]
    .find(item => item.textContent.includes(label));
  try {
    await act(async () => root.render(React.createElement(ImplementationPanel, {
      state, plan: state.proposal, Icon: () => null,
    })));
    assert.equal(firstBodyStarted, true);
    assert.equal(receiptReads, 1);
    assert.equal(button("Checking build status").disabled, true);
    const textarea = dom.window.document.querySelector("textarea");
    assert.equal(textarea.disabled, false);
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set.call(textarea, note);
      textarea.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
    assert.equal(textarea.value, note);
    assert.equal(dom.window.localStorage.getItem(noteKey), note);
    // Mounting already queues one existing receipt refresh. Keep that behavior:
    // both the first read and that queued read can have stalled response bodies.
    for (let attempt = 0; attempt < 2; attempt += 1) await act(async () => {
      for (const callback of [...deadlines.values()]) callback();
      await new Promise(resolve => realSetTimeout(resolve, 10));
    });
    assert.equal(dom.window.document.querySelector("h2").textContent, "Build status is unavailable",
      "a stalled read must leave loading so the novice can recover");
    assert.equal(firstSignal?.aborted, true);
    assert.equal(receiptReads, 2, "the existing mount refresh runs once without dispatching a build");
    assert.equal(button("Refresh build status").disabled, false);
    assert.equal(button("Build application") === undefined, true, "new build remains blocked until a known observation");
    assert.equal(textarea.value, note);
    assert.equal(dom.window.localStorage.getItem(noteKey), note);
    await act(async () => button("Refresh build status").click());
    assert.equal(receiptReads, 3);
    assert.equal(button("Refresh build status") === undefined, true);
    assert.equal(button("Build application").disabled, false);
    assert.equal(textarea.value, note);
    assert.equal(dom.window.localStorage.getItem(noteKey), note);
    assert.equal(calls.some(call => call.method === "POST"), false);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    deadlines.clear();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});
