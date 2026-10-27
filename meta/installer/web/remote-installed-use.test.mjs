import assert from "node:assert/strict";
import test from "node:test";
import { existsSync } from "node:fs";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
const webDir = existsSync(new URL("package.json", import.meta.url)) ? new URL("./", import.meta.url) : new URL("../../../web/", import.meta.url);
import { createInstalledUseDraftStore, installedUseTaskAllowed } from "./deployment-use.mjs";
import { canonicalRemoteInstalledBinding, remoteInstalledUseBinding, remoteInstalledUseDraftScope,
  createRemoteInstalledUseDraftStore, remoteInstalledUseReadKey, remoteInstalledUseUrl,
  remoteInstalledApplicationUse, remoteInstalledUseFeedback, remoteInstalledUseTaskHandoff,
  createRemoteInstalledUseImprovementStore, remoteInstalledUseImprovementHint } from "./remote-installed-use.mjs";

const state = { id: "11111111-1111-4111-8111-111111111111", version: 4 };
const build = { guide_id: state.id, guide_version: 4, current: true, source_verified: true,
  build_id: "synthetic-accepted-source", source_digest: "a".repeat(64), status: "completed" };
const receipt = { protocol: 1, guide_id: state.id, guide_version: 4,
  deployment_id: "22222222-2222-4222-8222-222222222222", build_id: build.build_id,
  accepted_source_digest: build.source_digest, package_sha256: "b".repeat(64),
  transferred_source_digest: "c".repeat(64), controller_sha256: "d".repeat(64),
  endpoint: { host: "synthetic-linux.local", user: "fixture-user", ssh_port: 22 },
  host: { uid: 1001, home: "/home/fixture-user", destination: "/home/fixture-user/src/observing",
    platform: "linux", python: "3.11.9" }, destination: "/home/fixture-user/src/observing",
  host_key_fingerprint: "SHA256:" + "A".repeat(43), status: "running", admitted: true,
  worker_live: false, observation: { available: true }, source_current: true, ready: true };
const firstUse = { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64),
  tasks: [{ label: "Plan tonight's session", request: "Compare cloud cover and Moon position for tonight.\nSuggest an observing window.",
    description: "Use a real location and observing preferences." }], detail: null };
const noDeclaredUiReceipt = { ...receipt, application_experiences: { protocol: 1, supported: true, status: "none",
  source_digest: receipt.accepted_source_digest, installed_source_digest: receipt.transferred_source_digest,
  metadata_sha256: null, entries: [] } };
const customReceipt = { ...receipt, application_experiences: { protocol: 1, supported: true, status: "resolved",
  source_digest: receipt.accepted_source_digest, installed_source_digest: receipt.transferred_source_digest,
  metadata_sha256: "f".repeat(64), entries: [{ id: "workshop", label: "Observing workspace", path: "/workspace",
    destination_port: 8792, listener_owned: true, listener_token: "9".repeat(64), reason: null, verified: false }] } };
function metadata(r = receipt, s = state, b = build, patch = {}) {
  const available = r.observation?.available === true && r.status !== "observation-unavailable";
  return { protocol: 1, guide_id: s.id, version: s.version, guide_version: r.guide_version,
    deployment_id: r.deployment_id, build_id: r.build_id, accepted_source_digest: r.accepted_source_digest,
    source_digest: r.accepted_source_digest, package_sha256: r.package_sha256,
    transferred_source_digest: r.transferred_source_digest, controller_sha256: r.controller_sha256,
    remote_binding: JSON.parse(remoteInstalledUseBinding(r, s)), scope: "frozen-remote-package-source",
    current: r.guide_version === s.version && r.build_id === b.build_id && r.accepted_source_digest === b.source_digest,
    status: r.status, project: { name: "Observing assistant", description: "An authored observing system." },
    entrypoint: { identity: "lead:main", name: "Observing lead" }, first_use: structuredClone(firstUse),
    observation: { scope: "saved-destination-observation", status: r.status, available,
      freshness: available ? "fresh" : "unavailable", admitted: available && r.admitted === true,
      source_current: available && r.source_current === true },
    installed_source_verified: available && r.admitted === true && r.source_current === true,
    native_runtime_available: available && r.ready === true && r.status === "running",
    application_outcomes_verified: false, continuation: { build_id: b.build_id, source_digest: b.source_digest,
      available: true, requires_acknowledgement: r.build_id !== b.build_id || r.accepted_source_digest !== b.source_digest }, ...patch };
}
const report = { task: firstUse.tasks[0].request, outcome: "partly", request: "Explain which forecast source was used.",
  continueCurrentCandidate: false };
function memoryStorage() {
  const records = new Map();
  return { getItem: (key) => records.get(key) ?? null, setItem: (key, value) => records.set(key, value),
    removeItem: (key) => records.delete(key), entries: () => [...records.entries()] };
}
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};

test("remote identity projects controller host details into the exact frozen source and destination binding", () => {
  const binding = JSON.parse(remoteInstalledUseBinding(receipt, state));
  assert.deepEqual(binding.host, { uid: 1001, home: receipt.host.home, destination: receipt.destination });
  assert.equal(Object.keys(binding).length, 12);
  assert.deepEqual(canonicalRemoteInstalledBinding(binding), binding);
  assert.equal(remoteInstalledUseBinding({ ...receipt, guide_id: "33333333-3333-4333-8333-333333333333" }, state), "");
  const malformed = [
    { endpoint: { ...binding.endpoint, host: {} } }, { endpoint: { ...binding.endpoint, user: 1 } },
    { endpoint: { ...binding.endpoint, ssh_port: "22" } }, { endpoint: { ...binding.endpoint, password: "public-fixture-canary" } },
    { endpoint: { ...binding.endpoint, host: " synthetic-linux.local " } },
    { host: { ...binding.host, uid: true } }, { host: { ...binding.host, home: "/home/../root" } },
    { host: { ...binding.host, home: "/home/fixture-user/" } }, { host: { ...binding.host, destination: "/other" } },
    { destination: "/" }, { accepted_source_digest: "other" }, { controller_sha256: {} },
    { guide_version: 0 }, { guide_id: "other-guide" }, { host_key_fingerprint: "untrusted" },
    { settings: { TEST_API_KEY: "public-fixture-canary" } },
  ];
  for (const patch of malformed) assert.equal(canonicalRemoteInstalledBinding({ ...binding, ...patch }), null);
});

test("frozen first jobs remain available without fresh SSH and never become installed or application result evidence", () => {
  const unavailable = { ...receipt, status: "observation-unavailable", last_known_status: "stopped",
    observation: { available: false } };
  const saved = metadata(unavailable);
  const use = remoteInstalledApplicationUse(saved, unavailable, state, build);
  assert.equal(use.installedVerified, false); assert.equal(use.nativeAvailable, false);
  assert.equal(use.observationAvailable, false); assert.equal(use.startingIdeas.tasks[0].label, firstUse.tasks[0].label);
  assert.ok(remoteInstalledUseFeedback(saved, unavailable, state, build, { ...report, outcome: "blocked" }));
  const stale = metadata(receipt, state, build, { observation: { scope: "saved-destination-observation",
    status: "running", freshness: "stale", available: false, admitted: false, source_current: false },
    installed_source_verified: false, native_runtime_available: false });
  assert.ok(remoteInstalledApplicationUse(stale, receipt, state, build));
  assert.equal(remoteInstalledApplicationUse({ ...stale, installed_source_verified: true }, receipt, state, build), null);
  assert.equal(remoteInstalledApplicationUse({ ...saved, scope: "verified-remote-installed-source" }, unavailable, state, build), null);
  assert.equal(remoteInstalledApplicationUse({ ...saved, application_outcomes_verified: true }, unavailable, state, build), null);
});

