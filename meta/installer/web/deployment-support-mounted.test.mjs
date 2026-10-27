import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import React, { act } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { deploymentActionIntent, storeDeploymentActionIntent, loadDeploymentActionIntent } from "./deployment-panel.mjs";
import { deploymentSupportIntent, storeDeploymentSupportIntent, loadDeploymentSupportIntent } from "./deployment-support.mjs";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-support-mounted-"));
const modulePath = join(directory, "panel.mjs");
after(() => rm(directory, { recursive: true, force: true }));
const bundled = await build({ stdin: { contents: 'export {DeploymentPanel} from "./deployment-panel.jsx"; export {DeploymentSupportPanel} from "./deployment-support.jsx";',
  resolveDir: fileURLToPath(new URL(".", import.meta.url)), loader: "jsx" },
  bundle: true, write: false, format: "esm", platform: "node", outfile: modulePath,
  sourcemap: "linked", logLevel: "silent", loader: { ".css": "empty" },
  plugins: [{ name: "shared-react", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, (args) => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
for (const file of bundled.outputFiles) await writeFile(file.path, file.contents);
const { DeploymentPanel, DeploymentSupportPanel } = await import(pathToFileURL(modulePath).href);
const guide = { id: "synthetic-support-guide", version: 2, proposal: { target: "mac" } };
const requestId = "7d0f1570-47af-4a98-93e0-9c644ae430cb";
const revision = "71846813-4766-4574-afbd-d9368c13126f";
const digest = "a".repeat(64), descriptor = "d".repeat(64);
const selected = { revision, descriptor_sha256: descriptor, phase: "prepared",
  before_service_digest: digest, after_service_digest: "b".repeat(64), retained_files: 41, staged_files: 42 };
const receiptFor = (id = guide.id) => ({ guide_id: id, guide_version: 2, deployment_id: "synthetic-owned-installation",
  build_id: "synthetic-accepted-build", package_sha256: "c".repeat(64), source_digest: "e".repeat(64),
  source_kind: "application", target: "local", destination: "/synthetic-owned/application",
  execution_mode: "background", status: "stopped", process_live: false, runtime_pid: null,
  service_cleanup_confirmed: true, service: { manager: "launchd" },
  support_request: { scope: "owned-stopped-supervisor-refresh", deployment_id: "synthetic-owned-installation",
    build_id: "synthetic-accepted-build", package_sha256: "c".repeat(64), requires_stopped: true,
    stopped_verified: true, available: true, prepare_available: true, apply_available: false,
    service_digest: digest, phase: "available", selected: null } });
const prepared = (receipt) => ({ ...receipt, support_request: { ...receipt.support_request,
  prepare_available: false, apply_available: true, phase: "prepared", selected: structuredClone(selected) } });
const applyIntent = () => deploymentSupportIntent(guide.id, { deployment_id: receiptFor().deployment_id,
  action: "apply-support", revision, descriptor_sha256: descriptor }, requestId);

// Production JSX with synthetic HTTP responses only. No server, native runtime,
// credentials, installer or operating-system action is used.
async function mount(verify, fixture = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const polls = new Map(), calls = [], receipts = [];
  let nextPoll = 0, currentGuide = structuredClone(guide), receipt = structuredClone(fixture.receipt || receiptFor());
  let observation = fixture.observation || null, loseReply = !!fixture.loseReply, holdReply = false, pendingReply = null;
  let releaseObservation = null;
  let statusUnavailable = false, holdNextStatus = false, pendingStatus = null;
  const nativeTimeout = globalThis.setTimeout, nativeClear = globalThis.clearTimeout;
  const respond = (value) => ({ ok: true, json: async () => structuredClone(value) });
  const replacement = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    setTimeout(callback, delay, ...args) {
      if (delay !== 1500) return nativeTimeout(callback, delay, ...args);
      const key = ++nextPoll; polls.set(key, () => callback(...args)); return key;
    },
    clearTimeout(key) { if (!polls.delete(key)) nativeClear(key); },
    fetch: async (url, options = {}) => {
      const method = options.method || "GET", body = options.body ? JSON.parse(options.body) : null;
      calls.push({ url, method, body });
      if (method === "POST") {
        assert.equal(url, `/api/guide/${currentGuide.id}/deployment/action`);
        if (body.action === "prepare-support") {
          receipt = prepared(receipt); receipt.support_request.selected.revision = body.action_request_id;
          receipt.support_request.selected.before_service_digest = body.expected_service_digest;
        } else if (body.action === "apply-support") {
          if (!loseReply) receipt.support_request = { ...receipt.support_request, available: false,
            apply_available: false, phase: "completed", service_digest: selected.after_service_digest,
            selected: { ...receipt.support_request.selected, phase: "completed" } };
        } else if (fixture.ordinaryRawReply && body.action === "stop") {
          receipt = { ...receipt, status: "stopped", process_live: false, runtime_pid: null,
            service_cleanup_confirmed: true, support_request: receiptFor(currentGuide.id).support_request,
            ...fixture.ordinaryStoppedReceipt };
        }
        const identity = body.action.endsWith("support")
          ? deploymentSupportIntent(currentGuide.id, Object.fromEntries(Object.entries(body).filter(([key]) => key !== "resume")), body.action_request_id)
          : deploymentActionIntent(currentGuide.id, body, body.action_request_id);
        observation = { ...receipt, action_request: { ...identity,
          phase: loseReply || fixture.ordinaryUncertain ? "pending" : "settled",
          outcome: loseReply || fixture.ordinaryUncertain ? null : "returned" } };
        let reply = structuredClone(observation);
        if (fixture.ordinaryRawReply && !body.action.endsWith("support")) {
          for (const key of ["current", "current_guide_version", "update_capabilities", "support_request"]) delete reply[key];
        }
        if (holdReply) return new Promise((resolve) => { pendingReply = () => resolve(respond(reply)); });
        if (loseReply) throw new Error("Synthetic connection ended after admission.");
        if (fixture.ordinaryUncertain && body.action === "restart") return respond(receipt);
        return respond(reply);
      }
      if (url.includes("/deployment/action/")) {
        const metadata = observation && { action_request: observation.action_request };
        if (fixture.ordinaryUncertain) return new Promise((resolve) => { releaseObservation = () => resolve(respond(metadata)); });
        return metadata ? respond(metadata)
          : { ok: false, status: 404, json: async () => ({ detail: "Synthetic command not yet observed." }) };
      }
      if (url === `/api/guide/${currentGuide.id}/deployment`) {
        if (statusUnavailable) throw new Error("Synthetic installation status unavailable.");
        if (holdNextStatus) {
          holdNextStatus = false;
          const observedReceipt = structuredClone(receipt);
          return new Promise((resolve) => { pendingStatus = () => resolve(respond(observedReceipt)); });
        }
        return respond(receipt);
      }
      return { ok: false, status: 404, json: async () => ({ detail: "No synthetic application metadata." }) };
    } };
  const before = new Map(Object.keys(replacement).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacement)) Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollIntoView = () => {};
  fixture.storage?.(dom.window.sessionStorage);
  const { createRoot } = await import("react-dom/client");
  let root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(DeploymentPanel, { state: currentGuide,
    plan: { target: "mac", project: { name: "Synthetic application" } }, build: null,
    designBusy: false, pendingManual: false, Icon: () => null,
    onReceipt: (value) => receipts.push(structuredClone(value)) })));
  const button = (name) => [...dom.window.document.querySelectorAll("button")].find((node) => node.textContent === name);
  try {
    await render();
    await verify({ calls, receipts, button, document: dom.window.document, storage: dom.window.sessionStorage,
      async poll() { const item = polls.entries().next().value; assert.ok(item); polls.delete(item[0]); await act(async () => item[1]()); },
      async click(name) { assert.ok(button(name), name); assert.equal(button(name).disabled, false); await act(async () => button(name).click()); },
      async remount() { await act(async () => root.unmount()); root = createRoot(dom.window.document.getElementById("root")); await render(); },
      async switchGuide(id) { currentGuide = { ...currentGuide, id }; receipt = receiptFor(id); observation = null; await render(); },
      async releaseReply() { assert.ok(pendingReply); await act(async () => pendingReply()); },
      async releaseObservation() { assert.ok(releaseObservation); await act(async () => releaseObservation()); },
      async releaseStatus() { assert.ok(pendingStatus); await act(async () => pendingStatus()); },
      holdNextStatus() { holdNextStatus = true; },
      holdReply() { holdReply = true; },
      receiveReplies() { loseReply = false; },
      unavailableStatus() { statusUnavailable = true; },
      availableStatus() { statusUnavailable = false; },
      setReceipt(value) { receipt = structuredClone(value); },
      setObservation(value) { observation = structuredClone(value); },
    });
  } finally {
    // Release synthetic transport waits even when an assertion fails.
    pendingReply?.(); pendingStatus?.(); releaseObservation?.();
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of before) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
    assert.equal(polls.size, 0, "Unmount cancels read-only polling.");
  }
}

