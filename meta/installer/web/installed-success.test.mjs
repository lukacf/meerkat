import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { installedUseStartingIdeas, installedUseScopeKey, installedUseSuccessBody, installedUseSuccessReport,
  installedUseSuccessBodyMatches, createInstalledUseSuccessStore, installedUseUrl, createInstalledUseDraftStore } from "./deployment-use.mjs";
import { installedUseSuccessNotAdmitted } from "./deployment-use.mjs";
import { remoteInstalledUseBinding, remoteInstalledUseDraftScope, remoteInstalledUseSuccessBody,
  remoteInstalledUseSuccessReport, createRemoteInstalledUseSuccessStore, remoteInstalledUseUrl } from "./remote-installed-use.mjs";

const source = "a".repeat(64), archive = "b".repeat(64), other = "c".repeat(64);
const state = { id: "11111111-1111-4111-8111-111111111111", version: 4 };
const build = { guide_id: state.id, guide_version: 4, current: false, source_verified: false,
  build_id: "unchecked-candidate", source_digest: other, status: "running" };
const firstUse = { scope: "authored-first-use-suggestions", metadata_sha256: "d".repeat(64), detail: null,
  tasks: [{ label: "Plan an observing session", request: "Check a real observing site and suggest tonight's targets.",
    inputs: ["Your observing site and time window", "Telescope limits and preferred targets"],
    prerequisites: ["A configured forecast connection", "Supply the telescope's limits before proposing a target"],
    expected_result: "An observing window with suitable targets and an explanation of uncertainty.",
    result_location: "The session card in your observing workspace." }] };
const local = { guide_id: state.id, guide_version: 4, deployment_id: "22222222-2222-4222-8222-222222222222",
  build_id: "installed-build", source_digest: source, package_sha256: archive, status: "stopped",
  process_live: false, health: "unknown", console_url: null, current: false };
const remote = { protocol: 1, guide_id: state.id, guide_version: 4, deployment_id: local.deployment_id,
  build_id: local.build_id, accepted_source_digest: source, package_sha256: archive,
  transferred_source_digest: other, controller_sha256: "e".repeat(64), admitted: true, remote_admission_requested: true,
  endpoint: { host: "synthetic-host.invalid", user: "fixture", ssh_port: 22 },
  host: { uid: 1001, home: "/home/fixture", destination: "/home/fixture/src/observing" },
  destination: "/home/fixture/src/observing", host_key_fingerprint: "SHA256:" + "A".repeat(43),
  status: "observation-unavailable", observation: { available: false }, source_current: false,
  ready: false, worker_live: false, last_known_status: "stopped",
  application_experiences: { protocol: 1, supported: true, status: "none", source_digest: source,
    installed_source_digest: other, metadata_sha256: null, entries: [] } };
function metadata(kind, receipt = kind === "local" ? local : remote, guide = state, patch = {}) {
  const base = { guide_id: guide.id, version: guide.version, guide_version: receipt.guide_version,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id, source_digest: receipt.source_digest || receipt.accepted_source_digest,
    package_sha256: receipt.package_sha256, status: receipt.status, project: { name: "Observing programme" },
    entrypoint: { identity: "lead:main", name: "Observing coordinator" }, native_runtime_available: false,
    application_outcomes_verified: false, first_use: structuredClone(firstUse), success_report: null,
    continuation: { available: false, build_id: build.build_id, source_digest: build.source_digest, requires_acknowledgement: true } };
  return kind === "local" ? { ...base, scope: "verified-installed-source", current: false, console_url: null, ...patch }
    : { ...base, protocol: 1, scope: "frozen-remote-package-source", current: false,
      accepted_source_digest: receipt.accepted_source_digest, transferred_source_digest: receipt.transferred_source_digest,
      controller_sha256: receipt.controller_sha256, remote_binding: JSON.parse(remoteInstalledUseBinding(receipt, guide)),
      installed_source_verified: false, observation: { scope: "saved-destination-observation", status: receipt.status,
        available: false, freshness: "unavailable", admitted: false, source_current: false }, ...patch };
}
const requestId = "33333333-3333-4333-8333-333333333333";
const response = (body, guide = state) => ({ ...body, scope: "human-reported-installed-success", guide_id: guide.id,
  outcome: "worked", recorded_at: "2026-10-03T08:12:34.123456+00:00", application_outcomes_verified: false });
const makeBody = (kind, value = metadata(kind), receipt = kind === "local" ? local : remote, guide = state, task = "  My real task\n  ", id = requestId) =>
  (kind === "local" ? installedUseSuccessBody : remoteInstalledUseSuccessBody)(value, receipt, guide, task, id);
const validate = (kind, value, m = metadata(kind), receipt = kind === "local" ? local : remote, guide = state, body = null) =>
  (kind === "local" ? installedUseSuccessReport : remoteInstalledUseSuccessReport)(value, m, receipt, guide, body);