test("metadata from a different package, host, guide or candidate cannot authorize feedback", () => {
  const value = metadata();
  for (const patch of [{ version: 5 }, { guide_version: 3 }, { guide_id: "33333333-3333-4333-8333-333333333333" },
    { deployment_id: "33333333-3333-4333-8333-333333333333" }, { build_id: "other-source" },
    { source_digest: "f".repeat(64) }, { transferred_source_digest: "f".repeat(64) },
    { package_sha256: "f".repeat(64) }, { status: "failed" },
    { remote_binding: { ...value.remote_binding, host_key_fingerprint: "SHA256:" + "B".repeat(43) } },
    { remote_binding: { ...value.remote_binding, endpoint: { ...value.remote_binding.endpoint, host: "other.local" } } }]) {
    assert.equal(remoteInstalledApplicationUse({ ...value, ...patch }, receipt, state, build), null);
    assert.equal(remoteInstalledUseFeedback({ ...value, ...patch }, receipt, state, build, report), null);
  }
  for (const candidate of [{ ...build, current: false }, { ...build, source_verified: false },
    { ...build, status: "running" }, { ...build, source_digest: "f".repeat(64) }])
    assert.equal(remoteInstalledUseFeedback(value, receipt, state, candidate, report), null);
});

test("feedback preserves the exact installed remote identity while explicitly improving a newer checked candidate", () => {
  const candidate = { ...build, build_id: "newer-candidate", source_digest: "f".repeat(64) };
  const value = metadata(receipt, state, candidate);
  assert.equal(remoteInstalledUseFeedback(value, receipt, state, candidate, report), null);
  const body = remoteInstalledUseFeedback(value, receipt, state, candidate, { ...report, continueCurrentCandidate: true });
  assert.deepEqual(body, { version: 4, remote_binding: JSON.parse(remoteInstalledUseBinding(receipt, state)),
    continuation_build_id: candidate.build_id, continuation_source_digest: candidate.source_digest,
    continue_current_candidate: true, outcome: report.outcome, request: report.request, task: report.task });
  assert.equal(body.remote_binding.accepted_source_digest, receipt.accepted_source_digest);
  for (const outcome of ["worked", "partly", "not_worked", "blocked"])
    assert.ok(remoteInstalledUseFeedback(metadata(), receipt, state, build, { ...report, outcome }));
  for (const bad of [{ outcome: "verified" }, { outcome: "" }, { request: "" }, { request: "x".repeat(6001) },
    { task: "x".repeat(3001) }])
    assert.equal(remoteInstalledUseFeedback(metadata(), receipt, state, build, { ...report, ...bad }), null);
});

test("active mutations and uncertain previous custody prevent feedback while retaining authored guidance", () => {
  for (const patch of [{ status: "installing" }, { status: "recovery-needed" }, { status: "stopping" },
    { worker_live: true }, { worker_live: "unknown" }, { pending_operation: { id: "pending", action: "retry-install" } },
    { status: "observation-unavailable", last_known_status: "installing", observation: { available: false } }]) {
    const selected = { ...receipt, ...patch }, value = metadata(selected);
    assert.ok(remoteInstalledApplicationUse(value, selected, state, build));
    assert.equal(remoteInstalledUseFeedback(value, selected, state, build, report), null);
  }
  assert.equal(remoteInstalledUseFeedback(metadata(), receipt, state, build, report, true), null);
  for (const pending_operation of [false, 0, "", [], {}]) {
    const selected = { ...receipt, pending_operation };
    assert.equal(remoteInstalledUseFeedback(metadata(selected), selected, state, build, report), null);
    assert.notEqual(remoteInstalledUseReadKey(selected, state, build), remoteInstalledUseReadKey(receipt, state, build));
  }
});

test("shared first-use parser rejects invalid suggestions without blocking a user's own bounded draft", () => {
  for (const first_use of [{ scope: "trial-result", tasks: firstUse.tasks, metadata_sha256: firstUse.metadata_sha256 },
    { ...firstUse, metadata_sha256: null }, { ...firstUse, tasks: [{ label: "Overlong", request: "x".repeat(3001) }] }]) {
    const value = metadata(receipt, state, build, { first_use });
    assert.deepEqual(remoteInstalledApplicationUse(value, receipt, state, build).startingIdeas.tasks, []);
    assert.ok(remoteInstalledUseFeedback(value, receipt, state, build, report));
  }
  assert.equal(installedUseTaskAllowed("\u{1f9ed}".repeat(3000)), true);
  assert.equal(installedUseTaskAllowed("\u{1f9ed}".repeat(3001)), false);
});

test("draft reload uses the whole remote identity and cannot collide with local deployment drafts", () => {
  const storage = memoryStorage(), binding = remoteInstalledUseBinding(receipt, state), scope = remoteInstalledUseDraftScope(receipt, state);
  const drafts = createRemoteInstalledUseDraftStore(storage, binding);
  assert.ok(drafts.update(scope, "task", report.task)); assert.ok(drafts.update(scope, "outcome", report.outcome));
  assert.ok(drafts.update(scope, "improvement", report.request));
  assert.deepEqual(createRemoteInstalledUseDraftStore(storage, binding).read(scope),
    { task: report.task, outcome: report.outcome, improvement: report.request });
  assert.equal(createInstalledUseDraftStore(storage).read(scope).task, "");
  for (const patch of [{ endpoint: { ...receipt.endpoint, host: "other.local" } },
    { host: { ...receipt.host, uid: 1002 } }, { host_key_fingerprint: "SHA256:" + "B".repeat(43) },
    { controller_sha256: "f".repeat(64) }, { transferred_source_digest: "f".repeat(64) },
    { accepted_source_digest: "f".repeat(64) }, { package_sha256: "f".repeat(64) },
    { build_id: "other-build" }, { guide_version: 3 },
    { destination: "/home/fixture-user/src/other", host: { ...receipt.host, destination: "/home/fixture-user/src/other" } }]) {
    const selected = { ...receipt, ...patch };
    assert.equal(createRemoteInstalledUseDraftStore(storage, remoteInstalledUseBinding(selected, state))
      .read(remoteInstalledUseDraftScope(selected, state)).task, "");
  }
  assert.ok(storage.entries().every(([, raw]) => Object.keys(JSON.parse(raw).draft).join(",") === "task,outcome,improvement"));
});

test("draft storage failures keep bounded page drafts without silently truncating prior edits", () => {
  const storage = { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("blocked"); }, removeItem() { throw new Error("blocked"); } };
  const scope = remoteInstalledUseDraftScope(receipt, state), drafts = createRemoteInstalledUseDraftStore(storage, remoteInstalledUseBinding(receipt, state));
  assert.equal(drafts.persistent(scope), false); assert.ok(drafts.update(scope, "task", "Keep this exact draft."));
  assert.equal(drafts.update(scope, "task", "x".repeat(3001)), false);
  assert.equal(drafts.read(scope).task, "Keep this exact draft.");
  assert.equal(drafts.update(scope, "settings", { TEST_API_KEY: "public-fixture-canary" }), false);
});

test("read and feedback URLs bind the selected deployment and read keys reflect candidate and custody changes", () => {
  assert.equal(remoteInstalledUseUrl("guide /1", "destination /2"), "/api/guide/guide%20%2F1/remote-deployment/use?deployment_id=destination%20%2F2");
  assert.equal(remoteInstalledUseUrl(state.id, receipt.deployment_id, "feedback"), "/api/guide/" + state.id + "/remote-deployment/feedback");
  for (const selected of [{ ...receipt, last_known_status: "installing" }, { ...receipt, status: "failed" },
    { ...receipt, observation: { available: false } }, { ...receipt, pending_operation: { id: "attempt", action: "restart" } }])
    assert.notEqual(remoteInstalledUseReadKey(selected, state, build), remoteInstalledUseReadKey(receipt, state, build));
  assert.notEqual(remoteInstalledUseReadKey(receipt, state, { ...build, source_digest: "f".repeat(64) }), remoteInstalledUseReadKey(receipt, state, build));
});

let componentPromise;
function productionComponent() {
  if (!componentPromise) componentPromise = (async () => {
    const require = createRequire(import.meta.url);
    const built = await bundle({ entryPoints: [fileURLToPath(new URL("remote-installed-use.jsx", import.meta.url))],
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
      nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
      plugins: [{ name: "shared-mounted-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
      } }] });
    return import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
  })();
  return componentPromise;
}

