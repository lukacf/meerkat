import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { deploymentPortObservation, deploymentDiagnostic } from "./deployment-panel.mjs";

const guide = { id: "synthetic-install-guide", version: 4, proposal: { target: "mac" } };
const checked = { guide_id: guide.id, guide_version: guide.version, build_id: "synthetic-build",
  status: "completed", current: true, acceptance_current: true, acceptance_trusted: true,
  ready_to_package: true, acceptance: { status: "pass" }, source_digest: "b".repeat(64) };
const installUrl = `/api/guide/${guide.id}/deployment`;
const stopped = { guide_id: guide.id, guide_version: guide.version, deployment_id: "synthetic-installation",
  build_id: checked.build_id, package_sha256: "a".repeat(64), source_digest: checked.source_digest,
  destination: "/synthetic-owned/project", execution_mode: "interactive", status: "stopped", process_live: false };
const freshOptions = { build_id: checked.build_id, guide_version: guide.version, current: true,
  supported: true, target: "mac", host_target: "mac", default_destination: stopped.destination, console_port: 18088,
  can_use_host_provider_credentials: true, host_provider_keys: ["OPENAI_API_KEY"],
  execution_modes: [{ id: "interactive", available: true, label: "While Launch is open" }], fields: [
    { key: "OPENAI_API_KEY", label: "Model provider key", required: true, secret: true },
    { key: "APP_USER", label: "Application user", required: true, secret: false },
    { key: "OPTIONAL_NOTE", label: "Optional note", required: false, secret: false },
  ] };