test("review and apply are distinct explicit support commands in the production panel", async () => {
  await mount(async ({ calls, button, click, storage, document }) => {
    assert.equal(calls.some((call) => call.method === "POST"), false);
    await click("Review update");
    assert.ok(button("Apply support update"));
    assert.equal(loadDeploymentSupportIntent(storage, guide.id), null);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await click("Apply support update");
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 2);
    assert.equal(posts[1].body.revision, posts[0].body.action_request_id);
    assert.equal(posts[1].body.descriptor_sha256, descriptor);
    assert.ok(document.body.textContent.includes("Installation support is up to date"));
    assert.equal(loadDeploymentSupportIntent(storage, guide.id), null);
  });
});

test("lost apply replies and cold remount never replay; explicit Resume keeps the same command", async () => {
  await mount(async ({ calls, button, click, remount, receiveReplies, storage, poll }) => {
    await click("Apply support update");
    const sent = calls.find((call) => call.method === "POST").body;
    assert.equal(button("Start installed application").disabled, true);
    await remount(); await poll();
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(loadDeploymentSupportIntent(storage, guide.id).action_request_id, sent.action_request_id);
    receiveReplies(); await click("Resume this update");
    const resumed = calls.filter((call) => call.method === "POST")[1].body;
    assert.deepEqual(resumed, { ...sent, resume: true });
    assert.equal(loadDeploymentSupportIntent(storage, guide.id), null);
  }, { receipt: prepared(receiptFor()), loseReply: true });
});