async function mountedUse(initial, verify, handlers = {}) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", webDir))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], admissions = [], copied = [], storage = handlers.storage || memoryStorage();
  Object.defineProperty(dom.window, "sessionStorage", { configurable: true, value: storage });
  let props = { state, receipt, build, blocked: false, onRepairReceipt: (value) => {
    admissions.push(value);
    if (handlers.followBuild) {
      props = { ...props, build: value };
      root.render(React.createElement(RemoteInstalledUse, props));
    }
  }, ...initial };
  let get = handlers.get || (() => metadata(props.receipt, props.state, props.build));
  let post = handlers.post || ((call) => ({ guide_id: props.state.id, guide_version: props.state.version,
    current: true, build_id: "synthetic-builder-improvement", status: "queued",
    request: "User's requested improvement:\n" + call.body.request + "\n\nSynthetic host context: " + JSON.stringify(call.body) }));
  let observe = handlers.observe || (() => props.build);
  let write = handlers.write || ((value) => copied.push(value));
  Object.defineProperty(dom.window.navigator, "clipboard", { configurable: true, value: { writeText: (value) => write(value) } });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: storage, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (target, init = {}) => {
      const call = { target, method: init.method || "GET", body: init.body ? JSON.parse(init.body) : null };
      calls.push(call);
      if (target === `/api/guide/${encodeURIComponent(props.state.id)}/implementation`) {
        assert.equal(call.method, "GET");
        const value = await observe(call);
        return { ok: true, json: async () => structuredClone(value) };
      }
      assert.ok([remoteInstalledUseUrl(props.state.id, props.receipt.deployment_id),
        remoteInstalledUseUrl(props.state.id, props.receipt.deployment_id, "feedback")].includes(target));
      if (call.method === "POST" && handlers.respondPost) {
        const response = await handlers.respondPost(call);
        if (response !== undefined) return response;
      }
      const value = await (call.method === "GET" ? get(call) : post(call));
      return { ok: true, json: async () => structuredClone(value) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client"), { RemoteInstalledUse } = await productionComponent();
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(RemoteInstalledUse, props)));
  const document = dom.window.document;
  const button = (label) => [...document.querySelectorAll("button")].find((node) => node.textContent === label);
  const task = () => document.querySelector(".deployment-use-field textarea");
  const form = () => document.querySelector("form");
  async function edit(field, value) {
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(field, value); field.dispatchEvent(new dom.window.Event("input", { bubbles: true })); });
  }
  async function click(label) { const selected = button(label); assert.ok(selected, label); await act(async () => selected.click()); }
  try {
    await render();
    await verify({ document, window: dom.window, calls, admissions, copied, storage, button, task, form, edit, click,
      async fill(outcome = "partly", request = report.request) {
        await edit(form().querySelector("textarea"), request);
        await act(async () => form().querySelector('input[value="' + outcome + '"]').click());
      },
      async rerender(patch) { props = { ...props, ...patch }; await render(); },
      async settle(action) { await act(async () => { action?.(); await Promise.resolve(); }); },
      setRead(handler) { get = handler; }, setPost(handler) { post = handler; }, setWrite(handler) { write = handler; },
      setBuildRead(handler) { observe = handler; },
    });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test("production mount only reads explicit remote guidance; choosing and copying an authored job does not send or open it", async () => {
  await mountedUse({}, async ({ document, calls, copied, task, click }) => {
    assert.deepEqual(calls.map((call) => [call.method, call.target]), [["GET", remoteInstalledUseUrl(state.id, receipt.deployment_id)]]);
    assert.equal(document.querySelector("a,iframe") === null, true);
    assert.match(document.body.textContent, /Suggestions from this installed version/);
    const idea = document.querySelector(".deployment-use-ideas button");
    await act(async () => idea.click());
    assert.equal(task().value, firstUse.tasks[0].request);
    await click("Copy task"); assert.deepEqual(copied, [firstUse.tasks[0].request]);
    assert.equal(calls.length, 1); assert.match(document.body.textContent, /Task copied\. Use it after the application's everyday entrypoint has been checked/);
    assert.equal(document.querySelector(".deployment-use-feedback-disclosure").open, false);
    assert.match(document.body.textContent, /manual report/); assert.match(document.body.textContent, /has not observed this application job or verified its result/);
  });
});

test("manual outcome and improvement explicitly submit the exact remote feedback once without applying it to Linux", async () => {
  await mountedUse({}, async ({ document, calls, admissions, task, form, edit, fill, click }) => {
    await edit(task(), "  A real job with my context.\nPlease preserve it.  ");
    await edit(form().querySelector("textarea"), "  The page did not show a forecast.  ");
    assert.equal(buttonDisabled(form()), true); assert.match(form().textContent, /Choose what happened above before sending/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    await fill("not_worked", "  The page did not show a forecast.  ");
    assert.equal(buttonDisabled(form()), false);
    await click("Ask builder to improve it");
    const posts = calls.filter((call) => call.method === "POST"); assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, remoteInstalledUseFeedback(metadata(), receipt, state, build,
      { task: task().value, outcome: "not_worked", request: form().querySelector("textarea").value }));
    assert.equal(admissions.length, 1); assert.equal(admissions[0].status, "queued");
    await click("Ask builder to improve it");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.match(document.body.textContent, /This installation has not changed/);
    assert.equal(task().value, "  A real job with my context.\nPlease preserve it.  ");
    assert.equal(posts[0].target.endsWith("/feedback"), true);
  });
});

test("an explicitly rejected remote report refreshes guidance and survives reload without locking or resending", async () => {
  for (const status of [409, 422]) {
    const storage = memoryStorage();
    await mountedUse({}, async ({ document, calls, fill, click, form, task, edit, admissions }) => {
      await edit(task(), "Keep my useful job."); await fill(); await click("Ask builder to improve it");
      assert.match(document.body.textContent, /builder did not receive this report/);
      assert.doesNotMatch(document.body.textContent, /Admission.*unconfirmed/);
      assert.equal(admissions.length, 0); assert.equal(task().value, "Keep my useful job.");
      assert.equal(form().querySelector("textarea").value, report.request);
      assert.equal(buttonDisabled(form()), false);
      assert.equal(calls.filter(call => call.method === "POST").length, 1);
      assert.equal(calls.filter(call => call.target === remoteInstalledUseUrl(state.id, receipt.deployment_id) && call.method === "GET").length, 2);
      assert.ok(![...document.querySelectorAll("button")].some(node => node.textContent === "Check Build status"));
    }, { storage, respondPost: () => ({ ok: false, status,
      json: async () => ({ detail: "Synthetic rejection before builder admission.", code: "installed_feedback_not_admitted" }) }) });
    await mountedUse({}, async ({ calls, form, task, click, admissions }) => {
      assert.equal(task().value, "Keep my useful job."); assert.equal(form().querySelector("textarea").value, report.request);
      assert.equal(buttonDisabled(form()), false); assert.equal(calls.filter(call => call.method === "POST").length, 0);
      await click("Ask builder to improve it");
      assert.equal(admissions.length, 1); assert.equal(calls.filter(call => call.method === "POST").length, 1);
    }, { storage });
  }
});

test("an uncoded remote409 or422 cannot unlock an unknown report on the unchanged predecessor", async () => {
  for (const status of [409, 422]) await mountedUse({}, async ({ document, calls, fill, click, form }) => {
    await fill(); await click("Ask builder to improve it");
    assert.match(document.body.textContent, /could not be confirmed/);
    await click("Check Build status"); await click("Ask builder to improve it");
    assert.equal(buttonDisabled(form()), true);
    assert.ok(![...document.querySelectorAll("button")].some(node => node.textContent === "Prepare another report"));
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  }, { respondPost: () => ({ ok: false, status, json: async () => ({ detail: "Synthetic uncertain response." }) }) });
});

test("a rejected remote report waits for fresh guidance and never sends itself", async () => {
  const pending = deferred(); let reads = 0;
  await mountedUse({}, async ({ document, calls, fill, click, form, settle }) => {
    await fill(); await click("Ask builder to improve it");
    assert.equal(buttonDisabled(form()), true);
    assert.match(document.body.textContent, /Loading this saved package/);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    await settle(() => pending.resolve(metadata()));
    assert.equal(buttonDisabled(form()), false);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  }, { get: () => ++reads === 1 ? metadata() : pending.promise,
    respondPost: () => ({ ok: false, status: 409,
      json: async () => ({ code: "installed_feedback_not_admitted", detail: "Synthetic busy operation." }) }) });
});

test("failed removal of rejected remote tracking keeps drafts and can be retried without another POST", async () => {
  const backing = memoryStorage(); let removable = false;
  const storage = { ...backing, removeItem(key) { if (removable) backing.removeItem(key); } };
  await mountedUse({}, async ({ document, calls, fill, click, form }) => {
    await fill(); await click("Ask builder to improve it");
    assert.match(document.body.textContent, /Saved request tracking could not be cleared/);
    assert.equal(buttonDisabled(form()), true);
    assert.equal(form().querySelector("textarea").value, report.request);
    await click("Retry clearing report tracking");
    assert.equal(buttonDisabled(form()), true); assert.equal(calls.filter(call => call.method === "POST").length, 1);
    removable = true; await click("Retry clearing report tracking");
    assert.equal(buttonDisabled(form()), false); assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.match(document.body.textContent, /builder did not receive this report/);
  }, { storage, respondPost: () => ({ ok: false, status: 409,
    json: async () => ({ code: "installed_feedback_not_admitted", detail: "Synthetic busy operation." }) }) });
});
const buttonDisabled = (form) => form.querySelector('button[type="submit"]').disabled;

test("the exact admitted remote improvement survives candidate refresh and exposes completion without applying it", async () => {
  await mountedUse({}, async ({ document, calls, fill, click, rerender, admissions }) => {
    await fill(); await click("Ask builder to improve it");
    const candidate = { ...build, build_id: admissions[0].build_id, source_digest: "f".repeat(64),
      ready_to_package: true, acceptance_trusted: true, acceptance_current: true,
      acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "f".repeat(64), status: "pass", tests_run: 3 } };
    const handoff = () => document.querySelector(".installed-improvement-handoff");
    await rerender({ build: { ...candidate, status: "checking", ready_to_package: false } });
    assert.match(handoff().textContent, /Your improvement is being built/);
    await rerender({ build: candidate });
    assert.match(handoff().textContent, /Your improvement is ready to review/);
    assert.equal(handoff().querySelector("a").getAttribute("href"), "#setup-implementation");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.ok(calls.every(call => !call.target.includes("/apply") && !call.target.includes("/install")));
    await rerender({ build: { ...candidate, ready_to_package: false,
      acceptance: { ...candidate.acceptance, status: "needs_repair" } } });
    assert.match(handoff().textContent, /Your improvement needs more work/);
    assert.ok(handoff().querySelector("a"));
    await rerender({ build: { ...candidate, build_id: "another-report-candidate" } });
    assert.doesNotMatch(handoff().textContent, /ready to review|needs more work/);
    assert.equal(handoff().querySelector("a") === null, true);
    await rerender({ receipt: { ...receipt, host_key_fingerprint: "SHA256:" + "B".repeat(43) } });
    assert.equal(handoff() === null, true, "Another destination must not inherit this report's admission.");
  });
});

test("polling the admitted remote candidate before its POST reply retains completion without replacing the selection", async () => {
  const pending = deferred();
  await mountedUse({}, async ({ document, calls, fill, click, rerender, settle, admissions, form }) => {
    await fill(); await click("Ask builder to improve it");
    const candidate = { ...build, build_id: "delayed-confirmed-improvement", source_digest: "f".repeat(64),
      ready_to_package: true, acceptance_trusted: true, acceptance_current: true,
      acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "f".repeat(64), status: "pass", tests_run: 3 } };
    await rerender({ build: candidate });
    await settle(() => pending.resolve({ guide_id: state.id, guide_version: state.version,
      current: true, build_id: candidate.build_id, status: "queued" }));
    const handoff = document.querySelector(".installed-improvement-handoff");
    assert.match(handoff.textContent, /Your improvement is ready to review/);
    assert.equal(handoff.querySelector("a").getAttribute("href"), "#setup-implementation");
    assert.deepEqual(admissions, [], "The older queued reply must not replace the completed candidate.");
    assert.equal(buttonDisabled(form()), true);
    assert.doesNotMatch(document.body.textContent, /could not be confirmed|not confirmed|setup changed/);
    assert.equal(document.querySelector(".deployment-error") === null, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  }, { post: () => pending.promise });
});

test("remote improvement hints stay isolated by every installed destination and source pin", () => {
  const storage = memoryStorage(), binding = remoteInstalledUseBinding(receipt, state), scope = remoteInstalledUseDraftScope(receipt, state);
  const body = remoteInstalledUseFeedback(metadata(), receipt, state, build, report);
  const hint = remoteInstalledUseImprovementHint(receipt, state, body);
  const store = createRemoteInstalledUseImprovementStore(storage, binding);
  assert.ok(store.save(scope, hint));
  assert.deepEqual(createRemoteInstalledUseImprovementStore(storage, binding).read(scope), hint);
  for (const change of [{ host_key_fingerprint: "SHA256:" + "B".repeat(43) }, { accepted_source_digest: "f".repeat(64) },
    { package_sha256: "f".repeat(64) }, { controller_sha256: "f".repeat(64) }, { transferred_source_digest: "f".repeat(64) },
    { endpoint: { ...receipt.endpoint, host: "another-host.local" } }, { build_id: "other-install" },
    { deployment_id: "33333333-3333-4333-8333-333333333333" }]) {
    const other = { ...receipt, ...change };
    assert.equal(createRemoteInstalledUseImprovementStore(storage, remoteInstalledUseBinding(other, state))
      .read(remoteInstalledUseDraftScope(other, state)), null);
  }
  assert.equal(remoteInstalledUseImprovementHint(receipt, state, { ...body, remote_binding: { ...body.remote_binding,
    host_key_fingerprint: "SHA256:" + "B".repeat(43) } }), null);
});

test("a remote reload restores only the exact fresh host admission and preserves the human report", async () => {
  const storage = memoryStorage(); let admitted;
  await mountedUse({}, async ({ fill, click, admissions }) => {
    await fill(); await click("Ask builder to improve it"); admitted = admissions[0];
  }, { storage });
  const candidate = { ...build, build_id: admitted.build_id, request: admitted.request, source_digest: "f".repeat(64),
    ready_to_package: true, acceptance_trusted: true, acceptance_current: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "f".repeat(64), status: "pass", tests_run: 3 } };
  await mountedUse({ build: candidate }, async ({ document, calls, form, click }) => {
    assert.equal(calls.filter(call => call.target.endsWith("/implementation")).length, 1);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    assert.match(document.body.textContent, /Your improvement is ready to review/);
    assert.equal(document.querySelector(".installed-improvement-handoff a").getAttribute("href"), "#setup-implementation");
    assert.equal(form().querySelector("textarea").value, report.request);
    assert.equal(form().querySelector('input[value="partly"]').checked, true);
    await click("Prepare another report");
    await act(async () => form().querySelector('input[type="checkbox"]').click());
    assert.equal(buttonDisabled(form()), true);
    assert.match(document.body.textContent, /Change your report before sending/);
  }, { storage, observe: () => candidate });
});

