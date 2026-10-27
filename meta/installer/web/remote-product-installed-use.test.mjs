import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { remoteProductControl } from "./remote-deployment.mjs";
import { remoteInstalledUseBinding, remoteInstalledUseBusy, remoteInstalledUseReadKey,
  remoteInstalledUseUrl, remoteInstalledUseFeedback, remoteInstalledUseSuccessBody,
  remoteInstalledUseTaskHandoff } from "./remote-installed-use.mjs";

// Public receipt/metadata specimens follow the original retained actor and
// remote_installed_feedback contracts. DOM, HTTP replies and native facts are
// explicitly inert. Production projections, component and shared hook are real.
const state = { id: "11111111-1111-4111-8111-111111111111", version: 4 };
const build = { guide_id: state.id, guide_version: 4, current: true, source_verified: true,
  build_id: "synthetic-accepted-source", source_digest: "a".repeat(64), status: "completed" };
const requestId = "55555555-5555-4555-8555-555555555555";
const task = "Compare the two observations and explain the difference.";
const report = { task, outcome: "partly", request: "Show the evidence behind that difference.", continueCurrentCandidate: false };

function fixture(status = "running") {
  const receipt = { protocol: 1, guide_id: state.id, guide_version: state.version,
    deployment_id: "22222222-2222-4222-8222-222222222222", build_id: build.build_id,
    accepted_source_digest: build.source_digest, package_sha256: "b".repeat(64),
    transferred_source_digest: "c".repeat(64), controller_sha256: "d".repeat(64),
    endpoint: { host: "synthetic-linux.local", user: "fixture-user", ssh_port: 22 },
    host: { uid: 1001, home: "/home/fixture-user", destination: "/home/fixture-user/src/observing",
      platform: "linux", python: "3.11.9" }, destination: "/home/fixture-user/src/observing",
    host_key_fingerprint: "SHA256:" + "A".repeat(43), status, admitted: true, remote_admission_requested: true,
    observation: { available: true }, source_current: true, ready: status === "running", source_binding_current: true,
    current_guide_version: state.version, product_runtime: true, product_scope: "retained-original-product-owner",
    install_operation_id: "33333333-3333-4333-8333-333333333333", product_control_manifest_sha256: "e".repeat(64),
    worker_live: true, cleanup_confirmed: false, service: {}, service_owned: false };
  const binding = { protocol: 1, install_operation_id: receipt.install_operation_id,
    control_manifest_sha256: receipt.product_control_manifest_sha256, initial_install_sha256: "f".repeat(64),
    binding: Object.fromEntries(["guide_id", "guide_version", "build_id", "accepted_source_digest", "package_sha256",
      "transferred_source_digest", "controller_sha256"].map((key) => [key, receipt[key]])),
    worker: { pid: 100, uid: 1001, start_ticks: "100", boot_id: "synthetic-boot" },
    command: { pid: 101, uid: 1001, start_ticks: "101", boot_id: "synthetic-boot" } };
  receipt.product_control = { owner_live: true, initial_install_complete: true, binding,
    factory_status: { guide_id: state.id, guide_version: state.version, deployment_id: receipt.deployment_id,
      build_id: build.build_id, package_sha256: receipt.package_sha256, destination: receipt.destination,
      source_verified: true, source_digest: receipt.accepted_source_digest, status,
      phase: status === "running" ? "ready" : "configure", execution_mode: "interactive" } };
  receipt.application_experiences = { protocol: 1, supported: true, status: "resolved",
    source_digest: receipt.accepted_source_digest, installed_source_digest: receipt.transferred_source_digest,
    metadata_sha256: "f".repeat(64), entries: [{ id: "workshop", label: "Observing workspace", path: "/workspace",
      destination_port: 8792, listener_owned: true, listener_token: "9".repeat(64), reason: null, verified: false }] };
  return receipt;
}

function actionRecord(receipt, phase, outcome = undefined) {
  return { guide_id: state.id, deployment_id: receipt.deployment_id, action_request_id: requestId,
    action: "restart", phase, ...(outcome === undefined ? {} : { outcome }) };
}