test("cold exact pending support intent is read-only until an explicit resume", async () => {
  const intent = applyIntent(), receipt = prepared(receiptFor());
  await mount(async ({ calls, button, remount, poll }) => {
    assert.ok(button("Resume this update"));
    assert.equal(button("Start installed application").disabled, true);
    await remount(); await poll();
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { receipt, observation: { action_request: { ...intent, phase: "pending", outcome: null } },
    storage: (storage) => storeDeploymentSupportIntent(storage, intent) });
});

test("pending ordinary commands and missing exact support observation keep support actions closed", async () => {
  const ordinary = deploymentActionIntent(guide.id, { deployment_id: receiptFor().deployment_id, action: "restart" }, requestId);
  await mount(async ({ calls, button }) => {
    assert.equal(button("Review update") === undefined, true);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { storage: (storage) => storeDeploymentActionIntent(storage, ordinary) });
  await mount(async ({ calls, button }) => {
    assert.equal(button("Resume this update") === undefined, true);
    assert.equal(button("Start installed application").disabled, true);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { receipt: prepared(receiptFor()), storage: (storage) => storeDeploymentSupportIntent(storage, applyIntent()) });
});

test("a late support response cannot select another guide or retire its saved command", async () => {
  await mount(async ({ calls, button, click, holdReply, switchGuide, releaseReply, storage, document }) => {
    holdReply(); await click("Review update");
    const first = calls.find((call) => call.method === "POST").body;
    await switchGuide("synthetic-second-guide");
    await releaseReply();
    assert.ok(loadDeploymentSupportIntent(storage, guide.id));
    assert.equal(loadDeploymentSupportIntent(storage, guide.id).action_request_id, first.action_request_id);
    assert.ok(button("Review update"));
    assert.equal(document.body.textContent.includes("Your support update is ready"), false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("unreadable command storage and unavailable status never enable support resume", async () => {
  await mount(async ({ calls, button, document }) => {
    assert.equal(button("Review update") === undefined, true);
    assert.ok(document.body.textContent.includes("browser storage"));
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { storage: (storage) => storage.setItem("meta-forge-deployment-support-v1:" + guide.id, "broken") });
  const receipt = prepared(receiptFor()), intent = applyIntent();
  await mount(async ({ calls, button, unavailableStatus, poll }) => {
    assert.ok(button("Resume this update"));
    unavailableStatus(); await poll();
    assert.equal(button("Resume this update") === undefined, true);
    assert.equal(button("Start installed application").disabled, true);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { receipt, observation: { action_request: { ...intent, phase: "pending", outcome: null } },
    storage: (storage) => storeDeploymentSupportIntent(storage, intent) });
});

test("minimal command metadata cannot release controls before delayed or failed status checks finish", async () => {
  for (const outcome of ["returned", "error"]) {
    await mount(async ({ calls, button, click, holdNextStatus, releaseStatus, unavailableStatus,
      availableStatus, setObservation, poll, storage, receipts }) => {
      holdNextStatus(); await click("Apply support update");
      const sent = calls.find((call) => call.method === "POST").body;
      const retained = loadDeploymentSupportIntent(storage, guide.id);
      assert.equal(retained.action_request_id, sent.action_request_id);
      assert.equal(button("Resume this update") === undefined, true);
      assert.equal(button("Start installed application").disabled, true);
      assert.equal(receipts.at(-1).support_request.phase, "prepared");
      await releaseStatus();
      assert.ok(button("Resume this update"));
      setObservation({ action_request: { ...retained, phase: "settled", outcome } });
      unavailableStatus(); await poll();
      assert.equal(button("Resume this update") === undefined, true);
      assert.equal(button("Apply support update") === undefined, true);
      assert.equal(button("Start installed application").disabled, true);
      assert.equal(loadDeploymentSupportIntent(storage, guide.id).action_request_id, sent.action_request_id,
        "Metadata settlement does not bypass the failed status check or retire its intent.");
      availableStatus(); await poll();
      assert.equal(loadDeploymentSupportIntent(storage, guide.id), null);
      assert.equal(button("Apply support update").disabled, false);
      assert.equal(button("Start installed application").disabled, false);
      assert.equal(calls.filter((call) => call.method === "POST").length, 1,
        "Neither passive metadata nor status observation sends a second command.");
    }, { receipt: prepared(receiptFor()), loseReply: true });
  }
});

test("pending metadata cannot authorize changed receipt ownership, stopped state or selected support", async () => {
  const intent = applyIntent(), receipt = prepared(receiptFor());
  const changes = [
    (value) => { value.build_id = "different-installed-build"; },
    (value) => { value.source_kind = "native-team"; },
    (value) => { value.status = "running"; value.process_live = true; value.runtime_pid = 123; },
    (value) => { value.support_request.stopped_verified = false; value.support_request.apply_available = false;
      value.support_request.available = false; },
    (value) => { value.support_request.selected.descriptor_sha256 = "f".repeat(64); },
    (value) => { value.support_request.service_digest = "f".repeat(64); },
  ];
  for (const change of changes) {
    await mount(async ({ calls, button, setReceipt, poll, storage }) => {
      assert.ok(button("Resume this update"));
      const changed = structuredClone(receipt); change(changed); setReceipt(changed); await poll();
      assert.equal(button("Resume this update") === undefined, true);
      assert.equal(button("Apply support update") === undefined, true);
      assert.equal(loadDeploymentSupportIntent(storage, guide.id).action_request_id, intent.action_request_id);
      assert.equal(calls.some((call) => call.method === "POST"), false);
    }, { receipt, observation: { action_request: { ...intent, phase: "pending", outcome: null } },
      storage: (storage) => storeDeploymentSupportIntent(storage, intent) });
  }
});

test("conflicting nested command identity cannot settle or resume the retained support intent", async () => {
  const intent = applyIntent(), receipt = prepared(receiptFor());
  await mount(async ({ calls, button, setObservation, poll, storage }) => {
    assert.ok(button("Resume this update"));
    setObservation({ action_request: { ...intent, descriptor_sha256: "f".repeat(64),
      phase: "settled", outcome: "returned" } });
    await poll();
    assert.equal(button("Resume this update") === undefined, true);
    assert.equal(button("Start installed application").disabled, true);
    assert.equal(loadDeploymentSupportIntent(storage, guide.id).descriptor_sha256, intent.descriptor_sha256);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { receipt, observation: { action_request: { ...intent, phase: "pending", outcome: null } },
    storage: (storage) => storeDeploymentSupportIntent(storage, intent) });
});

test("an uncertain ordinary command immediately closes the support action while its read is pending", async () => {
  await mount(async ({ calls, button, click, releaseObservation }) => {
    assert.equal(button("Review update").disabled, false);
    await click("Start installed application");
    assert.equal(button("Review update") === undefined, true);
    assert.equal(button("Start installed application").disabled, true);
    assert.ok(button("Check support status"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await releaseObservation();
    assert.equal(button("Review update") === undefined, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { ordinaryUncertain: true });
});

test("a settled raw Stop reply preserves the checked projection until a fresh status GET completes", async () => {
  const running = { ...receiptFor(), current: true, current_guide_version: guide.version,
    update_capabilities: { available: false, reason: "Synthetic running installation." },
    status: "running", process_live: true, runtime_pid: 123,
    support_request: { ...receiptFor().support_request, available: false, prepare_available: false,
      stopped_verified: false, phase: "unavailable", reason: "Stop this installation before updating support." } };
  await mount(async ({ calls, receipts, button, click, holdNextStatus, releaseStatus, storage, document }) => {
    assert.equal(button("Stop application").disabled, false);
    assert.equal(document.body.textContent.includes("belongs to an earlier proposal or build"), false);
    holdNextStatus();
    await click("Stop application");
    const sent = calls.find((call) => call.method === "POST").body;
    assert.equal(sent.action, "stop");
    assert.ok(sent.action_request_id);
    assert.equal(loadDeploymentActionIntent(storage, guide.id), null, "Only the exact settled command is retired.");
    const retained = receipts.at(-1);
    assert.equal(retained.status, "running", "The raw command result cannot replace the checked projection.");
    assert.equal(retained.current, true);
    assert.deepEqual(retained.update_capabilities, running.update_capabilities);
    assert.deepEqual(retained.support_request, running.support_request);
    assert.ok(button("Check support status"), "The retained support card stays visible while checking.");
    assert.equal(button("Stop application").disabled, true);
    assert.equal(button("Review update") === undefined, true);
    assert.equal(document.body.textContent.includes("belongs to an earlier proposal or build"), false);
    assert.ok(calls.filter((call) => call.url === `/api/guide/${guide.id}/deployment`).length >= 2,
      "Settlement immediately begins a fresh status read.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await releaseStatus();
    assert.equal(receipts.at(-1).status, "stopped");
    assert.equal(receipts.at(-1).current, true);
    assert.equal(button("Start installed application").disabled, false);
    assert.equal(button("Review update").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: running, ordinaryRawReply: true });
});

test("a late raw ordinary reply or status read cannot replace another guide or retire its command", async () => {
  const running = { ...receiptFor(), current: true, status: "running", process_live: true, runtime_pid: 123 };
  await mount(async ({ calls, receipts, button, click, holdReply, switchGuide, releaseReply, storage }) => {
    holdReply(); await click("Stop application");
    const sent = calls.find((call) => call.method === "POST").body;
    await switchGuide("synthetic-second-guide");
    await releaseReply();
    assert.equal(loadDeploymentActionIntent(storage, guide.id).action_request_id, sent.action_request_id);
    assert.equal(receipts.at(-1).guide_id, "synthetic-second-guide");
    assert.equal(button("Review update").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: running, ordinaryRawReply: true });
  await mount(async ({ calls, receipts, button, click, holdNextStatus, switchGuide, releaseStatus, storage }) => {
    holdNextStatus(); await click("Stop application");
    assert.equal(loadDeploymentActionIntent(storage, guide.id), null);
    assert.equal(calls.filter((call) => call.url === `/api/guide/${guide.id}/deployment`).length, 2,
      "The withheld read belongs to the first guide, before switching.");
    await switchGuide("synthetic-second-guide");
    await releaseStatus();
    assert.equal(receipts.at(-1).guide_id, "synthetic-second-guide");
    assert.equal(button("Review update").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: running, ordinaryRawReply: true });
});

test("a late raw command reply rechecks status even after a passive poll confirmed settlement", async () => {
  const running = { ...receiptFor(), current: true, status: "running", process_live: true, runtime_pid: 123 };
  await mount(async ({ calls, button, click, holdReply, poll, holdNextStatus, releaseReply, releaseStatus, storage, document }) => {
    holdReply(); await click("Stop application");
    await poll();
    assert.equal(loadDeploymentActionIntent(storage, guide.id), null);
    assert.equal(document.querySelector('[aria-label="Installation support"]').dataset.supportState, "eligible",
      "The passive read confirmed exact settlement and fresh status, while the POST reply remains pending.");
    holdNextStatus(); await releaseReply();
    assert.equal(button("Review update") === undefined, true, "The late raw reply cannot keep the old support confirmation.");
    assert.ok(button("Check support status"));
    assert.equal(button("Start installed application").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await releaseStatus();
    assert.equal(button("Review update").disabled, false);
    assert.equal(button("Start installed application").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: running, ordinaryRawReply: true });
});

test("the production card keeps planned counts and completed history distinct from a new review", () => {
  const render = (view, receipt = {}) => renderToStaticMarkup(React.createElement(DeploymentSupportPanel,
    { view, receipt, disabled: false, busy: false, onAction() {}, onCheck() {} }));
  const plan = render({ state: "prepared", action: "apply-support", selected });
  assert.match(plan, /support files to retain/);
  assert.doesNotMatch(plan, /support files retained|<details[^>]*\bopen\b/);
  assert.equal((plan.match(/Review saved/g) || []).length, 1);
  const prior = render({ state: "eligible", action: "prepare-support", selected: { ...selected, phase: "completed" } });
  assert.doesNotMatch(prior, /support files prepared|current support files/);
  const current = render({ state: "completed" }, { support_request: { phase: "current" } });
  assert.match(current, /already matches this Forge version/);
  assert.doesNotMatch(current, /update is complete/);
});

const failedBackground = (status = "failed") => ({ ...receiptFor(), status,
  current: false, current_guide_version: null, service_supervisor_pid: 123,
  service_observation: { owned: true, loaded: true, state: "waiting", pid: null },
  service: { manager: "launchd", public_summary: {
    manager: "launchd", state: "waiting", pid: null, definition_verified: true } },
  support_request: { ...receiptFor().support_request, stopped_verified: false,
    available: false, prepare_available: false, phase: "unavailable" } });

const stoppedBackground = () => ({ ...failedBackground(), status: "stopped", service_cleanup_confirmed: true,
  service_observation: { owned: true, loaded: false, state: "stopped", pid: null },
  service: { manager: "launchd", public_summary: {
    manager: "launchd", state: "stopped", pid: null, definition_verified: true } } });

test("failed background cleanup sends one Stop and waits for fresh stopped proof before private settings", async () => {
  for (const status of ["failed", "interrupted"]) await mount(async ({ calls, receipts, button, click,
    holdNextStatus, releaseStatus, storage }) => {
    assert.ok(button("Stop and verify cleanup"));
    assert.equal(button("Stop and verify cleanup").disabled, false);
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(button("Stop application") === undefined, true);
    holdNextStatus(); await click("Stop and verify cleanup");
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.equal(posts[0].body.action, "stop");
    assert.equal(posts[0].body.deployment_id, receiptFor().deployment_id);
    assert.match(posts[0].body.action_request_id, /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/);
    assert.equal(loadDeploymentActionIntent(storage, guide.id), null, "The exact settled Stop is retired.");
    assert.equal(receipts.at(-1).status, status, "Raw Stop cannot replace the checked failed projection.");
    assert.equal(button("Stop and verify cleanup").disabled, true);
    assert.equal(button("Retry saved installation").disabled, true);
    assert.equal(button("Change private settings") === undefined, true);
    await releaseStatus();
    assert.equal(receipts.at(-1).status, "stopped");
    assert.equal(button("Stop and verify cleanup") === undefined, true);
    assert.equal(button("Change private settings").disabled, false);
    assert.equal(button("Start installed application").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1,
      "Checking Stop does not start or reinstall the application.");
  }, { receipt: failedBackground(status), ordinaryRawReply: true, ordinaryStoppedReceipt: stoppedBackground() });
});

test("failed background cleanup keeps mutations closed after a status error and never replays Stop", async () => {
  await mount(async ({ calls, button, click, unavailableStatus, availableStatus, poll, storage, setReceipt }) => {
    unavailableStatus(); await click("Stop and verify cleanup");
    assert.equal(button("Stop and verify cleanup").disabled, true);
    assert.equal(button("Retry saved installation").disabled, true);
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(loadDeploymentActionIntent(storage, guide.id), null, "Only the exact settled command is retired.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    setReceipt(stoppedBackground()); availableStatus(); await poll();
    assert.equal(button("Change private settings").disabled, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: failedBackground(), ordinaryRawReply: true, ordinaryStoppedReceipt: stoppedBackground() });
});

test("failed background cleanup does not unlock private settings from unconfirmed stopped status", async () => {
  const changes = [
    { service_cleanup_confirmed: false }, { service_observation: { owned: false, loaded: false, state: "stopped", pid: null } },
    { service_observation: { owned: true, loaded: true, state: "running", pid: 456 } },
    { process_live: true, runtime_pid: 456 },
  ];
  for (const change of changes) await mount(async ({ calls, button, click }) => {
    await click("Stop and verify cleanup");
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: failedBackground(), ordinaryRawReply: true,
    ordinaryStoppedReceipt: { ...stoppedBackground(), ...change } });
});

test("failed background cleanup has no Stop control for unknown or foreign service evidence", async () => {
  const receipt = failedBackground();
  const changes = [
    { service_observation: { ...receipt.service_observation, owned: false } },
    { service_observation: { ...receipt.service_observation, state: "unknown" },
      service: { ...receipt.service, public_summary: { ...receipt.service.public_summary, state: "unknown" } } },
    { service: { ...receipt.service, public_summary: { ...receipt.service.public_summary, definition_verified: false } } },
    { service: { ...receipt.service, public_summary: { ...receipt.service.public_summary, manager: "systemd-user" } } },
    { guide_id: "foreign-guide" },
  ];
  for (const change of changes) await mount(async ({ calls, button }) => {
    assert.equal(button("Stop and verify cleanup") === undefined, true);
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { receipt: { ...receipt, ...change } });
});

test("failed background cleanup cannot send a second command while ordinary or support custody is unresolved", async () => {
  const ordinary = deploymentActionIntent(guide.id, { deployment_id: receiptFor().deployment_id,
    action: "stop" }, requestId);
  for (const support of [false, true]) {
    const intent = support ? applyIntent() : ordinary;
    await mount(async ({ calls, button, document }) => {
      assert.equal(button("Stop and verify cleanup").disabled, true);
      assert.equal(button("Change private settings") === undefined, true);
      await act(async () => button("Stop and verify cleanup").click());
      assert.equal(calls.some((call) => call.method === "POST"), false);
      assert.ok(document.body.textContent.includes("checking") || document.body.textContent.includes("Checking"));
    }, { receipt: failedBackground(), observation: { action_request: { ...intent, phase: "pending", outcome: null } },
      storage: (storage) => support ? storeDeploymentSupportIntent(storage, intent) : storeDeploymentActionIntent(storage, intent) });
  }
});

test("failed background cleanup ignores old Stop replies and checked status after switching guides", async () => {
  await mount(async ({ calls, button, click, holdReply, switchGuide, releaseReply, storage, receipts }) => {
    holdReply(); await click("Stop and verify cleanup");
    const sent = calls.find((call) => call.method === "POST").body;
    await switchGuide("synthetic-second-guide"); await releaseReply();
    assert.equal(loadDeploymentActionIntent(storage, guide.id).action_request_id, sent.action_request_id);
    assert.equal(receipts.at(-1).guide_id, "synthetic-second-guide");
    assert.equal(button("Stop and verify cleanup") === undefined, true);
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: failedBackground(), ordinaryRawReply: true, ordinaryStoppedReceipt: stoppedBackground() });
  await mount(async ({ calls, button, click, holdNextStatus, switchGuide, releaseStatus, receipts }) => {
    holdNextStatus(); await click("Stop and verify cleanup");
    await switchGuide("synthetic-second-guide"); await releaseStatus();
    assert.equal(receipts.at(-1).guide_id, "synthetic-second-guide");
    assert.equal(button("Stop and verify cleanup") === undefined, true);
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { receipt: failedBackground(), ordinaryRawReply: true, ordinaryStoppedReceipt: stoppedBackground() });
});

test("failed background native-team cleanup retains source identity and uses ordinary Stop without update support", async () => {
  const native = { ...failedBackground(), source_kind: "native-team", source_id: "f".repeat(64), build_id: null,
    update_capabilities: { background_activation_supported: false } };
  const stopped = { ...stoppedBackground(), source_kind: native.source_kind, source_id: native.source_id,
    source_digest: native.source_digest, build_id: null };
  await mount(async ({ calls, button, click, receipts }) => {
    assert.equal(button("Stop and verify cleanup").disabled, false);
    await click("Stop and verify cleanup");
    assert.equal(receipts.at(-1).source_id, native.source_id);
    assert.equal(button("Change private settings").disabled, false);
    assert.equal(button("Start installed agent team").disabled, false);
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.equal(posts[0].body.action, "stop");
    assert.equal(posts[0].body.deployment_id, native.deployment_id);
  }, { receipt: native, ordinaryRawReply: true, ordinaryStoppedReceipt: stopped });
});

test("failed background cleanup remains available with retained update history and an unavailable adapter", async () => {
  for (const status of ["completed", "staged"]) {
    const update = { update_id: "synthetic-retained-update", status, detail: "Synthetic retained update history." };
    const receipt = { ...failedBackground(), update,
      update_capabilities: { background_activation_supported: false } };
    await mount(async ({ calls, button, click }) => {
      assert.equal(button("Stop and confirm cleanup") === undefined, true, "The update adapter supplies no alternate Stop.");
      assert.ok(button("Stop and verify cleanup"));
      await click("Stop and verify cleanup");
      assert.equal(button("Change private settings").disabled, false);
      const posts = calls.filter((call) => call.method === "POST");
      assert.equal(posts.length, 1);
      assert.equal(posts[0].body.action, "stop");
    }, { receipt, ordinaryRawReply: true, ordinaryStoppedReceipt: { ...stoppedBackground(), update } });
  }
});

test("failed background cleanup preserves the single existing Stop when its update adapter is supported", async () => {
  const receipt = { ...failedBackground(), update: { update_id: "synthetic-supported-update", status: "staged" } };
  receipt.service.unit_sha256 = "f".repeat(64);
  receipt.update_capabilities = { background_activation_supported: true, requires_stopped: true,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id, package_sha256: receipt.package_sha256,
    manager: receipt.service.manager, unit_sha256: receipt.service.unit_sha256, host_manifest_sha256: "c".repeat(64) };
  await mount(async ({ calls, button, click, document }) => {
    assert.equal(button("Stop and verify cleanup") === undefined, true);
    assert.equal(button("Stop and confirm cleanup").disabled, false);
    assert.equal([...document.querySelectorAll("button")].filter((node) => node.textContent.startsWith("Stop")).length, 1);
    await click("Stop and confirm cleanup");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(calls.find((call) => call.method === "POST").body.action, "stop");
  }, { receipt, ordinaryRawReply: true, ordinaryStoppedReceipt: { ...stoppedBackground(), update: receipt.update } });
});
