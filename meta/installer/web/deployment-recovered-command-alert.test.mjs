import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
const webRoot = process.env.FORGE_TEST_WEB_ROOT
  ? pathToFileURL(`${resolve(process.env.FORGE_TEST_WEB_ROOT)}/`) : new URL(".", import.meta.url);
const panelSource = process.env.FORGE_DEPLOYMENT_PANEL_SOURCE
  ? resolve(process.env.FORGE_DEPLOYMENT_PANEL_SOURCE) : fileURLToPath(new URL("deployment-panel.jsx", webRoot));
const { deploymentSourceKey } = await import(new URL("deployment-panel.mjs", webRoot));

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
  const require = createRequire(new URL("package.json", webRoot));
  const { JSDOM } = createRequire(new URL("../../console/package.json", webRoot))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], polls = new Map();
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  let receipt = initialReceipt && structuredClone(initialReceipt), nextPoll = 0, loseInstallResponse = false;
  let loseStartResponse = false, returnStartResponse = false, failStatusReads = false, observedReceiptOverride = null;
  let actionRequest = null, actionReadback = "pending", optionsFailure = null, heldStatusRead = null;
  let selectedGuide = structuredClone(fixture.guide || guide), selectedBuild = structuredClone(fixture.build === null ? null : fixture.build || checked), heldReads = null;
  let selectedSourceKind = fixture.sourceKind || "application";
  const currentOptions = (target) => new URL(target, "http://localhost").searchParams.get("source_kind") === "native-team"
    ? { ...structuredClone(nativeOptions), guide_version: selectedGuide.version, current_guide_version: selectedGuide.version }
    : { ...structuredClone(freshOptions), build_id: selectedBuild?.build_id,
      source_digest: selectedBuild?.source_digest, current: selectedBuild?.current, current_guide_version: selectedGuide.version };
  const settingsOptions = () => ({ ...structuredClone(receipt.source_kind === "native-team" ? nativeOptions : freshOptions), scope: "verified-installed-private-configuration",
    version: selectedGuide.version, deployment_id: receipt.deployment_id, package_sha256: receipt.package_sha256,
    source_digest: receipt.source_digest, source_verified: true, requires_stopped: true,
    destination: receipt.destination, execution_mode: receipt.execution_mode,
    fields: (receipt.source_kind === "native-team" ? nativeOptions : freshOptions).fields.map((field) => ({ ...field, configured: true, removable: field.required === false })) });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage, localStorage: dom.window.localStorage,
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
      if (target === installUrl && method === "POST") {
        assert.equal(loseInstallResponse, true);
        receipt = { ...structuredClone(body.source_kind === "native-team" ? nativeStopped : stopped), status: "queued", destination: body.destination };
        throw new Error("The synthetic connection closed before the installation response arrived.");
      }
      if (target === `${installUrl}/action` && method === "POST") {
        assert.equal(body.action, "restart");
        assert.equal(loseStartResponse || returnStartResponse, true);
        assert.match(body.action_request_id, /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/);
        actionRequest = { version: 1, guide_id: selectedGuide.id, deployment_id: body.deployment_id,
          action: body.action, update_id: body.update_id ?? null, action_request_id: body.action_request_id };
        receipt = { ...structuredClone(healthyApplication), execution_mode: "background" };
        if (returnStartResponse) return { ok: true, json: async () => ({ ...structuredClone(receipt),
          action_request: { ...actionRequest, phase: "settled", outcome: "returned" } }) };
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
      if (target === installUrl && method === "GET" && heldStatusRead) {
        assert.equal(heldStatusRead.resolve, null);
        return new Promise((resolve) => { heldStatusRead.resolve = resolve; });
      }
      if (target === installUrl && method === "GET" && failStatusReads)
        throw new Error("The synthetic installation status is temporarily unavailable.");
      if (target === installUrl && method === "GET") return receipt
        ? { ok: true, json: async () => structuredClone(observedReceiptOverride || receipt) }
        : { ok: false, status: 404, json: async () => ({ detail: "No installation was admitted." }) };
      if ((target.startsWith(`${installUrl}/options?`) || target.startsWith(`${installUrl}/settings?`)) && method === "GET") {
        if (optionsFailure) throw new Error(optionsFailure);
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
  const built = await build({ entryPoints: [panelSource],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", webRoot))],
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onLoad({ filter: /deployment-panel\.jsx$/ }, args => args.path === panelSource
        ? { contents: readFileSync(panelSource, "utf8"), loader: "jsx", resolveDir: fileURLToPath(webRoot) } : undefined);
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
      designBusy: false, pendingManual: false, Icon: () => null })));
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
      loseStartResponse() { loseStartResponse = true; },
      failOptions(message) { optionsFailure = message; },
      returnStartResponse() { returnStartResponse = true; },
      loseStatusReads() { failStatusReads = true; },
      holdStatusRead() { assert.equal(heldStatusRead, null); heldStatusRead = { resolve: null }; },
      async releaseStatusRead() {
        assert.equal(typeof heldStatusRead?.resolve, "function");
        const resolve = heldStatusRead.resolve; heldStatusRead = null;
        failStatusReads = false;
        await act(async () => resolve({ ok: true, json: async () => structuredClone(receipt) }));
      },
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