function metadata(receipt, guide = state, checked = build, patch = {}) {
  return { protocol: 1, guide_id: guide.id, version: guide.version, guide_version: receipt.guide_version,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id, accepted_source_digest: receipt.accepted_source_digest,
    source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256,
    transferred_source_digest: receipt.transferred_source_digest, controller_sha256: receipt.controller_sha256,
    remote_binding: JSON.parse(remoteInstalledUseBinding(receipt, guide)), scope: "frozen-remote-package-source",
    current: true, status: receipt.status, project: { name: "Observing assistant", description: "An authored observing system." },
    entrypoint: { identity: "lead:main", name: "Observing lead" },
    first_use: { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64), entry_id: "workshop",
      tasks: [{ label: "Compare observations", request: task }], detail: null },
    observation: { scope: "saved-destination-observation", status: receipt.status, available: true,
      freshness: "fresh", admitted: true, source_current: true }, installed_source_verified: true,
    native_runtime_available: receipt.status === "running", application_outcomes_verified: false,
    success_reporting_available: true, success_report: null, success_report_detail: null,
    continuation: { build_id: checked.build_id, source_digest: checked.source_digest,
      available: true, requires_acknowledgement: false }, ...patch };
}

function requireIdle(receipt = fixture()) {
  assert.ok(remoteProductControl(receipt, state));
  assert.equal(remoteInstalledUseBusy(receipt, state), false);
  assert.ok(remoteInstalledUseFeedback(metadata(receipt), receipt, state, build, report));
  assert.ok(remoteInstalledUseSuccessBody(metadata(receipt), receipt, state, task, requestId));
  return receipt;
}

test("idle product feedback and success retain the live original owner on running and stopped applications", () => {
  for (const status of ["running", "stopped"]) {
    const receipt = requireIdle(fixture(status)), before = structuredClone(receipt), value = metadata(receipt);
    const feedback = remoteInstalledUseFeedback(value, receipt, state, build, report);
    const success = remoteInstalledUseSuccessBody(value, receipt, state, task, requestId);
    assert.deepEqual(feedback.remote_binding, JSON.parse(remoteInstalledUseBinding(receipt, state)));
    assert.equal(success.request_id, requestId);
    assert.equal(success.source_digest, receipt.accepted_source_digest);
    assert.deepEqual(receipt, before);
    assert.equal(receipt.worker_live, true);
    assert.equal(value.application_outcomes_verified, false);
  }
  assert.ok(remoteInstalledUseTaskHandoff(metadata(fixture()), fixture(), state, build, task));
  const stopped = fixture("stopped");
  assert.equal(remoteInstalledUseTaskHandoff(metadata(stopped), stopped, state, build, task), null);
});

const refusals = [
  ["missing original owner", (r) => { delete r.product_control; }],
  ["retired original owner", (r) => { r.product_control.owner_live = false; }],
  ["foreign original control binding", (r) => { r.product_control.binding.binding.package_sha256 = "0".repeat(64); }],
  ["foreign original source", (r) => { r.product_control.factory_status.source_digest = "0".repeat(64); }],
  ["foreign original guide version", (r) => { r.product_control.factory_status.guide_version++; }],
  ["malformed live worker", (r) => { r.worker_live = "unknown"; }],
  ["outer pending operation", (r) => { r.pending_operation = { id: requestId, action: "restart" }; }],
  ["canonical pending action", (r) => { r.product_control.action_request = actionRecord(r, "pending"); }],
  ["canonical unknown action", (r) => { r.product_control.action_request = actionRecord(r, "unknown"); }],
  ["foreign settled action", (r) => { r.product_control.action_request = { ...actionRecord(r, "settled", "returned"), deployment_id: requestId }; }],
  ["starting factory", (r) => { r.product_control.factory_status.status = "starting"; }],
  ["factory update window", (r) => { r.product_control.factory_status.phase = "update"; }],
  ["factory recovery window", (r) => { r.product_control.factory_status.phase = "recovery"; }],
  ["factory update verification", (r) => { r.product_control.factory_status.update = { status: "verifying" }; }],
];
for (const [label, change] of refusals) test(`retained product first-use refuses ${label} after a valid idle baseline`, () => {
  const receipt = requireIdle(), value = metadata(receipt), before = structuredClone(receipt);
  change(receipt);
  assert.equal(remoteInstalledUseBusy(receipt, state), true);
  assert.equal(remoteInstalledUseFeedback(value, receipt, state, build, report), null);
  assert.equal(remoteInstalledUseSuccessBody(value, receipt, state, task, requestId), null);
  assert.equal(remoteInstalledUseTaskHandoff(value, receipt, state, build, task), null);
  assert.equal(before.worker_live, true);
});

