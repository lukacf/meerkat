import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import React, { act } from "react";
import { applicationSetupKey, applicationSetupReadback, applicationSetupView } from "./application-setup.mjs";
import { deploymentApplicationStartupPending, deploymentActionIntent, storeDeploymentActionIntent,
  loadDeploymentActionIntent } from "./deployment-panel.mjs";
import { createGuideLoadGate } from "./wizard-state.mjs";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const bundled = await build({ entryPoints: [fileURLToPath(new URL("application-setup.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
  plugins: [{ name: "shared-mounted-react", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { ApplicationSetup } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);

const guide = () => ({ id: "synthetic-reading-guide", version: 7 });
function receiptFor(state = guide(), patch = {}) {
  return { guide_id: state.id, guide_version: 6, deployment_id: "synthetic-installed-reading",
    build_id: "synthetic-accepted-source", package_sha256: "a".repeat(64), source_digest: "b".repeat(64),
    updated_at: "2026-10-11T07:00:00Z", source_kind: "application", target: "local",
    execution_mode: "interactive", status: "stopped", phase: "configure", process_live: false, runtime_pid: null,
    runtime_candidate: { format: "forge-meta-toolkit-runtime-selection-v1",
      assembly: { sha256: "c".repeat(64) }, model_catalog: { sha256: "d".repeat(64) },
      meta: { version: "0.9.0", sha256: "e".repeat(64) }, toolkit: { version: "0.1.0", sha256: "f".repeat(64) } },
    installed_candidate_runtime_proof: { source: "meta-toolkit-product", version: "0.9.0",
      assembly_sha256: "c".repeat(64), catalog_sha256: "d".repeat(64),
      sdk: { meta: { version: "0.9.0", sha256: "e".repeat(64) }, toolkit: { version: "0.1.0", sha256: "f".repeat(64) } } },
    ...patch };
}
function readback(receipt = receiptFor(), state = guide(), patch = {}) {
  return { scope: "selected-stopped-application-inputs", guide_id: state.id,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    package_sha256: receipt.package_sha256, guide_version: receipt.guide_version,
    available: true, kind: "fresh", application_name: "Synthetic Reading", missing_fields: [], ...patch };
}
const handler = node => node[Object.keys(node).find(key => key.startsWith("__reactProps$"))].onClick;

test("ordinary Meta readback permits Start without preparation or background claims", () => {
  const state = guide(), receipt = receiptFor(state);
  const value = readback(receipt, state, { kind: "ordinary" });
  assert.equal(applicationSetupReadback(value, receipt, state)?.kind, "ordinary");
  assert.equal(applicationSetupReadback({ ...value, action: "prepare-setup" }, receipt, state), null);
  assert.equal(applicationSetupReadback({ ...value, background: { available: true } }, receipt, state)?.background, undefined);
});

test("ordinary Meta renders one Start action through the existing command owner", async () => {
  await mounted(async ({ button, actions, click }) => {
    assert.ok(button("Start installed application"));
    assert.equal(button("Prepare application setup") === undefined, true);
    assert.equal(button("Run in background") === undefined, true);
    await click("Start installed application");
    assert.deepEqual(actions, ["finish-setup"]);
  }, { responses: [readback(receiptFor(), guide(), { kind: "ordinary" })] });
});

// Only actual production JSX is mounted. Transport replies and the existing
// outer guide-selection lifetime are synthetic; no private input or runtime is used.
async function mounted(verify, fixture = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], actions = [], settings = [], checks = [], held = new Map();
  const gate = createGuideLoadGate(); gate.finish(gate.begin());
  let state = structuredClone(fixture.state || guide()), receipt = structuredClone(fixture.receipt || receiptFor(state));
  let attached = true, root;
  let props = { confirmed: true, pending: false, disabled: false, ...fixture.props };
  const selectionScope = {
    token: () => gate.token(),
    capture(selected, expected) {
      const token = gate.token(), identity = selected?.id;
      if (!attached || gate.pending() || !gate.current(expected) || state.id !== identity) return null;
      return () => attached && !gate.pending() && gate.current(token) && state.id === identity;
    },
  };
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (url, options = {}) => {
      const index = calls.length, call = { url, method: options.method || "GET", body: options.body };
      calls.push(call);
      assert.equal(call.method, "GET", "Setup presentation can only read host-owned availability.");
      assert.equal(call.body, undefined, "No browser-owned credentials or Configure inputs are sent.");
      const value = Object.hasOwn(fixture.responses || [], index)
        ? fixture.responses[index] : readback(receipt, state);
      const response = result => ({ ok: true, json: async () => structuredClone(result) });
      if (fixture.holdReads?.includes(index)) return new Promise(resolve => {
        held.set(index, result => { held.delete(index); resolve(response(result === undefined ? value : result)); });
      });
      if (value instanceof Error) throw value;
      return response(value);
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const render = () => act(async () => root.render(React.createElement(ApplicationSetup, {
    state, receipt, selectionScope, ...props,
    onAction: action => actions.push(action), onSettings: () => settings.push("settings"), onCheck: () => checks.push("check") })));
  const button = label => [...dom.window.document.querySelectorAll("button")].find(node => node.textContent === label);
  const assertReadOnly = () => {
    assert.ok(calls.every(call => call.method === "GET" && call.body === undefined));
    assert.equal(dom.window.document.querySelector("input, textarea, form") === null, true,
      "The setup handoff never solicits private Configure values in the browser.");
  };
  try {
    root = createRoot(dom.window.document.getElementById("root"));
    await render();
    await verify({ document: dom.window.document, calls, actions, settings, checks, button,
      async click(label) { const node = button(label); assert.ok(node, label); assert.equal(node.disabled, false); await act(async () => node.click()); },
      async invoke(callback) { await act(async () => callback()); },
      async refresh(patch = {}) {
        if (Object.hasOwn(patch, "state")) { state = structuredClone(patch.state); gate.finish(gate.begin()); }
        if (Object.hasOwn(patch, "receipt")) receipt = structuredClone(patch.receipt);
        props = { ...props, ...Object.fromEntries(Object.entries(patch).filter(([key]) => !["state", "receipt"].includes(key))) };
        await render();
      },
      async release(index, value) { assert.ok(held.has(index), `Held read ${index}`); await act(async () => held.get(index)(value)); },
      detachSelection() { attached = false; },
      beginSelection() { gate.begin(); },
      async unmount() { await act(async () => root.unmount()); root = null; },
      assertReadOnly,
    });
    assertReadOnly();
  } finally {
    await act(async () => { for (const release of [...held.values()]) release(null); });
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test("an exact stopped-product readback is tied to installed source, independently of a newer guide version", () => {
  const state = guide(), receipt = receiptFor(state), value = readback(receipt, state);
  const original = structuredClone({ state, receipt, value });
  assert.equal(deploymentApplicationStartupPending(receipt, state), true);
  assert.ok(applicationSetupKey(receipt, state));
  assert.deepEqual(applicationSetupReadback(value, receipt, state), {
    available: true, missing: [], kind: "fresh", name: "Synthetic Reading", reason: null });
  const snapshot = { key: applicationSetupKey(receipt, state), value, pending: false };
  assert.deepEqual(applicationSetupView({ receipt, state, confirmed: true, pending: false, snapshot }), {
    phase: "ready", name: "Synthetic Reading", kind: "fresh" });
  assert.deepEqual({ state, receipt, value }, original);
});

test("only an exact fresh preparation capability selects prepare-setup", () => {
  const state = guide(), receipt = receiptFor(state);
  const value = readback(receipt, state, { action: "prepare-setup" });
  assert.equal(applicationSetupReadback(value, receipt, state).action, "prepare-setup");
  for (const patch of [{ kind: "restore" }, { available: false }, { action: "invented-action" },
    { deployment_id: "another-installation" }])
    assert.equal(applicationSetupReadback({ ...value, ...patch }, receipt, state), null);
});

test("preparation uses the existing action owner and needs a new readback before finish-setup", async () => {
  const state = guide(), receipt = receiptFor(state);
  await mounted(async ({ document, calls, actions, click, button }) => {
    assert.match(document.body.textContent, /keep the application stopped/);
    assert.equal(button("Set up and start application") === undefined, true);
    await click("Prepare application setup");
    assert.deepEqual(actions, ["prepare-setup"]);
    assert.equal(calls.length, 1, "Effects still belong to the existing parent action owner.");
    assert.equal(button("Set up and start application") === undefined, true);
  }, { responses: [readback(receipt, state, { action: "prepare-setup" })] });
});

test("the preparation command retains only the existing journal identity", () => {
  const values = new Map(), storage = { getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value) };
  const intent = deploymentActionIntent("guide", { deployment_id: "installed", action: "prepare-setup" },
    "00000000-0000-4000-8000-000000000001");
  storeDeploymentActionIntent(storage, intent);
  assert.deepEqual(loadDeploymentActionIntent(storage, "guide"), intent);
  assert.deepEqual(Object.keys(intent).sort(), ["version", "guide_id", "deployment_id", "action_request_id", "action", "update_id"].sort());
  assert.throws(() => deploymentActionIntent("guide", { deployment_id: "installed", action: "prepare-setup", update_id: "foreign" },
    "00000000-0000-4000-8000-000000000002"));
});

test("completed Restore offers a separate explicit background choice using the existing action owner", async () => {
  const receipt = receiptFor(), state = guide();
  await mounted(async ({ document, calls, actions, button, click }) => {
    assert.ok(button("Start installed application"));
    assert.equal(button("Run in background").disabled, false);
    assert.match(document.body.textContent, /Runs while this user is logged in/);
    await click("Run in background");
    assert.deepEqual(actions, ["start-background"]);
    assert.equal(calls.length, 1);
  }, { responses: [readback(receipt, state, { kind: "restore", background: {
    available: true, persistence: "Runs while this user is logged in." } })] });
});

test("background intent retains its exact command identity without private setup or update fields", () => {
  const values = new Map(), storage = { getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value) };
  const intent = deploymentActionIntent("guide", { deployment_id: "installed", action: "start-background" },
    "00000000-0000-4000-8000-000000000003");
  storeDeploymentActionIntent(storage, intent);
  assert.deepEqual(loadDeploymentActionIntent(storage, "guide"), intent);
  assert.equal(intent.update_id, null);
  assert.deepEqual(Object.keys(intent).sort(), ["version", "guide_id", "deployment_id", "action_request_id", "action", "update_id"].sort());
  assert.throws(() => deploymentActionIntent("guide", { deployment_id: "installed", action: "start-background", update_id: "foreign" },
    "00000000-0000-4000-8000-000000000004"));
});

test("absent, malformed and unavailable background hints cannot unlock conversion or hide valid interactive Restore", async () => {
  const receipt = receiptFor(), state = guide();
  for (const background of [undefined, null, [], true, { available: false }, { available: "true" },
    { available: true, detail: "x".repeat(501) }, { available: true, persistence: " " },
    { available: true, private_selection_path: "/private/never-send" }]) {
    await mounted(async ({ actions, button, click }) => {
      assert.equal(button("Run in background") === undefined, true);
      await click("Start installed application");
      assert.deepEqual(actions, ["finish-setup"]);
    }, { responses: [readback(receipt, state, { kind: "restore", background })] });
  }
});

// Compare strings and booleans only: a failed assertion on a DOM node prints it.
const protectedFolderDetail = "macOS does not let a background login item read Desktop, Documents, Downloads, iCloud Drive or " +
  "cloud storage folders. To run this application in the background, install it again with the Forge folder and the " +
  "installation folder in a folder such as ~/MetaForge. Start installed application runs it while Forge is open.";

test("an unavailable background hint shows its reason and keeps interactive Restore", async () => {
  const receipt = receiptFor(), state = guide();
  await mounted(async ({ document, actions, button, click }) => {
    assert.equal(button("Run in background") === undefined, true);
    assert.equal(document.body.textContent.includes(protectedFolderDetail), true);
    await click("Start installed application");
    assert.deepEqual(actions, ["finish-setup"]);
  }, { responses: [readback(receipt, state, { kind: "restore",
    background: { available: false, detail: protectedFolderDetail } })] });
});

test("an available background hint never shows an unavailable reason", async () => {
  await mounted(async ({ document, button }) => {
    assert.equal(button("Run in background") !== undefined, true);
    assert.equal(document.body.textContent.includes(protectedFolderDetail), false);
  }, { responses: [readback(receiptFor(), guide(), { kind: "restore",
    background: { available: true, persistence: "Runs while this user is logged in.", detail: protectedFolderDetail } })] });
});

test("Fresh and preparation do not offer background startup even with a true hint", async () => {
  for (const patch of [{}, { action: "prepare-setup" }]) {
    await mounted(async ({ actions, button }) => {
      assert.equal(button("Run in background") === undefined, true);
      assert.deepEqual(actions, []);
    }, { responses: [readback(receiptFor(), guide(), { ...patch, background: { available: true } })] });
  }
});

test("captured background choice stays fenced through pending, disabled and replacement installation transitions", async () => {
  for (const transition of [{ pending: true }, { disabled: true }, { confirmed: false },
    { receipt: receiptFor(guide(), { deployment_id: "replacement-installation" }) }]) {
    await mounted(async ({ actions, button, refresh, invoke }) => {
      const stale = handler(button("Run in background"));
      await refresh(transition);
      await invoke(stale);
      assert.deepEqual(actions, []);
    }, { responses: [readback(receiptFor(), guide(), { kind: "restore", background: { available: true } })] });
  }
});

test("detached guide and unmounted setup cannot dispatch a captured background choice", async () => {
  for (const boundary of ["pending", "detached", "unmounted"]) {
    await mounted(async ({ actions, button, invoke, beginSelection, detachSelection, unmount }) => {
      const stale = handler(button("Run in background"));
      if (boundary === "pending") beginSelection();
      else if (boundary === "detached") detachSelection();
      else await unmount();
      await invoke(stale);
      assert.deepEqual(actions, []);
    }, { responses: [readback(receiptFor(), guide(), { kind: "restore", background: { available: true } })] });
  }
});

test("a late prepare callback cannot replace the later current finish action", async () => {
  const state = guide(), receipt = receiptFor(state);
  await mounted(async ({ calls, actions, click, button, invoke, refresh }) => {
    const originalPrepare = handler(button("Prepare application setup"));
    await refresh({ disabled: true });
    await invoke(originalPrepare);
    assert.deepEqual(actions, []);
    await refresh({ disabled: false });
    assert.equal(calls.length, 2);
    assert.ok(button("Set up and start application"));
    await invoke(originalPrepare);
    assert.deepEqual(actions, []);
    await click("Set up and start application");
    assert.deepEqual(actions, ["finish-setup"]);
  }, { responses: [readback(receipt, state, { action: "prepare-setup" }), readback(receipt, state)] });
});

test("missing or differently bound readbacks cannot unlock setup", () => {
  const state = guide(), receipt = receiptFor(state);
  for (const [field, wrong] of Object.entries({ scope: "foreign-scope", guide_id: "foreign-guide",
    deployment_id: "foreign-installation", build_id: "foreign-build", package_sha256: "0".repeat(64), guide_version: 7 })) {
    for (const absent of [false, true]) {
      const value = readback(receipt, state); if (absent) delete value[field]; else value[field] = wrong;
      assert.equal(applicationSetupReadback(value, receipt, state), null, `${field}: ${absent ? "missing" : "mismatched"}`);
    }
  }
  for (const change of [{ available: "true" }, { available: null }, { kind: "guess" }, { kind: null },
    { guide_version: true }, { guide_version: "6" }, { package_sha256: null }])
    assert.equal(applicationSetupReadback(readback(receipt, state, change), receipt, state), null);
});

test("paired missing or invalid installed guide versions are not an exact binding", () => {
  const state = guide();
  for (const version of [undefined, null, true, "6", -1, 1.5]) {
    const receipt = receiptFor(state, { guide_version: version }), value = readback(receipt, state);
    if (version === undefined) { delete receipt.guide_version; delete value.guide_version; }
    assert.equal(applicationSetupReadback(value, receipt, state), null, `Invalid original version ${String(version)}`);
  }
});

test("ordinary, live, remote, unproven and unsettled receipts cannot enter the stopped-product lane", () => {
  const state = guide();
  const invalid = [null, { guide_id: "foreign" }, { deployment_id: "" }, { build_id: "" },
    { package_sha256: "not-a-hash" }, { source_kind: "native-team" }, { source_kind: "foreign" },
    { target: "ssh" }, { execution_mode: "background" }, { service: { manager: "launchd" } },
    { status: "running" }, { status: "failed" }, { process_live: true }, { process_live: null }, { runtime_pid: 8 },
    { update: { status: "completed" } }, { interactive_cleanup: { boundary: "worker-not-admitted" } },
    { runtime_candidate: null }, { installed_candidate_runtime_proof: null }];
  for (const patch of invalid) {
    const receipt = patch === null ? null : receiptFor(state, patch);
    assert.equal(deploymentApplicationStartupPending(receipt, state), false, JSON.stringify(patch));
    assert.equal(applicationSetupKey(receipt, state), null);
  }
  for (const mutate of [proof => { proof.source = "source-report"; }, proof => { proof.version = "another-version"; },
    proof => { proof.assembly_sha256 = "0".repeat(64); }, proof => { proof.catalog_sha256 = "0".repeat(64); },
    proof => { proof.sdk.meta.sha256 = "0".repeat(64); }, proof => { proof.sdk.toolkit.version = "another-version"; }]) {
    const receipt = receiptFor(state); mutate(receipt.installed_candidate_runtime_proof);
    assert.equal(deploymentApplicationStartupPending(receipt, state), false);
  }
});

test("a staged or blocked update keeps the stopped original's Start; other update states do not", () => {
  const state = guide();
  for (const status of ["staged", "blocked"]) {
    const receipt = receiptFor(state, { update: { update_id: "synthetic-staged-update", status } });
    assert.equal(deploymentApplicationStartupPending(receipt, state), true, status);
  }
  for (const status of ["snapshotting", "applying", "verifying", "restoring", "recovering-candidate",
    "recovering-current", "recovery-needed", "completed", "rolled-back", undefined]) {
    const receipt = receiptFor(state, { update: { update_id: "synthetic-staged-update", status } });
    assert.equal(deploymentApplicationStartupPending(receipt, state), false, String(status));
  }
});

for (const [kind, title, label] of [["fresh", "Set up Synthetic Reading", "Set up and start application"],
  ["restore", "Start Synthetic Reading", "Start installed application"]]) {
  test(`a genuine bound ${kind} readback uses its own label and only the explicit finish-setup callback`, async () => {
    await mounted(async ({ document, calls, actions, settings, checks, click, button }) => {
      assert.equal(document.querySelector("h3").textContent, title);
      assert.equal(button(label).disabled, false);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.deepEqual(checks, []);
      assert.equal(calls.length, 1);
      assert.equal(calls[0].url, "/api/guide/synthetic-reading-guide/deployment/application-setup?deployment_id=synthetic-installed-reading");
      await click(label);
      assert.deepEqual(actions, ["finish-setup"]);
      assert.equal(calls.length, 1, "The component uses the existing effect owner callback, not a new POST.");
    }, { responses: [readback(receiptFor(), guide(), { kind })] });
  });
}

test("missing settings offers the relevant existing settings action without starting or collecting private values", async () => {
  await mounted(async ({ document, calls, actions, settings, click, button }) => {
    assert.match(document.body.textContent, /Needed: Runtime provider, Console login\./);
    assert.equal(button("Set up and start application") === undefined, true);
    assert.equal(button("Start installed application") === undefined, true);
    assert.equal(button("Review private settings").disabled, false);
    await click("Review private settings");
    assert.deepEqual(settings, ["settings"]); assert.deepEqual(actions, []); assert.equal(calls.length, 1);
  }, { responses: [readback(receiptFor(), guide(), { available: false, missing_fields: ["Runtime provider", "Console login"] })] });
});

test("a confirmed unavailable host explains the missing setup integration and only rechecks on request", async () => {
  const state = guide(), receipt = receiptFor(state);
  const unavailable = readback(receipt, state, { available: false, reason: "host_unavailable" });
  assert.equal(applicationSetupReadback(unavailable, receipt, state).reason, "host-unavailable");
  assert.equal(applicationSetupReadback({ ...unavailable, deployment_id: "another-installation" }, receipt, state), null);
  assert.equal(applicationSetupReadback(readback(receipt, state, { reason: "host_unavailable" }), receipt, state).reason, null);
  await mounted(async ({ document, calls, actions, settings, click, button }) => {
    assert.match(document.body.textContent, /Setup is not available yet/);
    assert.match(document.body.textContent, /Connecting the guide or changing private settings will not enable it/);
    assert.match(document.body.textContent, /Application setupNot available yet/);
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(button("Review private settings") === undefined, true);
    assert.equal(button("Set up and start application") === undefined, true);
    assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.equal(calls.length, 1);
    await click("Check setup status");
    assert.equal(calls.length, 2); assert.deepEqual(actions, []); assert.deepEqual(settings, []);
    assert.equal(button("Set up and start application").disabled, false);
    await click("Set up and start application");
    assert.deepEqual(actions, ["finish-setup"]);
  }, { responses: [unavailable, readback(receipt, state)] });
});

for (const [kind, label] of [["fresh", "Set up and start application"],
  ["restore", "Start installed application"]]) {
  test(`saved setup recovery only reads until an exact ${kind} selection is returned`, async () => {
    const state = guide(), receipt = receiptFor(state);
    const unavailable = readback(receipt, state, { available: false, reason: "source_unavailable",
      detail: "/private/synthetic-setup-inputs/DO-NOT-DISPLAY.json",
      missing_fields: ["A settings change cannot repair the selected source"] });
    await mounted(async ({ document, calls, actions, settings, checks, click, button }) => {
      assert.equal(document.querySelector("h3").textContent, "Saved setup needs recovery");
      assert.match(document.body.textContent, /Application setupNeeds recovery/);
      assert.match(document.body.textContent, /Changing private settings cannot repair these inputs/);
      assert.doesNotMatch(document.body.textContent, /DO-NOT-DISPLAY|\/private\/synthetic-setup-inputs|Needed:|Nothing is running|Application setupWaiting/);
      assert.equal(document.querySelector("section").getAttribute("aria-busy"), "false");
      for (const action of ["Prepare application setup", "Set up and start application",
        "Start installed application", "Change private settings", "Review private settings"])
        assert.equal(button(action) === undefined, true, action);
      assert.equal(document.querySelectorAll("button").length, 1);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.equal(calls.length, 1);
      await click("Check setup status");
      assert.deepEqual(checks, ["check"]); assert.equal(calls.length, 2);
      assert.equal(document.querySelector("h3").textContent, "Saved setup needs recovery");
      assert.deepEqual(actions, []); assert.deepEqual(settings, []);
      await click("Check setup status");
      assert.deepEqual(checks, ["check", "check"]); assert.equal(calls.length, 3);
      assert.equal(button(label).disabled, false);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []);
      await click(label);
      assert.deepEqual(actions, ["finish-setup"]); assert.deepEqual(settings, []);
      assert.equal(calls.length, 3, "Only the explicit existing action callback can continue setup.");
    }, { responses: [unavailable, unavailable, readback(receipt, state, { kind })] });
  });
}

test("previous setup recovery describes the recorded attempt and offers only a read-only check", async () => {
  const previous = readback(receiptFor(), guide(), { available: false, reason: "previous_attempt" });
  await mounted(async ({ document, calls, actions, settings, checks, click, button }) => {
    assert.equal(document.querySelector("h3").textContent, "Previous setup needs recovery");
    assert.match(document.body.textContent, /Setup was already attempted\. Forge needs the result of that attempt before it can start the application\./);
    assert.match(document.body.textContent, /Application setupNeeds recovery/);
    assert.match(document.body.textContent, /Private settingsNot checked/);
    assert.equal(document.querySelector("section").getAttribute("aria-busy"), "false");
    assert.doesNotMatch(document.body.textContent, /finish reporting|Waiting|Saving settings|Ready to start|Ready to set up/);
    assert.equal(button("Change private settings") === undefined, true);
    assert.equal(button("Review private settings") === undefined, true);
    assert.equal(document.querySelectorAll("button").length, 1);
    assert.equal(document.querySelectorAll(".deployment-setup-preserved").length, 1);
    assert.equal(document.querySelector(".deployment-setup-preserved").textContent,
      "Your files and saved settings stay in the folder you chose. Check setup status only reads this installation.");
    assert.equal(calls.length, 1); assert.deepEqual(actions, []); assert.deepEqual(settings, []);
    await click("Check setup status");
    assert.equal(calls.length, 2); assert.deepEqual(checks, ["check"]);
    assert.deepEqual(actions, []); assert.deepEqual(settings, []);
    assert.equal(document.querySelector("h3").textContent, "Previous setup needs recovery");
    assert.equal(button("Check setup status").disabled, false);
  }, { responses: [previous, previous] });
});

test("previous setup recovery recheck only enables an explicitly returned saved restore", async () => {
  await mounted(async ({ document, calls, actions, settings, click, button }) => {
    assert.equal(button("Start installed application") === undefined, true);
    await click("Check setup status");
    assert.equal(document.querySelector("h3").textContent, "Start Synthetic Reading");
    assert.equal(button("Start installed application").disabled, false);
    assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.equal(calls.length, 2);
    await click("Start installed application");
    assert.deepEqual(actions, ["finish-setup"]); assert.equal(calls.length, 2);
  }, { responses: [readback(receiptFor(), guide(), { available: false, reason: "previous_attempt" }),
    readback(receiptFor(), guide(), { kind: "restore" })] });
});

test("previous setup recovery check refuses pending and detached guide selections", async () => {
  for (const boundary of ["pending", "detached"]) {
    await mounted(async ({ calls, actions, settings, checks, button, invoke, beginSelection, detachSelection }) => {
      const check = handler(button("Check setup status"));
      if (boundary === "pending") beginSelection(); else detachSelection();
      await invoke(check);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.deepEqual(checks, []);
      assert.equal(calls.length, 1);
    }, { responses: [readback(receiptFor(), guide(), { available: false, reason: "previous_attempt" })] });
  }
});

for (const [name, value] of [["unknown availability", readback(receiptFor(), guide(), { available: false })],
  ["foreign installation reply", readback(receiptFor(), guide(), { deployment_id: "another-installation" })],
  ["failed host read", new Error("Synthetic setup availability unavailable")]]) {
  test(`${name} offers no enabled setup or ordinary start`, async () => {
    await mounted(async ({ document, calls, actions, button }) => {
      assert.equal(button("Set up and start application") === undefined, true);
      assert.equal(button("Start installed application") === undefined, true);
      assert.equal(button("Check setup status").disabled, false);
      assert.match(document.body.textContent, /cannot finish setup/);
      assert.deepEqual(actions, []); assert.equal(calls.length, 1);
    }, { responses: [value] });
  });
}

for (const flag of ["confirmed", "pending"]) {
  test(`an unconfirmed ${flag} transition removes the old ready action and does not assert stopped runtime`, async () => {
    await mounted(async ({ document, calls, actions, button, refresh, invoke }) => {
      const stale = handler(button("Set up and start application"));
      await refresh(flag === "confirmed" ? { confirmed: false } : { pending: true });
      assert.match(document.body.textContent, /Result not confirmed/);
      assert.doesNotMatch(document.body.textContent, /Nothing is running|InstallationChecked|Ready to continue/);
      assert.equal(button("Set up and start application") === undefined, true);
      await invoke(stale);
      assert.deepEqual(actions, []); assert.equal(calls.length, 1);
    });
  });
}

test("initial unconfirmed status does not read availability, assert stopped runtime or dispatch setup", async () => {
  await mounted(async ({ document, calls, actions, button }) => {
    assert.match(document.body.textContent, /Result not confirmed/);
    assert.doesNotMatch(document.body.textContent, /Nothing is running/);
    assert.equal(button("Set up and start application") === undefined, true);
    assert.deepEqual(calls, []); assert.deepEqual(actions, []);
  }, { props: { confirmed: false } });
});

for (const changed of ["guide", "installation"]) {
  test(`a delayed old ${changed} read cannot attach after the current selection changes`, async () => {
    await mounted(async ({ document, calls, actions, button, refresh, release }) => {
      assert.match(document.body.textContent, /Checking setup/);
      assert.equal(button("Set up and start application") === undefined, true);
      const nextState = changed === "guide" ? { id: "synthetic-next-guide", version: 9 } : guide();
      const nextReceipt = receiptFor(nextState, { deployment_id: "synthetic-next-installation", package_sha256: "1".repeat(64) });
      await refresh(changed === "guide" ? { state: nextState, receipt: nextReceipt } : { receipt: nextReceipt });
      assert.equal(document.querySelector("h3").textContent, "Set up Current Application");
      await release(0, readback(receiptFor(), guide(), { application_name: "OLD WRONG APPLICATION" }));
      assert.equal(document.querySelector("h3").textContent, "Set up Current Application");
      assert.doesNotMatch(document.body.textContent, /OLD WRONG APPLICATION/);
      assert.deepEqual(actions, []); assert.equal(calls.length, 2);
      assert.match(calls[1].url, /deployment_id=synthetic-next-installation$/);
    }, { holdReads: [0], responses: [readback(), readback(receiptFor(
      changed === "guide" ? { id: "synthetic-next-guide", version: 9 } : guide(),
      { deployment_id: "synthetic-next-installation", package_sha256: "1".repeat(64) }),
      changed === "guide" ? { id: "synthetic-next-guide", version: 9 } : guide(), { application_name: "Current Application" })] });
  });
}

test("a previously enabled setup callback cannot dispatch for a replacement installation", async () => {
  await mounted(async ({ actions, button, refresh, invoke, release }) => {
    const stale = handler(button("Set up and start application"));
    await refresh({ receipt: receiptFor(guide(), { deployment_id: "replacement-installation", package_sha256: "2".repeat(64) }) });
    await invoke(stale);
    assert.deepEqual(actions, []);
    await release(1);
    assert.equal(button("Set up and start application").disabled, false);
    await invoke(stale);
    assert.deepEqual(actions, [], "The old handler remains detached after the replacement's valid read.");
  }, { holdReads: [1] });
});

test("an outer pending or detached guide selection fences captured start, settings and check callbacks", async () => {
  for (const boundary of ["pending", "detached"]) {
    await mounted(async ({ calls, actions, settings, checks, button, invoke, beginSelection, detachSelection }) => {
      const start = handler(button("Set up and start application"));
      const edit = handler(button("Change private settings"));
      if (boundary === "pending") beginSelection(); else detachSelection();
      await invoke(start); await invoke(edit);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.deepEqual(checks, []);
      assert.equal(calls.length, 1);
    });
    await mounted(async ({ calls, actions, settings, checks, button, invoke, beginSelection, detachSelection }) => {
      const check = handler(button("Check setup status"));
      if (boundary === "pending") beginSelection(); else detachSelection();
      await invoke(check);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.deepEqual(checks, []);
      assert.equal(calls.length, 1);
    }, { responses: [readback(receiptFor(), guide(), { available: false })] });
  }
});

test("a captured setup callback cannot dispatch after its component is unmounted", async () => {
  await mounted(async ({ actions, button, invoke, unmount }) => {
    const stale = handler(button("Set up and start application"));
    await unmount(); await invoke(stale);
    assert.deepEqual(actions, [], "A detached UI callback cannot send setup through the still-current outer guide owner.");
  });
});

test("manual Check reads again without sending setup or private configuration", async () => {
  await mounted(async ({ document, calls, actions, settings, checks, click, button }) => {
    assert.equal(calls.length, 1); assert.equal(button("Check setup status").disabled, false);
    await click("Check setup status");
    assert.deepEqual(checks, ["check"]); assert.deepEqual(actions, []); assert.deepEqual(settings, []);
    assert.equal(calls.length, 2); assert.equal(calls[0].url, calls[1].url);
    assert.equal(document.querySelector("h3").textContent, "Set up Synthetic Reading");
    assert.equal(button("Set up and start application").disabled, false);
  }, { responses: [readback(receiptFor(), guide(), { available: false }), readback()] });
});
