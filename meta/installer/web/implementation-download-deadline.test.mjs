import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile, writeFile } from "node:fs/promises";
import { resolve, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const web = resolve(process.env.FORGE_DOWNLOAD_DEADLINE_WEB || dirname(fileURLToPath(import.meta.url)));
const require = createRequire(resolve(web, "package.json"));
const { build } = require("esbuild");
const React = require("react");
const { act } = React;
const { JSDOM } = createRequire(resolve(web, "../../console/package.json"))("jsdom");

async function loadPanels() {
  const sourceDir = process.env.FORGE_DOWNLOAD_DEADLINE_SOURCE_DIR;
  const requestSource = process.env.FORGE_DOWNLOAD_DEADLINE_REQUEST_SOURCE;
  const bundled = await build({
    absWorkingDir: web,
    stdin: { contents: 'export { ImplementationPanel } from "./implementation-panel.jsx"; export { DeploymentPanel } from "./deployment-panel.jsx";',
      resolveDir: web, sourcefile: "download-deadline-entry.mjs" },
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
    sourcemap: "external", sourcesContent: true, outfile: "download-deadline-panels.mjs",
    loader: { ".css": "empty" }, nodePaths: [resolve(web, "node_modules")],
    alias: { "@console-core": resolve(web, "../../packages/console-core/src") },
    plugins: [{ name: "owned-download-source", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
      builder.onLoad({ filter: /(?:implementation-panel\.jsx|deployment-panel\.jsx|state\.mjs)$/ }, async args => {
        const filename = args.path.slice(args.path.lastIndexOf("/") + 1);
        const source = args.path === resolve(web, "state.mjs") ? requestSource
          : sourceDir && args.path === resolve(web, filename) ? resolve(sourceDir, filename) : null;
        if (!source) return;
        return { contents: await readFile(source, "utf8"), loader: filename.endsWith(".jsx") ? "jsx" : "js",
          resolveDir: web };
      });
    } }],
  });
  const javascript = bundled.outputFiles.find(file => file.path.endsWith(".mjs"));
  const map = bundled.outputFiles.find(file => file.path.endsWith(".map"));
  if (process.env.FORGE_DOWNLOAD_DEADLINE_BUNDLE) {
    await writeFile(process.env.FORGE_DOWNLOAD_DEADLINE_BUNDLE, javascript.contents);
    await writeFile(`${process.env.FORGE_DOWNLOAD_DEADLINE_BUNDLE}.map`, map.contents);
  }
  return import(`data:text/javascript;base64,${Buffer.from(javascript.contents).toString("base64")}`);
}

