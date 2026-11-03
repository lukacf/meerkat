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
    onAction: action => { actions.push(action); fixture.onAction?.(action); }, onSettings: () => settings.push("settings"), onCheck: () => checks.push("check") })));
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


function reconnect(receipt = receiptFor(), state = guide(), patch = {}) {
  return { scope: "selected-stopped-application-inputs", guide_id: state.id,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    package_sha256: receipt.package_sha256, guide_version: receipt.guide_version,
    available: false, reason: "owner_reconnect", action: "prepare-setup",
    application_name: "Synthetic Reading", missing_fields: [], ...patch };
}

test("a reconnect hint retains installed A while live guide B remains separate from setup readiness", () => {
  const state = guide(), receipt = receiptFor(state), value = reconnect(receipt, state);
  const before = structuredClone({ state, receipt, value });
  assert.notEqual(state.version, receipt.guide_version);
  assert.equal(deploymentApplicationStartupPending(receipt, state), true);
  assert.equal(Object.hasOwn(value, "kind"), false);
  const checked = applicationSetupReadback(value, receipt, state);
  assert.equal(checked.available, false);
  assert.equal(checked.kind, undefined);
  assert.equal(checked.reason, "owner-reconnect");
  assert.equal(checked.action, "prepare-setup");
  const view = applicationSetupView({ state, receipt, confirmed: true, pending: false,
    snapshot: { key: applicationSetupKey(receipt, state), pending: false, value } });
  assert.equal(view.phase, "reconnect");
  assert.equal(view.available, false);
  assert.equal(view.kind, undefined);
  assert.equal(view.background, undefined);
  assert.deepEqual({ state, receipt, value }, before);
});

test("malformed ready and reconnect hybrids cannot become a recovery or startup capability", () => {
  const state = guide(), receipt = receiptFor(state);
  for (const patch of [{ available: true, kind: "fresh" }, { available: true, kind: "restore" },
    { kind: "fresh" }, { kind: "restore" }, { kind: "invented" }, { available: "false" },
    { action: "finish-setup" }, { action: "start-background" }, { action: null },
    { missing_fields: ["Runtime provider"] }, { missing_fields: "Runtime provider" }]) {
    const value = reconnect(receipt, state, patch);
    assert.equal(applicationSetupReadback(value, receipt, state), null, JSON.stringify(patch));
    assert.equal(applicationSetupView({ receipt, state, confirmed: true, pending: false,
      snapshot: { key: applicationSetupKey(receipt, state), pending: false, value } }).phase, "unavailable");
  }
  const missingAction = reconnect(receipt, state); delete missingAction.action;
  assert.equal(applicationSetupReadback(missingAction, receipt, state), null);
  assert.equal(applicationSetupReadback(reconnect(receipt, state, { reason: "unknown_reason" }), receipt, state), null);
});

test("reconnect requires every original receipt binding and a checked stopped product", () => {
  const state = guide(), receipt = receiptFor(state);
  for (const [field, wrong] of Object.entries({ scope: "foreign-scope", guide_id: "foreign-guide",
    deployment_id: "foreign-installation", build_id: "foreign-build", package_sha256: "0".repeat(64),
    guide_version: state.version })) {
    for (const absent of [false, true]) {
      const value = reconnect(receipt, state);
      if (absent) delete value[field]; else value[field] = wrong;
      assert.equal(applicationSetupReadback(value, receipt, state), null,
        `${field}: ${absent ? "missing" : "mismatched"}`);
    }
  }
  for (const patch of [{ status: "running" }, { process_live: true }, { runtime_pid: 123 },
    { target: "ssh" }, { execution_mode: "background" }, { installed_candidate_runtime_proof: null },
    { update: { status: "completed" } }, { interactive_cleanup: { boundary: "worker-not-admitted" } }]) {
    const changed = receiptFor(state, patch);
    assert.equal(applicationSetupReadback(reconnect(changed, state), changed, state), null, JSON.stringify(patch));
  }
});

test("Continue setup delegates only one explicit prepare intent and never claims completed or running setup", async () => {
  const state = guide(), receipt = receiptFor(state), values = new Map(), intents = [];
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  const requestId = "00000000-0000-4000-8000-000000000001";
  await mounted(async ({ document, calls, actions, settings, checks, click, button, refresh, invoke }) => {
    assert.equal(button("Continue setup").disabled, false);
    for (const label of ["Prepare application setup", "Set up and start application", "Start installed application", "Run in background"])
      assert.equal(button(label) === undefined, true, label);
    assert.match(document.body.textContent, /check this installation's saved setup/i);
    assert.match(document.body.textContent, /stays stopped|does not start the application/i);
    assert.doesNotMatch(document.body.textContent, /Ready to start|Ready to set up|Private settingsSaved|DO-NOT-DISPLAY|\/private\/synthetic/);
    assert.equal(document.querySelector("section").getAttribute("aria-busy"), "false");
    assert.deepEqual(actions, []); assert.deepEqual(settings, []); assert.deepEqual(checks, []);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "/api/guide/synthetic-reading-guide/deployment/application-setup?deployment_id=synthetic-installed-reading");
    const captured = handler(button("Continue setup"));
    await click("Continue setup");
    assert.deepEqual(actions, ["prepare-setup"]);
    assert.deepEqual(loadDeploymentActionIntent(storage, state.id), intents[0]);
    assert.deepEqual(intents[0], { version: 1, guide_id: state.id, deployment_id: receipt.deployment_id,
      action_request_id: requestId, action: "prepare-setup", update_id: null });
    assert.equal(calls.length, 1, "The existing action owner receives an intent, not another browser request.");
    await refresh({ pending: true });
    await invoke(captured); await invoke(captured);
    assert.equal(intents.length, 1, "Pending original command admission fences repeated captured clicks.");
    assert.deepEqual(actions, ["prepare-setup"]);
  }, { responses: [reconnect(receipt, state, { detail: "/private/synthetic/DO-NOT-DISPLAY.json" })],
    onAction: action => {
      const intent = deploymentActionIntent(state.id, { deployment_id: receipt.deployment_id, action }, requestId);
      storeDeploymentActionIntent(storage, intent); intents.push(intent);
    } });
});