const memoryStorage = () => {
  const values = new Map(); return { values, getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: (key) => values.delete(key) };
};
const deferred = () => {
  let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

test("authored walkthrough fields retain exact plaintext and reject malformed optional metadata without inventing a job", () => {
  assert.deepEqual(installedUseStartingIdeas(firstUse).tasks, firstUse.tasks);
  const task = firstUse.tasks[0];
  for (const patch of [{ inputs: "private" }, { prerequisites: [null] }, { inputs: Array(9).fill("input") },
    { inputs: ["x".repeat(401)] }, { prerequisites: ["hidden\u0000requirement"] }, { expected_result: "x".repeat(601) },
    { result_location: {} }, { result_location: "x".repeat(401) }]) {
    assert.deepEqual(installedUseStartingIdeas({ ...firstUse, tasks: [{ ...task, ...patch }] }), { tasks: [], detail: null });
  }
  const unicode = { ...task, inputs: ["\u{1f9ed}".repeat(400)], expected_result: "\u{1f9ed}".repeat(600) };
  assert.deepEqual(installedUseStartingIdeas({ ...firstUse, tasks: [unicode] }).tasks, [unicode]);
  assert.deepEqual(installedUseStartingIdeas(null), { tasks: [], detail: null });
});

for (const kind of ["local", "remote"]) {
  test(`${kind} human success requires the exact saved source and bounded nonempty task, independently of candidate or runtime`, () => {
    const body = makeBody(kind);
    assert.deepEqual(body, { version: 4, deployment_id: local.deployment_id, build_id: local.build_id,
      source_digest: source, package_sha256: archive, request_id: requestId, task: "My real task" });
    assert.ok(validate(kind, response(body), undefined, undefined, undefined, body));
    for (const task of ["", " ", "x".repeat(3001), "\u{1f9ed}".repeat(3001), "hidden\u0000task", "broken\ud800", "broken\udfff"]) {
      assert.equal(makeBody(kind, undefined, undefined, undefined, task), null);
    }
    const old = metadata(kind), receipt = kind === "local" ? local : remote;
    for (const patch of [{ deployment_id: "other" }, { build_id: "other" }, { package_sha256: other }, { source_digest: other },
      { scope: "unverified-source" }, { version: 3 }, { application_outcomes_verified: true }]) {
      assert.equal(makeBody(kind, { ...old, ...patch }, receipt), null);
    }
    const historicalGuide = { ...state, version: 7 }, historicalMetadata = metadata(kind, receipt, historicalGuide);
    assert.ok(validate(kind, response(body), historicalMetadata, receipt, historicalGuide));
    assert.equal(validate(kind, { ...response(body), version: 8 }, historicalMetadata, receipt, historicalGuide), null);
    for (const patch of [{ request_id: "44444444-4444-4444-8444-444444444444" }, { task: "Different task" },
      { source_digest: other }, { package_sha256: other }, { guide_id: "other" }, { deployment_id: "other" }, { build_id: "other" },
      { version: 3 }, { outcome: "partly" }, { scope: "verified-application-result" }, { application_outcomes_verified: true },
      { recorded_at: "not a timestamp" }]) {
      assert.equal(validate(kind, { ...response(body), ...patch }, undefined, undefined, undefined, body), null);
    }
  });
}

test("the bounded success store keeps one exact UUID/body across reload and cannot restore private fields or another install", () => {
  const storage = memoryStorage(), scope = installedUseScopeKey(local, state), body = makeBody("local");
  const saved = createInstalledUseSuccessStore(storage);
  assert.equal(saved.prepare(scope, body), true);
  assert.equal(saved.prepare(scope, { ...body, request_id: "44444444-4444-4444-8444-444444444444" }), false);
  assert.equal(saved.prepare(scope, { ...body, task: "new unsent task" }), false);
  const copy = saved.read(scope); copy.task = "mutating copy";
  assert.deepEqual(createInstalledUseSuccessStore(storage).read(scope), body);
  assert.equal(saved.persistent(scope), true);
  const [key, raw] = [...storage.values.entries()][0];
  assert.deepEqual(Object.keys(JSON.parse(raw).body).sort(), ["version", "deployment_id", "build_id", "source_digest", "package_sha256", "request_id", "task"].sort());
  for (const bad of ["malformed", "x".repeat(100001), JSON.stringify({ scope, body: { ...body, password: "PRIVATE_SETTING" } }),
    JSON.stringify({ scope, body: { ...body, task: "x".repeat(3001) } }), JSON.stringify({ scope: "other", body })]) {
    storage.values.set(key, bad); assert.equal(createInstalledUseSuccessStore(storage).read(scope), null);
  }
  storage.values.set(key, raw);
  assert.equal(createInstalledUseSuccessStore(storage).read(installedUseScopeKey({ ...local, package_sha256: other }, state)), null);
  assert.equal(saved.clear(scope, { ...body, task: "wrong" }), false);
  assert.equal(saved.clear(scope, body), true); assert.equal(saved.read(scope), null);
  for (const unavailable of [null, { getItem() { throw new Error("PRIVATE_STORAGE"); }, setItem() {}, removeItem() {} }]) {
    const open = createInstalledUseSuccessStore(unavailable);
    assert.equal(open.prepare(scope, body), true); assert.deepEqual(open.read(scope), body); assert.equal(open.persistent(scope), false);
  }
});

test("remote success retry storage belongs to the exact host key, destination and source", () => {
  const storage = memoryStorage(), binding = remoteInstalledUseBinding(remote, state), scope = remoteInstalledUseDraftScope(remote, state);
  const saved = createRemoteInstalledUseSuccessStore(storage, binding), body = makeBody("remote");
  assert.equal(saved.prepare(scope, body), true);
  assert.deepEqual(createRemoteInstalledUseSuccessStore(storage, binding).read(scope), body);
  for (const patch of [{ host_key_fingerprint: "SHA256:" + "B".repeat(43) },
    { endpoint: { ...remote.endpoint, host: "another-host.invalid" } }]) {
    const changed = { ...remote, ...patch }, otherBinding = remoteInstalledUseBinding(changed, state);
    assert.equal(createRemoteInstalledUseSuccessStore(storage, otherBinding).read(scope), null);
  }
});

test("only the exact host absence witness can release an unsaved success confirmation", () => {
  assert.equal(installedUseSuccessNotAdmitted({ status: 409, code: "installed_success_not_admitted" }), true);
  for (const error of [null, { status: 409 }, { status: 422, code: "installed_success_not_admitted" },
    { status: "409", code: "installed_success_not_admitted" }, { status: 409, code: "installed_feedback_not_admitted" }]) {
    assert.equal(installedUseSuccessNotAdmitted(error), false);
  }
  const scope = installedUseScopeKey(local, state), body = makeBody("local");
  for (const failure of ["remove", "no-op", "read", "readback", "replacement", "corrupt", "oversized"]) {
    const storage = memoryStorage(), store = createInstalledUseSuccessStore(storage);
    assert.equal(store.prepare(scope, body), true);
    const [key, raw] = [...storage.values.entries()][0];
    const originalGet = storage.getItem, originalRemove = storage.removeItem;
    if (failure === "remove") storage.removeItem = () => { throw new Error("PRIVATE_STORAGE"); };
    if (failure === "no-op") storage.removeItem = () => {};
    if (failure === "read") storage.getItem = () => { throw new Error("PRIVATE_STORAGE"); };
    if (failure === "readback") storage.removeItem = (key) => {
      originalRemove(key); storage.getItem = () => { throw new Error("PRIVATE_STORAGE"); };
    };
    if (failure === "replacement") storage.values.set(key, JSON.stringify({ scope, body: { ...body, task: "Another report" } }));
    if (failure === "corrupt") storage.values.set(key, "malformed");
    if (failure === "oversized") storage.values.set(key, "x".repeat(100001));
    const before = storage.values.get(key);
    assert.equal(store.reject(scope, body), false, failure);
    assert.deepEqual(store.read(scope), body, failure);
    if (failure !== "readback") assert.equal(storage.values.get(key), before, failure);
    storage.getItem = originalGet; storage.removeItem = originalRemove; storage.values.set(key, raw);
    assert.equal(store.reject(scope, { ...body, task: "Wrong report" }), false);
    assert.equal(store.reject(scope, body), true);
    assert.equal(store.read(scope), null);
    assert.equal(createInstalledUseSuccessStore(storage).read(scope), null);
  }
  const volatile = createInstalledUseSuccessStore(null);
  assert.equal(volatile.prepare(scope, body), true);
  assert.equal(volatile.reject(scope, body), true);
  assert.equal(volatile.read(scope), null);
});

test("remote success needs exact historic installation admission and idle custody without requiring a live SSH observation", () => {
  assert.equal(remote.observation.available, false);
  assert.equal(metadata("remote").native_runtime_available, false);
  assert.ok(makeBody("remote"));
  for (const flag of ["remote_admission_requested", "admitted"]) {
    for (const value of [false, undefined, null, "true", 1]) {
      const changed = { ...remote, [flag]: value };
      assert.equal(makeBody("remote", metadata("remote", changed), changed), null);
    }
  }
  for (const patch of [{ worker_live: true }, { worker_live: undefined }, { worker_live: "unknown" },
    { pending_operation: { id: "unconfirmed-operation" } }, { status: "installing" },
    { last_known_status: "installing" }]) {
    const changed = { ...remote, ...patch };
    assert.equal(makeBody("remote", metadata("remote", changed), changed), null);
  }
  assert.equal(makeBody("remote", metadata("remote", remote, state, { success_reporting_available: false })), null);
  const body = makeBody("remote"), historic = { ...remote, remote_admission_requested: false };
  assert.ok(validate("remote", response(body), metadata("remote", historic), historic));
});

test("local readiness blocks preparation while preserving an exact prior retry and source-bound human readback", () => {
  const body = makeBody("local"), scope = installedUseScopeKey(local, state), store = createInstalledUseSuccessStore(memoryStorage());
  assert.equal(store.prepare(scope, body), true);
  const unavailable = metadata("local", local, state, { success_reporting_available: false });
  assert.equal(makeBody("local", unavailable), null);
  assert.deepEqual(store.read(scope), body);
  assert.ok(validate("local", response(body), unavailable, local, state, body));
  const ready = metadata("local", local, state, { success_reporting_available: true });
  assert.ok(installedUseSuccessBodyMatches(store.read(scope), makeBody("local", ready)));
});

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const component = {};
for (const kind of ["local", "remote"]) {
  const result = await bundle({ entryPoints: [fileURLToPath(new URL(kind === "local" ? "deployment-use.jsx" : "remote-installed-use.jsx", import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    } }] });
  const loaded = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}`);
  component[kind] = kind === "local" ? loaded.InstalledApplicationUseMount : loaded.RemoteInstalledUse;
}

async function mounted(kind, verify, { post, initialMetadata, initialStorage } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], builds = [], storage = initialStorage || dom.window.sessionStorage;
  if (initialStorage) Object.defineProperty(dom.window, "sessionStorage", { configurable: true, value: initialStorage });
  let metadataPatch = initialMetadata || {}, current = { state, receipt: kind === "local" ? local : remote, build,
    blocked: true, onRepairReceipt: (value) => builds.push(value), Icon: () => null };
  const metadataForCurrent = () => metadata(kind, current.receipt, current.state, metadataPatch);
  let uuidCount = 0;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: storage, IS_REACT_ACT_ENVIRONMENT: true,
    crypto: { randomUUID: () => (++uuidCount).toString(16).padStart(8, "0") + "-0000-4000-8000-000000000000" },
    fetch: async (target, options = {}) => {
      const body = options.body ? JSON.parse(options.body) : null;
      calls.push({ target, method: options.method || "GET", body });
      if (!options.method) {
        assert.equal(target, kind === "local" ? installedUseUrl(current.state.id) : remoteInstalledUseUrl(current.state.id, current.receipt.deployment_id));
        return { ok: true, json: async () => structuredClone(metadataForCurrent()) };
      }
      assert.equal(options.method, "POST");
      assert.equal(target, kind === "local" ? installedUseUrl(current.state.id, "success") : remoteInstalledUseUrl(current.state.id, current.receipt.deployment_id, "success"));
      if (post) return await post(body, calls.filter((call) => call.method === "POST").length);
      return { ok: true, json: async () => response(body, current.state) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  let root = createRoot(dom.window.document.getElementById("root"));
  const props = () => kind === "local" ? { ...current, experiences: { deployment_id: current.receipt.deployment_id,
    build_id: current.receipt.build_id, source_digest: current.receipt.source_digest, package_sha256: current.receipt.package_sha256,
    metadata_sha256: null, scope: "declared-source", entries: [], verified: false } } : current;
  const rerender = async (patch = {}, sourcePatch = metadataPatch) => {
    current = { ...current, ...patch }; metadataPatch = sourcePatch;
    await act(async () => root.render(React.createElement(component[kind], props())));
  };
  const reload = async (patch = {}, sourcePatch = metadataPatch) => {
    await act(async () => root.unmount()); root = createRoot(dom.window.document.getElementById("root"));
    await rerender(patch, sourcePatch);
  };
  const edit = async (value, selector = ".deployment-use-field textarea") => {
    const field = dom.window.document.querySelector(selector);
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(field, value); field.dispatchEvent(new dom.window.Event("input", { bubbles: true })); });
  };
  const button = (label) => [...dom.window.document.querySelectorAll("button")].find((node) =>
    node.textContent === label || node.querySelector("strong")?.textContent === label);
  const click = async (label) => { const selected = button(label); assert.ok(selected, label); await act(async () => selected.click()); };
  try {
    await rerender();
    await verify({ document: dom.window.document, window: dom.window, calls, builds, storage, edit, click, button, rerender, reload,
      props: () => current, metadata: metadataForCurrent, uuidCount: () => uuidCount });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

for (const kind of ["local", "remote"]) {
  for (const held of ["headers", "JSON body"]) {
    test(`${kind} success report response deadline recovers held ${held} without automatic replay`, async (t) => {
      t.mock.timers.enable({ apis: ["setTimeout"] });
      const pending = deferred();
      const task = "  Check the real observing window\nKeep these exact draft details.  ";
      const improvement = "Keep my unsent improvement\nincluding this second line.";
      await mounted(kind, async ({ document, edit, click, button, calls, builds, uuidCount }) => {
        const posts = () => calls.filter((call) => call.method === "POST");
        const reads = () => calls.filter((call) => call.method === "GET");
        const assertDrafts = () => {
          assert.equal(document.querySelector(".deployment-use-field textarea").value, task);
          assert.equal(document.querySelector(".deployment-use-feedback textarea").value, improvement);
        };
        await edit(task); await edit(improvement, ".deployment-use-feedback textarea");
        await click("It worked");
        const original = structuredClone(posts()[0].body);
        assert.equal(original.task, task.trim());
        assert.equal(posts().length, 1); assert.equal(uuidCount(), 1);
        assert.equal(button("Saving your success report...").disabled, true);
        assert.equal(button("Recheck success reports").disabled, true);
        await act(async () => t.mock.timers.tick(29999));
        assert.equal(button("Saving your success report...").disabled, true);
        await act(async () => t.mock.timers.tick(1));
        assert.equal(button("Recheck success reports").disabled, false,
          "A response deadline must release readback while preserving the exact pending report");
        assert.equal(button("Retry success confirmation").disabled, false);
        assert.match(document.querySelector(".deployment-use-success").textContent, /could not be confirmed/);
        assertDrafts();
        await act(async () => t.mock.timers.tick(60000));
        assert.equal(posts().length, 1, "Timeout must not submit another report");
        const beforeCheck = reads().length;
        await click("Recheck success reports");
        assert.equal(reads().length, beforeCheck + 1);
        assert.equal(posts().length, 1, "Readback must not retry the pending POST");
        assert.equal(button("Retry success confirmation").disabled, false);
        assertDrafts();
        await click("Retry success confirmation");
        assert.equal(posts().length, 2); assert.deepEqual(posts()[1].body, original);
        assert.equal(uuidCount(), 1); assert.equal(builds.length, 0);
        assert.match(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
        assertDrafts();
        await act(async () => pending.resolve(held === "headers"
          ? { ok: true, json: async () => response(original) } : response(original)));
        assert.equal(posts().length, 2);
        assert.match(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
        assertDrafts();
      }, { post: async (body, count) => {
        if (count !== 1) return { ok: true, json: async () => response(body) };
        return held === "headers" ? pending.promise : { ok: true, json: () => pending.promise };
      } });
    });
  }

  test(`${kind} mounted walkthrough and successful first job preserve edits without builder, current candidate or running service`, async () => {
    await mounted(kind, async ({ document, edit, click, button, calls, builds }) => {
      assert.equal(button("It worked").disabled, true);
      await click(firstUse.tasks[0].label);
      const walkthrough = document.querySelector(".deployment-use-walkthrough");
      assert.match(walkthrough.textContent, /Your observing site and time window/);
      assert.match(walkthrough.textContent, /A configured forecast connection/);
      assert.match(walkthrough.textContent, /An observing window with suitable targets/);
      assert.match(walkthrough.textContent, /session card in your observing workspace/);
      assert.equal(walkthrough.querySelector("a") === null, true);
      const task = "  A customized real job\n\u{1f9ed}  ", improvement = "Unsent improvement text stays here.";
      await edit(task); await edit(improvement, ".deployment-use-feedback textarea");
      assert.match(document.querySelector(".deployment-use-walkthrough").textContent, /instructions describe the original suggested job/);
      assert.equal(button("It worked").disabled, false);
      await click("It worked");
      const posts = calls.filter((call) => call.method === "POST");
      assert.equal(posts.length, 1); assert.equal(posts[0].body.task, task.trim());
      assert.equal(document.querySelector(".deployment-use-field textarea").value, task);
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, improvement);
      assert.match(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      assert.match(document.querySelector(".deployment-use-success").textContent, /not independently verified/);
      assert.equal(builds.length, 0);
      await click("Choose the next job");
      assert.equal(document.activeElement.querySelector("strong")?.textContent, firstUse.tasks[0].label);
      await click("Write your own next job");
      assert.equal(document.activeElement === document.querySelector(".deployment-use-field textarea"), true);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, task);
    });
  });

  test(`${kind} absent or malformed authored walkthrough leaves a useful editable own-job path`, async () => {
    for (const first_use of [null, { ...firstUse, tasks: [{ ...firstUse.tasks[0], inputs: "invalid" }] }]) {
      await mounted(kind, async ({ document, edit, click, calls }) => {
        assert.equal(document.querySelector(".deployment-use-ideas") === null, true);
        assert.match(document.querySelector(".deployment-use-walkthrough").textContent, /Your own first job/);
        assert.match(document.querySelector(".deployment-use-walkthrough").textContent, /information the agents should use/);
        assert.doesNotMatch(document.querySelector(".deployment-use-walkthrough").textContent, /configured forecast|session card/);
        await edit("My own meaningful job"); await click("It worked");
        assert.equal(calls.filter((call) => call.method === "POST").length, 1);
      }, { initialMetadata: { first_use } });
    }
  });

  test(`${kind} malformed and failed success responses keep the same exact retry body and unsent drafts`, async () => {
    await mounted(kind, async ({ document, edit, click, calls, builds, uuidCount }) => {
      await edit("First task"); await edit("Unsent problem report", ".deployment-use-feedback textarea");
      await click("It worked");
      assert.match(document.querySelector(".deployment-use-success").textContent, /was not confirmed/);
      await click("Retry success confirmation");
      assert.match(document.querySelector(".deployment-use-success").textContent, /could not be confirmed/);
      await click("Retry success confirmation");
      const posts = calls.filter((call) => call.method === "POST");
      assert.equal(posts.length, 3); assert.deepEqual(posts[1].body, posts[0].body); assert.deepEqual(posts[2].body, posts[0].body);
      assert.equal(uuidCount(), 1); assert.equal(builds.length, 0);
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, "Unsent problem report");
      assert.match(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
    }, { post: async (body, count) => {
      if (count === 1) return { ok: true, json: async () => ({ ...response(body), task: "WRONG_TASK" }) };
      if (count === 2) throw new Error("PRIVATE_TRANSPORT_DETAIL");
      return { ok: true, json: async () => response(body) };
    } });
  });

  test(`${kind} task A-B-A edits retire a delayed success and retry the original report without confirming the current draft`, async () => {
    const pending = deferred();
    await mounted(kind, async ({ document, edit, button, click, calls, builds, uuidCount }) => {
      await edit("Task A");
      await act(async () => button("It worked").click());
      await edit("Task B"); await edit("Task A");
      await act(async () => pending.resolve({ ok: true, json: async () => response(calls.find((call) => call.method === "POST").body) }));
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      assert.match(document.querySelector(".deployment-use-success").textContent, /draft changed before success was confirmed/);
      await edit("New unsent task C"); await click("Retry success confirmation");
      const posts = calls.filter((call) => call.method === "POST");
      assert.deepEqual(posts[1].body, posts[0].body); assert.equal(uuidCount(), 1); assert.equal(builds.length, 0);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "New unsent task C");
      assert.match(document.querySelector(".deployment-use-success").textContent, /earlier success report was confirmed/);
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
    }, { post: async (body, count) => count === 1 ? pending.promise : { ok: true, json: async () => response(body) } });
  });

  test(`${kind} installed source A-B-A changes retire delayed reports and preserve the exact request for later retry`, async () => {
    const pending = deferred();
    await mounted(kind, async ({ document, edit, button, click, rerender, calls, builds, props }) => {
      await edit("Task A"); await act(async () => button("It worked").click());
      const original = props().receipt;
      const changed = kind === "local" ? { ...original, package_sha256: other }
        : { ...original, host_key_fingerprint: "SHA256:" + "B".repeat(43) };
      await rerender({ receipt: changed }); await rerender({ receipt: original });
      await act(async () => pending.resolve({ ok: true, json: async () => response(calls.find((call) => call.method === "POST").body) }));
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "Task A");
      await click("Retry success confirmation");
      const posts = calls.filter((call) => call.method === "POST");
      assert.deepEqual(posts[1].body, posts[0].body); assert.equal(builds.length, 0);
    }, { post: async (body, count) => count === 1 ? pending.promise : { ok: true, json: async () => response(body) } });
  });

  test(`${kind} tab reload replays exact ambiguous admission across guide version edits and restores current-source human readback`, async () => {
    await mounted(kind, async ({ document, edit, click, reload, calls, builds, uuidCount, props }) => {
      await edit("Task A"); await edit("Unsent improvement", ".deployment-use-feedback textarea");
      await click("It worked");
      const original = calls.find((call) => call.method === "POST").body;
      await reload({ state: { ...state, version: 5 } });
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "Task A");
      await click("Retry success confirmation");
      assert.deepEqual(calls.filter((call) => call.method === "POST")[1].body, original);
      assert.equal(uuidCount(), 1); assert.equal(builds.length, 0);
      assert.match(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      await reload({}, { success_report: response(original) });
      assert.match(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, "Unsent improvement");
      assert.equal(calls.filter((call) => call.method === "POST").length, 2);
      await edit("Different task");
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
    }, { post: async (body, count) => {
      if (count === 1) throw new Error("AMBIGUOUS_DELIVERY");
      return { ok: true, json: async () => response(body) };
    } });
  });

  test(`${kind} readback from a different task or source cannot confirm the current first job`, async () => {
    for (const patch of [{ task: "another task" }, { source_digest: other }, { package_sha256: other },
      { application_outcomes_verified: true }]) {
      await mounted(kind, async ({ document, edit, reload }) => {
        await edit("Task A"); await reload();
        assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      }, { initialMetadata: { success_report: { ...response(makeBody(kind, undefined, undefined, undefined, "Task A")), ...patch } } });
    }
  });

  test(`${kind} a never-delivered old-version report releases only on coded absence and requires a fresh human confirmation`, async () => {
    await mounted(kind, async ({ document, edit, click, reload, calls, builds, uuidCount, button }) => {
      await edit("Task A"); await edit("Unsent improvement", ".deployment-use-feedback textarea");
      await click("It worked");
      const original = calls.find((call) => call.method === "POST").body;
      await reload({ state: { ...state, version: 5 } });
      await edit("Current task B"); await click("Retry success confirmation");
      const posts = calls.filter((call) => call.method === "POST");
      assert.equal(posts.length, 2); assert.deepEqual(posts[1].body, original);
      assert.equal(uuidCount(), 1); assert.equal(builds.length, 0);
      assert.match(document.querySelector(".deployment-use-success").textContent, /earlier success report was not saved/);
      assert.doesNotMatch(document.body.textContent, /PRIVATE_HOST_DETAIL|Your success report is saved/);
      assert.equal(button("Retry success confirmation") === undefined, true); assert.equal(button("It worked").disabled, false);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "Current task B");
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, "Unsent improvement");
      await reload();
      assert.equal(calls.filter((call) => call.method === "POST").length, 2);
      await click("It worked");
      const next = calls.filter((call) => call.method === "POST")[2].body;
      assert.equal(next.version, 5); assert.equal(next.task, "Current task B");
      assert.notEqual(next.request_id, original.request_id); assert.equal(uuidCount(), 2);
      assert.match(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      assert.equal(builds.length, 0);
    }, { post: async (body, count) => {
      if (count === 1) throw new Error("LOST_BEFORE_DELIVERY");
      if (count === 2) return { ok: false, status: 409, json: async () => ({ code: "installed_success_not_admitted", detail: "PRIVATE_HOST_DETAIL" }) };
      return { ok: true, json: async () => response(body, { ...state, version: 5 }) };
    } });
  });

  test(`${kind} failed retry-record removal blocks new confirmations and clear retries never POST`, async () => {
    const storage = memoryStorage(), remove = storage.removeItem;
    let failRemoval = true, failedRemovals = 0;
    storage.removeItem = (key) => {
      if (failRemoval && key.includes("mobkit-installed-use-success-v1:")) { failedRemovals++; throw new Error("PRIVATE_BROWSER_STORAGE"); }
      remove(key);
    };
    await mounted(kind, async ({ document, edit, click, reload, calls, builds, uuidCount, button }) => {
      await edit("Task A"); await edit("Unsent improvement", ".deployment-use-feedback textarea"); await click("It worked");
      await reload({ state: { ...state, version: 5 } }); await click("Retry success confirmation");
      const original = calls.find((call) => call.method === "POST").body;
      assert.ok(failedRemovals > 0, "The actual browser store exercised the injected removal failure");
      assert.match(document.querySelector(".deployment-use-success").textContent, /browser retry record could not be cleared/);
      assert.equal(button("Retry success confirmation").disabled, true);
      await click("Clear unsaved confirmation");
      assert.equal(calls.filter((call) => call.method === "POST").length, 2); assert.equal(uuidCount(), 1);
      await reload();
      assert.equal(button("Clear unsaved confirmation") === undefined, true);
      await click("Retry success confirmation");
      assert.deepEqual(calls.filter((call) => call.method === "POST")[2].body, original);
      failRemoval = false; await click("Clear unsaved confirmation");
      assert.equal(calls.filter((call) => call.method === "POST").length, 3); assert.equal(uuidCount(), 1);
      assert.equal(button("It worked").disabled, false);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "Task A");
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, "Unsent improvement");
      assert.equal(builds.length, 0); assert.doesNotMatch(document.body.textContent, /PRIVATE_BROWSER_STORAGE/);
    }, { initialStorage: storage, post: async (body, count) => {
      if (count === 1) throw new Error("LOST_BEFORE_DELIVERY");
      return { ok: false, status: 409, json: async () => ({ code: "installed_success_not_admitted" }) };
    } });
  });

  for (const change of ["task", "source", "version", "outcome"]) test(`${kind} delayed absence after ${change} A-B-A does not release another selected operation`, async () => {
    const late = deferred();
    await mounted(kind, async ({ document, edit, click, button, calls, uuidCount, props, rerender }) => {
      await edit("Task A"); await act(async () => button("It worked").click());
      if (change === "task") { await edit("Task B"); await edit("Task A"); }
      if (change === "source") {
        const original = props().receipt;
        const changed = kind === "local" ? { ...original, package_sha256: other }
          : { ...original, host_key_fingerprint: "SHA256:" + "B".repeat(43) };
        await rerender({ receipt: changed }); await rerender({ receipt: original });
      }
      if (change === "version") { await rerender({ state: { ...state, version: 5 } }); await rerender({ state }); }
      if (change === "outcome") {
        await act(async () => document.querySelector('.deployment-use-feedback input[value="partly"]').click());
        await act(async () => document.querySelector('.deployment-use-feedback input[value="worked"]').click());
      }
      await act(async () => late.resolve({ ok: false, status: 409, json: async () => ({ code: "installed_success_not_admitted" }) }));
      assert.equal(button("It worked") === undefined, true);
      if (change !== "source") assert.match(document.querySelector(".deployment-use-success").textContent, /draft changed before success was confirmed/);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "Task A");
      assert.equal(button("Clear unsaved confirmation") === undefined, true);
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /earlier success report was not saved|Your success report is saved/);
      assert.equal(calls.filter((call) => call.method === "POST").length, 1); assert.equal(uuidCount(), 1);
      await click("Retry success confirmation");
      const posts = calls.filter((call) => call.method === "POST");
      assert.deepEqual(posts[1].body, posts[0].body); assert.equal(uuidCount(), 1);
      assert.equal(button("It worked").disabled, false);
    }, { post: async (body, count) => count === 1 ? late.promise
      : { ok: false, status: 409, json: async () => ({ code: "installed_success_not_admitted" }) } });
  });

  test(`${kind} corrupt success history blocks a new UUID while preserving ordinary job drafts and a source recheck`, async () => {
    await mounted(kind, async ({ document, edit, click, button, calls, uuidCount }) => {
      await edit("Task with unchecked history"); await edit("Unsent improvement", ".deployment-use-feedback textarea");
      assert.equal(button("It worked").disabled, true);
      assert.match(document.querySelector(".deployment-use-success").textContent, /Saved success reports could not be checked/);
      assert.doesNotMatch(document.body.textContent, /PRIVATE_STORAGE_PATH/);
      await click("Recheck success reports");
      assert.equal(button("It worked").disabled, true);
      assert.equal(uuidCount(), 0); assert.equal(calls.filter((call) => call.method === "POST").length, 0);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "Task with unchecked history");
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, "Unsent improvement");
    }, { initialMetadata: { success_report_detail: "PRIVATE_STORAGE_PATH" } });
  });

  test(`${kind} current guide version A-B-A and changed outcome retire an in-flight success even for unchanged task text`, async () => {
    const pending = deferred();
    await mounted(kind, async ({ document, edit, button, click, rerender, calls, props }) => {
      await edit("The same task"); await act(async () => button("It worked").click());
      await rerender({ state: { ...state, version: 5 } }); await rerender({ state });
      const partly = document.querySelector('.deployment-use-feedback input[value="partly"]');
      await act(async () => partly.click());
      await act(async () => pending.resolve({ ok: true, json: async () => response(calls.find((call) => call.method === "POST").body) }));
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      await click("Retry success confirmation");
      assert.deepEqual(calls.filter((call) => call.method === "POST")[1].body, calls.find((call) => call.method === "POST").body);
      assert.equal(partly.checked, true);
      assert.match(document.querySelector(".deployment-use-success").textContent, /An earlier success report is saved for this job/);
      assert.equal(calls.filter(call => call.method === "POST").length, 2);
      await act(async () => document.querySelector('.deployment-use-feedback input[value="blocked"]').click());
      assert.match(document.querySelector(".deployment-use-success").textContent, /An earlier success report is saved for this job/);
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      assert.equal(calls.filter(call => call.method === "POST").length, 2);
    }, { post: async (body, count) => count === 1 ? pending.promise : { ok: true, json: async () => response(body) } });
  });

  test(`${kind} a conflict preserves the pending UUID through recheck and reload without creating another admission`, async () => {
    await mounted(kind, async ({ document, edit, click, reload, calls, uuidCount }) => {
      await edit("The exact original task"); await click("It worked");
      const original = calls.find((call) => call.method === "POST").body;
      await edit("An unsent new task"); await click("Recheck success reports");
      assert.match(document.querySelector(".deployment-use-success").textContent, /earlier task is awaiting confirmation/);
      await click("Retry success confirmation"); await reload(); await click("Retry success confirmation");
      const posts = calls.filter((call) => call.method === "POST");
      assert.equal(posts.length, 3); assert.ok(posts.every((call) => installedUseSuccessBodyMatches(call.body, original)));
      assert.equal(uuidCount(), 1);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "An unsent new task");
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved|PRIVATE_CONFLICT_DETAIL/);
    }, { post: async () => ({ ok: false, status: 409, json: async () => ({ detail: "PRIVATE_CONFLICT_DETAIL" }) }) });
  });

  test(`${kind} a saved current-task success keeps a different pending confirmation reachable across recheck and reload`, async () => {
    const taskB = "  Saved task B.\nKeep this draft exactly.  ";
    const savedB = response(makeBody(kind, undefined, undefined, undefined, taskB));
    await mounted(kind, async ({ document, edit, click, button, reload, calls, builds, uuidCount }) => {
      const success = () => document.querySelector(".deployment-use-success");
      const reportedTask = () => success().querySelector(".deployment-use-reported-job p").textContent;
      const assertPendingAlongsideSaved = () => {
        assert.equal(document.querySelector(".deployment-use-field textarea").value, taskB);
        assert.equal(reportedTask(), savedB.task);
        assert.match(success().textContent, /An earlier task is awaiting confirmation\. Retry its exact report first/);
        assert.equal(success().querySelector('[aria-label="Task awaiting success confirmation"]').textContent, "Pending task A");
        const retry = button("Retry success confirmation");
        assert.ok(retry); assert.equal(retry.disabled, false); assert.equal(retry.type, "button");
      };
      await edit(taskB);
      assert.equal(reportedTask(), savedB.task);
      await edit("  Pending task A  "); await click("It worked");
      const original = calls.find((call) => call.method === "POST").body;
      assert.equal(original.task, "Pending task A");
      assert.match(success().textContent, /could not be confirmed/);
      await edit(taskB); assertPendingAlongsideSaved();
      await click("Recheck success reports"); assertPendingAlongsideSaved();
      await reload(); assertPendingAlongsideSaved();
      assert.equal(calls.filter((call) => call.method === "POST").length, 1);
      await click("Retry success confirmation");
      const posts = calls.filter((call) => call.method === "POST");
      assert.equal(posts.length, 2); assert.deepEqual(posts[1].body, original);
      assert.equal(uuidCount(), 1); assert.equal(builds.length, 0);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, taskB);
      assert.equal(reportedTask(), savedB.task);
      assert.match(success().textContent, /Your earlier success report was confirmed/);
      assert.equal([...success().querySelectorAll(".deployment-use-reported-job")].find(node =>
        node.querySelector("summary").textContent === "View the earlier reported job").querySelector("p").textContent, "Pending task A");
      assert.equal(success().querySelector('[aria-label="Task awaiting success confirmation"]') === null, true);
      assert.equal(button("Retry success confirmation") === undefined, true);
    }, { initialMetadata: { success_report: savedB }, post: async (body, count) => {
      if (count === 1) throw new Error("AMBIGUOUS_DELIVERY");
      return { ok: true, json: async () => response(body) };
    } });
  });

  test(`${kind} a saved success alongside an unsent different outcome is clearly historical and does not change the draft`, async () => {
    const saved = response(makeBody(kind, undefined, undefined, undefined, "Task A"));
    await mounted(kind, async ({ document, edit, reload, calls }) => {
      await edit("Task A");
      await act(async () => document.querySelector('.deployment-use-feedback input[value="partly"]').click());
      await edit("An unsent qualification", ".deployment-use-feedback textarea");
      await reload();
      assert.match(document.querySelector(".deployment-use-success").textContent, /An earlier success report is saved for this job/);
      assert.match(document.querySelector(".deployment-use-success").textContent, /current problem-report draft is separate/);
      assert.equal(document.querySelector('.deployment-use-feedback input[value="partly"]').checked, true);
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, "An unsent qualification");
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    }, { initialMetadata: { success_report: saved } });
  });

  test(`${kind} unavailable reporting explains the disabled action safely and leaves ordinary first-use drafts available`, async () => {
    await mounted(kind, async ({ document, edit, button, click, calls, uuidCount }) => {
      await edit("A real task"); await edit("Keep my improvement draft", ".deployment-use-feedback textarea");
      assert.equal(button("It worked").disabled, true);
      assert.match(document.querySelector(".deployment-use-success").textContent, /Success reporting is unavailable right now/);
      assert.doesNotMatch(document.body.textContent, /PRIVATE_ADMISSION_DETAIL/);
      await click("Recheck success reports");
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "A real task");
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, "Keep my improvement draft");
      assert.equal(uuidCount(), 0); assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    }, { initialMetadata: { success_reporting_available: false, success_reporting_detail: "PRIVATE_ADMISSION_DETAIL" } });
  });

  test(`${kind} current mutation custody retires a success response even if installed source metadata has not changed`, async () => {
    const pending = deferred();
    await mounted(kind, async ({ document, edit, button, click, rerender, calls, props }) => {
      await edit("A real task"); await act(async () => button("It worked").click());
      const original = props().receipt;
      await rerender({ receipt: { ...original, pending_operation: { id: "active-installation-action" } } });
      await act(async () => pending.resolve({ ok: true, json: async () => response(calls.find((call) => call.method === "POST").body) }));
      assert.doesNotMatch(document.querySelector(".deployment-use-success").textContent, /Your success report is saved/);
      assert.equal(button("Retry success confirmation").disabled, true);
      await rerender({ receipt: original }); await click("Retry success confirmation");
      assert.deepEqual(calls.filter((call) => call.method === "POST")[1].body, calls.find((call) => call.method === "POST").body);
    }, { post: async (body, count) => count === 1 ? pending.promise : { ok: true, json: async () => response(body) } });
  });
}


for (const kind of ["local", "remote"]) {
  test(`${kind} reopening an installed job keeps checked earlier success separate from the restored next-job draft`, async () => {
    const receipt = kind === "local" ? local : remote;
    const scope = kind === "local" ? installedUseScopeKey(receipt, state) : remoteInstalledUseDraftScope(receipt, state);
    const taskA = "Task A: assess tonight's observing window.";
    const taskB = "  Task B: prepare tomorrow's equipment.\nKeep my supplied limits.  ";
    const qualification = "  Unsent qualification for Task B.\nPreserve every character.  ";
    const saved = response(makeBody(kind, undefined, receipt, state, taskA));

    await mounted(kind, async ({ document, calls, builds, button, rerender, reload, edit, props, uuidCount }) => {
      const success = () => document.querySelector(".deployment-use-success");
      await edit(taskB);
      await act(async () => document.querySelector('.deployment-use-feedback input[value="partly"]').click());
      await edit(qualification, ".deployment-use-feedback textarea");
      await reload();
      assert.equal(document.querySelector(".deployment-use-field textarea").value, taskB,
        "The fresh component restores the exact next-job draft from this tab");
      const assertEarlier = () => {
        assert.match(success().textContent, /Your earlier success report was confirmed/);
        assert.equal(success().querySelector(".deployment-use-reported-job p").textContent, taskA);
        assert.doesNotMatch(success().textContent, /Your success report is saved|An earlier success report is saved for this job/);
        assert.ok(button("Choose the next job")); assert.ok(button("Write your own next job"));
        assert.ok(button("It worked"), "Earlier Task A does not affirm current Task B");
        assert.equal(document.querySelector(".deployment-use-field textarea").value, taskB);
        assert.equal(document.querySelector(".deployment-use-feedback textarea").value, qualification);
        assert.equal(document.querySelector('.deployment-use-feedback input[value="partly"]').checked, true);
      };
      assertEarlier();
      assert.equal(calls.length, 2); assert.ok(calls.every(call => call.method === "GET"));
      await reload(); assertEarlier();
      assert.equal(calls.length, 3);
      const original = props().receipt;
      // Same local draft scope still cannot attach a report from another source.
      const changed = kind === "local" ? { ...original, source_digest: other }
        : { ...original, accepted_source_digest: other };
      if (kind === "local") assert.equal(installedUseScopeKey(changed, state), scope);
      await rerender({ receipt: changed });
      assert.doesNotMatch(success()?.textContent || "", /earlier success report was confirmed|Your success report is saved|An earlier success report is saved for this job/);
      await rerender({ receipt: original }); assertEarlier();
      await edit(taskA);
      assert.match(success().textContent, /An earlier success report is saved for this job/);
      assert.equal(success().querySelector(".deployment-use-reported-job p").textContent, taskA);
      assert.equal([...success().querySelectorAll(".deployment-use-reported-job summary")].find(node =>
        node.textContent === "View the earlier reported job") === undefined, true, "The same saved record is not repeated as another job");
      assert.equal(button("It worked") === undefined, true);
      await edit(taskB); assertEarlier();
      assert.equal(calls.length, kind === "local" ? 3 : 5,
        "The unchanged local read key hides mismatched metadata; remote binding changes trigger fresh reads");
      assert.equal(calls.filter(call => call.method === "POST").length, 0);
      assert.equal(uuidCount(), 0); assert.deepEqual(builds, []);
    }, { initialMetadata: { success_report: saved } });
  });

  test(`${kind} long earlier report stays collapsed while the current job draft and next-job actions remain usable`, async () => {
    const prefix = "Task A: assess the supplied observing evidence.\n";
    const suffix = "\nExpected result: a useful observing plan.";
    const taskA = prefix + "e".repeat(3000 - prefix.length - suffix.length) + suffix;
    const taskB = "  Task B: prepare the next observing session.\nKeep my supplied limits.  ";
    const qualification = "  This next job needs a clearer weather explanation.\nKeep this unsent report.  ";
    const saved = response(makeBody(kind, undefined, undefined, undefined, taskA));
    assert.equal(Array.from(taskA).length, 3000);
    assert.ok(saved.task);

    await mounted(kind, async ({ document, calls, builds, edit, button, uuidCount, window }) => {
      await edit(taskB);
      await act(async () => document.querySelector('.deployment-use-feedback input[value="partly"]').click());
      await edit(qualification, ".deployment-use-feedback textarea");
      const success = document.querySelector(".deployment-use-success");
      const details = success.querySelector(".deployment-use-reported-job");
      assert.ok(details, "The exact earlier task has a reported-job disclosure");
      assert.equal(details.open, false, "A long earlier task is collapsed by default");
      assert.equal(details.hasAttribute("open"), false);
      assert.equal(details.querySelector("summary").textContent, "View the earlier reported job");
      assert.equal(details.querySelector("p").textContent, taskA);
      assert.equal(details.parentElement.querySelector(":scope > p").textContent,
        "Your earlier success report was confirmed. Your current task draft remains unchanged.");
      assert.equal(success.querySelector("q") === null, true, "No long task is quoted inline");
      assert.doesNotMatch(success.textContent, /Your success report is saved|An earlier success report is saved for this job/);
      const assertDrafts = () => {
        assert.equal(document.querySelector(".deployment-use-field textarea").value, taskB);
        assert.equal(document.querySelector(".deployment-use-feedback textarea").value, qualification);
        assert.equal(document.querySelector('.deployment-use-feedback input[value="partly"]').checked, true);
      };
      assertDrafts();
      const beforeCalls = calls.length;
      await act(async () => details.querySelector("summary").click());
      assert.equal(details.open, true, "The native summary expands the exact reported task");
      assert.equal(details.querySelector("p").textContent, taskA);
      await act(async () => details.querySelector("summary").click());
      assert.equal(details.open, false);
      const nextJob = button("Choose the next job"), ownJob = button("Write your own next job");
      assert.ok(nextJob); assert.equal(nextJob.disabled, false);
      assert.ok(ownJob); assert.equal(ownJob.disabled, false);
      assert.ok(details.compareDocumentPosition(nextJob) & window.Node.DOCUMENT_POSITION_FOLLOWING);
      await act(async () => nextJob.click());
      assert.equal(document.activeElement === document.querySelector(".deployment-use-ideas button"), true);
      await act(async () => ownJob.click());
      assert.equal(document.activeElement === document.querySelector(".deployment-use-field textarea"), true);
      assertDrafts();
      assert.equal(calls.length, beforeCalls, "Expanding history and choosing next-job actions send no request");
      assert.ok(calls.every(call => call.method === "GET"));
      assert.equal(uuidCount(), 0); assert.deepEqual(builds, []);
    }, { initialMetadata: { success_report: saved } });
  });
}