// Only inert transport responses run. The real two panels, their pending owner
// maps, error rendering, accepted source predicates and download handlers mount.
async function downloadJourney(route, Panels) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const state = { id: `download-deadline-${route}`, version: 8,
    proposal: { project: { name: "Observing plans", slug: "observing-plans" }, target: "mac" } };
  const receipt = { guide_id: state.id, guide_version: state.version, build_id: "checked-current-build",
    status: "completed", current: true, ready_to_package: true, acceptance_current: true,
    acceptance_trusted: true, acceptance: { status: "pass" }, source_digest: "a".repeat(64) };
  const base = `/api/guide/${state.id}`;
  const packageUrl = `${base}/implementation/package?build_id=${receipt.build_id}`;
  const note = "  Keep my next build note\nunchanged  ";
  const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
  const deadlines = new Map(), calls = [], downloads = [], urls = new Map(), revoked = new Set();
  let downloadAttempt = 0, awaitingBody = false, signal = null;
  const never = () => new Promise(() => {});
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage,
    sessionStorage: dom.window.sessionStorage, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    // Advance only the opted-in package deadline. Existing polling and UI
    // timers retain their real delays and are cleared by normal unmount.
    setTimeout(callback, delay, ...args) {
      if (delay !== 60000) return realSetTimeout(callback, delay, ...args);
      const token = {};
      deadlines.set(token, () => callback(...args));
      return token;
    },
    clearTimeout(token) {
      if (!deadlines.delete(token)) realClearTimeout(token);
    },
    fetch: async (target, options = {}) => {
      calls.push({ target, method: options.method || "GET" });
      assert.equal(options.method || "GET", "GET", "download recovery must not dispatch build or installation mutations");
      if (target === packageUrl) {
        downloadAttempt += 1;
        signal = options.signal;
        if (downloadAttempt === 1) return { ok: true, blob: () => { awaitingBody = true; return never(); } };
        if (downloadAttempt === 2) return never();
        if (downloadAttempt === 3) return { ok: false, status: 409,
          json: async () => ({ code: "inert_package_conflict", detail: "The checked package needs a fresh observation." }) };
        assert.equal(downloadAttempt, 4, "only explicit retries may issue another package GET");
        return { ok: true, blob: async () => new Blob(["exact inert checked package bytes"]) };
      }
      if (target === "/api/checker-setup") return { ok: true, json: async () => ({ status: "ready", can_prepare: false, detail: "Inert ready checker" }) };
      if (target === `${base}/implementation`) return { ok: true, json: async () => structuredClone(receipt) };
      if (target.startsWith(`${base}/implementation/timeline?`)) return { ok: true,
        json: async () => ({ build_id: receipt.build_id, frames: [], next_cursor: null, has_more: false }) };
      if (target === `${base}/deployment` || target === `${base}/remote-deployment`)
        return { ok: false, status: 404, json: async () => ({ detail: "No saved installation" }) };
      if (target === `${base}/gcp-deployment`) return { ok: true, json: async () => null };
      if (target === `${base}/deployment/options?build_id=${receipt.build_id}`) return { ok: true,
        json: async () => ({ build_id: receipt.build_id, guide_version: state.version, current: true,
          target: "mac", host_target: "linux", supported: false,
          handoff: { mode: "remote-package", status: "not-connected", package_url: packageUrl,
            steps: [], limitations: ["Inert exported package; live installation has not run."] } }) };
      assert.fail(`Unexpected inert download read: ${target}`);
    },
  };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const createUrl = URL.createObjectURL, revokeUrl = URL.revokeObjectURL;
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  URL.createObjectURL = blob => { const url = `blob:owned-${route}-${urls.size}`; urls.set(url, blob); return url; };
  URL.revokeObjectURL = url => revoked.add(url);
  dom.window.HTMLAnchorElement.prototype.click = function () { downloads.push({ filename: this.download, blob: urls.get(this.href) }); };
  dom.window.HTMLElement.prototype.scrollIntoView = () => {};
  const { createRoot } = await import(pathToFileURL(require.resolve("react-dom/client")).href);
  const root = createRoot(dom.window.document.getElementById("root"));
  const document = dom.window.document;
  const label = route === "checked" ? "Download checked application" : "Download accepted Mac package";
  const downloadButton = () => [...document.querySelectorAll("button")].find(item => item.textContent.includes(label));
  const pendingButton = () => [...document.querySelectorAll("button")].find(item => item.textContent.includes(route === "checked"
    ? "Preparing download..." : "Preparing accepted package..."));
  const clickDownload = () => act(async () => downloadButton().click());
  try {
    await act(async () => root.render(React.createElement(route === "checked" ? Panels.ImplementationPanel : Panels.DeploymentPanel,
      { state, plan: state.proposal, build: receipt, designBusy: false, pendingManual: false, Icon: () => null })));
    assert.ok(downloadButton(), `${route}: the exact checked application's package action is present`);
    if (route === "checked") {
      const textarea = document.querySelector("textarea");
      await act(async () => {
        Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set.call(textarea, note);
        textarea.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
      });
    }
    for (const phase of ["body", "headers"]) {
      await clickDownload();
      assert.equal(downloadAttempt, phase === "body" ? 1 : 2);
      assert.equal(pendingButton()?.disabled, true);
      if (phase === "body") assert.equal(awaitingBody, true, "the response arrived but its ZIP body is stalled");
      // The fake response ignores AbortSignal, so releasing the UI requires
      // the real shared deadline race rather than cooperative transport.
      await act(async () => {
        for (const callback of [...deadlines.values()]) callback();
        await new Promise(resolve => realSetTimeout(resolve, 5));
      });
      assert.equal(downloadButton()?.disabled, false,
        `${route}: a stalled ${phase} read must retire its exact pending owner and expose retry`);
      assert.equal(signal?.aborted, true);
      assert.match(document.querySelector('[role="alert"]').textContent, /taking too long.*Try again/);
      assert.equal(downloads.length, 0);
      assert.equal(downloadAttempt, phase === "body" ? 1 : 2, "timeout never retries the GET automatically");
      assert.equal(deadlines.size, 0);
      if (route === "checked") assert.equal(document.querySelector("textarea").value, note);
    }
    await clickDownload();
    assert.equal(downloadAttempt, 3);
    assert.equal(downloadButton().disabled, false);
    assert.match(document.querySelector('[role="alert"]').textContent, /The checked package needs a fresh observation/);
    assert.equal(downloads.length, 0, "a returned HTTP refusal must not create a download");
    await clickDownload();
    assert.equal(downloadAttempt, 4);
    assert.equal(downloadButton().disabled, false);
    assert.equal(downloads.length, 1);
    assert.equal(downloads[0].filename, "observing-plans-application.zip");
    assert.equal(await downloads[0].blob.text(), "exact inert checked package bytes");
    assert.equal(document.querySelector('[role="alert"]') === null, true);
    assert.equal(deadlines.size, 0);
    assert.deepEqual(calls.filter(call => call.target === packageUrl).map(call => call.method), ["GET", "GET", "GET", "GET"]);
    if (route === "checked") assert.equal(document.querySelector("textarea").value, note);
    // Let the existing local object-URL cleanup finish before globals restore.
    await new Promise(resolve => realSetTimeout(resolve, 1020));
    assert.equal(revoked.size, 1);
  } finally {
    await act(async () => root.unmount());
    dom.window.close(); deadlines.clear();
    URL.createObjectURL = createUrl; URL.revokeObjectURL = revokeUrl;
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

test("checked and remote package download deadlines release explicit retries without mutation or lost notes", async () => {
  const Panels = await loadPanels();
  const failures = [];
  for (const route of ["checked", "remote"]) {
    try { await downloadJourney(route, Panels); }
    catch (failure) { failures.push(`${route}: ${failure.stack}`); }
  }
  assert.equal(failures.length, 0, failures.join("\n\n"));
});