test("a retained pending reconnect intent survives remount without automatic submission or setup read", async () => {
  const state = guide(), receipt = receiptFor(state), values = new Map();
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  const intent = deploymentActionIntent(state.id, { deployment_id: receipt.deployment_id, action: "prepare-setup" },
    "00000000-0000-4000-8000-000000000002");
  const fixture = { responses: [reconnect(receipt, state)], onAction: action => {
    assert.equal(action, intent.action); storeDeploymentActionIntent(storage, intent);
  } };
  await mounted(async ({ actions, click }) => {
    assert.deepEqual(actions, []);
    await click("Continue setup");
    assert.deepEqual(actions, ["prepare-setup"]);
  }, fixture);
  const saved = [...values];
  await mounted(async ({ document, calls, actions, button }) => {
    assert.deepEqual(calls, []); assert.deepEqual(actions, []);
    assert.equal(button("Continue setup") === undefined, true);
    assert.match(document.body.textContent, /not be sent again automatically/);
    assert.deepEqual(loadDeploymentActionIntent(storage, state.id), intent);
    assert.deepEqual([...values], saved);
  }, { ...fixture, props: { pending: true } });
});

test("an explicit read-only recheck may expose reconnect but never performs recovery", async () => {
  const state = guide(), receipt = receiptFor(state);
  await mounted(async ({ document, calls, actions, settings, checks, click, button }) => {
    assert.equal(button("Continue setup") === undefined, true);
    await click("Check setup status");
    assert.equal(calls.length, 2); assert.deepEqual(checks, ["check"]);
    assert.deepEqual(actions, []); assert.deepEqual(settings, []);
    assert.equal(button("Continue setup").disabled, false);
    assert.match(document.body.textContent, /stays stopped|does not start the application/i);
    await click("Continue setup");
    assert.deepEqual(actions, ["prepare-setup"]); assert.equal(calls.length, 2);
  }, { responses: [readback(receipt, state, { available: false, reason: "source_unavailable" }), reconnect(receipt, state)] });
});

for (const [kind, label] of [["fresh", "Set up and start application"], ["restore", "Start installed application"]]) {
  test(`a later exact ${kind} readback replaces reconnect without replaying recovery or starting automatically`, async () => {
    const state = guide(), receipt = receiptFor(state);
    await mounted(async ({ document, calls, actions, button, refresh, invoke, click }) => {
      const stale = handler(button("Continue setup"));
      await refresh({ disabled: true }); await invoke(stale);
      assert.deepEqual(actions, []);
      await refresh({ disabled: false });
      assert.equal(calls.length, 2);
      assert.equal(button("Continue setup") === undefined, true);
      assert.equal(button(label).disabled, false);
      assert.doesNotMatch(document.body.textContent, /Saved setup to check/);
      await invoke(stale);
      assert.deepEqual(actions, [], "A previous reconnect view cannot dispatch against a returned startup selection.");
      await click(label);
      assert.deepEqual(actions, ["finish-setup"]); assert.equal(calls.length, 2);
    }, { responses: [reconnect(receipt, state), readback(receipt, state, { kind })] });
  });
}

test("pending, disabled and unconfirmed transitions refuse captured reconnect actions", async () => {
  for (const patch of [{ pending: true }, { disabled: true }, { confirmed: false }]) {
    await mounted(async ({ calls, actions, button, refresh, invoke }) => {
      const stale = handler(button("Continue setup"));
      await refresh(patch); await invoke(stale);
      assert.deepEqual(actions, [], JSON.stringify(patch));
      assert.equal(calls.length, 1);
    }, { responses: [reconnect()] });
  }
});