test("remote reload tracking cannot confirm a replacement, wrong host report or failed observation", async () => {
  const storage = memoryStorage(); let admitted;
  await mountedUse({}, async ({ fill, click, admissions }) => {
    await fill(); await click("Ask builder to improve it"); admitted = admissions[0];
  }, { storage });
  for (const change of [{ build_id: "replacement" }, { request: "A report from another source" },
    { guide_version: state.version + 1 }, { guide_id: "foreign" }, null]) {
    const candidate = { ...build, build_id: admitted.build_id, request: admitted.request, ...change };
    await mountedUse({ build: candidate }, async ({ document, calls, form, window }) => {
      assert.equal(document.querySelector(".installed-improvement-handoff") === null, true);
      assert.doesNotMatch(document.body.textContent, /ready to review|request for proposal \d+ was admitted/);
      assert.equal(buttonDisabled(form()), true);
      assert.equal(form().querySelector("textarea").value, report.request);
      await act(async () => form().dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })));
      assert.equal(calls.filter(call => call.method === "POST").length, 0);
    }, { storage, observe() { if (change === null) throw new TypeError("Synthetic unavailable read."); return candidate; } });
  }
});

test("remote reload keeps its improvement association when the parent immediately adopts the recovered candidate", async () => {
  const storage = memoryStorage(); let admitted;
  await mountedUse({}, async ({ fill, click, admissions }) => {
    await fill(); await click("Ask builder to improve it"); admitted = admissions[0];
  }, { storage });
  const candidate = { ...build, build_id: admitted.build_id, request: admitted.request, source_digest: "f".repeat(64),
    ready_to_package: true, acceptance_trusted: true, acceptance_current: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "f".repeat(64), status: "pass", tests_run: 3 } };
  await mountedUse({}, async ({ document, admissions, calls }) => {
    assert.match(document.body.textContent, /Your improvement is ready to review/);
    assert.equal(admissions.length, 1); assert.equal(admissions[0].build_id, candidate.build_id);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
  }, { storage, observe: () => candidate, followBuild: true });
});

