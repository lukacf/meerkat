import { after, test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const webRoot = process.env.FORGE_TEST_WEB_ROOT || fileURLToPath(new URL(".", import.meta.url));
const sourcePath = process.env.FORGE_PROVIDER_SETUP_SOURCE || join(webRoot, "provider-setup.jsx");
const require = createRequire(join(webRoot, "package.json"));
const React = require("react"), { act } = React;
const { build } = require("esbuild");
const { JSDOM } = createRequire(resolve(webRoot, "../../console/package.json"))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-provider-draft-readback-"));
const modulePath = join(directory, "provider-setup.mjs");
after(() => rm(directory, { recursive: true, force: true }));
const built = await build({
  stdin: { contents: await readFile(sourcePath, "utf8"), sourcefile: sourcePath, resolveDir: webRoot, loader: "jsx" },
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

const settings = (locked = null) => ({
  providers: ["openai", "anthropic", "gemini"].map(provider => ({
    provider, configured: provider !== "gemini", source: provider !== "gemini" ? "saved" : null,
    editable: true,
  })),
  effective_provider: locked || "openai", locked_provider: locked,
  provider_binding_error: null, runtime_active: false, work_active: false, load_error: null,
});

async function mounted(read, verify, props = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://offline.test/" });
  const values = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true };
  const names = [...Object.keys(values), "fetch", "localStorage", "sessionStorage"];
  const previous = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  const calls = [], runtime = [];
  let root;
  const forbidStorage = () => { throw new Error("Provider drafts cannot use browser storage."); };
  for (const [name, value] of Object.entries(values))
    Object.defineProperty(globalThis, name, { configurable: true, value });
  for (const name of ["localStorage", "sessionStorage"]) {
    Object.defineProperty(globalThis, name, { configurable: true, get: forbidStorage });
    Object.defineProperty(dom.window, name, { configurable: true, get: forbidStorage });
  }
  Object.defineProperty(globalThis, "fetch", { configurable: true, value: async (url, options = {}) => {
    const call = { url, method: options.method || "GET", body: options.body ? JSON.parse(options.body) : undefined };
    calls.push(call);
    const result = await read(call);
    if (result instanceof Error) throw result;
    return { ok: true, json: async () => result };
  } });
  const document = dom.window.document;
  const button = label => {
    const value = [...document.querySelectorAll("button")].find(item => item.textContent === label);
    assert.ok(value, label);
    return value;
  };
  const settle = async () => act(async () => new Promise(done => setTimeout(done, 5)));
  const click = async label => { await act(async () => button(label).click()); await settle(); };
  const choose = async provider => {
    const select = document.querySelector("select");
    assert.ok(select); assert.equal(select.disabled, false);
    await act(async () => {
      select.value = provider;
      select.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    });
  };
  const enter = async value => {
    const input = document.querySelector('input[type="password"]');
    assert.ok(input); assert.equal(input.disabled, false);
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(input, value);
      input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
      input.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    });
  };
  try {
    const { createRoot } = require("react-dom/client");
    root = createRoot(document.getElementById("root"));
    await act(async () => root.render(React.createElement(ProviderSetup,
      { onRuntime: value => runtime.push(value), onClose() {}, ...props })));
    await settle();
    await verify({ document, calls, runtime, button, click, choose, enter });
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [name, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
}

test("ordinary provider checks retain the unsaved provider/key pair while recovery and forced provider changes clear it", async () => {
  let mode = "ordinary", writes = 0;
  await mounted(call => {
    if (call.url === "/api/provider-settings" && call.method === "GET")
      return mode === "failed-read" ? new Error("Synthetic status read failure")
        : settings(mode === "locked" ? "anthropic" : null);
    if (call.url === "/api/architect/status" && call.method === "GET") return { status: "stopped", available: true };
    if (call.url === "/api/provider-settings" && call.method === "PUT") {
      writes += 1;
      assert.deepEqual(call.body, { provider: "anthropic", api_key: "fixture-anthropic-draft" });
      return new Error("Synthetic lost save response");
    }
    assert.fail(`${call.method} ${call.url}`);
  }, async ({ document, calls, button, click, choose, enter }) => {
    const pair = (provider, key) => {
      assert.equal(document.querySelector('input[type="password"]').value, key, "readback must retain the unsaved key with its provider");
      assert.equal(document.querySelector("select").value, provider, "an effective provider must not take another provider's draft");
      assert.ok(!document.body.textContent.includes("fixture-"), "synthetic keys must stay out of rendered text");
    };
    await choose("anthropic");
    await enter("fixture-anthropic-draft");
    pair("anthropic", "fixture-anthropic-draft");
    await click("Check saved settings");
    pair("anthropic", "fixture-anthropic-draft");
    assert.equal(button("Start guide").disabled, true);
    assert.ok(calls.every(call => call.method === "GET"), "a routine check cannot save a draft or start the guide");

    mode = "failed-read";
    await click("Check saved settings");
    pair("anthropic", "fixture-anthropic-draft");
    assert.match(document.body.textContent, /Current settings or team status could not be read/);
    mode = "ordinary";
    await click("Check saved settings");
    pair("anthropic", "fixture-anthropic-draft");
    assert.equal(document.querySelector('[role="alert"]') === null, true);

    await click("Save locally");
    assert.equal(writes, 1);
    pair("anthropic", "");
    assert.match(document.body.textContent, /Saving was not confirmed/);
    await enter("fixture-unconfirmed-replacement");
    assert.equal(button("Save locally").disabled, true);
    await click("Check saved settings");
    pair("openai", "");
    assert.doesNotMatch(document.body.textContent, /Saving was not confirmed/);
    assert.equal(writes, 1, "recovery readback never sends a replacement key");

    await choose("anthropic");
    await enter("fixture-switch-draft");
    await choose("gemini");
    pair("gemini", "");
    await enter("fixture-gemini-draft");
    mode = "locked";
    await click("Check saved settings");
    pair("anthropic", "");
    assert.equal(document.querySelector("select").disabled, true);
    assert.equal(button("Save locally").disabled, true);
    assert.equal(calls.filter(call => call.method === "PUT").length, 1);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
  });
});

for (const source of ["environment", "native_host_config"]) test(`same-provider ${source} readback retires the hidden key draft without saving or starting`, async () => {
  let mode = "editable";
  const draft = `fixture-retired-${source}-draft`;
  await mounted(call => {
    if (call.url === "/api/provider-settings" && call.method === "GET") {
      const next = settings("anthropic");
      if (mode === "managed") next.providers = next.providers.map(item => item.provider === "anthropic"
        ? { ...item, source, editable: false } : item);
      return next;
    }
    if (call.url === "/api/architect/status" && call.method === "GET") return { status: "stopped", available: true };
    assert.fail(`${call.method} ${call.url}`);
  }, async ({ document, calls, button, click, enter }) => {
    const readOnly = () => {
      assert.ok(calls.every(call => call.method === "GET" && call.body === undefined),
        "checking ownership cannot save the draft or start the guide");
      assert.equal(calls.filter(call => call.method === "PUT" || call.method === "POST").length, 0);
      assert.ok(!JSON.stringify(calls).includes(draft), "readback cannot submit the synthetic key");
      assert.ok(!document.body.textContent.includes(draft), "the synthetic key cannot appear in rendered text");
    };
    assert.equal(document.querySelector("select").value, "anthropic");
    assert.equal(document.querySelector("select").disabled, true);
    await enter(draft);
    assert.equal(document.querySelector('input[type="password"]').value, draft);
    assert.equal(button("Start guide").disabled, true, "an unsaved editable draft blocks Start");

    mode = "managed";
    await click("Check saved settings");
    assert.equal(document.querySelector("select").value, "anthropic");
    assert.equal(document.querySelector("select").disabled, true);
    assert.equal(document.querySelector('input[type="password"]') === null, true, "host-managed credentials have no key field");
    assert.ok(![...document.querySelectorAll("button")].some(item => item.textContent === "Save locally"));
    assert.match(document.body.textContent, source === "environment" ? /Configured by the server environment/ : /configured by this Forge host/);
    assert.equal(button("Start guide").disabled, false, "the retired hidden draft must no longer block Start");
    readOnly();

    mode = "editable";
    await click("Check saved settings");
    assert.equal(document.querySelector("select").value, "anthropic");
    assert.equal(document.querySelector('input[type="password"]').value, "",
      "a discarded key must not return when the same provider becomes editable again");
    assert.equal(button("Save locally").disabled, true);
    assert.equal(button("Start guide").disabled, false);
    readOnly();
  });
});


test("first-run host model needs no replacement key and retains the unsent idea", async () => {
  await mounted(call => {
    assert.equal(call.method, "GET");
    assert.equal(call.url, "/api/provider-settings");
    return { ...settings("openai"), effective_model: "fixture-host-selected-model",
      providers: settings("openai").providers.map(item => item.provider === "openai"
        ? { ...item, source: "native_host_config", editable: false } : item) };
  }, async ({ document, calls, button }) => {
    assert.equal(document.querySelector('input[type="password"]') === null, true);
    assert.match(document.body.textContent, /fixture-host-selected-model/);
    assert.match(document.body.textContent, /Configured by this Forge host/);
    assert.match(document.body.textContent, /Not sent/);
    assert.match(document.body.textContent, /fixture unsent project idea/);
    assert.equal(button("Start guide").disabled, false);
    assert.ok(![...document.querySelectorAll("a")].some(link => link.textContent.includes("Get an API key")));
    assert.ok(![...document.querySelectorAll("button")].some(item => item.textContent === "Save locally"));
    assert.equal(calls.length, 1, "opening setup only reads host configuration");
  }, { firstRun: true, idea: "fixture unsent project idea" });
});