test("product activity refresh identity changes without rewriting ordinary read keys or accepting stale metadata", () => {
  const receipt = requireIdle(), value = metadata(receipt), idle = remoteInstalledUseReadKey(receipt, state, build);
  receipt.product_control.action_request = actionRecord(receipt, "pending");
  const pending = remoteInstalledUseReadKey(receipt, state, build);
  receipt.product_control.action_request = actionRecord(receipt, "settled", "returned");
  assert.notEqual(remoteInstalledUseReadKey(receipt, state, build), pending);
  receipt.product_control.factory_status.phase = "update";
  const updating = remoteInstalledUseReadKey(receipt, state, build);
  receipt.product_control.factory_status.phase = "ready";
  assert.notEqual(remoteInstalledUseReadKey(receipt, state, build), updating);
  assert.notEqual(idle, pending);
  const stale = { ...value, source_digest: "0".repeat(64) };
  assert.equal(remoteInstalledUseSuccessBody(stale, receipt, state, task, requestId), null);
  assert.equal(remoteInstalledUseFeedback(stale, receipt, state, build, report), null);
  assert.equal(remoteInstalledUseSuccessBody({ ...value, success_reporting_available: false }, receipt, state, task, requestId), null);
  assert.equal(remoteInstalledUseFeedback({ ...value, continuation: { ...value.continuation, available: false } },
    receipt, state, build, report), null);
});

test("ordinary remote operations retain their original live and unknown worker refusals", () => {
  const ordinary = fixture();
  for (const key of ["product_runtime", "product_scope", "product_control", "install_operation_id", "product_control_manifest_sha256"])
    delete ordinary[key];
  ordinary.worker_live = false;
  const value = metadata(ordinary);
  assert.ok(remoteInstalledUseFeedback(value, ordinary, state, build, report));
  assert.ok(remoteInstalledUseSuccessBody(value, ordinary, state, task, requestId));
  const expectedKey = JSON.stringify([remoteInstalledUseBinding(ordinary, state), state.version, ordinary.status,
    ordinary.observation.available, ordinary.last_known_status, ordinary.source_current, ordinary.ready, false,
    ordinary.pending_operation, build.build_id, build.source_digest, build.status, build.current, build.source_verified]);
  assert.equal(remoteInstalledUseReadKey(ordinary, state, build), expectedKey);
  for (const worker of [true, "unknown"]) {
    ordinary.worker_live = worker;
    assert.equal(remoteInstalledUseFeedback(value, ordinary, state, build, report), null);
    assert.equal(remoteInstalledUseSuccessBody(value, ordinary, state, task, requestId), null);
  }
});

let component;
async function productionComponent() {
  if (!component) component = (async () => {
    const require = createRequire(import.meta.url);
    const result = await bundle({ entryPoints: [fileURLToPath(new URL("remote-installed-use.jsx", import.meta.url))],
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
      nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
      plugins: [{ name: "original-shared-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
      } }] });
    return import("data:text/javascript;base64," + Buffer.from(result.outputFiles[0].text).toString("base64"));
  })();
  return component;
}