test("a pending remote POST retains its exact late confirmation without publishing into a remounted view", async () => {
  const storage = memoryStorage(), pending = deferred();
  let originalHint;
  await mountedUse({}, async ({ fill, click, document }) => {
    await fill(); await click("Ask builder to improve it");
    assert.match(document.body.textContent, /Waiting for admission/);
    const tracked = storage.entries().find(([key]) => key.includes("mobkit-installed-use-improvement-v1:"));
    assert.ok(tracked); originalHint = JSON.parse(tracked[1]).hint;
    assert.equal(originalHint.buildId, null);
  }, { storage, post: () => pending.promise });
  await mountedUse({}, async ({ document, calls, form, settle, window }) => {
    await settle(() => pending.resolve({ guide_id: state.id, guide_version: state.version,
      build_id: "old-mounted-reply", current: true, status: "queued", request: "A saved host request." }));
    assert.match(document.body.textContent, /Admission.*unconfirmed/);
    assert.equal(document.querySelector(".installed-improvement-handoff") === null, true);
    assert.equal(buttonDisabled(form()), true);
    await act(async () => form().dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    const tracked = storage.entries().find(([key]) => key.includes("mobkit-installed-use-improvement-v1:"));
    assert.deepEqual(JSON.parse(tracked[1]).hint, { ...originalHint,
      buildId: "old-mounted-reply", hostRequest: "A saved host request." },
    "The exact original stored report is confirmed without publishing into the remounted view.");
  }, { storage });
});

test("a changed remote destination cannot consume a delayed recovery read or inherit its report link", async () => {
  const storage = memoryStorage(), pending = deferred(); let admitted;
  await mountedUse({}, async ({ fill, click, admissions }) => {
    await fill(); await click("Ask builder to improve it"); admitted = admissions[0];
  }, { storage });
  const candidate = { ...build, build_id: admitted.build_id, request: admitted.request };
  await mountedUse({ build: candidate }, async ({ document, admissions, calls, rerender, settle, form }) => {
    await rerender({ receipt: { ...receipt, host_key_fingerprint: "SHA256:" + "B".repeat(43) } });
    await settle(() => pending.resolve(candidate));
    assert.equal(document.querySelector(".installed-improvement-handoff") === null, true);
    assert.equal(document.querySelector(".deployment-use-submitted") === null, true);
    assert.equal(form().querySelector("textarea").value, "");
    assert.equal(admissions.length, 0);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
  }, { storage, observe: () => pending.promise });
});

test("remote reload uses fresh source, acceptance and cleanup instead of saved readiness", async () => {
  const storage = memoryStorage(); let admitted;
  await mountedUse({}, async ({ fill, click, admissions }) => {
    await fill(); await click("Ask builder to improve it"); admitted = admissions[0];
  }, { storage });
  const accepted = { ...build, build_id: admitted.build_id, request: admitted.request, source_digest: "f".repeat(64),
    ready_to_package: true, acceptance_trusted: true, acceptance_current: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "f".repeat(64), status: "pass", tests_run: 3 } };
  for (const change of [{ cleanup_pending: true }, { source_verified: false }, { ready_to_package: false },
    { acceptance_trusted: false }, { acceptance: { ...accepted.acceptance, status: "needs_repair" } }]) {
    const candidate = { ...accepted, ...change };
    await mountedUse({ build: candidate }, async ({ document, calls, form }) => {
      assert.ok(document.querySelector(".installed-improvement-handoff"));
      assert.doesNotMatch(document.body.textContent, /ready to review/);
      assert.equal(buttonDisabled(form()), true);
      assert.equal(calls.filter(call => call.method === "POST").length, 0);
    }, { storage, observe: () => candidate });
  }
});

