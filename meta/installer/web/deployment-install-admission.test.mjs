import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { deploymentSourceKey } from "./deployment-panel.mjs";

const guide = { id: "synthetic-install-guide", version: 4, proposal: { target: "mac" } };
const checked = { guide_id: guide.id, guide_version: guide.version, build_id: "synthetic-build",
  status: "completed", current: true, acceptance_current: true, acceptance_trusted: true,
  ready_to_package: true, acceptance: { status: "pass" }, source_digest: "b".repeat(64) };
const installUrl = `/api/guide/${guide.id}/deployment`;
const stopped = { guide_id: guide.id, guide_version: guide.version, deployment_id: "synthetic-installation",
  build_id: checked.build_id, package_sha256: "a".repeat(64), source_digest: checked.source_digest,
  destination: "/synthetic-owned/project", execution_mode: "interactive", status: "stopped", process_live: false };
const freshOptions = { build_id: checked.build_id, guide_version: guide.version, current: true,
  source_digest: checked.source_digest,
  supported: true, target: "mac", host_target: "mac", default_destination: stopped.destination, console_port: 18088,
  can_use_host_provider_credentials: true, host_provider_keys: ["OPENAI_API_KEY"],
  execution_modes: [{ id: "interactive", available: true, label: "While Launch is open" },
    { id: "background", available: true, label: "Keep running in the background" }], fields: [
    { key: "OPENAI_API_KEY", label: "Model provider key", required: true, secret: true },
    { key: "APP_USER", label: "Application user", required: true, secret: false },
    { key: "OPTIONAL_NOTE", label: "Optional note", required: false, secret: false },
  ] };
const nativeGuide = { id: guide.id, version: 4, status: "ready", proposal: null, question: null,
  blueprint: { target: "mac" }, brief: { purpose: "Synthetic native study" }, readiness: { ready: true } };
const nativeOptions = { ...freshOptions, source_kind: "native-team", source_id: "c".repeat(64),
  source_digest: "d".repeat(64), build_id: null, fields: [freshOptions.fields[0]] };
const nativeStopped = { ...stopped, ...nativeOptions, source_kind: "native-team", status: "stopped", process_live: false };

