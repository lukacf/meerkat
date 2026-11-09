import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { canonicalRemoteInstalledBinding, remoteInstalledUseBinding, remoteInstalledUseDraftScope,
  createRemoteInstalledUseDraftStore, remoteInstalledApplicationUse, remoteInstalledUseFeedback,
  remoteInstalledUseSuccessBody, remoteInstalledUseTaskHandoff, remoteInstalledUseUrl } from "./remote-installed-use.mjs";

const state = { id: "11111111-1111-4111-8111-111111111111", version: 4 };
const build = { guide_id: state.id, guide_version: 4, build_id: "public-checked-source", current: true,
  source_verified: true, source_digest: "a".repeat(64), status: "completed" };
const endpoint = { transport: "gcp-iap", operation_id: "33333333-3333-4333-8333-333333333333",
  account: "fixture@example.com", project: "fixture-project", zone: "us-central1-a", instance: "fixture-application",
  instance_id: "123456789", ownership_token: "4".repeat(32), host: "compute.123456789", user: "mobkit", ssh_port: 22 };
const receipt = { protocol: 1, target: "gcp", cloud_operation_id: endpoint.operation_id,
  guide_id: state.id, guide_version: 4, deployment_id: "22222222-2222-4222-8222-222222222222", build_id: build.build_id,
  accepted_source_digest: build.source_digest, package_sha256: "b".repeat(64), transferred_source_digest: "c".repeat(64),
  controller_sha256: "d".repeat(64), endpoint, host: { uid: 1001, home: "/home/mobkit", destination: "/home/mobkit/src/fixture-application" },
  destination: "/home/mobkit/src/fixture-application", host_key_fingerprint: "SHA256:" + "A".repeat(43),
  status: "running", admitted: true, remote_admission_requested: true, worker_live: false,
  pending_operation: null, observation: { available: true }, source_current: true, ready: true,
  application_experiences: { protocol: 1, supported: true, status: "resolved", source_digest: build.source_digest,
    installed_source_digest: "c".repeat(64), metadata_sha256: "e".repeat(64),
    entries: [{ id: "work", label: "Application workspace", path: "/work", destination_port: 8792,
      listener_owned: true, listener_token: "9".repeat(64), reason: null, verified: false }] } };
const idea = { label: "Try a public fixture job", request: "Summarize the supplied public input.",
  description: "Use the saved application workspace.", expected_result: "A summary with sources.", result_location: "The workspace result panel." };
function metadata(r = receipt) {
  const observed = r.status !== "observation-unavailable" && r.observation?.available === true;
  return { protocol: 1, scope: "frozen-remote-package-source", guide_id: state.id, version: state.version,
    guide_version: r.guide_version, deployment_id: r.deployment_id, build_id: r.build_id,
    package_sha256: r.package_sha256, accepted_source_digest: r.accepted_source_digest, source_digest: r.accepted_source_digest,
    transferred_source_digest: r.transferred_source_digest, controller_sha256: r.controller_sha256,
    remote_binding: JSON.parse(remoteInstalledUseBinding(r, state)), current: true, status: r.status,
    installed_source_verified: observed, native_runtime_available: observed && r.status === "running",
    application_outcomes_verified: false, project: { name: "Public fixture application" }, entrypoint: { identity: "lead:main", name: "Application guide" },
    first_use: { scope: "authored-first-use-suggestions", metadata_sha256: "f".repeat(64), entry_id: "work", tasks: [idea], detail: null },
    observation: { scope: "saved-destination-observation", status: r.status, available: observed,
      freshness: observed ? "fresh" : "unavailable", admitted: observed, source_current: observed },
    continuation: { available: true, build_id: build.build_id, source_digest: build.source_digest, requires_acknowledgement: false } };
}
const report = { task: idea.request, outcome: "partly", request: "Explain the source of the answer.", continueCurrentCandidate: false };
function storage() {
  const rows = new Map();
  return { getItem: (key) => rows.get(key) ?? null, setItem: (key, value) => rows.set(key, value), removeItem: (key) => rows.delete(key) };
}