async function mounted(initial, verify, initialRead = null) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  let props = { state, receipt: fixture(), build, blocked: false, onRepairReceipt() {}, ...initial };
  let read = initialRead ? () => initialRead(props) : () => metadata(props.receipt, props.state, props.build);
  const calls = [], handoffs = [];
  props.onTaskHandoff = (value) => handoffs.push(value);
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (url, init = {}) => {
      const call = { url, method: init.method || "GET", body: init.body ? JSON.parse(init.body) : null };
      calls.push(call);
      assert.ok([remoteInstalledUseUrl(props.state.id, props.receipt.deployment_id),
        remoteInstalledUseUrl(props.state.id, props.receipt.deployment_id, "success"),
        remoteInstalledUseUrl(props.state.id, props.receipt.deployment_id, "feedback")].includes(url));
      let value;
      if (call.method === "GET") value = await read();
      else if (url.endsWith("/success")) value = { ...call.body, guide_id: props.state.id,
        outcome: "worked", scope: "human-reported-installed-success", application_outcomes_verified: false,
        recorded_at: "2026-10-12T10:00:00+00:00" };
      else value = { guide_id: props.state.id, guide_version: props.state.version, current: true,
        build_id: "synthetic-builder-improvement", status: "queued",
        request: "User's requested improvement:\n" + call.body.request + "\n\nSynthetic host context: " + JSON.stringify(call.body) };
      return { ok: true, json: async () => structuredClone(value) };
    } };
  const prior = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  let root;
  try {
    const { createRoot } = await import("react-dom/client"), { RemoteInstalledUse } = await productionComponent();
    root = createRoot(dom.window.document.getElementById("root"));
    const render = () => act(async () => root.render(React.createElement(RemoteInstalledUse, props)));
    const button = (label) => [...dom.window.document.querySelectorAll("button")].find((node) => node.textContent === label);
    const taskField = () => dom.window.document.querySelector(".deployment-use-field textarea");
    async function edit(field, value) {
      assert.ok(field);
      const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set;
      await act(async () => { setter.call(field, value); field.dispatchEvent(new dom.window.Event("input", { bubbles: true })); });
    }
    await render();
    await verify({ calls, handoffs, button, taskField, edit, document: dom.window.document,
      async click(label) { const target = button(label); assert.ok(target, label); await act(async () => target.click()); },
      async fillFeedback() {
        await edit(dom.window.document.querySelector(".deployment-use-feedback textarea"), report.request);
        await act(async () => dom.window.document.querySelector('input[value="partly"]').click());
      },
      async rerender(patch) { props = { ...props, ...patch }; await render(); },
      setRead(handler) { read = handler; },
      async settle(action) { await act(async () => { action(); await Promise.resolve(); }); },
    });
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test("mounted product first-use uses the actual shared success hook and sends only explicit source-bound human reports", async () => {
  const receipt = requireIdle();
  await mounted({ receipt }, async ({ calls, handoffs, taskField, edit, click, button, fillFeedback, document }) => {
    await edit(taskField(), task);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.equal(button("It worked").disabled, false);
    assert.ok(handoffs.at(-1));
    assert.equal(handoffs.at(-1).task, task);
    await click("It worked");
    const success = calls.find((call) => call.url.endsWith("/success"));
    assert.ok(success);
    assert.equal(success.body.source_digest, receipt.accepted_source_digest);
    assert.match(document.body.textContent, /Your success report is saved/);
    assert.match(document.body.textContent, /Forge has not independently verified the result/);
    await fillFeedback();
    assert.equal(button("Ask builder to improve it").disabled, false);
    await click("Ask builder to improve it");
    const feedback = calls.find((call) => call.url.endsWith("/feedback"));
    assert.deepEqual(feedback.body.remote_binding, JSON.parse(remoteInstalledUseBinding(receipt, state)));
    assert.equal(feedback.body.task, task);
    assert.equal(calls.filter((call) => call.method === "POST").length, 2);
    assert.equal(receipt.worker_live, true);
    assert.deepEqual(receipt.service, {});
  });
});

test("mounted product pending-to-settled and idle-to-unknown transitions refetch metadata without changing outer worker or losing drafts", async () => {
  const pending = fixture(); pending.product_control.action_request = actionRecord(pending, "pending");
  await mounted({ receipt: pending }, async ({ calls, button, taskField, edit, fillFeedback, rerender, setRead, settle }) => {
    await edit(taskField(), task); await fillFeedback();
    assert.equal(button("It worked").disabled, true);
    assert.equal(button("Ask builder to improve it").disabled, true);
    const initialGets = calls.filter((call) => call.method === "GET").length;
    const idle = structuredClone(pending); idle.product_control.action_request = actionRecord(idle, "settled", "returned");
    let release;
    const delayed = new Promise((resolve) => { release = resolve; });
    setRead(() => delayed);
    await rerender({ receipt: idle });
    assert.equal(calls.filter((call) => call.method === "GET").length, initialGets + 1);
    assert.ok(!button("It worked") || button("It worked").disabled);
    assert.equal(taskField().value, task);
    await settle(() => release(metadata(idle)));
    assert.equal(button("It worked").disabled, false);
    assert.equal(button("Ask builder to improve it").disabled, false);
    const unknown = structuredClone(idle); unknown.product_control.action_request = actionRecord(unknown, "unknown");
    setRead(() => metadata(unknown, state, build, { success_reporting_available: false,
      continuation: { build_id: build.build_id, source_digest: build.source_digest, available: false, requires_acknowledgement: false } }));
    await rerender({ receipt: unknown });
    assert.equal(calls.filter((call) => call.method === "GET").length, initialGets + 2);
    assert.equal(button("It worked").disabled, true);
    assert.equal(button("Ask builder to improve it").disabled, true);
    assert.equal(taskField().value, task);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.equal(unknown.worker_live, true);
  }, ({ receipt, state: guide, build: checked }) => metadata(receipt, guide, checked, {
    success_reporting_available: false,
    continuation: { build_id: checked.build_id, source_digest: checked.source_digest, available: false, requires_acknowledgement: false },
  }));
});

test("mounted product update completion refreshes unavailable metadata even when outer status is still running", async () => {
  const updating = fixture(); updating.product_control.factory_status.phase = "update";
  await mounted({ receipt: updating }, async ({ calls, button, taskField, edit, rerender, setRead }) => {
    await edit(taskField(), task);
    assert.equal(button("It worked").disabled, true);
    const previousGets = calls.filter((call) => call.method === "GET").length;
    const idle = fixture();
    assert.equal(idle.status, updating.status);
    assert.equal(idle.ready, updating.ready);
    assert.equal(idle.worker_live, updating.worker_live);
    setRead(() => metadata(idle));
    await rerender({ receipt: idle });
    assert.equal(calls.filter((call) => call.method === "GET").length, previousGets + 1);
    assert.equal(button("It worked").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.equal(taskField().value, task);
  }, ({ receipt, state: guide, build: checked }) => metadata(receipt, guide, checked, {
    success_reporting_available: false,
    continuation: { build_id: checked.build_id, source_digest: checked.source_digest, available: false, requires_acknowledgement: false },
  }));
});

for (const [label, patch] of [
  ["guide operation", { state: { ...state, operation: { status: "running" } } }],
  ["unconfirmed installation command", { commandUnconfirmed: true }],
]) test(`mounted product shared success hook preserves ${label} refusal`, async () => {
  await mounted(patch, async ({ calls, button, taskField, edit }) => {
    await edit(taskField(), task);
    assert.equal(button("It worked").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

for (const [label, change] of [
  ["live installation worker", (r) => { r.worker_live = true; }],
  ["ordinary update phase", (r) => { r.phase = "update"; }],
]) test(`mounted ordinary shared success hook retains its ${label} guard`, async () => {
  const ordinary = fixture();
  for (const key of ["product_runtime", "product_scope", "product_control", "install_operation_id", "product_control_manifest_sha256"])
    delete ordinary[key];
  ordinary.worker_live = false;
  change(ordinary);
  await mounted({ receipt: ordinary }, async ({ calls, button, taskField, edit }) => {
    await edit(taskField(), task);
    assert.equal(button("It worked").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});