// Mount the production panel with synthetic transport responses. No server,
// native admission, credentials, installation or operating-system action runs.
async function mountedDeployment(initialReceipt, verify, fixture = {}) {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], polls = new Map();
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  let receipt = initialReceipt && structuredClone(initialReceipt), nextPoll = 0, loseInstallResponse = false;
  let loseStartResponse = false, returnStartResponse = false, failStatusReads = false, observedReceiptOverride = null;
  let holdInstallResponse = false;
  const heldInstallBodies = [];
  let holdStartResponse = false;
  const heldStartBodies = [];
  let holdStageResponse = false;
  const heldStageBodies = [];
  let holdSettingsResponse = false;
  const heldSettingsBodies = [];
  let actionRequest = null, actionReadback = "pending";
  let selectedGuide = structuredClone(fixture.guide || guide), selectedBuild = structuredClone(fixture.build === null ? null : fixture.build || checked), heldReads = null;
  let selectedSourceKind = fixture.sourceKind || "application";
  const currentOptions = (target) => {
    const value = new URL(target, "http://localhost").searchParams.get("source_kind") === "native-team"
      ? { ...structuredClone(nativeOptions), guide_version: selectedGuide.version, current_guide_version: selectedGuide.version }
      : { ...structuredClone(freshOptions), build_id: selectedBuild?.build_id,
        source_digest: selectedBuild?.source_digest, current: selectedBuild?.current, current_guide_version: selectedGuide.version };
    return fixture.options ? fixture.options(value) : value;
  };
  const settingsOptions = () => ({ ...structuredClone(receipt.source_kind === "native-team" ? nativeOptions : freshOptions), scope: "verified-installed-private-configuration",
    version: selectedGuide.version, deployment_id: receipt.deployment_id, package_sha256: receipt.package_sha256,
    source_digest: receipt.source_digest, source_verified: true, requires_stopped: true,
    destination: receipt.destination, execution_mode: receipt.execution_mode,
    fields: (receipt.source_kind === "native-team" ? nativeOptions : freshOptions).fields.map((field) => ({ ...field, configured: true, removable: field.required === false })) });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true,
    setTimeout(callback, delay, ...args) {
      if (delay !== 1500) return originalSetTimeout(callback, delay, ...args);
      const token = { syntheticPoll: ++nextPoll };
      polls.set(token, () => callback(...args));
      return token;
    },
    clearTimeout(token) {
      if (polls.delete(token)) return;
      originalClearTimeout(token);
    },
    fetch: async (target, options = {}) => {
      const method = options.method || "GET";
      const body = options.body ? JSON.parse(options.body) : null;
      calls.push({ target, method, body });
      if (target === `${installUrl}/settings` && method === "POST") {
        assert.equal(holdSettingsResponse, true);
        assert.equal(body.deployment_id, receipt.deployment_id);
        receipt = { ...receipt, settings_updated_at: "2026-10-11T12:00:00Z" };
        return { ok: true, json: () => new Promise((resolve, reject) => {
          heldSettingsBodies.push({ value: structuredClone(receipt), resolve, reject, pending: true, signal: options.signal });
        }) };
      }
      if (target === installUrl && method === "POST") {
        assert.equal(loseInstallResponse || holdInstallResponse, true);
        receipt = { ...structuredClone(body.source_kind === "native-team" ? nativeStopped : stopped), status: "queued", destination: body.destination };
        if (holdInstallResponse) return { ok: true, json: () => new Promise((resolve, reject) => {
          heldInstallBodies.push({ value: structuredClone(receipt), resolve, reject, pending: true, signal: options.signal });
        }) };
        throw new Error("The synthetic connection closed before the installation response arrived.");
      }
      if (target === `${installUrl}/update-stage` && method === "POST") {
        assert.equal(holdStageResponse, true);
        assert.deepEqual(body, { deployment_id: receipt.deployment_id, version: selectedGuide.version,
          build_id: selectedBuild.build_id });
        receipt = { ...receipt, update: { update_id: "synthetic-staged-update", status: "staged",
          to_build_id: body.build_id, guide_version: body.version, source_digest: selectedBuild.source_digest,
          blockers: [], detail: "Synthetic update compatibility is staged." } };
        return { ok: true, json: () => new Promise((resolve, reject) => {
          heldStageBodies.push({ value: structuredClone(receipt), resolve, reject, pending: true, signal: options.signal });
        }) };
      }
      if (target === `${installUrl}/action` && method === "POST") {
        assert.equal(body.action, fixture.startAction || "restart");
        if (body.action === "start-background") assert.deepEqual(Object.keys(body).sort(),
          ["action", "action_request_id", "deployment_id"]);
        assert.equal(loseStartResponse || returnStartResponse || holdStartResponse, true);
        assert.match(body.action_request_id, /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/);
        actionRequest = { version: 1, guide_id: selectedGuide.id, deployment_id: body.deployment_id,
          action: body.action, update_id: body.update_id ?? null, action_request_id: body.action_request_id };
        receipt = { ...structuredClone(healthyApplication), execution_mode: "background" };
        if (holdStartResponse) {
          const value = { ...structuredClone(receipt), action_request: { ...actionRequest, phase: "settled", outcome: "returned" } };
          return { ok: true, json: () => new Promise((resolve, reject) => {
            heldStartBodies.push({ value, resolve, reject, pending: true, signal: options.signal });
          }) };
        }
        if (returnStartResponse) {
          actionReadback = { phase: "settled", outcome: "returned" };
          return { ok: true, json: async () => ({ ...structuredClone(receipt),
            action_request: { ...actionRequest, ...actionReadback } }) };
        }
        failStatusReads = true;
        throw new Error("The synthetic connection closed after the background start was admitted.");
      }
      if (target.startsWith(`${installUrl}/action/`) && method === "GET") {
        assert.ok(actionRequest);
        assert.equal(target, `${installUrl}/action/${actionRequest.deployment_id}/${actionRequest.action_request_id}`);
        if (actionReadback === "missing") return { ok: false, status: 404,
          json: async () => ({ detail: "The exact action request is not available." }) };
        if (actionReadback === "unavailable") throw new Error("The exact action request is temporarily unavailable.");
        return { ok: true, json: async () => ({ action_request: { ...actionRequest,
          phase: "pending", outcome: null, ...(typeof actionReadback === "object" ? actionReadback : {}) } }) };
      }
      if (target === installUrl && method === "GET" && failStatusReads)
        throw new Error("The synthetic installation status is temporarily unavailable.");
      if (target === installUrl && method === "GET") return receipt
        ? { ok: true, json: async () => structuredClone(observedReceiptOverride || receipt) }
        : { ok: false, status: 404, json: async () => ({ detail: "No installation was admitted." }) };
      if (target.startsWith(`${installUrl}/application-setup?`) && method === "GET") {
        assert.equal(fixture.startAction, "start-background");
        assert.equal(new URL(target, "http://localhost").searchParams.get("deployment_id"), receipt.deployment_id);
        return { ok: true, json: async () => ({ scope: "selected-stopped-application-inputs",
          guide_id: selectedGuide.id, deployment_id: receipt.deployment_id, build_id: receipt.build_id,
          package_sha256: receipt.package_sha256, guide_version: receipt.guide_version,
          available: true, kind: "restore", missing_fields: [], background: { available: true,
            persistence: "Runs while this user is logged in." } }) };
      }
      if ((target.startsWith(`${installUrl}/options?`) || target.startsWith(`${installUrl}/settings?`)) && method === "GET") {
        const value = target.startsWith(`${installUrl}/settings?`) ? settingsOptions() : currentOptions(target);
        const port = new URL(target, "http://localhost").searchParams.get("console_port");
        if (port !== null) value.console_port_observation = { scope: "local-console-port-observation", port: Number(port),
          status: "available", observed_at: "2026-10-04T06:00:00Z" };
        const result = { ok: true, json: async () => structuredClone(value) };
        if (heldReads) return new Promise((resolve) => heldReads.push({ target, result, resolve }));
        return result;
      }
      if (target === `${installUrl}/experiences` && method === "GET")
        return { ok: true, json: async () => ({ deployment_id: receipt.deployment_id,
          build_id: receipt.build_id, package_sha256: receipt.package_sha256, source_digest: receipt.source_digest,
          metadata_sha256: fixture.applicationUse ? "e".repeat(64) : null,
          scope: "declared-source", verified: false, entries: fixture.applicationUse ? [{ id: "records", label: "Records workshop",
            path: "/application", url: "http://127.0.0.1:18089/application", port: 18089, status: "declared", verified: false }] : [] }) };
      if (target === `${installUrl}/use` && method === "GET")
        return receipt.source_kind === "native-team" ? { ok: true, json: async () => ({ scope: "verified-installed-source",
          ...structuredClone(receipt), version: selectedGuide.version, application_outcomes_verified: false,
          project: { name: "Installed study team" }, entrypoint: { identity: "study:main", name: "Study companion" },
          native_runtime_available: receipt.status === "running", console_url: receipt.console_url }) }
          : fixture.applicationUse ? { ok: true, json: async () => ({ scope: "verified-installed-source",
            ...structuredClone(receipt), version: selectedGuide.version, application_outcomes_verified: false,
            project: { name: "Installed records workshop" }, entrypoint: { identity: "records:main", name: "Records companion" },
            native_runtime_available: receipt.status === "running", console_url: receipt.console_url }) }
          : { ok: false, status: 404, json: async () => ({ detail: "Synthetic first-use metadata is not provided." }) };
      assert.fail(`Unexpected synthetic transport action: ${method} ${target}`);
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.HTMLElement.prototype.scrollIntoView = () => {};
  const { createRoot } = await import("react-dom/client");
  const built = await build({ entryPoints: [fileURLToPath(new URL("deployment-panel.jsx", import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] });
  const { DeploymentPanel } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  let root = createRoot(dom.window.document.getElementById("root"));
  const document = dom.window.document;
  const button = (label) => [...document.querySelectorAll("button")].find((node) => node.textContent === label);
  const privateForm = () => document.querySelector(".deployment-form");
  const hostSelection = () => privateForm()?.querySelector('.deployment-toggle input[type="checkbox"]');
  async function enter(key, value) {
    const input = privateForm().querySelector(`[name="${key}"]`);
    assert.ok(input);
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set;
    await act(async () => {
      setter.call(input, value);
      input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
    assert.equal(input.value, value);
  }
  async function poll(nextReceipt) {
    receipt = structuredClone(nextReceipt);
    const first = polls.entries().next().value;
    assert.ok(first, "The production deployment poll must be scheduled.");
    polls.delete(first[0]);
    await act(async () => first[1]());
  }
  const render = () => act(async () => root.render(React.createElement(DeploymentPanel,
    { state: selectedGuide, plan: { project: { name: "Synthetic app", slug: "synthetic-app" } },
      build: selectedBuild, initialSourceKind: selectedSourceKind, sourceNavigation: fixture.sourceNavigation,
      designBusy: false, pendingManual: false, Icon: () => null,
      selectionScope: fixture.selectionScope, onReceipt: fixture.onReceipt })));
  try {
    await render();
    await verify({ document, window: dom.window, calls, button, privateForm, hostSelection, enter, poll,
      async revise(nextGuide, nextBuild, nextSourceKind = selectedSourceKind) {
        selectedGuide = structuredClone(nextGuide); selectedBuild = structuredClone(nextBuild);
        selectedSourceKind = nextSourceKind; await render();
      },
      holdReads() { assert.equal(heldReads, null); heldReads = []; },
      async releaseRead(index = 0, failure = false) {
        assert.ok(heldReads?.[index], "The production panel must have requested fresh host options.");
        const [pending] = heldReads.splice(index, 1);
        await act(async () => pending.resolve(failure ? { ok: false, status: 503,
          json: async () => ({ detail: "Synthetic options are temporarily unavailable." }) } : pending.result));
      },
      stopHoldingReads() { assert.equal(heldReads?.length, 0); heldReads = null; },
      loseResponse() { loseInstallResponse = true; },
      holdInstallResponse() { holdInstallResponse = true; },
      installBodyPending(index = 0) { return heldInstallBodies[index]?.pending === true; },
      async finishInstallResponse(index = 0, failure = false) {
        const pending = heldInstallBodies[index];
        assert.ok(pending?.pending, "The original installation response body must remain pending.");
        assert.equal(pending.signal, undefined, "Returning cannot abort or resend installation.");
        pending.pending = false;
        await act(async () => failure ? pending.reject(new TypeError("Synthetic late installation body failure."))
          : pending.resolve(structuredClone(pending.value)));
      },
      loseStartResponse() { loseStartResponse = true; },
      returnStartResponse() { returnStartResponse = true; },
      holdStartResponse() { holdStartResponse = true; },
      holdStageResponse() { holdStageResponse = true; },
      holdSettingsResponse() { holdSettingsResponse = true; },
      settingsBodyPending(index = 0) { return heldSettingsBodies[index]?.pending === true; },
      async finishSettingsResponse(index = 0, failure = false) {
        const pending = heldSettingsBodies[index];
        assert.ok(pending?.pending, "The original settings response body must remain pending.");
        assert.equal(pending.signal, undefined, "Reopening cannot abort or repeat the original settings save.");
        pending.pending = false;
        await act(async () => failure ? pending.reject(new TypeError("Synthetic late settings body failure."))
          : pending.resolve(structuredClone(pending.value)));
      },
      stageBodyPending(index = 0) { return heldStageBodies[index]?.pending === true; },
      async finishStageResponse(index = 0, failure = false) {
        const pending = heldStageBodies[index];
        assert.ok(pending?.pending, "The original staging response body must still be pending.");
        assert.equal(pending.signal, undefined, "Status checks cannot abort the staging request.");
        pending.pending = false;
        await act(async () => failure ? pending.reject(new TypeError("Synthetic staging response body failure."))
          : pending.resolve(structuredClone(pending.value)));
      },
      startBodyPending(index = 0) { return heldStartBodies[index]?.pending === true; },
      async finishStartResponse(index = 0, failure = false) {
        const pending = heldStartBodies[index];
        assert.ok(pending?.pending, "The original POST response.json must still be pending.");
        assert.equal(pending.signal, undefined, "Presentation recovery cannot abort the original command request.");
        pending.pending = false;
        await act(async () => failure ? pending.reject(new TypeError("Synthetic late command body failure."))
          : pending.resolve(structuredClone(pending.value)));
      },
      replaceActionRequest(value) {
        actionRequest = { ...actionRequest, ...value };
        dom.window.sessionStorage.setItem("meta-forge-deployment-action-v1:" + selectedGuide.id, JSON.stringify(actionRequest));
      },
      loseStatusReads() { failStatusReads = true; },
      restoreStatusReads(value = null) { failStatusReads = false; observedReceiptOverride = value; },
      actionReadback(value) { actionReadback = value; },
      async remount() {
        await act(async () => root.unmount());
        assert.equal(polls.size, 0);
        root = createRoot(dom.window.document.getElementById("root"));
        await render();
      },
      currentReceipt: () => structuredClone(receipt) });
  } finally {
    await act(async () => root.unmount());
    await act(async () => {
      for (const pending of heldInstallBodies) if (pending.pending) {
        pending.pending = false; pending.resolve(structuredClone(pending.value));
      }
      for (const pending of heldStartBodies) if (pending.pending) {
        pending.pending = false; pending.resolve(structuredClone(pending.value));
      }
      for (const pending of heldStageBodies) if (pending.pending) {
        pending.pending = false; pending.resolve(structuredClone(pending.value));
      }
    });
    const pendingPolls = polls.size;
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
    assert.equal(pendingPolls, 0, "Unmount must cancel the production deployment poll.");
  }
}

const healthyApplication = { ...stopped, current: true, status: "running", process_live: true, health: "ready",
  console_url: "http://127.0.0.1:18088/console", bootstrap: { complete: true, ready: true, counts: { broken: 0 }, identities: {} } };

const recoverableBackgroundStopped = { ...stopped, execution_mode: "background", runtime_pid: null,
  service: { manager: "launchd" }, service_observation: { owned: true, loaded: false, state: "stopped" } };

const productStopped = { ...stopped, target: "local", phase: "configure", runtime_pid: null,
  runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1",
    assembly: { sha256: "c".repeat(64) }, model_catalog: { sha256: "d".repeat(64) },
    meta: { version: "0.9.0", sha256: "e".repeat(64) }, toolkit: { version: "0.1.0", sha256: "f".repeat(64) } },
  installed_candidate_runtime_proof: { source: "meta-toolkit-product", version: "0.9.0",
    assembly_sha256: "c".repeat(64), catalog_sha256: "d".repeat(64), sdk: {
      meta: { version: "0.9.0", sha256: "e".repeat(64) }, toolkit: { version: "0.1.0", sha256: "f".repeat(64) } } } };

test("mounted completed Restore background choice posts only the exact journal IDs and waits for fresh status", async () => {
  await mountedDeployment(productStopped, async ({ window, calls, button, returnStartResponse }) => {
    assert.equal(button("Run in background").disabled, false);
    returnStartResponse();
    await act(async () => button("Run in background").click());
    const posts = calls.filter(call => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.deepEqual(Object.keys(posts[0].body).sort(), ["action", "action_request_id", "deployment_id"]);
    assert.equal(posts[0].body.action, "start-background");
    assert.equal(posts[0].body.deployment_id, productStopped.deployment_id);
    assert.equal(window.sessionStorage.getItem("meta-forge-deployment-action-v1:" + guide.id), null);
    assert.equal(button("Run in background") === undefined, true);
    assert.ok(calls.some(call => call.method === "GET" && call.target === installUrl));
  }, { startAction: "start-background" });
});

test("lost completed Restore background response retains the exact intent across remount without dispatching again", async () => {
  await mountedDeployment(productStopped, async ({ window, calls, button, loseStartResponse,
    restoreStatusReads, actionReadback, remount }) => {
    loseStartResponse();
    await act(async () => button("Run in background").click());
    const post = calls.find(call => call.method === "POST");
    const key = "meta-forge-deployment-action-v1:" + guide.id;
    const retained = window.sessionStorage.getItem(key);
    assert.equal(JSON.parse(retained).action, "start-background");
    assert.equal(JSON.parse(retained).action_request_id, post.body.action_request_id);
    restoreStatusReads(productStopped);
    actionReadback({ phase: "pending", outcome: null });
    await remount();
    assert.equal(window.sessionStorage.getItem(key), retained);
    assert.equal(button("Run in background") === undefined, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.ok(calls.some(call => call.method === "GET" && call.target ===
      `${installUrl}/action/${productStopped.deployment_id}/${post.body.action_request_id}`));
  }, { startAction: "start-background" });
});

const privateRefreshCandidate = { ...checked, build_id: "synthetic-private-runtime-refresh", source_digest: "e".repeat(64) };
const privateRefreshReason = "This private runtime candidate is admitted only through the checked update of an existing stopped installation. Use that exact update or its forward recovery.";
const refreshAvailability = value => ({ ...value, fresh_install: value.build_id === privateRefreshCandidate.build_id
  ? { available: false, reason: privateRefreshReason } : { available: true, reason: null } });

test("private runtime refresh without an installed target hides fresh setup and an ordinary source restores it", async () => {
  await mountedDeployment(null, async ({ document, calls, button, privateForm, revise }) => {
    assert.match(document.body.textContent, /private runtime candidate.*checked update.*existing stopped installation/);
    assert.equal(button("Install on this computer") === undefined, true);
    assert.equal(button("Install into another folder") === undefined, true);
    assert.equal(button("Install and start") === undefined, true);
    assert.equal(privateForm() === null, true);
    assert.equal(button("Stage accepted update") === undefined, true, "No installed target is fabricated for update staging.");
    assert.ok(calls.every(call => call.method === "GET"), "Checking availability cannot install or update.");

    await revise(guide, checked);
    assert.equal(document.body.textContent.includes(privateRefreshReason), false, "The candidate restriction cannot follow an ordinary source.");
    assert.ok(button("Install on this computer"));
    assert.equal(button("Install on this computer").disabled, false);
    await act(async () => button("Install on this computer").click());
    assert.ok(privateForm(), "The ordinary source still opens its actual private setup.");
    assert.ok(button("Install and start"));
    assert.ok(calls.every(call => call.method === "GET"));
  }, { build: privateRefreshCandidate, options: refreshAvailability });
});

test("private runtime refresh preserves installed settings drafts and explicitly stages its exact update without fresh setup", async () => {
  await mountedDeployment(stopped, async ({ document, calls, button, privateForm, enter, poll,
    holdStageResponse, stageBodyPending, finishStageResponse }) => {
    assert.match(document.body.textContent, /private runtime candidate.*checked update.*existing stopped installation/);
    assert.equal(button("Install into another folder") === undefined, true);
    assert.equal(button("Install on this computer") === undefined, true);
    assert.equal(privateForm() === null, true);
    assert.ok(button("Stage accepted update"));
    assert.equal(button("Stage accepted update").disabled, false, "Local update compatibility stays available despite fresh-install refusal.");
    assert.equal(button("Change private settings").disabled, false, "The restriction belongs to the candidate, not installed settings.");
    await act(async () => button("Change private settings").click());
    assert.ok(privateForm());
    assert.ok(button("Save private settings"));
    assert.equal(button("Install and start") === undefined, true, "The only mounted form edits the existing installation.");
    const draft = "  Preserve my exact installed organiser draft.  ";
    await enter("APP_USER", draft);
    await poll(stopped);
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, draft);
    assert.ok(calls.every(call => call.method === "GET"), "Reading status does not save, stage or install the private draft.");
    await act(async () => button("Cancel").click());
    assert.equal(privateForm() === null, true);
    assert.equal(button("Install into another folder") === undefined, true);

    holdStageResponse();
    await act(async () => button("Stage accepted update").click());
    assert.equal(stageBodyPending(), true);
    const commands = calls.filter(call => call.method === "POST");
    assert.deepEqual(commands, [{ target: `${installUrl}/update-stage`, method: "POST", body: {
      deployment_id: stopped.deployment_id, version: guide.version, build_id: privateRefreshCandidate.build_id } }]);
    await finishStageResponse();
    assert.equal(document.querySelector(".deployment-update")?.dataset.updateStatus, "staged");
    assert.equal(button("Install into another folder") === undefined, true);
    assert.equal(button("Install and start") === undefined, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1, "Explicit staging never applies an update or fresh installation.");
  }, { build: privateRefreshCandidate, options: refreshAvailability });
});

for (const mismatch of ["stale", "wrong-source"]) test(`private runtime refresh restriction ignores ${mismatch} options`, async () => {
  await mountedDeployment(null, async ({ document, calls, button, privateForm }) => {
    assert.equal(document.body.textContent.includes(privateRefreshReason), false, "Only the current selected source may explain its restriction.");
    assert.equal(button("Install on this computer") === undefined, true);
    assert.equal(button("Install into another folder") === undefined, true);
    assert.equal(button("Install and start") === undefined, true);
    assert.equal(privateForm() === null, true);
    assert.ok(calls.every(call => call.method === "GET"));
  }, { build: privateRefreshCandidate, options: value => ({ ...refreshAvailability(value),
    ...(mismatch === "stale" ? { guide_version: guide.version - 1 }
      : { source_kind: "native-team", build_id: null, source_id: "f".repeat(64) }) }) });
});

for (const scenario of [{ receipt: stopped, failure: false, label: "stopped success" },
  { receipt: healthyApplication, failure: true, label: "running failure" }])
test(`staging wait explains the pending response without replay or readback release: ${scenario.label}`, async () => {
  const candidate = { ...checked, build_id: "synthetic-update-candidate" };
  await mountedDeployment(scenario.receipt, async ({ document, calls, button, holdStageResponse,
    stageBodyPending, finishStageResponse, loseStatusReads, restoreStatusReads }) => {
    assert.ok(button("Stage accepted update"), "The panel has automatically checked the accepted candidate's deployment options.");
    holdStageResponse();
    await act(async () => button("Stage accepted update").click());
    const waiting = () => document.querySelector('[data-stage-response="pending"]');
    assert.ok(waiting());
    assert.equal(waiting().getAttribute("role"), "status");
    assert.match(waiting().textContent, /checking update compatibility/);
    assert.match(waiting().textContent, /Nothing is being applied/);
    assert.match(waiting().textContent, /actions will be available when this check returns/);
    assert.equal(stageBodyPending(), true);
    const before = calls.length;
    await act(async () => waiting().querySelector("button").click());
    assert.ok(calls.slice(before).some(call => call.target === installUrl && call.method === "GET"));
    assert.equal(calls.filter(call => call.method === "POST").length, 1, "Checking status never replays or applies the staged request.");
    assert.equal(stageBodyPending(), true, "A committed stage observation does not settle its original request.");
    assert.equal(document.querySelector(".deployment-update").dataset.updateStatus, "staged");
    assert.ok(waiting(), "The original pending response explanation remains after status readback.");
    assert.equal(button("Return to checked installation") === undefined, true, "Stage has no exact request-id settlement authority.");
    if (scenario.receipt.status === "stopped") {
      assert.equal(button("Apply and verify update").disabled, true);
      assert.equal(button("Change private settings").disabled, true);
    } else assert.equal(button("Stop application").disabled, true);
    if (scenario.receipt.status === "stopped") {
      loseStatusReads();
      await act(async () => waiting().querySelector("button").click());
      assert.equal(waiting().querySelector("button") === null, true, "Reconnect guidance supplies the existing status action.");
      assert.equal([...document.querySelectorAll("button")].filter(node => node.textContent === "Check installation status").length, 1);
      assert.equal(stageBodyPending(), true);
      restoreStatusReads();
      await act(async () => button("Check installation status").click());
      assert.ok(waiting().querySelector("button"));
      assert.equal(button("Apply and verify update").disabled, true);
      assert.equal(calls.filter(call => call.method === "POST").length, 1);
    }
    await finishStageResponse(0, scenario.failure);
    assert.equal(waiting() === null, true, "The presentation marker clears only when the original response settles.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    if (scenario.receipt.status === "stopped") assert.equal(button("Apply and verify update").disabled, false);
    else assert.equal(button("Stop application").disabled, false);
  }, { build: candidate });
});

test("staging wait retires with guide selection and ignores a late response failure", async () => {
  let token = {}, pendingSelection = false;
  const scope = { token: () => token, capture(selected, expected) {
    const captured = token;
    return () => !pendingSelection && selected.id === guide.id && token === captured &&
      (expected === undefined || expected === captured);
  } };
  const candidate = { ...checked, build_id: "synthetic-update-candidate" };
  await mountedDeployment(stopped, async ({ document, calls, button, holdStageResponse,
    stageBodyPending, finishStageResponse, revise }) => {
    holdStageResponse();
    await act(async () => button("Stage accepted update").click());
    const check = document.querySelector('[data-stage-response="pending"] button');
    assert.ok(check);
    token = {}; pendingSelection = true;
    const before = calls.length;
    await act(async () => check.click());
    assert.equal(calls.length, before, "A retained status button cannot read a retired guide selection.");
    assert.equal(stageBodyPending(), true, "Navigation does not abort the original staging response.");
    await revise(guide, candidate);
    assert.equal(document.querySelector('[data-stage-response="pending"]') === null, true);
    const beforeLateReply = calls.length;
    await finishStageResponse(0, true);
    assert.equal(document.querySelector(".deployment-panel > .deployment-error") === null, true);
    assert.equal(calls.length, beforeLateReply, "An old failure cannot observe or mutate the retired selection.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  }, { build: candidate, selectionScope: scope });
});

for (const failure of [false, true]) test(`installed command response recovery leaves the original body pending and fences late ${failure ? "error" : "success"}`, async () => {
  await mountedDeployment(recoverableBackgroundStopped, async ({ document, window, calls, button, enter,
    holdStartResponse, startBodyPending, finishStartResponse, actionReadback, restoreStatusReads }) => {
    const savedTask = "Keep the exact unsent job while returning to the checked installation.";
    const task = () => document.querySelector('.deployment-use-field textarea');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(task(), savedTask); task().dispatchEvent(new window.Event("input", { bubbles: true })); });
    holdStartResponse();
    await act(async () => button("Start installed application").click());
    assert.equal(startBodyPending(), true);
    assert.equal(button("Return to checked installation") === undefined, true, "A pending command has no recovery authority.");
    actionReadback({ phase: "settled", outcome: "returned" });
    restoreStatusReads(recoverableBackgroundStopped);
    const before = calls.length;
    await act(async () => button("Check installation status").click());
    assert.deepEqual(calls.slice(before).filter(call => call.target.includes("/action/") || call.target === installUrl)
      .map(call => call.target), [calls.find(call => call.target.includes("/action/")).target, installUrl]);
    const recover = button("Return to checked installation");
    assert.ok(recover, "Exact command settlement and a fresh matching receipt offer explicit presentation recovery.");
    assert.match(document.body.textContent, /original command response.*pending/i);
    assert.equal(button("Start installed application").disabled, true, "Readback alone does not release the local owner.");
    await act(async () => recover.click());
    assert.equal(startBodyPending(), true, "Returning leaves the original response promise pending.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.equal(task().value, savedTask);
    assert.equal(button("Start installed application").disabled, false);
    assert.equal(button("Return to checked installation") === undefined, true);
    await act(async () => button("Change private settings").click());
    await enter("APP_USER", "Keep this exact new private draft");
    const callsBeforeLateReply = calls.length;
    await finishStartResponse(0, failure);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "Keep this exact new private draft");
    assert.equal(task().value, savedTask);
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "stopped");
    assert.equal(document.querySelector(".deployment-panel > .deployment-error") === null, true);
    assert.equal(calls.length, callsBeforeLateReply, "The detached response cannot trigger another status read or mutation.");
  }, { applicationUse: true });
});

test("installed command response recovery requires its exact command and a current matching fresh receipt", async () => {
  await mountedDeployment(recoverableBackgroundStopped, async ({ document, calls, button, holdStartResponse,
    actionReadback, restoreStatusReads, loseStatusReads, replaceActionRequest, poll, finishStartResponse }) => {
    holdStartResponse();
    await act(async () => button("Start installed application").click());
    for (const evidence of [{ phase: "pending" }, { phase: "settled", outcome: "returned", action_request_id: "11111111-1111-4111-8111-111111111111" },
      { phase: "settled", outcome: "returned", deployment_id: "another-installation" }]) {
      actionReadback(evidence);
      await act(async () => button("Check installation status").click());
      assert.equal(button("Return to checked installation") === undefined, true);
    }
    actionReadback({ phase: "settled", outcome: "returned" });
    loseStatusReads();
    await act(async () => button("Check installation status").click());
    assert.equal(button("Return to checked installation") === undefined, true, "Settled command alone cannot replace a failed receipt read.");
    restoreStatusReads({ ...healthyApplication, deployment_id: "another-installation" });
    await act(async () => button("Check installation status").click());
    assert.equal(button("Return to checked installation") === undefined, true, "A fresh foreign installation cannot release this owner.");
    replaceActionRequest({ action_request_id: "22222222-2222-4222-8222-222222222222" });
    restoreStatusReads({ ...healthyApplication, execution_mode: "background" });
    await poll({ ...healthyApplication, execution_mode: "background" });
    assert.equal(button("Return to checked installation") === undefined, true, "Another settled command cannot release this pending command's owner.");
    assert.equal(button("Stop application").disabled, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    const beforeLateReply = calls.length;
    await finishStartResponse(0, true);
    assert.equal(document.querySelector(".deployment-panel > .deployment-error")?.textContent ?? null, null,
      "An old response cannot replace presentation after a different command was checked.");
    assert.equal(calls.length, beforeLateReply, "The superseded response cannot start another readback.");
  });
});

test("installed command response recovery cannot adopt a retired selection before rerender", async () => {
  let token = {}, pendingSelection = false;
  const scope = { token: () => token, capture(selected, expected) {
    const captured = token;
    return () => !pendingSelection && selected.id === guide.id && token === captured && (expected === undefined || expected === captured);
  } };
  await mountedDeployment(recoverableBackgroundStopped, async ({ document, calls, button, holdStartResponse,
    actionReadback, restoreStatusReads, startBodyPending, finishStartResponse }) => {
    holdStartResponse(); await act(async () => button("Start installed application").click());
    actionReadback({ phase: "settled", outcome: "returned" });
    restoreStatusReads({ ...healthyApplication, execution_mode: "background" });
    await act(async () => button("Check installation status").click());
    const recover = button("Return to checked installation"); assert.ok(recover);
    token = {}; pendingSelection = true;
    const before = calls.length;
    await act(async () => recover.click());
    assert.equal(startBodyPending(), true);
    assert.equal(button("Stop application").disabled, true);
    await finishStartResponse(0, true);
    assert.equal(calls.length, before, "Retired selection cannot adopt or publish the late failure.");
    assert.equal(document.querySelector(".deployment-panel > .deployment-error") === null, true);
  }, { selectionScope: scope });
});

test("installed command response recovery never lets a remounted old response release a new busy owner", async () => {
  await mountedDeployment(recoverableBackgroundStopped, async ({ calls, button, holdStartResponse, actionReadback,
    restoreStatusReads, remount, startBodyPending, finishStartResponse }) => {
    holdStartResponse(); await act(async () => button("Start installed application").click());
    actionReadback({ phase: "settled", outcome: "returned" }); restoreStatusReads(recoverableBackgroundStopped);
    await act(async () => button("Check installation status").click());
    assert.ok(button("Return to checked installation"));
    await remount();
    assert.equal(button("Return to checked installation") === undefined, true, "The new mount does not inherit the previous local response owner.");
    await act(async () => button("Start installed application").click());
    assert.equal(startBodyPending(0), true); assert.equal(startBodyPending(1), true);
    const before = calls.length;
    await finishStartResponse(0, true);
    assert.equal(calls.length, before);
    assert.equal(startBodyPending(1), true);
    assert.equal(button("Start installed application").disabled, true, "The previous mount's response cannot release the new operation.");
    assert.equal(calls.filter(call => call.method === "POST").length, 2, "Only the two explicit human Start clicks dispatch commands.");
  });
});

test("a lost committed background Start and failed observation block another mutation until read-only recovery", async () => {
  await mountedDeployment({ ...stopped, execution_mode: "background" }, async ({ document, window, calls, button,
    loseStartResponse, restoreStatusReads, actionReadback, remount }) => {
    const draft = document.querySelector('.deployment-use-field textarea');
    const savedTask = "Keep this unsent task while checking the already admitted start.";
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => {
      setter.call(draft, savedTask);
      draft.dispatchEvent(new window.Event("input", { bubbles: true }));
    });
    assert.equal(button("Start installed application").disabled, false);
    loseStartResponse();
    await act(async () => button("Start installed application").click());
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "stopped",
      "The failed observation keeps the previous receipt visible, without claiming a new status.");
    assert.equal(button("Start installed application").disabled, true,
      "An unobserved admitted start must not offer a second restart of the background service.");
    await act(async () => button("Start installed application").click());
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(document.querySelector('.deployment-use-field textarea').value, savedTask);
    assert.match(document.querySelector(".deployment-panel > .deployment-error").textContent, /after the background start was admitted/);
    const check = button("Check installation status");
    assert.ok(check, "Recovery offers observation rather than another start.");
    restoreStatusReads({ ...stopped, execution_mode: "background", updated_at: "2026-10-08T12:00:00Z" });
    await act(async () => check.click());
    assert.equal(button("Start installed application").disabled, true,
      "An ordinary successful stopped observation, even with a newer timestamp, does not resolve the pending command.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await remount();
    assert.equal(button("Start installed application").disabled, true,
      "The exact unresolved command survives remount instead of offering a second Start.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    actionReadback({ phase: "settled", outcome: "returned" });
    restoreStatusReads();
    const beforeRecovery = calls.length;
    await act(async () => button("Check installation status").click());
    assert.deepEqual(calls.slice(beforeRecovery).filter((call) => call.target === installUrl || call.target.includes("/action/"))
      .map((call) => call.target), [calls.find((call) => call.target.includes("/action/")).target, installUrl],
      "The fresh installation receipt is read only after the exact command has settled.");
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "running");
    assert.equal(button("Start installed application") === undefined, true);
    assert.equal(button("Stop application").disabled, false);
    assert.equal(button("Check installation status") === undefined, true);
    assert.equal(document.querySelector('.deployment-use-field textarea').value, savedTask);
    // Error display was retained throughout readback. A remount starts a fresh display.
    assert.equal(calls.filter((call) => call.method === "POST").length, 1,
      "Successful observation never replays the admitted operation.");
  }, { applicationUse: true });
});

test("absent, unknown or foreign settled action evidence cannot unlock a lost Start", async () => {
  await mountedDeployment({ ...stopped, execution_mode: "background" }, async ({ calls, button,
    loseStartResponse, restoreStatusReads, actionReadback }) => {
    loseStartResponse();
    await act(async () => button("Start installed application").click());
    restoreStatusReads({ ...stopped, execution_mode: "background" });
    for (const evidence of ["missing", "unavailable", { phase: "unknown" },
      { phase: "settled", outcome: "returned", deployment_id: "foreign-installation" },
      { phase: "settled", outcome: "returned", guide_id: "foreign-guide" },
      { phase: "settled", outcome: "returned", action_request_id: "00000000-0000-0000-0000-000000000000" },
      { phase: "settled", outcome: "returned", action: "stop" },
      { phase: "settled", outcome: "returned", update_id: "foreign-update" }]) {
      actionReadback(evidence);
      await act(async () => button("Check installation status").click());
      assert.equal(button("Start installed application").disabled, true);
      assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    }
    actionReadback({ phase: "settled", outcome: "error" });
    await act(async () => button("Check installation status").click());
    assert.equal(button("Start installed application").disabled, false,
      "A settled failed host call and fresh stopped receipt permit a new explicit attempt.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("a directly returned exact settled Start retires its transport intent without replay", async () => {
  await mountedDeployment({ ...stopped, execution_mode: "background" }, async ({ document, calls, button,
    returnStartResponse, remount }) => {
    returnStartResponse();
    await act(async () => button("Start installed application").click());
    assert.equal(button("Stop application").disabled, false);
    const submitted = calls.find(call => call.method === "POST");
    assert.ok(calls.some(call => call.method === "GET" && call.target ===
      `${installUrl}/action/${submitted.body.deployment_id}/${submitted.body.action_request_id}`),
    "The exact host action observation confirms the returned response before enabling another action.");
    assert.ok(calls.some((call, index) => index > calls.indexOf(submitted) && call.method === "GET" && call.target === installUrl),
      "Command settlement still needs a fresh installation status observation.");
    assert.equal(document.querySelector(".deployment-panel > .deployment-error") === null, true);
    const readsBeforeRemount = calls.filter(call => call.target.includes("/action/")).length;
    await remount();
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "running");
    assert.equal(button("Stop application").disabled, false);
    assert.equal(calls.filter((call) => call.target.includes("/action/")).length, readsBeforeRemount,
      "The exact settled intent is retired and remount does not recover it again.");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("reload follows the installed custom application before the latest build loads and keeps its checked UI handoff", async () => {
  await mountedDeployment(healthyApplication, async ({ document, calls }) => {
    assert.equal(document.querySelector('input[name="deployment-source"][value="application"]').checked, true);
    assert.equal(document.querySelector(".deployment-panel h2").textContent, "Your custom application is running");
    assert.match(document.querySelector(".deployment-location").textContent, /Installed custom application - proposal 4\. Current proposal 4/);
    assert.match(document.querySelector(".deployment-location").textContent, /matches the current proposal/);
    assert.doesNotMatch(document.body.textContent, /earlier proposal or build/);
    const app = document.querySelector('section[aria-label="Use your application"]');
    assert.equal(app.querySelector("h3").textContent, "Use Installed records workshop");
    assert.equal([...app.querySelectorAll("a")].find((node) => node.textContent.startsWith("Open Records workshop")).href,
      "http://127.0.0.1:18089/application");
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { guide: nativeGuide, build: null, sourceKind: "native-team", applicationUse: true });
});

test("an earlier installed custom app stays distinct from the current proposal and manual installation choice survives polling", async () => {
  const laterGuide = { ...nativeGuide, version: 5 };
  const candidate = { ...checked, guide_version: 5, build_id: "later-candidate", status: "failed", ready_to_package: false };
  await mountedDeployment({ ...healthyApplication, current: false }, async ({ document, calls, poll, revise }) => {
    assert.equal(document.querySelector('input[name="deployment-source"][value="application"]').checked, true);
    assert.match(document.querySelector(".deployment-location").textContent, /proposal 4\. Current proposal 5/);
    assert.match(document.body.textContent, /installed custom application belongs to an earlier proposal or build/);
    assert.ok(document.querySelector('section[aria-label="Use your application"]'));
    await act(async () => document.querySelector('input[name="deployment-source"][value="native-team"]').click());
    await poll({ ...healthyApplication, current: false });
    await revise(laterGuide, candidate, "application");
    assert.equal(document.querySelector('input[name="deployment-source"][value="native-team"]').checked, true);
    assert.match(document.body.textContent, /Agent team is selected for a separate installation/);
    assert.equal(document.querySelector(".deployment-panel h2").textContent, "Your custom application is running");
    assert.ok(document.querySelector('section[aria-label="Use your application"]'));
    assert.equal(calls.some((call) => call.method === "POST"), false);
  }, { guide: laterGuide, build: candidate, sourceKind: "native-team", applicationUse: true });
});

test("an explicit navigation choice remains authoritative over the installed default", async () => {
  await mountedDeployment(healthyApplication, async ({ document, poll }) => {
    assert.equal(document.querySelector('input[name="deployment-source"][value="native-team"]').checked, true);
    assert.match(document.body.textContent, /Agent team is selected for a separate installation/);
    await poll(healthyApplication);
    assert.equal(document.querySelector('input[name="deployment-source"][value="native-team"]').checked, true);
    assert.equal(document.querySelector(".deployment-panel h2").textContent, "Your custom application is running");
  }, { guide: nativeGuide, sourceKind: "application", applicationUse: true, sourceNavigation: {
    guideId: nativeGuide.id, version: nativeGuide.version, sourceKind: "native-team",
    key: deploymentSourceKey(checked, nativeGuide, "native-team"),
  } });
});

test("reload identifies an installed native team even when the proposed default is a custom application", async () => {
  await mountedDeployment({ ...nativeStopped, ...healthyApplication, ...nativeOptions,
    source_kind: "native-team", status: "running", process_live: true }, async ({ document }) => {
    assert.equal(document.querySelector('input[name="deployment-source"][value="native-team"]').checked, true);
    assert.equal(document.querySelector(".deployment-panel h2").textContent, "Your agent team is running");
    assert.ok(document.querySelector('section[aria-label="Use your agent team"] a'));
    assert.equal(document.querySelector('section[aria-label="Use your application"]') === null, true);
  }, { guide: nativeGuide, sourceKind: "application" });
});

test("an installed choice cannot borrow readiness or entry links from an unmatched or unhealthy receipt", async () => {
  await mountedDeployment({ ...healthyApplication, guide_id: "another-guide" }, async ({ document, calls }) => {
    assert.equal(document.querySelector('input[name="deployment-source"][value="native-team"]').checked, true);
    assert.equal(document.querySelector('section[aria-label="Use your application"]') === null, true);
    assert.doesNotMatch(document.querySelector(".deployment-panel h2").textContent, /is running/);
    assert.equal(calls.some((call) => /\/(use|experiences)$/.test(call.target)), false);
  }, { guide: nativeGuide, sourceKind: "native-team", applicationUse: true });
  await mountedDeployment({ ...healthyApplication, process_live: false }, async ({ document }) => {
    assert.equal(document.querySelector('input[name="deployment-source"][value="application"]').checked, true);
    assert.match(document.querySelector(".deployment-panel h2").textContent, /health check/);
    assert.equal(document.querySelector('section[aria-label="Use your application"] a') === null, true);
  }, { guide: nativeGuide, sourceKind: "native-team", applicationUse: true });
});

test("stopped applications lead with Start and keep their draft through a running observation in either execution mode", async () => {
  for (const executionMode of ["interactive", "background"]) {
    await mountedDeployment({ ...healthyApplication, status: "stopped", process_live: false,
      execution_mode: executionMode }, async ({ document, window, calls, poll }) => {
      const app = () => document.querySelector('section[aria-label="Use your application"]');
      assert.equal(app().querySelector("h3").textContent, "Your application is stopped");
      assert.equal(app().querySelector(".deployment-use-interfaces h4").textContent, "Open after starting the application");
      assert.match(app().textContent, /Start the installed application to open its interfaces/);
      assert.doesNotMatch(app().textContent, /Sign in if asked|Open the application UI/);
      assert.equal(app().querySelector("a") === null, true);
      assert.match(document.body.textContent, /The application is stopped\. Closing this page leaves it stopped\./);
      assert.doesNotMatch(document.body.textContent, /Closing this page keeps the application running|can keep running after Forge closes/);
      const draft = app().querySelector(".deployment-use-field textarea");
      const savedTask = "Keep this unsent job while the installed application starts.";
      assert.equal(draft.disabled, false);
      const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
      await act(async () => {
        setter.call(draft, savedTask);
        draft.dispatchEvent(new window.Event("input", { bubbles: true }));
      });
      await poll({ ...healthyApplication, execution_mode: executionMode });
      assert.equal(app().querySelector("h3").textContent, "Use Installed records workshop");
      assert.equal(app().querySelector(".deployment-use-interfaces h4").textContent, "Open the application UI");
      assert.match(app().textContent, /Sign in if asked/);
      assert.equal(app().querySelector(".deployment-use-field textarea").value, savedTask);
      assert.equal([...app().querySelectorAll("a")].find((node) => node.textContent.startsWith("Open Records workshop")).href,
        "http://127.0.0.1:18089/application");
      assert.doesNotMatch(document.body.textContent, /Closing this page leaves it stopped/);
      assert.match(document.body.textContent, executionMode === "background"
        ? /managed by your operating system and can keep running after Forge closes/
        : /Closing this page keeps the application running/);
      assert.equal(calls.some((call) => call.method === "POST"), false);
    }, { applicationUse: true });
  }
});

for (const failure of [false, true]) test(`fresh installation response recovery preserves admission and fences late ${failure ? "error" : "success"}`, async () => {
  await mountedDeployment(null, async ({ document, window, calls, button, privateForm, hostSelection, enter, poll,
    holdInstallResponse, installBodyPending, finishInstallResponse, currentReceipt }) => {
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-first-install-choice");
    await act(async () => hostSelection().click());
    holdInstallResponse();
    await act(async () => button("Install and start").click());
    assert.equal(installBodyPending(), true);
    assert.equal(button("Return to checked installation") === undefined, true);
    const admitted = { ...currentReceipt(), ...healthyApplication };
    for (const changed of [{ build_id: "another-build" }, { source_digest: "f".repeat(64) },
      { destination: "/synthetic-owned/another-project" }, { execution_mode: "background" },
      { guide_version: guide.version + 1 }]) {
      await poll({ ...admitted, ...changed });
      assert.equal(button("Return to checked installation") === undefined, true,
        "A different source, destination, lifecycle or proposal cannot release this installation presentation.");
    }
    await poll(admitted);
    const recover = button("Return to checked installation");
    assert.ok(recover, "A fresh exact admission offers an explicit return while the original POST body is held.");
    assert.match(document.body.textContent, /installation has been admitted.*original installation response is still pending/i);
    assert.doesNotMatch(document.body.textContent, /command has finished/);
    assert.equal(privateForm() === null, true, "Observed admission clears private inputs before returning.");
    assert.equal(button("Stop application").disabled, true, "Polling alone does not release this local presentation owner.");
    const draft = () => document.querySelector('.deployment-use-field textarea');
    const task = "Keep my unsent first useful task while recovering the saved installation.";
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(draft(), task); draft().dispatchEvent(new window.Event("input", { bubbles: true })); });
    await act(async () => recover.click());
    assert.equal(installBodyPending(), true);
    assert.equal(button("Stop application").disabled, false);
    assert.equal(button("Return to checked installation") === undefined, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.equal(draft().value, task);
    await poll({ ...admitted, status: "stopped", process_live: false });
    await act(async () => button("Change private settings").click());
    await enter("APP_USER", "Keep this newer private draft");
    const beforeLateReply = calls.length;
    await finishInstallResponse(0, failure);
    assert.equal(document.querySelector('[name="APP_USER"]').value, "Keep this newer private draft");
    assert.equal(draft().value, task);
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "stopped");
    assert.equal(document.querySelector(".deployment-panel > .deployment-error") === null, true);
    assert.equal(calls.length, beforeLateReply, "The detached body cannot publish old status, read again or resend installation.");
  }, { applicationUse: true });
});

test("fresh installation response recovery uses the server home for tilde and dot-segment destinations", async () => {
  await mountedDeployment(null, async ({ document, window, calls, button, privateForm, hostSelection, enter, poll,
    holdInstallResponse, installBodyPending, finishInstallResponse, currentReceipt }) => {
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-path-choice");
    await act(async () => hostSelection().click());
    const folder = privateForm().querySelector('input:not([name])');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    await act(async () => { setter.call(folder, " ~/src/./temporary/../team/ ");
      folder.dispatchEvent(new window.Event("input", { bubbles: true })); });
    holdInstallResponse(); await act(async () => button("Install and start").click());
    assert.equal(calls.find(call => call.method === "POST").body.destination, " ~/src/./temporary/../team/ ");
    const admitted = { ...currentReceipt(), ...healthyApplication, destination: "/synthetic-home/src/team" };
    await poll(admitted);
    const recover = button("Return to checked installation");
    assert.ok(recover, "The exact canonical destination is derived from the explicit server home, not browser home or default path shape.");
    await act(async () => recover.click());
    assert.equal(installBodyPending(), true);
    assert.equal(button("Stop application").disabled, false);
    const beforeLateReply = calls.length;
    await finishInstallResponse(0, true);
    assert.equal(calls.length, beforeLateReply);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "running");
  }, { options: (value) => ({ ...value, home_directory: "/synthetic-home" }) });
  await mountedDeployment(null, async ({ window, calls, button, privateForm, hostSelection, enter, poll,
    holdInstallResponse, currentReceipt }) => {
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-unsupported-home-choice");
    await act(async () => hostSelection().click());
    const folder = privateForm().querySelector('input:not([name])');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    await act(async () => { setter.call(folder, "~another-user/src/team");
      folder.dispatchEvent(new window.Event("input", { bubbles: true })); });
    holdInstallResponse(); await act(async () => button("Install and start").click());
    await poll({ ...currentReceipt(), ...healthyApplication, destination: "/synthetic-home/src/team" });
    assert.equal(button("Return to checked installation") === undefined, true, "An unsupported named home cannot be guessed from another user or the host default.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  }, { options: (value) => ({ ...value, home_directory: "/synthetic-home" }) });
});

test("fresh installation response recovery cannot revive after source selection changes away and back", async () => {
  await mountedDeployment(null, async ({ document, calls, button, hostSelection, enter, poll, revise,
    holdInstallResponse, installBodyPending, finishInstallResponse, currentReceipt }) => {
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-original-choice");
    await act(async () => hostSelection().click());
    holdInstallResponse(); await act(async () => button("Install and start").click());
    const admitted = { ...currentReceipt(), ...healthyApplication };
    await poll(admitted);
    const previousReturn = button("Return to checked installation");
    assert.ok(previousReturn);
    const otherBuild = { ...checked, build_id: "another-build", source_digest: "f".repeat(64) };
    await revise(guide, otherBuild);
    await revise(guide, checked);
    await poll(admitted);
    assert.equal(button("Return to checked installation") === undefined, true,
      "Returning to equal source values cannot revive a retired response owner.");
    const beforeLateReply = calls.length;
    await act(async () => previousReturn.click());
    assert.equal(installBodyPending(), true);
    assert.equal(button("Stop application").disabled, true);
    await finishInstallResponse(0, true);
    assert.equal(calls.length, beforeLateReply);
    assert.equal(document.querySelector(".deployment-panel > .deployment-error") === null, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  });
});

test("mounted private setup closes after a lost install response and stays cleared through later status polling", async () => {
  await mountedDeployment(null, async ({ document, calls, button, privateForm, hostSelection, enter, poll, loseResponse, currentReceipt }) => {
    await act(async () => button("Install on this computer").click());
    assert.ok(privateForm());
    await enter("APP_USER", "synthetic-user-choice");
    const optional = privateForm().querySelector(".deployment-optional-settings");
    optional.open = true;
    await enter("OPTIONAL_NOTE", "synthetic-customization");
    await act(async () => hostSelection().click());
    assert.equal(hostSelection().checked, true);
    assert.equal(button("Install and start").disabled, false);
    loseResponse();
    await act(async () => button("Install and start").click());
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.equal(posts[0].target, installUrl);
    assert.deepEqual(posts[0].body, { version: guide.version, build_id: checked.build_id,
      destination: stopped.destination, settings: { APP_USER: "synthetic-user-choice",
        OPTIONAL_NOTE: "synthetic-customization", MOBKIT_PORT: "18088" },
      use_host_provider_credentials: true, execution_mode: "interactive" });
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "queued");
    assert.equal(privateForm() === null, true);
    assert.equal(button("Install and start") === undefined, true);
    for (const status of ["installing", "running", "stopped"]) {
      await poll({ ...currentReceipt(), status });
      assert.equal(privateForm() === null, true, `${status} must not reopen the previously submitted private form.`);
      if (status === "running") {
        assert.match(document.body.textContent, /stop the application first/);
        assert.equal(button("Change private settings") === undefined, true);
      }
    }
    await act(async () => button("Install into another folder").click());
    assert.ok(privateForm());
    for (const key of ["APP_USER", "OPTIONAL_NOTE", "OPENAI_API_KEY"])
      assert.equal(privateForm().querySelector(`[name="${key}"]`).value, "");
    assert.equal(hostSelection().checked, false);
    assert.equal(privateForm().querySelector(".deployment-optional-settings").open, false);
    assert.equal(button("Install and start").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("unavailable custom application selection keeps its prerequisite and lets a ready native team resume private setup", async () => {
  await mountedDeployment(null, async ({ document, calls, button, privateForm }) => {
    const selector = value => document.querySelector(`input[name="deployment-source"][value="${value}"]`);
    assert.equal(selector("native-team").checked, true);
    assert.ok(button("Install on this computer"));
    const optionsBefore = calls.filter(call => call.target.startsWith(`${installUrl}/options?`)).length;
    await act(async () => selector("application").click());
    assert.ok(selector("application"), "The selected unavailable source retains the installation selector.");
    assert.equal(selector("application").checked, true);
    assert.match(document.body.textContent, /must pass independent acceptance before installation\. Build or improve it first/);
    assert.equal(button("Install on this computer") === undefined, true);
    assert.equal(button("Install and start") === undefined, true);
    assert.equal(privateForm() === null, true);
    assert.equal(calls.filter(call => call.target.startsWith(`${installUrl}/options?`)).length, optionsBefore,
      "Selecting an unavailable application admits no destination preparation.");
    assert.ok(calls.every(call => call.method === "GET"));

    await act(async () => selector("native-team").click());
    assert.equal(selector("native-team").checked, true);
    assert.match(document.body.textContent, /No custom application build is needed/);
    assert.doesNotMatch(document.body.textContent, /must pass independent acceptance before installation/);
    assert.equal(button("Install on this computer").disabled, false);
    await act(async () => button("Install on this computer").click());
    assert.ok(privateForm(), "The user can return to the eligible team's ordinary private setup.");
    assert.equal(privateForm().querySelector('[name="APP_USER"]') === null, true);
    assert.equal(button("Install and start").disabled, true, "Provider prerequisites still block admission.");
    assert.ok(calls.every(call => call.method === "GET"), "No installation or other mutation was admitted.");
    assert.ok(calls.filter(call => call.target.startsWith(`${installUrl}/options?`))
      .every(call => new URL(call.target, "http://localhost").searchParams.get("source_kind") === "native-team"));
  }, { guide: nativeGuide, build: null, sourceKind: "native-team" });
});

test("native private setup admits its exact source once and exposes its checked conversation owner and healthy Console", async () => {
  await mountedDeployment(null, async ({ document, calls, button, privateForm, hostSelection, poll, loseResponse, currentReceipt, revise }) => {
    assert.match(document.body.textContent, /No custom application build is needed/);
    await act(async () => button("Install on this computer").click());
    assert.ok(privateForm());
    assert.equal(privateForm().querySelector('[name="APP_USER"]') === null, true);
    await act(async () => hostSelection().click());
    assert.equal(button("Install and start").disabled, false);
    loseResponse();
    await act(async () => button("Install and start").click());
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { version: 4, source_kind: "native-team", source_id: nativeOptions.source_id,
      source_digest: nativeOptions.source_digest, destination: stopped.destination, settings: { MOBKIT_PORT: "18088" },
      use_host_provider_credentials: true, execution_mode: "interactive" });
    assert.equal(Object.hasOwn(posts[0].body, "build_id"), false);
    assert.equal(privateForm() === null, true);
    await poll({ ...currentReceipt(), status: "running", process_live: true, health: "ready",
      console_url: "http://127.0.0.1:18088/console", bootstrap: { complete: true, ready: true, counts: { broken: 0 }, identities: {} } });
    assert.equal(document.querySelector('section[aria-label="Use your agent team"] a').href, "http://127.0.0.1:18088/console");
    assert.match(document.querySelector('section[aria-label="Use your agent team"]').textContent, /Study companion.*study:main/);
    assert.ok(button("Stop agent team"));
    assert.equal(document.querySelector('section[aria-label="Use your application"]') === null, true);
    assert.equal(calls.some((call) => /\/experiences$/.test(call.target)), false);
    assert.ok(calls.some((call) => /\/use$/.test(call.target) && call.method === "GET"));
    assert.equal(calls.some((call) => /implementation|rehearsal|update-stage/.test(call.target)), false);
    await poll({ ...currentReceipt(), status: "stopped", process_live: false });
    await revise({ ...nativeGuide, version: 5, status: "refining", proposal: { target: "linux" }, readiness: { ready: false } }, null);
    assert.ok(button("Start installed agent team"), "Installed lifecycle remains available after the guide changes");
    assert.ok(button("Change private settings"));
    assert.equal(document.querySelector('section[aria-label="Use your agent team"] a') === null, true);
    assert.equal(button("Install on this computer") === undefined, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  }, { guide: nativeGuide, build: null, sourceKind: "native-team" });
});

test("switching native and application sources clears private drafts and retires old handlers and delayed native reads", async () => {
  await mountedDeployment(null, async (ui) => {
    const { document, calls, button, privateForm, enter } = ui;
    await act(async () => button("Install on this computer").click());
    await enter("OPENAI_API_KEY", "fictional-unsent-native-key");
    const oldSubmit = privateForm()[Object.keys(privateForm()).find((key) => key.startsWith("__reactProps$"))].onSubmit;
    ui.holdReads();
    await act(async () => button("Check this port").click());
    await act(async () => document.querySelector('input[name="deployment-source"][value="application"]').click());
    assert.equal(privateForm() === null, true);
    await act(async () => oldSubmit({ preventDefault() {} }));
    await ui.releaseRead();
    assert.equal(privateForm() === null, true, "A retired native port response cannot reopen its private form");
    await ui.releaseRead();
    ui.stopHoldingReads();
    await act(async () => button("Install on this computer").click());
    assert.equal(privateForm().querySelector('[name="OPENAI_API_KEY"]').value, "");
    assert.ok(privateForm().querySelector('[name="APP_USER"]'));
    await act(async () => oldSubmit({ preventDefault() {} }));
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    for (const storage of [window.sessionStorage, window.localStorage])
      assert.equal([...Array(storage.length)].some((_, index) => storage.getItem(storage.key(index)).includes("fictional-unsent-native-key")), false);
  }, { guide: nativeGuide, sourceKind: "native-team" });
});

test("a changed accepted native plan clears its private draft and cannot reuse the old source selection", async () => {
  await mountedDeployment(null, async ({ calls, button, privateForm, enter, revise }) => {
    await act(async () => button("Install on this computer").click());
    await enter("OPENAI_API_KEY", "fictional-native-draft");
    const oldSubmit = privateForm()[Object.keys(privateForm()).find((key) => key.startsWith("__reactProps$"))].onSubmit;
    await revise({ ...nativeGuide, version: 5, brief: { purpose: "A changed accepted purpose" } }, null);
    assert.equal(privateForm() === null, true);
    await act(async () => oldSubmit({ preventDefault() {} }));
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    await act(async () => button("Install on this computer").click());
    assert.equal(privateForm().querySelector('[name="OPENAI_API_KEY"]').value, "");
  }, { guide: nativeGuide, build: null, sourceKind: "native-team" });
});

test("an authoritative active receipt closes installed settings and clears optional removal and host-copy choices", async () => {
  await mountedDeployment(stopped, async ({ calls, button, privateForm, hostSelection, enter, poll }) => {
    await act(async () => button("Change private settings").click());
    assert.ok(privateForm());
    await enter("APP_USER", "synthetic-renamed-user");
    privateForm().querySelector(".deployment-optional-settings").open = true;
    const removal = privateForm().querySelector('.deployment-remove input[type="checkbox"]');
    assert.ok(removal);
    await act(async () => { removal.click(); hostSelection().click(); });
    assert.equal(removal.checked, true);
    assert.equal(hostSelection().checked, true);
    assert.equal(privateForm().querySelector('[name="OPTIONAL_NOTE"]').disabled, true);
    await poll({ ...stopped, status: "starting" });
    assert.equal(privateForm() === null, true);
    await poll(stopped);
    assert.equal(privateForm() === null, true);
    await act(async () => button("Change private settings").click());
    assert.ok(privateForm());
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "");
    assert.equal(privateForm().querySelector('[name="OPTIONAL_NOTE"]').value, "");
    assert.equal(privateForm().querySelector('[name="OPTIONAL_NOTE"]').disabled, false);
    assert.equal(privateForm().querySelector('.deployment-remove input[type="checkbox"]').checked, false);
    assert.equal(hostSelection().checked, false);
    assert.equal(privateForm().querySelector(".deployment-optional-settings").open, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("a guide clarification retains typed installation settings until fresh options bind the unchanged application", async () => {
  await mountedDeployment(null, async (ui) => {
    const { document, window, calls, button, privateForm, hostSelection, enter } = ui;
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-organiser");
    privateForm().querySelector(".deployment-optional-settings").open = true;
    await enter("OPTIONAL_NOTE", "Keep these exact unsent details");
    await enter("MOBKIT_PORT", "19089");
    await act(async () => {
      hostSelection().click();
      privateForm().querySelector('input[type="radio"][value="background"]').click();
      const folder = privateForm().querySelector("label input:not([name])");
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set.call(folder, "/synthetic-owned/chosen-folder");
      folder.dispatchEvent(new window.Event("input", { bubbles: true }));
    });
    const propsKey = Object.keys(privateForm()).find((key) => key.startsWith("__reactProps$"));
    const oldSubmit = privateForm()[propsKey].onSubmit;
    ui.holdReads();
    await act(async () => button("Check this port").click());
    const nextGuide = { ...guide, version: 5 }, nextBuild = { ...checked, current_guide_version: 5 };
    await ui.revise(nextGuide, nextBuild);
    assert.ok(privateForm(), "A transcript-only revision must keep the same-source private form visible");
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-organiser");
    assert.equal(privateForm().querySelector('[name="OPTIONAL_NOTE"]').value, "Keep these exact unsent details");
    assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "19089");
    assert.equal(privateForm().querySelector("label input:not([name])").value, "/synthetic-owned/chosen-folder");
    assert.equal(hostSelection().checked, true);
    assert.equal(privateForm().querySelector('input[type="radio"][value="background"]').checked, true);
    assert.equal(button("Install and start").disabled, true);
    assert.equal(button("Check this port").disabled, true);
    assert.match(privateForm().textContent, /Rechecking.*draft stays here/);
    await act(async () => privateForm().dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })));
    await act(async () => oldSubmit({ preventDefault() {} }));
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    await ui.releaseRead();
    assert.equal(button("Install and start").disabled, true, "An old version's port response cannot authorize installation");
    assert.doesNotMatch(privateForm().textContent, /This port was available/);
    await ui.releaseRead(0, true);
    ui.stopHoldingReads();
    assert.ok(privateForm());
    assert.equal(button("Install and start").disabled, true);
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-organiser");
    await act(async () => button("Check deployment destination").click());
    assert.equal(button("Install and start").disabled, false);
    assert.equal(hostSelection().checked, true);
    assert.equal(privateForm().querySelector('input[type="radio"][value="background"]').checked, true);
    assert.equal(privateForm().querySelector('[name="OPTIONAL_NOTE"]').value, "Keep these exact unsent details");
    await act(async () => oldSubmit({ preventDefault() {} }));
    assert.equal(calls.filter((call) => call.method === "POST").length, 0, "A retained old-version handler stays retired after revalidation");
    for (const storage of [window.sessionStorage, window.localStorage])
      assert.equal([...Array(storage.length)].some((_, index) => /synthetic-organiser|exact unsent details/.test(storage.getItem(storage.key(index)))), false);
    ui.loseResponse();
    await act(async () => button("Install and start").click());
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { version: 5, build_id: checked.build_id, destination: "/synthetic-owned/chosen-folder",
      settings: { APP_USER: "synthetic-organiser", OPTIONAL_NOTE: "Keep these exact unsent details", MOBKIT_PORT: "19089" },
      use_host_provider_credentials: true, execution_mode: "background" });
    assert.equal(privateForm() === null, true, "An admitted installation consumes the private draft even if its response is lost");
  });
});

test("a changed canonical proposal keeps the old form inactive and a changed source clears its private draft", async () => {
  await mountedDeployment(null, async (ui) => {
    const { document, calls, button, privateForm, hostSelection, enter } = ui;
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-old-source-user");
    await act(async () => hostSelection().click());
    ui.holdReads();
    await act(async () => button("Check this port").click());
    await ui.revise({ ...guide, version: 5, proposal: { target: "linux" } }, { ...checked, current: false });
    assert.ok(privateForm());
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-old-source-user");
    assert.equal(hostSelection().checked, true);
    assert.equal(button("Install and start").disabled, true);
    assert.equal(privateForm().querySelector('[name="APP_USER"]').disabled, true);
    assert.match(privateForm().textContent, /has not been sent.*must be checked/);
    await ui.releaseRead();
    ui.stopHoldingReads();
    assert.equal(button("Install and start").disabled, true);
    assert.doesNotMatch(privateForm().textContent, /This port was available/);
    const replacement = { ...checked, build_id: "synthetic-replacement", source_digest: "c".repeat(64), current_guide_version: 5 };
    await ui.revise({ ...guide, version: 5 }, replacement);
    assert.equal(privateForm() === null, true);
    await act(async () => button("Install on this computer").click());
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "");
    assert.equal(hostSelection().checked, false);
    assert.equal(privateForm().querySelector('input[type="radio"][value="interactive"]').checked, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("installed settings survive a clarification only after an exact refreshed installed-source check", async () => {
  await mountedDeployment(stopped, async (ui) => {
    const { calls, button, privateForm, hostSelection, enter } = ui;
    await act(async () => button("Change private settings").click());
    await enter("APP_USER", "synthetic-installed-user");
    privateForm().querySelector(".deployment-optional-settings").open = true;
    const removal = privateForm().querySelector('.deployment-remove input[type="checkbox"]');
    await act(async () => { removal.click(); hostSelection().click(); });
    ui.holdReads();
    await ui.revise({ ...guide, version: 5 }, { ...checked, current_guide_version: 5 });
    assert.ok(privateForm());
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-installed-user");
    assert.equal(hostSelection().checked, true);
    assert.equal(removal.checked, true);
    assert.equal(button("Save private settings").disabled, true);
    await ui.releaseRead();
    ui.stopHoldingReads();
    assert.equal(button("Save private settings").disabled, false);
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-installed-user");
    assert.equal(privateForm().querySelector('.deployment-remove input[type="checkbox"]').checked, true);
    assert.equal(hostSelection().checked, true);
    await ui.poll({ ...stopped, deployment_id: "synthetic-other-installation", destination: "/synthetic-owned/other-folder" });
    assert.equal(privateForm() === null, true, "A different installation must retire the private draft");
    await act(async () => button("Change private settings").click());
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "");
    assert.equal(hostSelection().checked, false);
    assert.equal(privateForm().querySelector('.deployment-remove input[type="checkbox"]').checked, false);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("failed installed settings revalidation offers an explicit retry without losing the private draft", async () => {
  await mountedDeployment(stopped, async (ui) => {
    const { calls, button, privateForm, hostSelection, enter } = ui;
    const user = "  Synthetic organis\u00f6r  ", key = "  synthetic-unsent-settings-key  ";
    await act(async () => button("Change private settings").click());
    await enter("APP_USER", user);
    await enter("OPENAI_API_KEY", key);
    await enter("MOBKIT_PORT", "19091");
    privateForm().querySelector(".deployment-optional-settings").open = true;
    const removal = privateForm().querySelector('.deployment-remove input[type="checkbox"]');
    await act(async () => { removal.click(); hostSelection().click(); });
    const assertDraft = () => {
      assert.equal(privateForm().querySelector('[name="APP_USER"]').value, user);
      assert.equal(privateForm().querySelector('[name="OPENAI_API_KEY"]').value, key);
      assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "19091");
      assert.equal(privateForm().querySelector('.deployment-remove input[type="checkbox"]').checked, true);
      assert.equal(hostSelection().checked, true);
      assert.equal(privateForm().querySelector(".deployment-optional-settings").open, true);
    };
    ui.holdReads();
    await ui.revise({ ...guide, version: 5 }, { ...checked, current_guide_version: 5 });
    assert.equal(calls.at(-1).target, `${installUrl}/settings?deployment_id=${stopped.deployment_id}`);
    assert.equal(button("Save private settings").disabled, true);
    await ui.releaseRead(0, true);
    ui.stopHoldingReads();
    assertDraft();
    assert.match(privateForm().textContent, /draft is kept in this page.*must be checked/);
    assert.equal(button("Save private settings").disabled, true);
    const retry = button("Recheck private settings");
    assert.ok(retry, "A failed preserved-draft settings read must offer recovery without Cancel");
    assert.equal(retry.disabled, false);
    const readsBeforeRetry = calls.length;
    ui.holdReads();
    await act(async () => retry.click());
    assert.equal(calls.length, readsBeforeRetry + 1);
    assert.deepEqual(calls.at(-1), {
      target: `${installUrl}/settings?deployment_id=${stopped.deployment_id}`, method: "GET", body: null,
    });
    assert.equal(button("Recheck private settings").disabled, true);
    assert.equal(button("Save private settings").disabled, true);
    assertDraft();
    await ui.releaseRead();
    ui.stopHoldingReads();
    assertDraft();
    assert.equal(button("Save private settings").disabled, false);
    assert.equal(button("Recheck private settings") === undefined, true);
    assert.equal(calls.filter((call) => call.method !== "GET").length, 0,
      "Revalidation must not configure, deploy or start the application");
    for (const storage of [ui.window.sessionStorage, ui.window.localStorage])
      assert.equal([...Array(storage.length)].some((_, index) => {
        const value = storage.getItem(storage.key(index));
        return value.includes(user) || value.includes(key);
      }), false, "Private replacements must remain outside browser storage");
  });
});


test("a held private-settings save explains reopening without claiming settlement or repeating the write", async () => {
  for (const lateFailure of [false, true]) {
    let selectionToken = 1, selectionPending = false;
    const scope = { token: () => selectionToken, capture(selected, expectedToken) {
      if (selectionPending || selected.id !== guide.id || expectedToken !== selectionToken) return null;
      const captured = selectionToken;
      return () => !selectionPending && captured === selectionToken;
    } };
    await mountedDeployment(stopped, async (ui) => {
      await act(async () => ui.button("Change private settings").click());
      await ui.enter("APP_USER", "Synthetic saved user");
      ui.holdSettingsResponse();
      await act(async () => ui.privateForm().dispatchEvent(new ui.window.Event("submit", { bubbles: true, cancelable: true })));
      const notice = () => ui.privateForm()?.querySelector('[role="status"]');
      assert.ok(ui.settingsBodyPending());
      assert.match(notice().textContent, /Your setups.*select this setup again.*installation status/);
      assert.match(notice().textContent, /original save may still finish.*does not send it again/);
      assert.equal(ui.button("Saving private settings...").disabled, true);
      assert.equal(ui.button("Cancel").disabled, true);
      const writes = () => ui.calls.filter((call) => call.method === "POST");
      assert.equal(writes().length, 1);
      await ui.poll(ui.currentReceipt());
      assert.ok(notice(), "A current stopped receipt cannot settle this particular settings write.");
      assert.ok(ui.settingsBodyPending());
      selectionToken += 1; selectionPending = true;
      await ui.revise(guide, checked);
      selectionPending = false;
      await ui.revise(guide, checked);
      assert.equal(ui.privateForm().querySelector('[name="APP_USER"]').value, "Synthetic saved user",
        "Reloading the same setup preserves the private draft in this mounted panel.");
      assert.equal(ui.button("Save private settings").disabled, false);
      assert.ok(ui.settingsBodyPending(), "Returning to the saved setup leaves the original write alone.");
      await ui.enter("APP_USER", "Synthetic new unsent draft");
      await ui.finishSettingsResponse(0, lateFailure);
      assert.equal(ui.privateForm().querySelector('[name="APP_USER"]').value, "Synthetic new unsent draft");
      assert.equal(notice() === null, true);
      assert.equal(ui.button("Save private settings").disabled, false);
      assert.doesNotMatch(ui.document.body.textContent, /Synthetic late settings body failure/);
      assert.equal(writes().length, 1, "Status checks and reopening do not resend settings or start the application.");
    }, { selectionScope: scope });
  }
});

test("deployment lifetime copy follows failed recovery and unconfirmed saved status", async () => {
  for (const executionMode of ["interactive", "background"]) {
    const running = { ...healthyApplication, execution_mode: executionMode };
    const failed = { ...running, build_id: "previous-saved-build", package_sha256: "c".repeat(64),
      source_digest: "d".repeat(64), status: "failed", phase: "recovery", process_live: false,
      runtime_pid: null, console_url: null, bootstrap: null, health: "failed",
      detail: "The source update needs recovery; no running-system recovery is claimed.",
      update: { update_id: "saved-recovery-transaction", status: "recovery-needed", rollback_available: true,
        from_build_id: "previous-saved-build", to_build_id: checked.build_id,
        guide_version: guide.version, source_digest: checked.source_digest, blockers: [],
        detail: "Candidate startup and previous-source recovery both failed. Retained records need recovery.",
        activation_error: "Candidate startup failure A.", recovery_error: "Previous-source startup failure B." } };
    const runningCopy = executionMode === "background"
      ? /managed by your operating system and can keep running after Forge closes/
      : /Closing this page keeps the application running/;
    const anyRunningCopy = /Closing this page keeps the application running|can keep running after Forge closes/;
    await mountedDeployment(running, async ({ document, calls, button, poll, loseStatusReads, restoreStatusReads }) => {
      const panel = () => document.querySelector(".deployment-panel");
      assert.match(panel().textContent, runningCopy, "Known running status retains its existing lifetime guidance.");
      await poll(failed);
      assert.equal(panel().dataset.deploymentStatus, "failed");
      assert.equal(panel().querySelector("h2").textContent, "This deployment needs attention");
      assert.match(panel().textContent, /Recover this application/);
      assert.ok(panel().textContent.includes(failed.update.activation_error));
      assert.ok(panel().textContent.includes(failed.update.recovery_error));
      assert.doesNotMatch(panel().textContent, anyRunningCopy,
        "A saved double-start failure must not claim that closing the page keeps an application running.");
      assert.match(panel().textContent, /Your files and native history stay in the deployment folder/);
      if (executionMode === "interactive") {
        assert.ok(button("Retry current application"));
        assert.ok(button("Recover previous application"));
      }
      for (const status of ["interrupted", "checking"]) {
        await poll({ ...failed, status });
        assert.doesNotMatch(panel().textContent, anyRunningCopy, `${status} does not establish a running application.`);
      }
      await poll(running);
      assert.match(panel().textContent, runningCopy);
      loseStatusReads();
      await poll(running);
      assert.equal(panel().dataset.deploymentStatus, "running", "The last checked running receipt remains visible.");
      assert.match(panel().textContent, /Last checked:/);
      assert.match(panel().textContent, /Current installation status is unconfirmed/);
      assert.doesNotMatch(panel().textContent, anyRunningCopy,
        "A historical running receipt must not present its old lifetime claim as current.");
      restoreStatusReads();
      await poll(running);
      assert.match(panel().textContent, runningCopy, "A fresh checked running observation restores the existing guidance.");
      await poll({ ...running, status: "stopped", process_live: false, runtime_pid: null, console_url: null,
        bootstrap: null, health: "unknown" });
      assert.match(panel().textContent,
        /The application is stopped\. Closing this page leaves it stopped\. Your files and native history stay in the deployment folder\./);
      assert.doesNotMatch(panel().textContent, anyRunningCopy);
      assert.ok(calls.every(call => call.method === "GET"), "This presentation control sends no installation or recovery command.");
    });
  }
});


test("installed source export remains available in the real panel after a newer build fails", async () => {
  const failed = { ...checked, build_id: "failed-newer-build", source_digest: "f".repeat(64),
    status: "failed", ready_to_package: false, acceptance_current: false, acceptance_trusted: false };
  await mountedDeployment(stopped, async ({ document, button, calls }) => {
    const details = document.querySelector(".deployment-installed-package");
    assert.ok(details, "The saved local installation exposes its retained package independently of build B.");
    const download = button("Download installed package");
    assert.ok(download);
    assert.equal(download.disabled, false);
    assert.match(details.textContent, /even if a newer build needs repair/);
    assert.equal(button("Download accepted Mac package") === undefined, true);
    assert.ok(calls.every(call => call.method === "GET"), "Rendering recovery never submits a build or installation command.");
  }, { build: failed });
});