test("GCP installed binding preserves all eleven canonical endpoint fields without changing the twelve outer fields", () => {
  const before = structuredClone(receipt), binding = JSON.parse(remoteInstalledUseBinding(receipt, state));
  assert.equal(Object.keys(binding).length, 12);
  assert.deepEqual(binding.endpoint, endpoint);
  assert.deepEqual(Object.keys(binding.endpoint), Object.keys(endpoint).sort());
  assert.deepEqual(canonicalRemoteInstalledBinding(binding), binding);
  const reverse = { ...binding, endpoint: Object.fromEntries(Object.entries(endpoint).reverse()) };
  assert.equal(JSON.stringify(canonicalRemoteInstalledBinding(reverse)), JSON.stringify(binding));
  assert.deepEqual(receipt, before);
});

test("GCP installed bindings reject missing, extra or malformed endpoint identity and cannot impersonate Linux receipts", () => {
  const binding = JSON.parse(remoteInstalledUseBinding(receipt, state));
  for (const name of Object.keys(endpoint)) {
    const missing = { ...endpoint }; delete missing[name];
    assert.equal(canonicalRemoteInstalledBinding({ ...binding, endpoint: missing }), null, name);
  }
  for (const patch of [{ transport: "ssh" }, { operation_id: "other" }, { account: "not-an-account" }, { project: "bad" },
    { zone: "-invalid" }, { instance: "-invalid" }, { instance_id: "0" }, { instance_id: "18446744073709551616" },
    { ownership_token: "wrong" }, { host: "compute.999" }, { user: "root" }, { ssh_port: 2222 },
    { secret: "public-fixture-canary" }]) {
    assert.equal(canonicalRemoteInstalledBinding({ ...binding, endpoint: { ...endpoint, ...patch } }), null);
  }
  for (const patch of [{ target: "linux" }, { target: undefined }, { cloud_operation_id: "44444444-4444-4444-8444-444444444444" },
    { cloud_operation_id: undefined }, { endpoint: { host: "fixture-linux.local", user: "mobkit", ssh_port: 22 } }])
    assert.equal(remoteInstalledUseBinding({ ...receipt, ...patch }, state), "");
  const linux = { ...receipt, target: "linux", endpoint: { host: "fixture-linux.local", user: "mobkit", ssh_port: 22 } };
  assert.deepEqual(JSON.parse(remoteInstalledUseBinding(linux, state)).endpoint, linux.endpoint);
  assert.equal(remoteInstalledUseBinding({ ...linux, target: undefined }, state), remoteInstalledUseBinding(linux, state));
});

test("first jobs, feedback, success and task handoff retain the exact owned GCP endpoint and source", () => {
  const value = metadata(), use = remoteInstalledApplicationUse(value, receipt, state, build);
  assert.equal(use.name, "Public fixture application");
  assert.equal(use.startingIdeas.tasks[0].request, idea.request);
  const body = remoteInstalledUseFeedback(value, receipt, state, build, report);
  assert.deepEqual(body.remote_binding.endpoint, endpoint);
  assert.equal(body.remote_binding.controller_sha256, receipt.controller_sha256);
  assert.equal(body.continuation_source_digest, build.source_digest);
  const success = remoteInstalledUseSuccessBody(value, receipt, state, idea.request, "55555555-5555-4555-8555-555555555555");
  assert.equal(success.deployment_id, receipt.deployment_id);
  assert.equal(success.source_digest, receipt.accepted_source_digest);
  const handoff = remoteInstalledUseTaskHandoff(value, receipt, state, build, idea.request);
  assert.equal(handoff.entryId, "work"); assert.deepEqual(JSON.parse(handoff.binding).endpoint, endpoint);
  assert.equal(remoteInstalledApplicationUse({ ...value, remote_binding: { ...value.remote_binding,
    endpoint: { ...endpoint, account: "other@example.com" } } }, receipt, state, build), null);
  assert.equal(remoteInstalledUseFeedback(value, receipt, state, { ...build, current: false }, report), null);
});