test("lost remote admission remains uncertain through reload and a changed settled candidate cannot resend the old report", async () => {
  const storage = memoryStorage();
  await mountedUse({}, async ({ fill, click, document }) => {
    await fill(); await click("Ask builder to improve it"); assert.match(document.body.textContent, /could not be confirmed/);
  }, { storage, post() { throw new TypeError("Synthetic lost reply."); } });
  await mountedUse({}, async ({ document, calls, form, rerender, click, window }) => {
    assert.match(document.body.textContent, /Admission.*unconfirmed/);
    assert.equal(buttonDisabled(form()), true);
    assert.ok(![...document.querySelectorAll("button")].some(node => node.textContent === "Prepare another report"));
    const candidate = { ...build, build_id: "later-settled-work", source_digest: "f".repeat(64) };
    await rerender({ build: candidate }); await click("Check Build status");
    assert.match(document.body.textContent, /does not confirm whether your earlier report was admitted/);
    await click("Prepare another report");
    await act(async () => form().querySelector('input[type="checkbox"]').click());
    await act(async () => form().dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(buttonDisabled(form()), true);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    assert.match(document.body.textContent, /Change your report before sending/);
  }, { storage });
});

test("denied remote improvement storage remains usable in the open page with a clear reload limitation", async () => {
  const storage = { getItem() { throw new Error("PRIVATE_STORAGE_DETAIL"); }, setItem() { throw new Error("QUOTA_DETAIL"); }, removeItem() {} };
  await mountedUse({}, async ({ document, fill, click, admissions }) => {
    await fill(); await click("Ask builder to improve it");
    assert.equal(admissions.length, 1);
    assert.match(document.body.textContent, /Improvement tracking is available in this open page only/);
    assert.doesNotMatch(document.body.textContent, /PRIVATE_STORAGE_DETAIL|QUOTA_DETAIL/);
  }, { storage });
});

test("a remote parent display callback failure cannot turn a confirmed feedback admission into an unknown request", async () => {
  let callbacks = 0;
  await mountedUse({ onRepairReceipt() { callbacks++; throw new Error("Synthetic parent display failure."); } },
    async ({ document, calls, fill, click, form }) => {
      await fill(); await click("Ask builder to improve it");
      assert.equal(callbacks, 1);
      assert.equal(document.querySelector(".installed-improvement-handoff").dataset.buildId, "synthetic-builder-improvement");
      assert.match(document.body.textContent, /was admitted/);
      assert.doesNotMatch(document.body.textContent, /could not be confirmed|not confirmed/);
      assert.equal(document.querySelector(".deployment-error") === null, true);
      assert.equal(buttonDisabled(form()), true);
      await click("Ask builder to improve it");
      assert.equal(calls.filter(call => call.method === "POST").length, 1);
    });
});

test("stopped, failed and unavailable destinations keep frozen suggestions and allow a manual blocked-setup report", async () => {
  for (const status of ["stopped", "failed", "observation-unavailable"]) {
    const selected = { ...receipt, status, ready: false, observation: { available: status !== "observation-unavailable" }, last_known_status: "failed" };
    await mountedUse({ receipt: selected }, async ({ document, calls, fill, click, admissions }) => {
      assert.ok(document.querySelector(".deployment-use-ideas button"));
      assert.equal(document.querySelector("a") === null, true);
      await fill("blocked", "The setup stopped before I could sign in.");
      assert.match(document.body.textContent, /report a blocked setup even if you could not try the job/);
      await click("Ask builder to improve it");
      assert.equal(calls.filter((call) => call.method === "POST").length, 1); assert.equal(admissions.length, 1);
      assert.doesNotMatch(document.body.textContent, /workflow verified|sign-in verified|useful task completed/);
    });
  }
});

test("old remote source requires a fresh explicit acknowledgement for the selected candidate", async () => {
  const candidate = { ...build, build_id: "candidate-B", source_digest: "f".repeat(64) };
  await mountedUse({ build: candidate }, async ({ document, window, form, fill, click, calls, rerender }) => {
    await fill(); assert.equal(buttonDisabled(form()), true);
    const acknowledgement = form().querySelector('input[type="checkbox"]'); assert.ok(acknowledgement);
    assert.equal(acknowledgement.checked, false);
    await act(async () => acknowledgement.click());
    assert.equal(buttonDisabled(form()), false); await click("Ask builder to improve it");
    assert.equal(calls.find((call) => call.method === "POST").body.continue_current_candidate, true);
    await rerender({ build: { ...candidate, source_digest: "9".repeat(64) } });
    assert.equal(form().querySelector('input[type="checkbox"]').checked, false);
    assert.equal(form().querySelector("textarea").value, report.request);
    assert.equal(buttonDisabled(form()), true);
    assert.match(window.document.body.textContent, /This installation stays unchanged/);
  });
});

test("losing a feedback response requires a checked settled candidate and a changed report before another POST", async () => {
  await mountedUse({}, async ({ document, calls, task, edit, fill, click, form, admissions, setPost, rerender }) => {
    await edit(task(), "Keep my first job."); await fill("blocked", "The application page did not respond.");
    await click("Ask builder to improve it");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1); assert.equal(admissions.length, 0);
    assert.equal(task().value, "Keep my first job."); assert.equal(form().querySelector("textarea").value, "The application page did not respond.");
    assert.match(document.body.textContent, /Check Build before retrying/); assert.equal(buttonDisabled(form()), true);
    await click("Ask builder to improve it"); assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    setPost(() => ({ guide_id: state.id, guide_version: state.version, current: true, build_id: "confirmed-next", status: "queued" }));
    await click("Check Build status");
    assert.equal(calls.at(-1).method, "GET"); assert.equal(calls.at(-1).target.endsWith("/implementation"), true);
    assert.equal(buttonDisabled(form()), true, "The previous build cannot resolve an uncertain admission.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await rerender({ build: { ...build, build_id: "observed-settled-repair", source_digest: "f".repeat(64) } });
    await click("Check Build status"); await click("Prepare another report");
    await act(async () => form().querySelector('input[type="checkbox"]').click());
    assert.equal(buttonDisabled(form()), true, "The unchanged report must not be resent.");
    await fill("partly", "A separate new report: retain the useful result and explain its source.");
    assert.equal(buttonDisabled(form()), false);
    await click("Ask builder to improve it"); assert.equal(admissions.at(-1).build_id, "confirmed-next");
  }, { post() { throw new Error("Synthetic lost POST response - no destination was contacted."); } });
});

test("receipt and unrelated candidate changes retain a known admission without publishing the late response", async () => {
  const pending = deferred();
  await mountedUse({}, async ({ document, calls, fill, click, form, rerender, settle, admissions }) => {
    await fill(); await click("Ask builder to improve it");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await rerender({ receipt: { ...receipt, status: "stopped", ready: false } });
    assert.equal(buttonDisabled(form()), true);
    await click("Sending your improvement request...");
    const candidate = { ...build, build_id: "candidate-B", source_digest: "f".repeat(64) };
    await rerender({ build: candidate });
    assert.equal(buttonDisabled(form()), true); assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await settle(() => pending.resolve({ guide_id: state.id, guide_version: 4, current: true, build_id: "late-admission", status: "queued" }));
    assert.equal(admissions.length, 0); assert.equal(buttonDisabled(form()), true);
    const handoff = document.querySelector(".installed-improvement-handoff");
    assert.equal(handoff.dataset.buildId, "late-admission");
    assert.match(handoff.textContent, /was admitted/);
    assert.equal(handoff.querySelector("a") === null, true);
    assert.doesNotMatch(document.body.textContent, /ready to review|could not be confirmed|setup changed/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { post: () => pending.promise });
});

test("out-of-order guidance and a changed destination cannot publish another remote owner's authored jobs or reports", async () => {
  const pending = deferred(); let first = true;
  await mountedUse({}, async ({ document, task, edit, rerender, settle, calls, admissions }) => {
    await edit(task(), "Old host draft.");
    const selected = { ...receipt, endpoint: { ...receipt.endpoint, host: "new-host.local" } };
    await rerender({ receipt: selected });
    assert.equal(task().value, ""); assert.match(document.body.textContent, /Plan tonight's session/);
    await settle(() => pending.resolve(metadata(receipt, state, build,
      { project: { name: "STALE OWNER", description: "Stale." }, first_use: { ...firstUse,
        tasks: [{ label: "STALE JOB", request: "Do not publish this stale job." }] } })));
    assert.doesNotMatch(document.body.textContent, /STALE OWNER|STALE JOB/);
    assert.equal(task().value, ""); assert.ok(calls.every((call) => call.method === "GET")); assert.equal(admissions.length, 0);
  }, { get(call) { if (first) { first = false; return pending.promise; }
    return metadata({ ...receipt, endpoint: { ...receipt.endpoint, host: "new-host.local" } }); } });
});

test("an unmounted remote owner cannot publish its in-flight builder receipt", async () => {
  const pending = deferred();
  await mountedUse({}, async ({ document, fill, click, rerender, settle, admissions, task, calls }) => {
    await fill(); await click("Ask builder to improve it");
    await rerender({ receipt: { ...receipt, host_key_fingerprint: "SHA256:" + "B".repeat(43) } });
    assert.equal(task().value, "");
    await settle(() => pending.resolve({ guide_id: state.id, guide_version: 4, current: true, build_id: "wrong-owner", status: "queued" }));
    assert.equal(admissions.length, 0); assert.doesNotMatch(document.body.textContent, /Your report was sent/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { post: () => pending.promise });
});

test("mounted reload retains human drafts for the exact remote owner and nothing from private setup", async () => {
  const storage = memoryStorage();
  await mountedUse({}, async ({ task, edit, fill }) => { await edit(task(), "My reload draft."); await fill("worked", "A human report only."); }, { storage });
  await mountedUse({ receipt: { ...receipt, status: "failed", ready: false } }, async ({ task, form, calls }) => {
    assert.equal(task().value, "My reload draft."); assert.equal(form().querySelector("textarea").value, "A human report only.");
    assert.equal(form().querySelector('input[value="worked"]').checked, true);
    assert.ok(calls.every((call) => call.method === "GET"));
  }, { storage });
  for (const [, raw] of storage.entries()) {
    const saved = JSON.parse(raw); assert.deepEqual(Object.keys(saved.draft), ["task", "outcome", "improvement"]);
    assert.doesNotMatch(raw, /TEST_API_KEY|password|settings|local_url/);
  }
});

test("production copy failure offers selection, and an over-limit edit preserves the exact prior task", async () => {
  await mountedUse({}, async ({ document, task, edit, click, calls }) => {
    await edit(task(), "Keep this task exactly."); await edit(task(), "\u{1f9ed}".repeat(3001));
    assert.equal(task().value, "Keep this task exactly.");
    assert.match(document.body.textContent, /Your previous draft remains here/);
    await click("Copy task"); await click("Select task to copy");
    assert.equal(task().selectionStart, 0); assert.equal(task().selectionEnd, task().value.length);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  }, { write() { throw new Error("Synthetic clipboard refusal."); } });
});

test("mounted remote pending clipboard copy offers selection of the current task without stale copied feedback", async () => {
  const pending = deferred(), copied = [];
  let completed = false;
  await mountedUse({}, async ({ document, task, edit, click, calls, admissions, button, settle }) => {
    const original = "  Original remote job.\nKeep these limits.  ", current = "  Edited \u{1f9ed} remote job.\nKeep every character.  ";
    await edit(task(), original); await click("Copy task");
    assert.equal(button("Copying task...").disabled, true);
    assert.equal(button("Select task to copy").disabled, false);
    assert.match(document.querySelector(".deployment-use-copy [role='status']").textContent, /Clipboard copy is still pending/);
    assert.doesNotMatch(document.body.textContent, /Task copied|could not be copied/);
    assert.equal(task().disabled, false);
    await edit(task(), current); await click("Select task to copy");
    assert.equal(document.activeElement === task(), true);
    assert.equal(task().value, current);
    assert.equal(task().selectionStart, 0); assert.equal(task().selectionEnd, current.length);
    assert.equal(completed, false, "Manual selection leaves the original clipboard write pending");
    assert.deepEqual(copied, [original]);
    assert.equal(button("Copying task...").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0); assert.deepEqual(admissions, []);
    await settle(() => pending.resolve());
    assert.equal(completed, true);
    assert.equal(task().value, current);
    assert.equal(button("Copy task").disabled, false);
    assert.equal(button("Copying task...") === undefined, true);
    assert.equal(button("Select task to copy") === undefined, true);
    assert.doesNotMatch(document.body.textContent, /Task copied|Clipboard copy is still pending|could not be copied/);
    assert.deepEqual(copied, [original]);
  }, { write: async (value) => { copied.push(value); await pending.promise; completed = true; } });
});

test("bounded continuation reasons explain recovery gating without interpreting arbitrary objects or clearing reports", async () => {
  const selected = { ...receipt, status: "recovery-needed" };
  const reason = "Finish the owned remote operation or resolve its recovery state before asking the builder to improve the application.";
  await mountedUse({ receipt: selected }, async ({ document, task, edit, fill, form, calls }) => {
    await edit(task(), "Please keep this job."); await fill("blocked", "Cleanup is uncertain.");
    assert.equal(buttonDisabled(form()), true); assert.match(document.body.textContent, /resolve its recovery state/);
    assert.equal(task().value, "Please keep this job."); assert.equal(calls.length, 1);
  }, { get: () => metadata(selected, state, build, { continuation: { build_id: build.build_id,
    source_digest: build.source_digest, available: false, requires_acknowledgement: false, reason } }) });
  for (const reason of [{ toString: "unsafe" }, "x".repeat(601), "Control\u0000text"])
    assert.equal(remoteInstalledApplicationUse(metadata(receipt, state, build,
      { continuation: { build_id: build.build_id, source_digest: build.source_digest, available: false, reason } }), receipt, state, build).continuation.reason, null);
});

test("a falsey pending-custody mutation immediately withdraws old feedback eligibility while its metadata recheck is pending", async () => {
  const pending = deferred();
  await mountedUse({}, async ({ calls, form, fill, rerender, setRead, settle, click }) => {
    await fill(); assert.equal(buttonDisabled(form()), false);
    setRead(() => pending.promise);
    const selected = { ...receipt, pending_operation: false };
    await rerender({ receipt: selected });
    assert.equal(buttonDisabled(form()), true);
    await click("Ask builder to improve it"); assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    await settle(() => pending.resolve(metadata(selected, state, build, { continuation: { build_id: build.build_id,
      source_digest: build.source_digest, available: false, requires_acknowledgement: false,
      reason: "Finish the owned remote operation or resolve its recovery state before asking the builder to improve the application." } })));
    assert.equal(buttonDisabled(form()), true); assert.equal(form().querySelector("textarea").value, report.request);
  });
});

test("production first-job panel does not mount or read authored jobs before a remote installation is admitted", async () => {
  for (const status of ["connecting", "configuration-required", "not-installed", "failed"]) {
    const selected = { ...noDeclaredUiReceipt, status, admitted: false, ready: false };
    await mountedUse({ receipt: selected }, async ({ document, calls }) => {
      assert.equal(document.querySelector(".remote-installed-use") === null, true);
      assert.equal(document.body.textContent, ""); assert.deepEqual(calls, []);
    });
  }
  await mountedUse({ receipt: { ...noDeclaredUiReceipt, status: "configuration-required", admitted: undefined } },
    async ({ document, calls }) => { assert.equal(document.querySelector(".remote-installed-use") === null, true); assert.deepEqual(calls, []); });
});

test("source-joined missing optional interface declarations expose a native contact without identifying the application's human channel", () => {
  const use = remoteInstalledApplicationUse(metadata(noDeclaredUiReceipt), noDeclaredUiReceipt, state, build);
  assert.deepEqual(use.entrypoint, { identity: "lead:main", name: "Observing lead" });
  assert.equal(use.interfaces.status, "none"); assert.equal(use.interfaces.supported, true);
  assert.equal(use.noDeclaredUi, true);
  assert.equal("consoleOnly" in use, false); assert.equal("humanChannel" in use, false);
  for (const application_experiences of [undefined, { ...noDeclaredUiReceipt.application_experiences, supported: false },
    { ...noDeclaredUiReceipt.application_experiences, status: "unavailable" },
    { ...noDeclaredUiReceipt.application_experiences, source_digest: "f".repeat(64) },
    { ...noDeclaredUiReceipt.application_experiences, installed_source_digest: "f".repeat(64) },
    { ...noDeclaredUiReceipt.application_experiences, entries: customReceipt.application_experiences.entries }]) {
    const selected = { ...receipt, application_experiences }, value = remoteInstalledApplicationUse(metadata(selected), selected, state, build);
    assert.notEqual(value.interfaces.status, "none"); assert.equal(value.noDeclaredUi, false);
  }
  const custom = remoteInstalledApplicationUse(metadata(customReceipt), customReceipt, state, build);
  assert.equal(custom.interfaces.status, "resolved"); assert.equal(custom.interfaces.entries[0].label, "Observing workspace");
  assert.equal(custom.noDeclaredUi, false);
});

test("missing optional links keep native chat guidance conditional and preserve a custom UI's user authority", async () => {
  await mountedUse({ receipt: noDeclaredUiReceipt }, async ({ document, calls, task, edit, click, copied }) => {
    assert.match(document.body.textContent, /No application links are declared in this package/);
    assert.match(document.body.textContent, /Native contact: Observing lead lead:main/);
    const help = [...document.querySelectorAll(".deployment-use-help")].find((node) => node.textContent.includes("Native contact:"));
    assert.equal(help.closest("details") === null, true, "The remote contact remains visible without opening administration");
    assert.ok(help.compareDocumentPosition(document.querySelector(".deployment-use-copy")) & document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING,
      "The remote contact precedes optional request preparation");
    const paragraphs = [...help.querySelectorAll("p")];
    const nativeDirections = paragraphs.find((node) => node.textContent.includes("Paste your copied task"));
    assert.match(nativeDirections.textContent, /^If this app uses native chat for everyday work,/);
    assert.match(nativeDirections.textContent, /choose lead:main in the sidebar/);
    assert.match(nativeDirections.textContent, /Paste your copied task or type a job, then send it/);
    assert.match(nativeDirections.textContent, /Stay in that same conversation for replies, tool activity and follow-up questions/);
    assert.match(nativeDirections.textContent, /Look for the response under the request you sent/);
    assert.match(help.textContent, /If this app uses its own UI, use that UI's login, job and result controls/);
    assert.match(help.textContent, /Native Console remains for administration and inspection/);
    assert.doesNotMatch(document.body.textContent, /Console-only|no custom UI|Application contact:|saved result|persisted result|authorized application/);
    await edit(task(), "Try my real task."); await click("Copy task");
    assert.deepEqual(copied, ["Try my real task."]);
    assert.match(document.body.textContent, /Task copied\. Use the application's everyday UI or native chat, if that is its everyday interface/);
    assert.equal(calls.length, 1); assert.equal(calls[0].method, "GET");
    assert.equal(document.querySelector("a,iframe") === null, true);
    assert.doesNotMatch(document.body.innerHTML, /[?&](task|draft|identity|session_id|token)=/);
    assert.match(document.body.textContent, /has not observed this application job or verified its result/);
  });
});

test("custom remote apps retain their own task and result controls rather than routing work through native admin Console", async () => {
  await mountedUse({ receipt: customReceipt }, async ({ document, calls, task, edit, click }) => {
    assert.match(document.body.textContent, /application's own controls to send the job and find its result/);
    assert.match(document.body.textContent, /Native Console remains for administration and inspection/);
    assert.doesNotMatch(document.body.textContent, /Native contact:|choose lead:main|Paste your copied task or type a job|same conversation for replies/);
    await edit(task(), "A task for the custom workspace."); await click("Copy task");
    assert.match(document.body.textContent, /Task copied\. Use it in your application/);
    assert.equal(calls.length, 1); assert.equal(document.querySelector("a,iframe") === null, true);
  });
});

test("unsupported and stale remote interface declarations keep drafts and withhold Console-as-application send instructions", async () => {
  for (const application_experiences of [undefined,
    { ...noDeclaredUiReceipt.application_experiences, source_digest: "f".repeat(64) },
    { ...noDeclaredUiReceipt.application_experiences, status: "unavailable" }]) {
    await mountedUse({ receipt: { ...receipt, application_experiences } }, async ({ document, calls, task, edit, click }) => {
      await edit(task(), "Wait for the right application entrypoint."); await click("Copy task");
      assert.match(document.body.textContent, /everyday entrypoint has not been checked/);
      assert.match(document.body.textContent, /native Console for administration and inspection/);
      assert.doesNotMatch(document.body.textContent, /choose lead:main|Paste your copied task or type a job|same conversation for replies/);
      assert.equal(task().value, "Wait for the right application entrypoint."); assert.equal(calls.length, 1);
    });
  }
});

test("a stopped installation without declared links retains conditional future chat guidance without claiming its conversation is available", async () => {
  const selected = { ...noDeclaredUiReceipt, status: "stopped", ready: false };
  await mountedUse({ receipt: selected }, async ({ document, calls, task, edit }) => {
    assert.match(document.body.textContent, /When the installation is running, use the controls above to connect and open native Console/);
    assert.match(document.body.textContent, /Your draft stays here/);
    assert.match(document.body.textContent, /Native contact: Observing lead lead:main/);
    assert.match(document.body.textContent, /If this app uses native chat for everyday work/);
    assert.match(document.body.textContent, /If this app uses its own UI/);
    assert.equal(document.querySelector("a,iframe") === null, true, "A stopped remote owner does not expose a fabricated direct Console link");
    await edit(task(), "Keep this task for later.");
    assert.equal(task().value, "Keep this task for later."); assert.ok(calls.every((call) => call.method === "GET"));
  });
});


const handoffMetadata = (r = customReceipt) => metadata(r, state, build, { first_use: { ...firstUse, entry_id: "workshop" } });

test("remote task projection requires exact installed source, explicit interface opt-in and available native observation", () => {
  const task = "  My editable remote job \u{1f9ed}\nwith constraints.  ";
  assert.deepEqual(remoteInstalledUseTaskHandoff(handoffMetadata(), customReceipt, state, build, task), {
    readKey: remoteInstalledUseReadKey(customReceipt, state, build), binding: remoteInstalledUseBinding(customReceipt, state),
    entryId: "workshop", task });
  for (const patch of [{ first_use: firstUse }, { first_use: { ...firstUse, entry_id: "unknown" } },
    { installed_source_verified: false }, { native_runtime_available: false }, { source_digest: "0".repeat(64) },
    { remote_binding: { ...handoffMetadata().remote_binding, endpoint: { ...receipt.endpoint, host: "other.invalid" } } }])
    assert.equal(remoteInstalledUseTaskHandoff({ ...handoffMetadata(), ...patch }, customReceipt, state, build, task), null);
  const stale = { ...customReceipt, status: "observation-unavailable", ready: false, observation: { available: false } };
  assert.equal(remoteInstalledUseTaskHandoff(handoffMetadata(stale), stale, state, build, task), null);
  assert.equal(remoteInstalledUseTaskHandoff(handoffMetadata(), customReceipt, state, build, "bad\ud800"), null);
  assert.equal(remoteInstalledUseTaskHandoff(handoffMetadata(), customReceipt, state, build, "bad\u0000"), null);
});

test("mounted remote first-task projection follows the editable draft and withdraws on health or source changes without sending", async () => {
  const published = [], emit = (value) => published.push(value);
  await mountedUse({ receipt: customReceipt, onTaskHandoff: emit }, async ({ document, task, edit, copied, click, rerender, calls, admissions }) => {
    assert.equal(published.at(-1), null);
    await act(async () => document.querySelector(".deployment-use-ideas button").click());
    assert.equal(published.at(-1).task, firstUse.tasks[0].request);
    const current = "  Edited remote task \u00e5 \u{1f9ed}\nKeep this context.  ";
    await edit(task(), current);
    assert.equal(published.at(-1).task, current);
    assert.equal(published.at(-1).binding, remoteInstalledUseBinding(customReceipt, state));
    assert.equal(published.at(-1).entryId, "workshop");
    await click("Copy task"); assert.deepEqual(copied, [current]);
    await rerender({ blocked: true }); assert.equal(published.at(-1), null);
    assert.equal(task().value, current);
    await rerender({ blocked: false }); assert.equal(published.at(-1).task, current);
    await rerender({ receipt: { ...customReceipt, ready: false } });
    assert.equal(published.at(-1), null);
    assert.equal(task().value, current);
    await rerender({ receipt: customReceipt }); assert.equal(published.at(-1).task, current);
    await rerender({ receipt: { ...customReceipt, accepted_source_digest: "0".repeat(64) } });
    assert.equal(published.at(-1), null);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.deepEqual(admissions, []);
  }, { get: () => handoffMetadata() });
  assert.equal(published.at(-1), null, "Unmount revokes the parent projection.");
});


test("valid stale remote guidance rechecks after unchanged status and restores the exact edited handoff", async () => {
  const published = [], emit = (value) => published.push(value), refreshed = deferred();
  const stale = { ...handoffMetadata(), installed_source_verified: false, native_runtime_available: false,
    observation: { ...handoffMetadata().observation, available: false, freshness: "stale",
      admitted: false, source_current: false } };
  assert.ok(remoteInstalledApplicationUse(stale, customReceipt, state, build), "Stale guidance remains valid source metadata.");
  await mountedUse({ receipt: customReceipt, onTaskHandoff: emit },
    async ({ document, window, calls, admissions, task, form, edit, fill, click, rerender, setRead, settle }) => {
      const taskNode = task(), reportNode = form().querySelector("textarea");
      const disclosure = document.querySelector(".deployment-use-feedback-disclosure");
      const outcomeNode = form().querySelector('input[value="blocked"]');
      const edited = "  My edited remote first job.\nKeep this exact context.  ";
      const problem = "Keep this problem draft while rechecking guidance.";
      await edit(taskNode, edited); await fill("blocked", problem);
      disclosure.open = true; reportNode.focus(); reportNode.setSelectionRange(5, 12);
      const selectedRange = [reportNode.selectionStart, reportNode.selectionEnd];
      assert.equal(window.document.activeElement === reportNode, true);
      assert.equal(published.at(-1), null);
      assert.equal(calls.length, 1);
      setRead(() => refreshed.promise);
      // A fresh status read can return identical public receipt fields while
      // the source-scoped first-use observation still needs its own recheck.
      await rerender({ receipt: structuredClone(customReceipt) });
      assert.equal(calls.length, 1, "An identical receipt does not refresh first-use metadata.");
      assert.equal(published.at(-1), null);
      await click("Recheck first-job guidance");
      assert.equal(calls.length, 2);
      assert.equal(published.at(-1), null, "Pending guidance cannot authorize an editable handoff.");
      for (const phase of ["pending", "recovered"]) {
        if (phase === "recovered") await settle(() => refreshed.resolve(handoffMetadata()));
        assert.equal(task() === taskNode, true); assert.equal(form().querySelector("textarea") === reportNode, true);
        assert.equal(document.querySelector(".deployment-use-feedback-disclosure") === disclosure, true);
        assert.equal(taskNode.value, edited); assert.equal(reportNode.value, problem);
        assert.equal(outcomeNode.checked, true); assert.equal(disclosure.open, true);
        assert.equal(window.document.activeElement === reportNode, true);
        assert.deepEqual([reportNode.selectionStart, reportNode.selectionEnd], selectedRange);
      }
      assert.deepEqual(published.at(-1), {
        readKey: remoteInstalledUseReadKey(customReceipt, state, build),
        binding: remoteInstalledUseBinding(customReceipt, state), entryId: "workshop", task: edited,
      });
      assert.deepEqual(calls.map((call) => [call.method, call.target]), [
        ["GET", remoteInstalledUseUrl(state.id, customReceipt.deployment_id)],
        ["GET", remoteInstalledUseUrl(state.id, customReceipt.deployment_id)],
      ]);
      assert.deepEqual(admissions, []);
    }, { get: () => stale });
});
