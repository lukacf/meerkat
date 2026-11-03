import { after, test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const web = fileURLToPath(new URL(".", import.meta.url));
const require = createRequire(join(web, "package.json"));
const React = require("react"), { act } = React;
const { build } = require("esbuild");
const { JSDOM } = createRequire(resolve(web, "../../console/package.json"))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-saved-build-status-"));
after(() => rm(directory, { recursive: true, force: true }));
const modulePath = join(directory, "status.mjs");
const built = await build({
  stdin: { contents: 'export { SavedSetupBuildStatus } from "./saved-setup-build-status.jsx"; export { savedSetupBuildStatus } from "./saved-setup-build-status.mjs";',
    resolveDir: web, loader: "js" },
  bundle: true, write: false, platform: "node", format: "esm", outfile: modulePath,
  sourcemap: "external", sourcesContent: true, loader: { ".css": "empty" }, logLevel: "silent",
  alias: { "@console-core": resolve(web, "../../packages/console-core/src") },
  plugins: [{ name: "same-react", setup(bundler) {
    bundler.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
for (const output of built.outputFiles) await writeFile(output.path, output.contents);
const { SavedSetupBuildStatus, savedSetupBuildStatus } = await import(pathToFileURL(modulePath).href);

const setupA = { id: "8483e14d-c729-481f-b87b-16fa02f4fe0a", name: "Observing club", stage: "review", status: "ready", updated_at: "2026-10-03T19:10:00Z" };
const setupB = { ...setupA, id: "808a45df-8767-4749-b830-66cb4143b0b6", name: "Workshop handover" };
const buildA = "f3612e9d-0f10-4373-9c80-4d232ff4e7a9";
const buildB = "149e9e64-ecdb-4565-83b9-ed3a59200ce6";
const sensitive = "fixture-private-path-and-token-canary";
const receipt = (setup = setupA, change = {}) => ({
  guide_id: setup.id, guide_version: 4, current_guide_version: 6, current: true,
  build_id: setup === setupB ? buildB : buildA, status: "running", phase: "implementation",
  source_verified: false, ready_to_package: false, cleanup_pending: false, cleanup_in_progress: false,
  cleanup_custody_lost: false, stop_requested: false,
  request: sensitive, summary: sensitive, detail: sensitive,
  timing: { version: 1, build_id: setup === setupB ? buildB : buildA, scope: "host_build", overlap: true,
    elapsed_ms: 1762800, finished: false, phases: [] },
  ...change,
});
const checked = (setup = setupA) => receipt(setup, {
  status: "completed", phase: "complete", source_verified: true, ready_to_package: true,
  timing: { ...receipt(setup).timing, finished: true },
  source_digest: "a".repeat(64), acceptance_trusted: true, acceptance_current: true,
  acceptance: { issuer: "mobkit-launch-acceptance-host", status: "pass", tests_run: 17, source_digest: "a".repeat(64) },
});

test("canonical saved build phases distinguish writing, checking, independent review and repairs from guide status", () => {
  for (const [change, label] of [
    [{ status: "queued" }, "Waiting to build"], [{}, "Building your application"],
    [{ status: "checking" }, "Checking the application"],
    [{ phase: "independent_review" }, "Independent review in progress"],
    [{ phase: "automatic_repair" }, "Repairing review findings"],
  ]) {
    const view = savedSetupBuildStatus(setupA, receipt(setupA, change));
    assert.equal(view.kind, "running"); assert.equal(view.label, label);
    assert.equal(view.action, "Return to build"); assert.equal(view.poll, true);
    assert.match(view.detail, /Server shutdown interrupts the build and preserves its saved work/);
    assert.doesNotMatch(JSON.stringify(view), new RegExp(sensitive));
  }
});

test("a versionless saved setup uses canonical current input binding and requires independent checked completion", () => {
  assert.equal(Object.hasOwn(setupA, "version"), false, "actual guide/list rows omit version");
  const ready = savedSetupBuildStatus(setupA, checked());
  assert.equal(ready.kind, "checked"); assert.equal(ready.action, "Review completed build");
  assert.equal(ready.currentGuideVersion, 6, "canonical content currency can advance beyond the original build version");
  assert.equal(ready.guideVersion, 4); assert.equal(ready.poll, false);
  assert.match(ready.detail, /do not establish installation or live use/);
  for (const change of [
    { source_verified: false }, { ready_to_package: false }, { acceptance_trusted: false },
    { acceptance_current: false }, { acceptance: { ...checked().acceptance, tests_run: 0 } },
    { acceptance: { ...checked().acceptance, source_digest: "b".repeat(64) } },
    { cleanup_pending: true }, { cleanup_in_progress: true }, { cleanup_custody_lost: true },
  ]) {
    const view = savedSetupBuildStatus(setupA, { ...checked(), ...change });
    assert.notEqual(view.kind, "checked"); assert.notEqual(view.action, "Review completed build");
  }
});

test("earlier designs and interrupted or failed work remain visible without becoming ready or resuming", () => {
  const stale = savedSetupBuildStatus(setupA, receipt(setupA, { current: false, current_guide_version: null }));
  assert.equal(stale.kind, "stale"); assert.equal(stale.action, "Return to build");
  assert.equal(stale.currentGuideVersion, null);
  assert.equal(savedSetupBuildStatus({ ...setupA, version: 5 }, checked()).kind, "stale");
  const interrupted = savedSetupBuildStatus(setupA, receipt(setupA, { status: "interrupted" }));
  assert.equal(interrupted.kind, "interrupted"); assert.equal(interrupted.poll, false);
  assert.match(interrupted.detail, /does not resume automatically/);
  assert.equal(savedSetupBuildStatus(setupA, receipt(setupA, { status: "failed" })).kind, "attention");
});

test("unknown or foreign canonical carriers cannot fabricate a build navigation intent", () => {
  for (const saved of [null, receipt(setupB), receipt(setupA, { build_id: "" }),
    receipt(setupA, { current: undefined }), receipt(setupA, { current_guide_version: null }),
    receipt(setupA, { guide_version: true }), receipt(setupA, { status: "installed" })]) {
    const view = savedSetupBuildStatus(setupA, saved);
    assert.equal(view.kind, "unknown"); assert.equal(view.action, null); assert.equal(view.poll, false);
  }
});

async function mounted(read, verify, initial = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://offline.test/" });
  const realTimeout = globalThis.setTimeout, realClear = globalThis.clearTimeout;
  const polls = new Map(), deadlines = new Map(), calls = [], intents = [];
  const values = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    setTimeout: (callback, delay, ...args) => {
      const timers = delay === 6000 ? polls : delay === 30000 ? deadlines : null;
      if (!timers) return realTimeout(callback, delay, ...args);
      const token = {}; timers.set(token, () => callback(...args)); return token;
    },
    clearTimeout: token => { if (!polls.delete(token) && !deadlines.delete(token)) realClear(token); },
    fetch: async (url, options = {}) => {
      const call = { url, method: options.method || "GET", body: options.body, signal: options.signal };
      calls.push(call); assert.equal(call.method, "GET", "saved-build status cannot admit work");
      assert.equal(call.body, undefined);
      const result = await read(call);
      if (result?.response) return result.response;
      return { ok: !result?.httpStatus, status: result?.httpStatus || 200,
        json: async () => result?.httpStatus ? { detail: sensitive } : result };
    } };
  const previous = new Map(Object.keys(values).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(values)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = require("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  const settle = () => new Promise(done => realTimeout(done, 5));
  let props = { setup: setupA, onOpenBuild: intent => intents.push(intent), ...initial };
  const render = async next => {
    props = { ...props, ...next };
    await act(async () => { root.render(React.createElement(SavedSetupBuildStatus, props)); await settle(); });
  };
  const button = label => [...dom.window.document.querySelectorAll("button")].find(item => item.textContent === label);
  const click = async label => { assert.ok(button(label), label); await act(async () => { button(label).click(); await settle(); }); };
  const advance = async timers => {
    assert.equal(timers.size, 1); const [key, callback] = timers.entries().next().value;
    timers.delete(key); await act(async () => { callback(); await settle(); });
  };
  try {
    await render({});
    await verify({ document: dom.window.document, calls, intents, render, button, click,
      poll: () => advance(polls), expire: () => advance(deadlines), polls, settle });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test("a mounted saved build progresses through review and checked completion, polls only while active and navigates by explicit click", async () => {
  let reads = 0;
  await mounted(() => ++reads === 1 ? receipt() : reads === 2 ? receipt(setupA, { phase: "independent_review" }) : checked(),
    async ({ document, calls, intents, poll, polls, click }) => {
      assert.match(document.body.textContent, /Building your application/); assert.deepEqual(intents, []);
      assert.match(document.body.textContent, /Recorded so far: 29m 22s/);
      await poll(); assert.match(document.body.textContent, /Independent review in progress/);
      await poll(); assert.match(document.body.textContent, /Checked build ready to review/);
      assert.equal(polls.size, 0, "terminal build status is read once and then stops polling");
      assert.equal(calls.length, 3); assert.deepEqual(intents, []);
      assert.doesNotMatch(document.body.textContent, new RegExp(sensitive));
      await click("Review completed build");
      assert.deepEqual(intents, [{ guideId: setupA.id, buildId: buildA, guideVersion: 4, currentGuideVersion: 6 }]);
      assert.equal(calls.length, 3, "navigation does not submit, resume or install an application");
    });
});

test("unavailable readback offers an explicit GET recheck and interrupted work returns without automatic resumption", async () => {
  let reads = 0;
  await mounted(() => ++reads === 1 ? { httpStatus: 503 } : receipt(setupA, { status: "interrupted" }),
    async ({ document, calls, intents, click, polls }) => {
      assert.match(document.body.textContent, /Build status unavailable/);
      assert.doesNotMatch(document.body.textContent, new RegExp(sensitive));
      assert.equal(polls.size, 0); assert.deepEqual(intents, []);
      await click("Check build status"); assert.match(document.body.textContent, /Build interrupted/);
      assert.equal(calls.length, 2); assert.deepEqual(intents, []);
      await click("Return to build"); assert.equal(intents.length, 1); assert.equal(calls.length, 2);
    });
});

test("a late retired guide response cannot publish into the next saved setup", async () => {
  let finishA, reads = 0;
  const pending = new Promise(resolve => { finishA = resolve; });
  await mounted(call => ++reads === 1 ? { response: { ok: true, json: () => pending } } : checked(setupB),
    async ({ document, calls, intents, render, settle }) => {
      assert.match(document.body.textContent, /Checking application build/);
      const original = calls[0];
      await render({ setup: setupB });
      assert.equal(original.signal.aborted, true);
      assert.match(document.body.textContent, /Checked build ready to review/);
      await act(async () => { finishA(receipt()); await settle(); });
      assert.doesNotMatch(document.body.textContent, /Building your application/);
      assert.equal(document.querySelector('[data-build-status="checked"]') !== null, true);
      assert.deepEqual(intents, []); assert.equal(calls.length, 2);
    });
});

test("captured navigation handlers lose authority on disable, setup replacement and withdrawn row eligibility", async () => {
  await mounted(call => call.url.includes(setupB.id) ? checked(setupB) : checked(),
    async ({ document, intents, render, button, settle, calls }) => {
      const original = button("Review completed build");
      const props = Object.keys(original).find(key => key.startsWith("__reactProps$"));
      const captured = original[props].onClick;
      const invoke = async () => act(async () => { captured({ stopPropagation() {} }); await settle(); });
      await render({ disabled: true }); await invoke(); assert.deepEqual(intents, []);
      await render({ disabled: false, setup: setupB }); await invoke(); assert.deepEqual(intents, []);
      await render({ isCurrent: () => false });
      assert.equal(document.querySelector(".saved-setup-build-status") === null, true);
      assert.equal(calls.length, 2, "withdrawn eligibility never admits another read");
      assert.deepEqual(intents, []);
    });
});

test("only eligible visible rows read build status and an absent build adds no misleading label or action", async () => {
  await mounted(() => ({ httpStatus: 404 }), async ({ document, calls, render, polls, intents }) => {
    assert.equal(calls.length, 0); assert.equal(document.querySelector(".saved-setup-build-status") === null, true);
    await render({ active: true }); assert.equal(calls.length, 1);
    assert.equal(document.querySelector(".saved-setup-build-status") === null, true);
    assert.equal(polls.size, 0); assert.deepEqual(intents, []);
    await render({ active: false }); assert.equal(calls.length, 1);
  }, { active: false });
});

test("a stalled canonical body reaches bounded recovery and its late reply cannot restore a stale status", async () => {
  let finish, reads = 0;
  const pending = new Promise(resolve => { finish = resolve; });
  await mounted(() => ++reads === 1 ? { response: { ok: true, json: () => pending } } : checked(),
    async ({ document, calls, intents, expire, click, settle }) => {
      assert.match(document.body.textContent, /Checking application build/);
      await expire(); assert.match(document.body.textContent, /Build status unavailable/);
      assert.equal(calls[0].signal.aborted, true);
      await click("Check build status"); assert.match(document.body.textContent, /Checked build ready to review/);
      await act(async () => { finish(receipt()); await settle(); });
      assert.doesNotMatch(document.body.textContent, /Building your application/);
      assert.equal(calls.length, 2); assert.deepEqual(intents, []);
    });
});

test("a held background poll preserves the focused build action and compact labels retain full accessible meaning", async () => {
  let finishPoll, reads = 0;
  const held = new Promise(resolve => { finishPoll = resolve; });
  await mounted(() => ++reads === 1 ? receipt() : reads === 2 ? held : checked(),
    async ({ document, calls, intents, button, poll, render, settle, click, polls }) => {
      const original = button("Return to build");
      original.focus(); assert.equal(document.activeElement === original, true);
      await poll();
      assert.equal(button("Return to build") === original, true, "background refresh must retain the known action node");
      assert.equal(document.activeElement === original, true, "background refresh must preserve keyboard focus");
      assert.match(document.body.textContent, /Building your application/);
      assert.doesNotMatch(document.body.textContent, /Checking application build/);
      assert.equal(original.getAttribute("aria-label"), "Return to build for Observing club");
      await click("Return to build"); assert.equal(intents.length, 1); assert.equal(calls.length, 2);
      await render({ compact: true });
      assert.equal(button("Open build") === original, true); assert.equal(document.activeElement === original, true);
      assert.equal(original.getAttribute("aria-label"), "Return to build for Observing club");
      await act(async () => { finishPoll({ httpStatus: 503 }); await settle(); });
      assert.match(document.body.textContent, /Build status unavailable/);
      assert.equal(button("Open build") === undefined, true, "a real failed observation retires the navigation action");
      assert.equal(button("Check status").getAttribute("aria-label"), "Check build status for Observing club");
      assert.equal(polls.size, 0); assert.equal(intents.length, 1);
      await click("Check status"); assert.match(document.body.textContent, /Checked build ready to review/);
      assert.equal(button("Review build").getAttribute("aria-label"), "Review completed build for Observing club");
      await click("Review build"); assert.equal(intents.length, 2); assert.equal(calls.length, 3);
      assert.deepEqual(intents[1], { guideId: setupA.id, buildId: buildA, guideVersion: 4, currentGuideVersion: 6 });
    });
});