test("each valid GCP operation and destination identity owns a distinct first-job draft", () => {
  const memory = storage(), binding = remoteInstalledUseBinding(receipt, state), scope = remoteInstalledUseDraftScope(receipt, state);
  const drafts = createRemoteInstalledUseDraftStore(memory, binding);
  assert.ok(drafts.update(scope, "task", report.task));
  assert.equal(createRemoteInstalledUseDraftStore(memory, binding).read(scope).task, report.task);
  const alternatives = [{ account: "other@example.com" }, { project: "other-project" }, { zone: "us-central1-b" },
    { instance: "other-application" }, { ownership_token: "6".repeat(32) }, { instance_id: "987654321", host: "compute.987654321" },
    { operation_id: "66666666-6666-4666-8666-666666666666" }];
  for (const patch of alternatives) {
    const next = { ...receipt, endpoint: { ...endpoint, ...patch }, cloud_operation_id: patch.operation_id || endpoint.operation_id };
    const nextBinding = remoteInstalledUseBinding(next, state);
    assert.ok(nextBinding); assert.notEqual(nextBinding, binding);
    assert.equal(createRemoteInstalledUseDraftStore(memory, nextBinding).read(remoteInstalledUseDraftScope(next, state)).task, "");
    assert.equal(remoteInstalledApplicationUse(metadata(), next, state, build), null);
  }
});

test("unavailable cloud observation preserves saved first jobs without claiming native availability or successful outcomes", () => {
  const unavailable = { ...receipt, status: "observation-unavailable", last_known_status: "stopped", observation: { available: false } };
  const value = metadata(unavailable), use = remoteInstalledApplicationUse(value, unavailable, state, build);
  assert.equal(use.startingIdeas.tasks[0].request, idea.request);
  assert.equal(use.nativeAvailable, false); assert.equal(use.installedVerified, false);
  assert.equal(remoteInstalledUseTaskHandoff(value, unavailable, state, build, idea.request), null);
  assert.equal(remoteInstalledApplicationUse({ ...value, application_outcomes_verified: true }, unavailable, state, build), null);
  assert.deepEqual(remoteInstalledUseFeedback(value, unavailable, state, build, { ...report, outcome: "blocked" }).remote_binding.endpoint, endpoint);
});

let component;
async function productionComponent() {
  if (!component) component = (async () => {
    const require = createRequire(import.meta.url);
    const built = await bundle({ entryPoints: [fileURLToPath(new URL("remote-installed-use.jsx", import.meta.url))],
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
      plugins: [{ name: "shared-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
      } }] });
    return import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
  })();
  return component;
}

test("the real first-use component renders GCP suggestions and submits feedback with its exact cloud identity", async () => {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], memory = storage(), admitted = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: memory, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (url, init = {}) => {
      calls.push({ url, body: init.body ? JSON.parse(init.body) : null });
      assert.ok([remoteInstalledUseUrl(state.id, receipt.deployment_id), remoteInstalledUseUrl(state.id, receipt.deployment_id, "feedback")].includes(url));
      return { ok: true, json: async () => url.endsWith("/feedback")
        ? { guide_id: state.id, guide_version: state.version, current: true, build_id: "public-improvement", status: "queued" }
        : metadata() };
    } };
  const prior = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  let root;
  try {
    const { createRoot } = await import("react-dom/client"), { RemoteInstalledUse } = await productionComponent();
    root = createRoot(dom.window.document.getElementById("root"));
    await act(async () => root.render(React.createElement(RemoteInstalledUse, { state, receipt, build, onRepairReceipt: (value) => admitted.push(value) })));
    assert.match(dom.window.document.body.textContent, /Try Public fixture application/);
    const ideaButton = [...dom.window.document.querySelectorAll("button")].find((node) => node.querySelector("strong")?.textContent === idea.label);
    assert.ok(ideaButton);
    await act(async () => ideaButton.click());
    assert.equal(dom.window.document.querySelector(".deployment-use-field textarea").value, idea.request);
    assert.match(dom.window.document.body.textContent, /A summary with sources/);
    assert.match(dom.window.document.body.textContent, /The workspace result panel/);
    await act(async () => dom.window.document.querySelector('input[value="partly"]').click());
    const field = dom.window.document.querySelector(".deployment-use-feedback textarea");
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set.call(field, report.request);
      field.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
    const submit = dom.window.document.querySelector('.deployment-use-feedback button[type="submit"]');
    assert.equal(submit.disabled, false);
    await act(async () => submit.click());
    const posted = calls.filter((row) => row.body);
    assert.equal(posted.length, 1); assert.deepEqual(posted[0].body.remote_binding.endpoint, endpoint);
    assert.equal(posted[0].body.task, idea.request); assert.equal(posted[0].body.request, report.request);
    assert.equal(admitted.length, 1);
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) descriptor ? Object.defineProperty(globalThis, key, descriptor) : delete globalThis[key];
  }
});