// Mount the production panel with synthetic transport responses. No server,
// native admission, credentials, installation or operating-system action runs.
async function mountedPorts(initialReceipt, verify) {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], polls = new Map(), deadlines = new Map();
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  let receipt = initialReceipt && structuredClone(initialReceipt), nextPoll = 0, loseInstallResponse = false;
  let delayed = null, failCheck = false, rejectInstall = false;
  let selectedGuide = structuredClone(guide), selectedBuild = structuredClone(checked);
  const portOptions = (port, settings = false) => ({ ...(settings ? settingsOptions() : structuredClone(freshOptions)),
    source_digest: settings ? receipt.source_digest : checked.source_digest,
    console_port_observation: { scope: "local-console-port-observation", port,
      status: port === 18088 ? "occupied" : "available", observed_at: "2026-10-03T08:00:00Z",
      ...(port === 18088 ? { alternate_port: 59123 } : {}) } });
  const settingsOptions = () => ({ ...structuredClone(freshOptions), scope: "verified-installed-private-configuration",
    version: guide.version, deployment_id: receipt.deployment_id, package_sha256: receipt.package_sha256,
    source_digest: receipt.source_digest, source_verified: true, requires_stopped: true,
    destination: receipt.destination, execution_mode: receipt.execution_mode,
    fields: freshOptions.fields.map((field) => ({ ...field, configured: true, removable: field.required === false })) });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage, localStorage: dom.window.localStorage,
    IS_REACT_ACT_ENVIRONMENT: true,
    setTimeout(callback, delay, ...args) {
      if (delay === 30000 || delay === 120000) {
        const token = { syntheticDeadline: true, delay };
        deadlines.set(token, () => callback(...args));
        return token;
      }
      if (delay !== 1500) return originalSetTimeout(callback, delay, ...args);
      const token = { syntheticPoll: ++nextPoll };
      polls.set(token, () => callback(...args));
      return token;
    },
    clearTimeout(token) {
      if (polls.delete(token) || deadlines.delete(token)) return;
      originalClearTimeout(token);
    },
    fetch: async (target, options = {}) => {
      const method = options.method || "GET";
      const body = options.body ? JSON.parse(options.body) : null;
      calls.push({ target, method, body, signal: options.signal });
      if (target === installUrl && method === "POST") {
        if (rejectInstall) return { ok: false, status: 422, json: async () => ({ detail: "Console port is unavailable on this computer. Choose another port in private setup or check again." }) };
        receipt = { ...structuredClone(stopped), status: "queued", destination: body.destination };
        if (loseInstallResponse) throw new Error("The synthetic connection closed before the installation response arrived.");
        return { ok: true, json: async () => structuredClone(receipt) };
      }
      if (target === installUrl && method === "GET") return receipt
        ? { ok: true, json: async () => structuredClone(receipt) }
        : { ok: false, status: 404, json: async () => ({ detail: "No installation was admitted." }) };
      if ((target.startsWith(`${installUrl}/options?`) || target.startsWith(`${installUrl}/settings?`)) && method === "GET") {
        const url = new URL(target, "http://localhost"), settings = url.pathname.endsWith("/settings");
        const supplied = url.searchParams.get("console_port"), port = supplied === null ? 18088 : Number(supplied);
        if (supplied !== null && failCheck) throw new Error("Synthetic observation connection loss");
        const result = supplied === null && settings ? settingsOptions() : portOptions(port, settings);
        if (supplied !== null && delayed) {
          const pending = delayed;
          const waiting = new Promise(resolve => {
            pending.resolve = () => resolve(pending.phase === "body" ? result : { ok: true, json: async () => result });
          });
          return pending.phase === "body" ? { ok: true, json: () => waiting } : await waiting;
        }
        return { ok: true, json: async () => result };
      }
      if (target === `${installUrl}/experiences` && method === "GET")
        return { ok: true, json: async () => ({ deployment_id: receipt.deployment_id,
          build_id: receipt.build_id, package_sha256: receipt.package_sha256, source_digest: receipt.source_digest,
          scope: "declared-source", verified: false, entries: [] }) };
      if (target === `${installUrl}/use` && method === "GET")
        return { ok: false, status: 404, json: async () => ({ detail: "Synthetic first-use metadata is not provided." }) };
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
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] });
  const { DeploymentPanel } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  const root = createRoot(dom.window.document.getElementById("root"));
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
  try {
    await act(async () => root.render(React.createElement(DeploymentPanel,
      { state: selectedGuide, plan: { project: { name: "Synthetic app", slug: "synthetic-app" } },
        build: selectedBuild, designBusy: false, pendingManual: false, Icon: () => null })));
    await verify({ document, window: dom.window, calls, button, privateForm, hostSelection, enter, poll,
      loseResponse() { loseInstallResponse = true; }, rejectInstall(value = true) { rejectInstall = value; },
      failCheck(value = true) { failCheck = value; },
      delayCheck(phase = "headers") { delayed = { phase }; },
      async expireCheck() {
        assert.equal(deadlines.size, 1, "Only the entered port read should be pending.");
        const [token, callback] = deadlines.entries().next().value;
        assert.equal(token.delay, 120000, "Native setup checks use their explicit bounded deadline.");
        deadlines.delete(token);
        await act(async () => callback());
      },
      async finishCheck() { const pending = delayed; delayed = null; assert.equal(typeof pending?.resolve, "function"); await act(async () => pending.resolve()); },
      async changeSource() {
        selectedGuide = { ...selectedGuide, version: selectedGuide.version + 1 };
        selectedBuild = { ...selectedBuild, guide_version: selectedGuide.version, source_digest: "c".repeat(64) };
        await act(async () => root.render(React.createElement(DeploymentPanel, { state: selectedGuide, build: selectedBuild,
          plan: { project: { name: "Synthetic app", slug: "synthetic-app" } }, designBusy: false, pendingManual: false, Icon: () => null })));
      }, currentReceipt: () => structuredClone(receipt) });
  } finally {
    await act(async () => root.unmount());
    const pendingPolls = polls.size;
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
    assert.equal(pendingPolls, 0, "Unmount must cancel the production deployment poll.");
    assert.equal(deadlines.size, 0, "Settled and retired reads must clear their deadlines.");
  }
}

