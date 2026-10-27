import assert from "node:assert/strict";
import { after, test } from "node:test";
import React, { act, useState } from "react";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-provider-held-work-"));
const modulePath = join(directory, "provider-setup.mjs");
after(() => rm(directory, { recursive: true, force: true }));
const built = await build({
  entryPoints: [fileURLToPath(new URL("provider-setup.jsx", import.meta.url))],
  bundle: true, write: false, platform: "node", format: "esm", loader: { ".css": "empty" },
  outfile: modulePath, sourcemap: "linked", sourcesContent: true, logLevel: "silent",
  plugins: [{ name: "real-react", setup(bundler) {
    bundler.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
for (const output of built.outputFiles) await writeFile(output.path, output.contents);
const { ProviderSetup } = await import(pathToFileURL(modulePath).href);

const idea = "  Keep my observing club's history private.\nCompare each night's weather and targets.  ";
const settings = ({ configured = false, active = false, held = false } = {}) => ({
  providers: ["openai", "anthropic", "gemini"].map(provider => ({
    provider, configured: configured && provider === "openai",
    source: configured && provider === "openai" ? "saved" : null, editable: !active,
  })),
  effective_provider: configured ? "openai" : null,
  runtime_active: active, runtime_status: active ? "ready" : "stopped", work_active: held,
  load_error: null, provider_binding_error: null,
});

const outsideClosedDetails = node => {
  for (let parent = node.parentElement; parent; parent = parent.parentElement)
    if (parent.tagName === "DETAILS" && !parent.open) return false;
  return true;
};

// Mount the actual component with inert HTTP projections and callback observations.
// No native runtime, credentials, guide request or installed application is created.
async function mounted({ ready = false, read }, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://offline.test/" });
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
  };
  const names = [...Object.keys(replacements), "fetch", "localStorage", "sessionStorage"];
  const previous = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  const calls = [], runtimes = [];
  let closes = 0, root;
  for (const [name, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, name, { configurable: true, value });
  for (const name of ["localStorage", "sessionStorage"])
    Object.defineProperty(globalThis, name, { configurable: true, get() {
      throw new Error("Provider setup cannot store browser credentials.");
    } });
  Object.defineProperty(globalThis, "fetch", { configurable: true, value: async (url, options = {}) => {
    const call = { url, method: options.method || "GET", body: options.body ? JSON.parse(options.body) : undefined };
    calls.push(call);
    const result = await read(call);
    if (result instanceof Error) throw result;
    return { ok: true, json: async () => result };
  } });
  function Host() {
    const [guideReady, setGuideReady] = useState(ready);
    return React.createElement(ProviderSetup, {
      firstRun: true, idea, guideReady,
      onClose: () => { closes += 1; },
      onRuntime: runtime => { runtimes.push(runtime); setGuideReady(runtime.status === "ready"); },
    });
  }
  const document = dom.window.document;
  const button = label => {
    const result = [...document.querySelectorAll("button")].find(item => item.textContent === label);
    assert.ok(result, label);
    return result;
  };
  const settle = async () => act(async () => new Promise(resolve => setTimeout(resolve, 5)));
  const click = async label => { await act(async () => button(label).click()); await settle(); };
  try {
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root"));
    await act(async () => root.render(React.createElement(Host)));
    await settle();
    await verify({ document, calls, runtimes, button, click, closes: () => closes });
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [name, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
}

function retainedIdea(document) {
  assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
  assert.match(document.querySelector(".setup-provider-idea").textContent, /Not sent/);
}

function heldOnboarding({ document, button }) {
  assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "work");
  assert.equal(document.querySelector("h2").textContent, "Existing work needs attention");
  const notice = document.querySelector(".setup-provider-work");
  assert.ok(notice, "The blocked primary action has an explanatory work notice.");
  assert.equal(outsideClosedDetails(notice), true, "The reason is not hidden inside settings.");
  assert.ok(notice.textContent.trim().length > 0);
  const check = button("Check saved settings");
  assert.equal(check.disabled, false);
  assert.equal(outsideClosedDetails(check), true, "Read-only checking is a direct novice action.");
  assert.equal(document.querySelector('input[type="password"]').disabled, true);
  assert.equal(button("Save locally").disabled, true);
  retainedIdea(document);
}

test("held configured work explains blocked startup and rechecks until the existing hold clears without starting", async () => {
  let held = true;
  await mounted({ read: call => {
    assert.equal(call.method, "GET", "Held work recovery must never change credentials or restart an owner.");
    if (call.url === "/api/provider-settings") return settings({ configured: true, held });
    assert.equal(call.url, "/api/architect/status");
    return { status: "stopped", available: true, startup_retry_required: false };
  } }, async ({ document, calls, runtimes, button, click, closes }) => {
    heldOnboarding({ document, button });
    assert.equal(button("Start guide").disabled, true);
    assert.equal(calls.length, 1);
    assert.deepEqual(runtimes, []);
    await click("Start guide");
    await click("Save locally");
    assert.equal(calls.length, 1, "Disabled mutation actions do not admit a request.");
    await click("Check saved settings");
    heldOnboarding({ document, button });
    assert.equal(button("Start guide").disabled, true);
    assert.equal(calls.length, 3);
    assert.deepEqual(calls.map(call => [call.method, call.url]), [
      ["GET", "/api/provider-settings"],
      ["GET", "/api/provider-settings"], ["GET", "/api/architect/status"],
    ]);
    assert.deepEqual(runtimes, [{ status: "stopped", available: true, startup_retry_required: false }]);
    held = false;
    await click("Check saved settings");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "start");
    assert.equal(document.querySelector("h2").textContent, "Start your setup guide");
    assert.equal(document.querySelector(".setup-provider-work") === null, true);
    assert.equal(button("Start guide").disabled, false);
    assert.equal(outsideClosedDetails(button("Start guide")), true);
    assert.equal(document.querySelector('input[type="password"]').disabled, false);
    retainedIdea(document);
    assert.equal(closes(), 0);
    assert.equal(calls.length, 5);
    assert.ok(calls.every(call => call.method === "GET"));
    assert.equal(document.querySelector(".setup-provider-return") === null, true,
      "Clearing a hold is not evidence that the guide started or provider access works.");
  });
});

test("held work without an effective provider explains the locked key and offers a direct read-only check", async () => {
  await mounted({ read: call => {
    assert.equal(call.method, "GET");
    if (call.url === "/api/provider-settings") return settings({ held: true });
    assert.equal(call.url, "/api/architect/status");
    return { status: "stopped", available: false, startup_retry_required: false };
  } }, async ({ document, calls, button, click, closes }) => {
    heldOnboarding({ document, button });
    assert.equal(document.querySelector(".setup-provider-configured") === null, true);
    assert.equal(button("Start guide").disabled, true);
    await click("Save locally");
    assert.equal(calls.length, 1);
    await click("Check saved settings");
    heldOnboarding({ document, button });
    assert.equal(document.querySelector(".setup-provider-configured") === null, true);
    assert.equal(document.querySelector(".setup-provider-return") === null, true);
    assert.equal(closes(), 0);
    assert.deepEqual(calls.map(call => [call.method, call.url]), [
      ["GET", "/api/provider-settings"],
      ["GET", "/api/provider-settings"], ["GET", "/api/architect/status"],
    ]);
  });
});

test("a confirmed ready guide with held work keeps the existing unsent-draft return without any runtime action", async () => {
  await mounted({ ready: true, read: call => {
    assert.equal(call.method, "GET");
    assert.equal(call.url, "/api/provider-settings");
    return settings({ configured: true, active: true, held: true });
  } }, async ({ document, calls, runtimes, button, click, closes }) => {
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(document.querySelector("h2").textContent, "Ready for your first message");
    const action = button("Back to my draft");
    assert.equal(action.disabled, false);
    assert.equal(outsideClosedDetails(action), true);
    assert.match(document.querySelector(".setup-provider-return p").textContent,
      /Next: send your idea\. Your draft is still unsent\./);
    assert.equal(document.querySelector('input[type="password"]').disabled, true);
    assert.equal(button("Save locally").disabled, true);
    assert.equal(button("Stop setup team").disabled, true);
    retainedIdea(document);
    await click("Save locally");
    await click("Stop setup team");
    await click("Back to my draft");
    assert.equal(closes(), 1, "Returning uses only the original close callback.");
    assert.deepEqual(calls.map(call => [call.method, call.url]), [["GET", "/api/provider-settings"]],
      "The ready return does not change credentials, start or stop the runtime, check it again, or send the draft.");
    assert.deepEqual(runtimes, []);
    retainedIdea(document);
  });
});
