import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-selection-recovery-"));
after(() => rm(directory, { recursive: true, force: true }));
const entry = join(directory, "entry.mjs"), output = join(directory, "bundle.mjs");
await writeFile(entry, `export * from ${JSON.stringify(fileURLToPath(new URL("build-selection-recovery.jsx", import.meta.url)))};\nexport { ImplementationPanel } from ${JSON.stringify(fileURLToPath(new URL("implementation-panel.jsx", import.meta.url)))};`);
const built = await build({ entryPoints: [entry], bundle: true, write: false, format: "esm", platform: "node",
  outfile: output, sourcemap: "linked", logLevel: "silent", loader: { ".css": "empty" },
  nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
  plugins: [{ name: "shared-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
for (const file of built.outputFiles) await writeFile(file.path, file.contents);
const { BuildSelectionRecovery, ImplementationPanel } = await import(pathToFileURL(output).href);
const guide = { id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", version: 8,
  proposal: { target: "mac", project: { name: "Synthetic saved application" } } };
const row = (number, overrides = {}) => ({ build_id: `${String(number).padStart(8, "0")}-bbbb-4bbb-8bbb-bbbbbbbbbbbb`,
  guide_version: 5, selection_token: String(number).repeat(64), status: "queued",
  cleanup_pending: true, cleanup_custody_lost: true, created_at: "2026-10-11T04:00:00Z", ...overrides });
const page = (candidates = [row(1)], overrides = {}) => ({ guide_id: guide.id, version: guide.version,
  candidates, unavailable_count: 0, ...overrides });
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const ok = value => ({ ok: true, json: async () => value });
const conflict = () => ({ ok: false, status: 409, json: async () => ({
  detail: "Saved work has no selected build.", code: "build_selection_unavailable",
}) });

// The mounted production UI uses inert transport. No live Forge, runtime or
// application process is contacted and no actual build selection is changed.
async function mounted(verify, options = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const descriptors = new Map(), calls = [], timers = new Map();
  const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
  let epoch = 1, selected = guide.id, refreshed = 0, unmounted = false;
  let transport = options.transport || (async (url, value) => value.method === "POST"
    ? ok({ guide_id: guide.id, version: guide.version, build_id: JSON.parse(value.body).build_id, selection_restored: true })
    : ok(page()));
  const scope = { token: () => epoch, capture(state, expected) {
    const captured = epoch;
    return () => selected === state.id && epoch === captured && (expected === undefined || expected === captured);
  } };
  const replace = (name, value) => {
    descriptors.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  };
  for (const [name, value] of Object.entries({ window: dom.window, document: dom.window.document,
    navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage,
    IS_REACT_ACT_ENVIRONMENT: true, requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    fetch: async (url, value = {}) => { calls.push({ url, value }); return transport(url, value); },
    setTimeout(callback, delay, ...args) {
      if (delay < 1200) return realSetTimeout(callback, delay, ...args);
      const token = { delay }; timers.set(token, callback); return token;
    }, clearTimeout(token) { if (!timers.delete(token)) realClearTimeout(token); },
  })) replace(name, value);
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  let props = { state: guide, selectionScope: scope, onRefresh: () => { refreshed += 1; }, blocked: false,
    plan: { project: { name: "Synthetic saved application" } }, Icon: () => null, ...options.props };
  const render = async next => { props = { ...props, ...next }; await act(async () => root.render(
    React.createElement(options.panel ? ImplementationPanel : BuildSelectionRecovery, props))); };
  const settle = async () => act(async () => { await new Promise(done => realSetTimeout(done, 10)); });
  const buttons = () => [...dom.window.document.querySelectorAll("button")];
  const restore = index => buttons().filter(button => button.textContent === "Restore saved build")[index || 0];
  try {
    await render({}); await settle();
    await verify({ document: dom.window.document, calls, render, settle, restore, buttons,
      async click(button) { await act(async () => button?.click()); },
      setTransport(value) { transport = value; }, refreshes: () => refreshed,
      depart() { selected = "another-guide"; epoch += 1; }, returnToGuide() { selected = guide.id; epoch += 1; },
      async expire(delay) { await act(async () => { for (const [token, callback] of timers) if (token.delay === delay) { timers.delete(token); callback(); } }); },
      async unmount() { await act(async () => root.unmount()); unmounted = true; },
    });
  } finally {
    if (!unmounted) await act(async () => root.unmount());
    for (const [name, value] of descriptors) {
      if (value) Object.defineProperty(globalThis, name, value); else delete globalThis[name];
    }
    dom.window.close();
  }
}

test("multiple retained builds require an explicit exact choice without rebuilding", async () => mounted(async ui => {
  assert.equal(ui.calls.length, 1);
  assert.match(ui.document.body.textContent, /Worker shutdown is unconfirmed/);
  assert.equal(ui.buttons().filter(button => button.textContent === "Restore saved build").length, 2);
  await ui.click(ui.restore(1)); await ui.settle();
  assert.equal(ui.calls.length, 2);
  assert.equal(ui.calls[1].url, `/api/guide/${guide.id}/implementation/saved-selection`);
  assert.deepEqual(JSON.parse(ui.calls[1].value.body), { version: 8, build_id: row(2).build_id, selection_token: row(2).selection_token });
  assert.equal(ui.refreshes(), 1);
  assert.match(ui.document.body.textContent, /Selection restored/);
}, { transport: async (url, value) => value.method === "POST"
  ? ok({ guide_id: guide.id, version: 8, build_id: row(2).build_id, selection_restored: true }) : ok(page([row(1), row(2)])) }));

test("partial saved work is preserved without an offered restore or build", async () => mounted(async ui => {
  assert.equal(ui.restore() === undefined, true);
  assert.match(ui.document.body.textContent, /No complete saved build/);
  assert.match(ui.document.body.textContent, /lacks the information.*preserved/);
  assert.equal(ui.calls.some(call => call.value.method === "POST"), false);
}, { transport: async () => ok(page([], { unavailable_count: 2 })) }));

test("a stale or crossed candidate projection cannot be selected", async () => mounted(async ui => {
  assert.equal(ui.restore() === undefined, true);
  assert.match(ui.document.querySelector('[role="alert"]').textContent, /saved build list changed/);
  assert.equal(ui.calls.length, 1);
}, { transport: async () => ok(page([row(1)], { version: 7 })) }));

test("a held restore admits one write and an unknown outcome needs a fresh read", async () => mounted(async ui => {
  const held = deferred();
  ui.setTransport(async () => held.promise);
  const button = ui.restore();
  await act(async () => { button.click(); button.click(); });
  assert.equal(ui.calls.filter(call => call.value.method === "POST").length, 1);
  held.resolve({ ok: false, status: 502, json: async () => ({ detail: "Outcome unconfirmed." }) });
  await ui.settle();
  assert.equal(ui.refreshes(), 1);
  assert.equal(ui.restore() === undefined, true);
  assert.match(ui.document.body.textContent, /Outcome unconfirmed/);
  ui.setTransport(async () => ok(page([row(1, { selection_token: "f".repeat(64) })])));
  await ui.click(ui.buttons().find(button => button.textContent === "Check saved work again"));
  await ui.settle();
  assert.equal(ui.restore().disabled, false);
  assert.equal(ui.calls.filter(call => call.value.method === "POST").length, 1);
}));

for (const phase of ["headers", "body"]) {
  test(`a stalled restore ${phase} response releases the wait and checks without replaying the selection`, async () => mounted(async ui => {
    const held = deferred();
    const response = { guide_id: guide.id, version: guide.version, build_id: row(1).build_id, selection_restored: true };
    ui.setTransport(async (_url, value) => value.method === "POST"
      ? phase === "headers" ? held.promise : { ok: true, json: () => held.promise }
      : ok(page([row(2, { project_name: "Fresh saved work" })])));
    try {
      await ui.click(ui.restore());
      assert.match(ui.document.body.textContent, /Restoring selection/);
      await ui.expire(30000); await ui.settle();
      const alert = ui.document.querySelector('[role="alert"]');
      assert.ok(alert, "A stalled restore response must release the waiting view into an uncertain outcome.");
      assert.match(alert.textContent, /may still finish/i);
      assert.doesNotMatch(ui.document.body.textContent, /Selection restored|Restoring selection/);
      assert.equal(ui.calls.find(call => call.value.method === "POST").value.signal.aborted, true);
      assert.equal(ui.refreshes(), 1, "The parent checks the ordinary selected-build status.");
      assert.equal(ui.calls.filter(call => call.value.method === "POST").length, 1);
      assert.equal(ui.restore() === undefined, true, "An uncertain response cannot offer another write before a read.");
      await ui.click(ui.buttons().find(button => button.textContent === "Check saved work again"));
      await ui.settle();
      assert.match(ui.document.body.textContent, /Fresh saved work/);
      assert.equal(ui.restore().disabled, false);
      held.resolve(phase === "headers" ? ok(response) : response); await ui.settle();
      assert.match(ui.document.body.textContent, /Fresh saved work/);
      assert.doesNotMatch(ui.document.body.textContent, /Selection restored/);
      assert.equal(ui.refreshes(), 1, "A late successful response cannot reclaim presentation or refresh again.");
      assert.equal(ui.calls.filter(call => call.value.method === "POST").length, 1);
      assert.equal(ui.calls.filter(call => call.value.method !== "POST").length, 2,
        "Only the original and explicitly requested saved-work reads are issued.");
    } finally {
      held.resolve(phase === "headers" ? ok(response) : response);
      await ui.settle();
    }
  }));
}

test("guide departure and return retire a held restore before DOM rerender", async () => mounted(async ui => {
  const held = deferred(); ui.setTransport(async () => held.promise);
  await ui.click(ui.restore()); ui.depart(); ui.returnToGuide();
  held.resolve(ok({ guide_id: guide.id, version: 8, build_id: row(1).build_id, selection_restored: true }));
  await ui.settle();
  assert.equal(ui.refreshes(), 0);
  assert.doesNotMatch(ui.document.body.textContent, /Selection restored/);
  await ui.click(ui.buttons().find(button => /Restoring selection/.test(button.textContent)));
  assert.equal(ui.calls.filter(call => call.value.method === "POST").length, 1);
}));

test("blocked or unmounted recovery cannot admit or present a late selection", async () => mounted(async ui => {
  await ui.render({ blocked: true });
  assert.equal(ui.restore() === undefined, true);
  await ui.render({ blocked: false });
  const held = deferred(); ui.setTransport(async () => held.promise);
  await ui.click(ui.restore()); await ui.unmount();
  held.resolve(ok({ guide_id: guide.id, version: 8, build_id: row(1).build_id, selection_restored: true }));
  await ui.settle(); assert.equal(ui.refreshes(), 0);
}));

test("blocked saved-work recovery describes a pause and begins one read when available", async () => mounted(async ui => {
  assert.equal(ui.calls.length, 0);
  assert.match(ui.document.querySelector('[role="status"]').textContent, /Checking saved work is paused/);
  assert.doesNotMatch(ui.document.body.textContent, /Checking saved work\.\.\.|No complete saved build|Selection restored/);
  assert.equal(ui.restore() === undefined, true);
  await ui.render({ blocked: false }); await ui.settle();
  assert.equal(ui.calls.length, 1);
  assert.equal(ui.restore().disabled, false);
  assert.doesNotMatch(ui.document.body.textContent, /is paused/);
  assert.equal(ui.calls.some(call => call.value.method === "POST"), false);
}, { props: { blocked: true } }));

test("pausing a held saved-work read hides its late result without treating it as missing work", async () => {
  const held = deferred();
  await mounted(async ui => {
    assert.equal(ui.calls.length, 1);
    await ui.render({ blocked: true });
    assert.equal(ui.calls[0].value.signal.aborted, true);
    assert.match(ui.document.body.textContent, /Checking saved work is paused/);
    held.resolve(ok(page([row(2, { project_name: "Retired result" })]))); await ui.settle();
    assert.equal(ui.restore() === undefined, true);
    assert.doesNotMatch(ui.document.body.textContent, /Retired result|No complete saved build|Checking saved work\.\.\./);
    ui.setTransport(async () => ok(page([row(1, { project_name: "Current result" })])));
    await ui.render({ blocked: false }); await ui.settle();
    assert.equal(ui.calls.length, 2);
    assert.match(ui.document.body.textContent, /Current result/);
    assert.doesNotMatch(ui.document.body.textContent, /Retired result|is paused/);
    assert.equal(ui.calls.some(call => call.value.method === "POST"), false);
  }, { transport: async () => held.promise });
});

for (const flag of ["designBusy", "pendingManual"]) {
  test(`the implementation panel pauses saved-work checks during ${flag}`, async () => mounted(async ui => {
    assert.match(ui.document.body.textContent, /Your saved build needs attention/);
    assert.match(ui.document.body.textContent, /Checking saved work is paused/);
    assert.equal(ui.calls.filter(call => call.url.endsWith("/saved-selection")).length, 0);
    await ui.render({ [flag]: false }); await ui.settle();
    assert.equal(ui.calls.filter(call => call.url.endsWith("/saved-selection")).length, 1);
    assert.equal(ui.restore().disabled, false);
    assert.equal(ui.calls.some(call => call.value.method === "POST"), false);
  }, { panel: true, props: { [flag]: true }, transport: async url => {
    if (url === "/api/checker-setup") return ok({ status: "ready", can_prepare: false });
    if (url.endsWith("/saved-selection")) return ok(page());
    if (url.endsWith("/implementation")) return conflict();
    throw new Error(`Unexpected synthetic request: ${url}`);
  } }));
}

test("saved-work read deadline covers a held response body without claiming absence", async () => mounted(async ui => {
  await ui.expire(30000); await ui.settle();
  assert.match(ui.document.querySelector('[role="alert"]').textContent, /taking too long/);
  assert.doesNotMatch(ui.document.body.textContent, /No complete saved build/);
  assert.equal(ui.calls.filter(call => call.value.method === "POST").length, 0);
}, { transport: async () => ({ ok: true, json: async () => deferred().promise }) }));

test("the actual implementation panel keeps recovery mounted through its status recheck and retains the note", async () => {
  let heldStatus = null;
  await mounted(async ui => {
    const note = ui.document.querySelector("textarea");
    Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set.call(note, "Keep this builder note");
    await act(async () => note.dispatchEvent(new window.Event("input", { bubbles: true })));
    const recovery = ui.document.querySelector(".implementation-selection-recovery");
    assert.ok(recovery);
    assert.match(ui.document.body.textContent, /Your saved build needs attention/);
    heldStatus = deferred(); await ui.expire(3000); await ui.settle();
    assert.equal(ui.document.querySelector(".implementation-selection-recovery") === recovery, true);
    assert.equal(ui.calls.filter(call => call.url.endsWith("/saved-selection")).length, 1);
    assert.equal(note.value, "Keep this builder note");
    assert.equal(ui.calls.some(call => call.value.method === "POST"), false);
    heldStatus.resolve(conflict()); await ui.settle();
  }, { panel: true, transport: async (url) => {
    if (url === "/api/checker-setup") return ok({ status: "ready", can_prepare: false });
    if (url.endsWith("/saved-selection")) return ok(page());
    if (url.endsWith("/implementation")) return heldStatus ? heldStatus.promise : conflict();
    throw new Error(`Unexpected synthetic request: ${url}`);
  } });
});