test("occupied default offers an explicit alternate that changes only the current draft", async () => {
  await mountedPorts(null, async ({ document, calls, button, privateForm, enter, hostSelection }) => {
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-custom-user");
    await act(async () => hostSelection().click());
    const destination = privateForm().querySelector("input:not([name])").value;
    assert.match(document.body.textContent, /This port is unavailable on this computer/);
    assert.doesNotMatch(document.body.textContent, /another application|existing application/i);
    assert.equal(button("Install and start").disabled, true);
    await act(async () => button("Use port 59123").click());
    assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "59123");
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-custom-user");
    assert.equal(hostSelection().checked, true);
    assert.equal(privateForm().querySelector("input:not([name])").value, destination);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.doesNotMatch(document.body.textContent, /This port is unavailable on this computer/);
    await act(async () => button("Check this port").click());
    assert.match(document.body.textContent, /available when checked/);
    assert.match(document.body.textContent, /can change before startup/);
    assert.equal(calls.at(-1).target, `${installUrl}/options?build_id=${checked.build_id}&console_port=59123`);
    assert.equal(button("Install and start").disabled, false);
    await act(async () => button("Install and start").click());
    const posts = calls.filter((call) => call.method === "POST");
    assert.equal(posts.length, 1);
    assert.equal(posts[0].body.settings.MOBKIT_PORT, "59123");
    assert.equal(posts[0].body.settings.APP_USER, "synthetic-custom-user");
    assert.equal(posts[0].body.build_id, checked.build_id);
    assert.equal(posts[0].body.version, guide.version);
    assert.equal(posts[0].body.destination, destination);
    assert.equal(privateForm() === null, true);
  });
});