test("an unavailable status check labels saved installation details until a fresh check returns", async () => {
  for (const receipt of [healthyApplication, stopped]) {
    const saved = { ...receipt, detail: receipt.status === "running"
      ? "This application and its Console are available." : "This application is stopped." };
    await mountedDeployment(saved, async ({ document, calls, button, poll, loseStatusReads, restoreStatusReads }) => {
      const heading = () => document.querySelector(".deployment-panel > h2").textContent;
      const currentTitle = receipt.status === "running" ? "Your custom application is running" : "Your custom application is stopped";
      const command = receipt.status === "running" ? "Stop application" : "Start installed application";
      assert.equal(heading(), currentTitle);
      assert.equal(button(command).disabled, false);

      loseStatusReads();
      await poll(saved);
      assert.equal(heading(), "Installation status unavailable");
      const savedDetail = [...document.querySelectorAll(".deployment-panel > p")]
        .find(node => node.textContent.includes(saved.detail));
      assert.ok(savedDetail?.textContent.startsWith("Last saved update: "));
      assert.equal(button(command).disabled, true);
      if (receipt.status === "running") {
        const directConsole = document.querySelector('.deployment-interface-scope a');
        assert.ok(directConsole);
        assert.equal(directConsole.getAttribute("aria-disabled"), "true");
        assert.equal(directConsole.hasAttribute("href"), false);
      }
      assert.equal(calls.filter(call => call.method === "POST").length, 0);

      restoreStatusReads();
      await poll(saved);
      assert.equal(heading(), currentTitle);
      assert.equal(button(command).disabled, false);
      assert.equal(document.querySelector(".deployment-panel > h2 + p").textContent,
        receipt.status === "running" ? "Your agents and Console are available. Try a first job below." : saved.detail);
      assert.equal(calls.filter(call => call.method === "POST").length, 0,
        "A status retry must not start, stop or reinstall the application.");
    });
  }
});

test("a held status recovery labels the retained stopped result while checking the admitted start", async () => {
  await mountedDeployment({ ...stopped, execution_mode: "background", detail: "This application is stopped." },
    async ({ document, calls, button, loseStartResponse, holdStatusRead, releaseStatusRead }) => {
      holdStatusRead();
      loseStartResponse();
      await act(async () => button("Start installed application").click());
      try {
        assert.equal(document.querySelector(".deployment-panel > h2").textContent, "Checking installation status");
        assert.ok([...document.querySelectorAll(".deployment-panel > p")]
          .some(node => node.textContent === "Last saved update: This application is stopped."));
        assert.equal(button("Start installed application").disabled, true);
        assert.equal(calls.filter(call => call.method === "POST").length, 1);
      } finally { await releaseStatusRead(); }
      assert.equal(document.querySelector(".deployment-panel > h2").textContent, "Your custom application is running");
      assert.equal(button("Stop application").disabled, true,
        "Fresh runtime status does not establish settlement of the original start command.");
      assert.equal(calls.filter(call => call.method === "POST").length, 1);
    });
});

// New mounted control only. All transports, receipts and credentials are inert.
// The current proposal has no runnable package; recovery must not depend on an
// unrelated options request clearing an old command-observation message.
test("recovered exact Start retires only its transient alert after successful settlement and a matching fresh receipt", async () => {
  const transportMessage = "The synthetic connection closed after the background start was admitted.";
  for (const scenario of ["returned", "error", "new same-text error"]) {
    await mountedDeployment({ ...stopped, execution_mode: "background" }, async ({ document, calls, button,
      loseStartResponse, restoreStatusReads, actionReadback, failOptions, revise }) => {
      const alert = () => document.querySelector(".deployment-panel > .deployment-error");
      const installReads = () => calls.filter(call => call.target === installUrl && call.method === "GET").length;
      assert.equal(button("Install on this computer") === undefined, true);
      assert.equal(calls.some(call => call.target.includes("/options?")), false);
      assert.equal(button("Start installed application").disabled, false);
      loseStartResponse();
      await act(async () => button("Start installed application").click());
      assert.equal(alert().textContent, transportMessage);
      assert.equal(button("Start installed application").disabled, true);
      assert.equal(calls.filter(call => call.method === "POST").length, 1);

      // A fresh runtime observation alone cannot establish command settlement.
      restoreStatusReads({ ...healthyApplication, execution_mode: "background" });
      await act(async () => button("Check installation status").click());
      assert.equal(alert().textContent, transportMessage);
      assert.equal(button("Stop application").disabled, true);
      assert.equal(calls.filter(call => call.method === "POST").length, 1);

      if (scenario === "new same-text error") {
        // This genuine later options error deliberately has identical text.
        // It replaces the older transport error rather than belonging to Start.
        failOptions(transportMessage);
        await revise({ ...guide, version: 5 }, { ...checked, guide_version: 5 });
        assert.ok(calls.some(call => call.target.includes("/options?")));
        assert.equal(alert().textContent, transportMessage);
      }
      const readCount = installReads();
      actionReadback({ phase: "settled", outcome: scenario === "error" ? "error" : "returned" });
      restoreStatusReads(scenario === "error" ? { ...stopped, execution_mode: "background" }
        : { ...healthyApplication, execution_mode: "background" });
      await act(async () => button("Check installation status").click());
      assert.equal(installReads(), readCount + 1, "Settlement must be paired with a new receipt read.");
      assert.equal(button("Check installation status") === undefined, true);
      assert.equal(calls.filter(call => call.method === "POST").length, 1, "Observation never replays Start.");
      if (scenario === "returned") {
        assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "running");
        assert.equal(button("Stop application").disabled, false);
        assert.equal(!!alert(), false, "Successful exact recovery must retire the obsolete transport alert.");
      } else {
        assert.equal(alert().textContent, transportMessage, "Actual failed commands and newer unrelated errors stay visible.");
        if (scenario === "error") assert.equal(button("Start installed application").disabled, false);
      }
    }, { guide: { ...guide, version: 5 }, build: null, applicationUse: true });
  }
});