test("changed guide or receipt identity refuses the old reconnect click even after a valid new read", async () => {
  const changes = [
    { state: { id: "synthetic-other-guide", version: 9 } },
    { state: { ...guide(), version: 8 } },
    ...Object.entries({ deployment_id: "synthetic-other-installation", build_id: "synthetic-other-build",
      package_sha256: "1".repeat(64), source_digest: "2".repeat(64), guide_version: 5,
      updated_at: "2026-10-11T08:00:00Z" }).map(([field, value]) => ({ receipt: receiptFor(guide(), { [field]: value }) })),
  ];
  for (const change of changes) {
    const nextState = change.state || guide();
    const nextReceipt = change.receipt || receiptFor(nextState);
    await mounted(async ({ calls, actions, button, refresh, invoke, release, click }) => {
      const stale = handler(button("Continue setup"));
      await refresh(change.state ? { state: nextState, receipt: nextReceipt } : { receipt: nextReceipt });
      await invoke(stale);
      assert.deepEqual(actions, [], JSON.stringify(change));
      assert.equal(calls.length, 2);
      assert.equal(button("Continue setup") === undefined, true);
      await release(1, reconnect(nextReceipt, nextState));
      assert.equal(button("Continue setup").disabled, false);
      await invoke(stale);
      assert.deepEqual(actions, [], "A new valid owner read cannot revive the previous view's callback.");
      await click("Continue setup");
      assert.deepEqual(actions, ["prepare-setup"]);
    }, { responses: [reconnect()], holdReads: [1] });
  }
});

test("an old suspended reconnect observation cannot replace a current different owner", async () => {
  const nextState = { id: "synthetic-other-guide", version: 9 };
  const nextReceipt = receiptFor(nextState, { deployment_id: "synthetic-other-installation", package_sha256: "1".repeat(64) });
  await mounted(async ({ document, calls, actions, button, refresh, release, click }) => {
    assert.equal(button("Continue setup") === undefined, true);
    await refresh({ state: nextState, receipt: nextReceipt });
    assert.equal(button("Continue setup").disabled, false);
    await release(0, reconnect(receiptFor(), guide(), { application_name: "OLD WRONG APPLICATION" }));
    assert.doesNotMatch(document.body.textContent, /OLD WRONG APPLICATION/);
    assert.match(document.querySelector("h3").textContent, /Current Application/);
    assert.deepEqual(actions, []); assert.equal(calls.length, 2);
    await click("Continue setup");
    assert.deepEqual(actions, ["prepare-setup"]);
  }, { holdReads: [0], responses: [reconnect(), reconnect(nextReceipt, nextState, { application_name: "Current Application" })] });
});

test("detached, changing and unmounted selections cannot dispatch a captured reconnect action", async () => {
  for (const boundary of ["detached", "changing", "unmounted"]) {
    await mounted(async ({ calls, actions, button, invoke, detachSelection, beginSelection, unmount }) => {
      const stale = handler(button("Continue setup"));
      if (boundary === "detached") detachSelection();
      else if (boundary === "changing") beginSelection();
      else await unmount();
      await invoke(stale);
      assert.deepEqual(actions, [], boundary); assert.equal(calls.length, 1);
    }, { responses: [reconnect()] });
  }
});

test("contradictory reconnect replies do not mount a Continue or native startup action", async () => {
  for (const patch of [{ available: true, kind: "fresh" }, { kind: "restore" },
    { action: "finish-setup" }, { missing_fields: ["Runtime provider"] }]) {
    await mounted(async ({ document, calls, actions, settings, button, click }) => {
      for (const label of ["Continue setup", "Prepare application setup", "Set up and start application", "Start installed application", "Run in background"])
        assert.equal(button(label) === undefined, true, JSON.stringify(patch));
      assert.doesNotMatch(document.body.textContent, /Ready to start|Ready to set up|Saved setup found/);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []);
      await click("Check setup status");
      assert.deepEqual(actions, []); assert.equal(calls.length, 2);
      assert.equal(button("Continue setup").disabled, false);
    }, { responses: [reconnect(receiptFor(), guide(), patch), reconnect()] });
  }
});

test("reconnect cannot borrow a background capability or leak raw private server details", async () => {
  await mounted(async ({ document, actions, settings, button }) => {
    assert.equal(button("Continue setup").disabled, false);
    assert.equal(button("Run in background") === undefined, true);
    assert.doesNotMatch(document.body.textContent, /DO-NOT-DISPLAY|\/private\/synthetic|Claims a native grant/);
    assert.deepEqual(actions, []); assert.deepEqual(settings, []);
  }, { responses: [reconnect(receiptFor(), guide(), { detail: "/private/synthetic/DO-NOT-DISPLAY.json",
    background: { available: true, persistence: "Claims a native grant" } })] });
});

test("failed or foreign reads need an explicit fresh observation before reconnect is offered", async () => {
  for (const first of [new Error("Synthetic observation failed"), reconnect(receiptFor(), guide(), { guide_id: "foreign-guide" })]) {
    await mounted(async ({ calls, actions, settings, button, click }) => {
      assert.equal(button("Continue setup") === undefined, true);
      assert.deepEqual(actions, []); assert.deepEqual(settings, []);
      await click("Check setup status");
      assert.equal(calls.length, 2); assert.equal(button("Continue setup").disabled, false);
      assert.deepEqual(actions, []);
    }, { responses: [first, reconnect()] });
  }
});
