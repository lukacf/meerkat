import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build as bundle } from "esbuild";
import React, { act } from "react";
import { createInstalledUseDraftStore, installedUseUrl } from "./deployment-use.mjs";
import { nativeTeamTaskScopeKey, createNativeTeamTaskDraftStore, getNativeTeamTaskDraftStore } from "./native-team-task-draft.mjs";

const guide = { id: "saved-guide", version: 7 };
const receipt = { guide_id: guide.id, guide_version: 4, deployment_id: "installed-team-1",
  source_kind: "native-team", build_id: null, source_id: "a".repeat(64), source_digest: "b".repeat(64),
  package_sha256: "c".repeat(64), status: "running", health: "ready", process_live: true,
  console_url: "http://127.0.0.1:18880/console", bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
const scope = nativeTeamTaskScopeKey(receipt, guide);
function memoryStorage() {
  const values = new Map();
  return { values, getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
}
function metadata(current = receipt, currentGuide = guide) {
  return { scope: "verified-installed-source", application_outcomes_verified: false,
    source_kind: "native-team", build_id: null, deployment_id: current.deployment_id,
    source_id: current.source_id, source_digest: current.source_digest, package_sha256: current.package_sha256,
    guide_version: current.guide_version, version: currentGuide.version, status: current.status,
    project: { name: "Installed team" }, entrypoint: { identity: "lead:main", name: "Team lead" },
    native_runtime_available: current.status === "running", console_url: current.console_url };
}

test("draft ownership follows exact installed team source, not health or current proposal", () => {
  assert.ok(scope);
  assert.equal(nativeTeamTaskScopeKey({ ...receipt, status: "stopped", health: "unknown", console_url: null,
    process_live: false }, { ...guide, version: 8 }), scope);
  for (const change of [{ deployment_id: "other" }, { source_id: "d".repeat(64) },
    { source_digest: "d".repeat(64) }, { package_sha256: "d".repeat(64) }, { guide_version: 5 }])
    assert.notEqual(nativeTeamTaskScopeKey({ ...receipt, ...change }, guide), scope);
});

test("foreign, incomplete and custom application receipts cannot select team drafts", () => {
  for (const change of [{ guide_id: "foreign" }, { source_kind: "application" }, { build_id: "custom-build" },
    { source_id: "invalid" }, { source_digest: null }, { package_sha256: "invalid" },
    { deployment_id: "" }, { guide_version: true }, { guide_version: 0 }])
    assert.equal(nativeTeamTaskScopeKey({ ...receipt, ...change }, guide), "");
});

test("same source restores exact writing and different sources keep separate drafts", () => {
  const storage = memoryStorage(), drafts = createNativeTeamTaskDraftStore(storage);
  const other = nativeTeamTaskScopeKey({ ...receipt, source_digest: "d".repeat(64) }, guide);
  const writing = "Keep my line breaks.\n\n  Review these three requests.  ";
  assert.equal(drafts.update(scope, writing), true);
  assert.equal(drafts.update(other, "Different installed source"), true);
  assert.equal(createNativeTeamTaskDraftStore(storage).read(scope), writing);
  assert.equal(createNativeTeamTaskDraftStore(storage).read(other), "Different installed source");
  assert.equal(drafts.update(scope, ""), true);
  assert.equal(createNativeTeamTaskDraftStore(storage).read(scope), "");
  assert.equal(createNativeTeamTaskDraftStore(storage).read(other), "Different installed source");
});

test("native namespace cannot overwrite custom application drafts even with a crafted equal scope", () => {
  const storage = memoryStorage(), custom = createInstalledUseDraftStore(storage);
  assert.equal(custom.update(scope, "task", "Existing application writing"), true);
  const native = createNativeTeamTaskDraftStore(storage);
  assert.equal(native.read(scope), "");
  assert.equal(native.update(scope, "Team writing"), true);
  assert.equal(custom.read(scope).task, "Existing application writing");
  assert.equal(createInstalledUseDraftStore(storage).read(scope).task, "Existing application writing");
});

test("storage failure retains page writing through panel remount and rejects invalid edits", () => {
  const page = { sessionStorage: { getItem() { throw Error("Unavailable"); }, setItem() {}, removeItem() {} } };
  const drafts = getNativeTeamTaskDraftStore(page);
  assert.equal(drafts.update(scope, "Page-only task"), true);
  assert.equal(drafts.persistent(scope), false);
  assert.equal(getNativeTeamTaskDraftStore(page).read(scope), "Page-only task");
  assert.equal(drafts.update(scope, "a".repeat(3001)), false);
  assert.equal(drafts.read(scope), "Page-only task");
  assert.equal(drafts.update("foreign-scope", "Never retained"), false);
  assert.equal(drafts.read("foreign-scope"), "");
  assert.equal(drafts.read(scope), "Page-only task");
});

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const compiled = await bundle({ entryPoints: [fileURLToPath(new URL("deployment-use.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
  plugins: [{ name: "shared-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { InstalledNativeTeamUse } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`);
const settle = async () => { for (let index = 0; index < 10; index++) await Promise.resolve(); };
function deferred() { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; }
const button = (document, label) => [...document.querySelectorAll("button")].find(node => node.textContent.trim() === label);

async function mounted(verify, { current = receipt, transport, clipboard, storage, selectionScope } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  if (storage) Object.defineProperty(dom.window, "sessionStorage", { value: storage });
  const calls = [], copies = [], submissions = [];
  Object.defineProperty(dom.window.navigator, "clipboard", { configurable: true,
    value: clipboard || { writeText: async text => { copies.push(text); } } });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (url, options = {}) => {
      calls.push({ url: String(url), method: options.method || "GET", body: options.body });
      assert.equal(String(url), installedUseUrl(guide.id)); assert.equal(options.method || "GET", "GET");
      assert.equal(options.body, undefined);
      if (transport) return transport(url, options);
      return { ok: true, json: async () => metadata(current) };
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  let props = { state: guide, receipt: current, Icon: () => null, selectionScope };
  const draw = () => React.createElement("form", { onSubmit: event => { event.preventDefault(); submissions.push("submit"); } },
    React.createElement(InstalledNativeTeamUse, props));
  const rerender = async next => { props = { ...props, ...next }; await act(async () => { root.render(draw()); await settle(); }); };
  const edit = async value => {
    const field = dom.window.document.querySelector("textarea");
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(field, value); field.dispatchEvent(new dom.window.Event("input", { bubbles: true })); await settle(); });
  };
  const click = async label => { const target = button(dom.window.document, label); assert.ok(target, label);
    await act(async () => { target.click(); await settle(); }); };
  try {
    await rerender({});
    await verify({ document: dom.window.document, window: dom.window, edit, click, rerender, calls, copies });
    assert.deepEqual(submissions, []);
    assert.ok(calls.every(call => call.method === "GET" && call.body === undefined));
    assert.ok([...dom.window.document.querySelectorAll("button")].every(node => node.type === "button"));
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of previous) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; }
  }
}

test("actual stopped team keeps an editable draft and copying never starts or submits it", async () => {
  await mounted(async ({ document, edit, click, copies }) => {
    assert.equal(document.querySelector("textarea").disabled, false);
    await edit("Inspect the actual saved result.\nThen ask me what is missing.");
    await click("Copy task");
    assert.deepEqual(copies, ["Inspect the actual saved result.\nThen ask me what is missing."]);
    assert.match(document.body.textContent, /Task copied.*before sending/);
    assert.equal(document.querySelector("a[href]") === null, true);
    assert.match(document.body.textContent, /Start this installed team using its installation controls/);
  }, { current: { ...receipt, status: "stopped", console_url: null, health: "unknown", process_live: false } });
});

test("failed metadata leaves draft editable and source changes restore each source's own writing", async () => {
  await mounted(async ({ document, edit, click, rerender, copies }) => {
    await edit("Original installed source task");
    assert.equal(document.querySelector("a[href]") === null, true);
    assert.match(document.body.textContent, /conversation owner could not be checked/);
    await click("Copy task");
    await rerender({ receipt: { ...receipt, source_digest: "d".repeat(64) } });
    assert.equal(document.querySelector("textarea").value, "");
    await edit("Different source task");
    await rerender({ receipt: { ...receipt, status: "stopped", console_url: null, health: "unknown" }, state: { ...guide, version: 8 } });
    assert.equal(document.querySelector("textarea").value, "Original installed source task");
    assert.deepEqual(copies, ["Original installed source task"]);
  }, { transport: async () => ({ ok: false, status: 503, json: async () => ({ detail: "Unavailable" }) }) });
});

test("failed clipboard exposes manual selection of the exact retained task", async () => {
  await mounted(async ({ document, edit, click }) => {
    const task = "  Keep exactly this text.\nSecond line.  ";
    await edit(task); await click("Copy task");
    assert.match(document.body.textContent, /could not be copied/);
    await click("Select task to copy");
    const field = document.querySelector("textarea");
    assert.equal(document.activeElement === field, true);
    assert.equal(field.value.slice(field.selectionStart, field.selectionEnd), task);
    assert.ok(document.querySelector('a[href="http://127.0.0.1:18880/console"]'));
  }, { clipboard: { writeText: async () => { throw Error("Clipboard permission refused"); } } });
});

test("pending clipboard offers manual copy and its late result cannot label a newer draft copied", async () => {
  const pending = deferred(), copied = [];
  await mounted(async ({ document, edit, click }) => {
    await edit("Earlier task"); await click("Copy task");
    assert.ok(button(document, "Select task to copy"));
    await edit("Newer human writing");
    assert.equal(button(document, "Copy task").disabled, false);
    await act(async () => { pending.resolve(); await settle(); });
    assert.equal(document.querySelector("textarea").value, "Newer human writing");
    assert.doesNotMatch(document.body.textContent, /Task copied/);
    assert.deepEqual(copied, ["Earlier task"]);
  }, { clipboard: { writeText: text => { copied.push(text); return pending.promise; } } });
});

test("page-only storage and oversized edit preserve the prior draft without a job send", async () => {
  await mounted(async ({ document, edit, click, copies }) => {
    await edit("Keep this task in the open page");
    await edit("x".repeat(3001));
    assert.equal(document.querySelector("textarea").value, "Keep this task in the open page");
    assert.match(document.body.textContent, /3,000-character limit/);
    assert.match(document.body.textContent, /Browser draft storage is unavailable/);
    await click("Copy task");
    assert.deepEqual(copies, ["Keep this task in the open page"]);
  }, { storage: { getItem() { throw Error("Storage denied"); }, setItem() {}, removeItem() {} } });
});

test("invalidated guide selection refuses edits, copy and Console navigation", async () => {
  let available = true;
  const selectionScope = { token: () => 1, capture: () => () => available };
  await mounted(async ({ document, edit, click, rerender, copies }) => {
    await edit("Original task");
    available = false; await rerender({});
    assert.equal(document.querySelector("textarea").disabled, true);
    assert.equal(button(document, "Copy task").disabled, true);
    assert.equal(document.querySelector("a[href]") === null, true);
    await click("Copy task"); assert.deepEqual(copies, []);
    available = true; await rerender({});
    assert.equal(document.querySelector("textarea").value, "Original task");
  }, { selectionScope });
});