test("a delayed port response cannot restore a claim after a newer port edit", async () => {
  await mountedPorts(null, async ({ document, calls, button, privateForm, enter, delayCheck, finishCheck }) => {
    await act(async () => button("Install on this computer").click());
    await enter("MOBKIT_PORT", "59123");
    delayCheck();
    await act(async () => button("Check this port").click());
    assert.ok(button("Checking port..."));
    await enter("MOBKIT_PORT", "59124");
    await finishCheck();
    assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "59124");
    assert.doesNotMatch(document.body.textContent, /available when checked/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("port observation failure preserves form fields and permits explicit recheck", async () => {
  await mountedPorts(null, async ({ document, calls, button, privateForm, enter, failCheck }) => {
    await act(async () => button("Install on this computer").click());
    await enter("APP_USER", "synthetic-draft");
    await enter("MOBKIT_PORT", "59123");
    failCheck();
    await act(async () => button("Check this port").click());
    assert.match(document.body.textContent, /port check could not be completed/);
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-draft");
    assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "59123");
    assert.doesNotMatch(document.body.textContent, /available when checked/);
    failCheck(false);
    await act(async () => button("Check this port").click());
    assert.match(document.body.textContent, /available when checked/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

for (const intent of ["install", "settings"]) for (const phase of ["headers", "body"]) {
  test(`${intent} port-check deadline releases stalled ${phase} without clearing or submitting private fields`, async () => {
    await mountedPorts(intent === "settings" ? stopped : null, async ui => {
      await act(async () => ui.button(intent === "settings" ? "Change private settings" : "Install on this computer").click());
      await ui.enter("APP_USER", "synthetic-kept-user");
      await ui.enter("OPENAI_API_KEY", "synthetic-kept-private-value");
      await ui.enter("MOBKIT_PORT", "59123");
      const destination = ui.privateForm().querySelector("input:not([name])").value;
      ui.delayCheck(phase);
      await act(async () => ui.button("Check this port").click());
      const entered = ui.calls.at(-1);
      assert.equal(entered.target, intent === "settings"
        ? `${installUrl}/settings?deployment_id=${stopped.deployment_id}&console_port=59123`
        : `${installUrl}/options?build_id=${checked.build_id}&console_port=59123`);
      assert.equal(entered.method, "GET");
      assert.equal(entered.body, null);
      assert.ok(ui.button("Checking port..."));
      await ui.expireCheck();
      assert.equal(entered.signal.aborted, true);
      assert.match(ui.document.body.textContent, /port check could not be completed/);
      assert.equal(ui.button("Check this port").disabled, false);
      assert.equal(ui.privateForm().querySelector('[name="APP_USER"]').value, "synthetic-kept-user");
      assert.equal(ui.privateForm().querySelector('[name="OPENAI_API_KEY"]').value, "synthetic-kept-private-value");
      assert.equal(ui.privateForm().querySelector('[name="MOBKIT_PORT"]').value, "59123");
      assert.equal(ui.privateForm().querySelector("input:not([name])").value, destination);
      await ui.finishCheck();
      assert.doesNotMatch(ui.document.body.textContent, /available when checked/,
        "A late response cannot publish an observation after its deadline.");
      await act(async () => ui.button("Check this port").click());
      assert.match(ui.document.body.textContent, /available when checked/);
      assert.equal(ui.calls.filter(call => call.target.includes("console_port=59123")).length, 2,
        "Only the explicit retry adds another port read.");
      assert.equal(ui.calls.some(call => call.method !== "GET"), false,
        "Port timeout, late response and recheck cannot install or save settings.");
      assert.doesNotMatch(ui.document.body.textContent, /synthetic-kept-private-value/,
        "The retained private value stays inside its input, not status copy.");
    });
  });
}

test("source changes invalidate a delayed check and do not reopen an old setup draft", async () => {
  await mountedPorts(null, async ({ document, button, privateForm, enter, delayCheck, finishCheck, changeSource }) => {
    await act(async () => button("Install on this computer").click());
    await enter("MOBKIT_PORT", "59123");
    delayCheck();
    await act(async () => button("Check this port").click());
    await changeSource();
    await finishCheck();
    assert.equal(privateForm() === null, true);
    assert.doesNotMatch(document.body.textContent, /available when checked/);
  });
});

test("reopening setup waits for its pending port read without publishing the abandoned observation", async () => {
  await mountedPorts(null, async ({ document, calls, button, privateForm, enter, delayCheck, finishCheck }) => {
    await act(async () => button("Install on this computer").click());
    await enter("MOBKIT_PORT", "59123");
    delayCheck();
    await act(async () => button("Check this port").click());
    await act(async () => button("Cancel").click());
    const before = calls.length;
    await act(async () => button("Install on this computer").click());
    assert.equal(calls.length, before, "Reopening cannot overlap a native package check for the same build.");
    assert.equal(privateForm() === null, true);
    assert.match(document.body.textContent, /Checking your deployment destination/);
    await finishCheck();
    await enter("MOBKIT_PORT", "59123");
    assert.doesNotMatch(document.body.textContent, /available when checked/);
    assert.ok(button("Check this port"));
  });
});

test("another selected installation clears its old form and invalidates its delayed port check", async () => {
  await mountedPorts(stopped, async ({ document, button, privateForm, enter, poll, delayCheck, finishCheck }) => {
    await act(async () => button("Change private settings").click());
    await enter("MOBKIT_PORT", "59123");
    delayCheck();
    await act(async () => button("Check this port").click());
    await poll({ ...stopped, deployment_id: "other-installation", source_digest: "c".repeat(64) });
    await finishCheck();
    assert.doesNotMatch(document.body.textContent, /available when checked/);
    assert.equal(privateForm() === null, true);
    assert.equal(button("Check this port") === undefined, true);
  });
});

test("known install rejection keeps the selected source and fields for recovery", async () => {
  await mountedPorts(null, async ({ document, calls, button, privateForm, enter, hostSelection, rejectInstall }) => {
    await act(async () => button("Install on this computer").click());
    await enter("MOBKIT_PORT", "59123");
    await enter("APP_USER", "synthetic-draft");
    await act(async () => hostSelection().click());
    rejectInstall();
    await act(async () => button("Install and start").click());
    assert.match(document.body.textContent, /unavailable on this computer/);
    assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "59123");
    assert.equal(privateForm().querySelector('[name="APP_USER"]').value, "synthetic-draft");
    assert.equal(hostSelection().checked, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(document.querySelector(".deployment-panel").dataset.deploymentStatus, "not-installed");
  });
});

test("lost installation response still reconciles the admitted receipt and clears the draft", async () => {
  await mountedPorts(null, async ({ calls, button, privateForm, enter, hostSelection, loseResponse, poll, currentReceipt }) => {
    await act(async () => button("Install on this computer").click());
    await enter("MOBKIT_PORT", "59123");
    await enter("APP_USER", "synthetic-draft");
    await act(async () => hostSelection().click());
    loseResponse();
    await act(async () => button("Install and start").click());
    assert.equal(privateForm() === null, true);
    await poll({ ...currentReceipt(), status: "running" });
    assert.equal(privateForm() === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("a stopped installation checks its explicit draft port through its own source-bound settings route", async () => {
  await mountedPorts(stopped, async ({ document, calls, button, privateForm, enter }) => {
    await act(async () => button("Change private settings").click());
    assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "");
    await enter("MOBKIT_PORT", "18088");
    await act(async () => button("Check this port").click());
    assert.equal(calls.at(-1).target, `${installUrl}/settings?deployment_id=${stopped.deployment_id}&console_port=18088`);
    assert.match(document.body.textContent, /This port is unavailable on this computer/);
    await act(async () => button("Use port 59123").click());
    assert.equal(privateForm().querySelector('[name="MOBKIT_PORT"]').value, "59123");
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.equal(privateForm().querySelector("input:not([name])").value, stopped.destination);
  });
});

test("the real installation panel puts the latest retained update before earlier activity", async () => {
  const progress = Array.from({ length: 100 }, (_, index) => ({ message: `Retained update ${index + 1}` }));
  await mountedPorts({ ...stopped, status: "starting", progress }, async ui => {
    const activity = [...ui.document.querySelectorAll("details")]
      .find(details => details.querySelector(":scope > summary")?.textContent === "Deployment activity");
    assert.ok(activity?.open, "An active installation keeps its activity visible.");
    const latest = activity.querySelector(":scope > ol");
    assert.equal(latest.querySelector("li").textContent, "Retained update 100",
      "The first visible update must describe the latest retained progress.");
    assert.equal(latest.querySelectorAll("li").length, 5);
    const earlier = activity.querySelector(":scope > details");
    assert.ok(earlier && !earlier.open);
    assert.equal(earlier.querySelectorAll("li").length, 95);
    assert.equal(ui.calls.some(call => call.method !== "GET"), false);
  });
});

test("pure observation validation rejects foreign source, remote scope, stale port and malformed metadata", () => {
  const options = { ...freshOptions, source_digest: checked.source_digest, console_port_observation: {
    scope: "local-console-port-observation", port: 18088, status: "occupied", observed_at: "2026-10-03T08:00:00Z", alternate_port: 59123 } };
  assert.equal(deploymentPortObservation(options, checked, guide, "18088").alternatePort, 59123);
  for (const change of [{ source_digest: "c".repeat(64) }, { source_digest: undefined }, { supported: false }, { target: "gcp" },
    { console_port_observation: { ...options.console_port_observation, status: "ready" } },
    { console_port_observation: { ...options.console_port_observation, observed_at: "invalid" } }])
    assert.equal(deploymentPortObservation({ ...options, ...change }, checked, guide, "18088"), null);
  assert.equal(deploymentPortObservation(options, checked, guide, "59123"), null);
  assert.equal(deploymentPortObservation(options, checked, guide, "18088.0"), null);
  assert.equal(deploymentPortObservation({ ...options, console_port_observation: { ...options.console_port_observation,
    alternate_port: "59123" } }, checked, guide, "18088").alternatePort, null);
});

test("port startup diagnosis uses exact actual attempt and package rather than generic child exit", () => {
  const attempt = "ea14e989-d0da-4404-8b77-fc1f7e5f47f4";
  const receipt = { ...stopped, status: "failed", phase: "start", startup_attempt_id: attempt,
    startup_port_observation: { scope: "local-console-port-observation", port: 59123, status: "occupied",
      attempt_id: attempt, deployment_id: stopped.deployment_id, package_sha256: stopped.package_sha256 } };
  assert.match(deploymentDiagnostic(receipt), /Console port 59123 is unavailable on this computer/);
  assert.doesNotMatch(deploymentDiagnostic(receipt), /another application|existing application/i);
  for (const change of [{ attempt_id: "foreign" }, { package_sha256: "d".repeat(64) }, { deployment_id: "foreign" },
    { status: "available" }])
    assert.doesNotMatch(deploymentDiagnostic({ ...receipt, startup_port_observation: { ...receipt.startup_port_observation, ...change } }), /port 59123 is unavailable/);
  assert.doesNotMatch(deploymentDiagnostic({ ...receipt, startup_port_observation: undefined }), /port .* is unavailable/);
});
