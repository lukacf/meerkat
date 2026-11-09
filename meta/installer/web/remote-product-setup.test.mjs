import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { remoteProductControl, remoteControlBindingKey, remoteActionAvailable, remoteActionBody, remoteProductActionSettled } from "./remote-deployment.mjs";
import { remoteInstalledCanConfigure, remoteInstalledSettingsOptions, remoteInstalledSettingsPacket } from "./remote-private-settings.mjs";
import { remoteApplicationSetupKey, remoteApplicationSetupReadback, remoteApplicationSetupView } from "./remote-application-setup.mjs";

const UUID = "11111111-1111-4111-8111-111111111111";
const state = { id: "synthetic-remote-guide", version: 4, proposal: { target: "linux" } };
const build = { guide_id: state.id, guide_version: 4, build_id: "synthetic-accepted-build", source_digest: "a".repeat(64),
  status: "completed", current: true, ready_to_package: true, acceptance_current: true, acceptance_trusted: true, acceptance: { status: "pass" } };
const options = { build_id: build.build_id, guide_version: 4, current: true, supported: false, target: "linux", host_target: "mac" };
function fixture() {
  const receipt = { protocol: 1, deployment_id: UUID, guide_id: state.id, guide_version: 4, build_id: build.build_id,
    accepted_source_digest: build.source_digest, package_sha256: "b".repeat(64), transferred_source_digest: "c".repeat(64),
    controller_sha256: "d".repeat(64), destination: "/synthetic-owned/src/workshop", endpoint: { host: "workshop.local", user: "volunteer", ssh_port: 22 },
    source_binding_current: true, current_guide_version: 4, status: "stopped", observation: { available: true }, source_current: true,
    product_runtime: true, product_scope: "retained-original-product-owner", install_operation_id: UUID,
    product_control_manifest_sha256: "e".repeat(64), worker_live: true, cleanup_confirmed: false,
    can_configure: true, private_settings_supported: true, can_prepare_setup: true, can_finish_setup: false, can_start_background: false };
  const selected = { format: "forge-meta-toolkit-runtime-selection-v1", assembly: { sha256: "c".repeat(64) },
    model_catalog: { sha256: "d".repeat(64) }, meta: { version: "0.9.0", sha256: "e".repeat(64) }, toolkit: { version: "0.1.0", sha256: "f".repeat(64) } };
  const factory = { guide_id: state.id, guide_version: 4, deployment_id: UUID, build_id: build.build_id, package_sha256: receipt.package_sha256,
    destination: receipt.destination, source_digest: build.source_digest, source_verified: true, status: "stopped", phase: "configure",
    target: "local", execution_mode: "interactive", process_live: false, runtime_pid: null, updated_at: "2026-10-12T10:00:00Z", runtime_candidate: selected,
    installed_candidate_runtime_proof: { source: "meta-toolkit-product", version: "0.9.0", assembly_sha256: selected.assembly.sha256,
      catalog_sha256: selected.model_catalog.sha256, sdk: { meta: selected.meta, toolkit: selected.toolkit } } };
  const binding = { protocol: 1, install_operation_id: UUID, control_manifest_sha256: receipt.product_control_manifest_sha256,
    initial_install_sha256: "f".repeat(64), binding: Object.fromEntries(["guide_id", "guide_version", "build_id", "accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"].map((key) => [key, receipt[key]])),
    worker: { pid: 100, uid: 501, start_ticks: "100", boot_id: "synthetic-boot" }, command: { pid: 101, uid: 501, start_ticks: "101", boot_id: "synthetic-boot" } };
  receipt.product_control = { binding, owner_live: true, initial_install_complete: true, factory_status: factory };
  return receipt;
}
function readback(receipt) {
  return { scope: "selected-stopped-application-inputs", guide_id: state.id, guide_version: receipt.guide_version,
    deployment_id: receipt.deployment_id, build_id: receipt.build_id, package_sha256: receipt.package_sha256,
    source_digest: receipt.accepted_source_digest, remote_control_binding: structuredClone(receipt.product_control.binding),
    available: true, kind: "fresh", action: "prepare-setup", application_name: "Synthetic Observatory", missing_fields: [] };
}
function metadata(receipt) {
  return { version: 4, guide_version: 4, deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256, console_port: 18088,
    scope: "verified-remote-installed-private-configuration", supported: true, source_verified: true, requires_stopped: true,
    can_use_host_provider_credentials: false, remote_control_binding: structuredClone(receipt.product_control.binding),
    fields: [{ key: "API_KEY", label: "Provider key", required: true, secret: true, configured: true, removable: false }] };
}

test("live product owner uses finite original binding without conventional cleanup claims", () => {
  const receipt = fixture(); assert.ok(remoteProductControl(receipt, state));
  assert.equal(remoteInstalledCanConfigure(receipt, options, build, state), true);
  for (const mutate of [r => { r.product_scope = "byte-install-stopped"; }, r => { r.worker_live = false; },
    r => { r.cleanup_confirmed = true; }, r => { r.product_control.owner_live = false; },
    r => { r.product_control.binding.command.start_ticks = ""; }, r => { r.product_control.factory_status.source_verified = false; },
    r => { r.product_control.binding.binding.controller_sha256 = "0".repeat(64); }]) {
    const bad = structuredClone(receipt); mutate(bad);
    assert.equal(remoteProductControl(bad, state), null); assert.equal(remoteInstalledCanConfigure(bad, options, build, state), false);
    assert.equal(remoteApplicationSetupKey(bad, options, build, state), null);
  }
});

test("setup and settings replies must select the exact retained actor, not just package", () => {
  const receipt = fixture(), setup = readback(receipt), settings = metadata(receipt);
  assert.ok(remoteApplicationSetupReadback(setup, receipt, options, build, state));
  assert.ok(remoteInstalledSettingsOptions(settings, receipt, options, build, state));
  for (const response of [setup, settings]) {
    response.remote_control_binding.command.pid++;
    assert.equal(response.scope === "selected-stopped-application-inputs"
      ? remoteApplicationSetupReadback(response, receipt, options, build, state)
      : remoteInstalledSettingsOptions(response, receipt, options, build, state), null);
  }
  const other = readback(receipt); other.source_digest = "0".repeat(64);
  assert.equal(remoteApplicationSetupReadback(other, receipt, options, build, state), null);
});

test("pending original command blocks all new mutation while setup remains a waiting view", () => {
  const receipt = fixture(), key = remoteApplicationSetupKey(receipt, options, build, state), value = readback(receipt);
  receipt.pending_operation = { id: UUID, action: "prepare-setup" };
  assert.equal(remoteApplicationSetupView({ receipt, options, build, state, confirmed: true, snapshot: { key, value } }).phase, "unconfirmed");
  for (const action of ["prepare-setup", "finish-setup", "start-background", "restart", "stop"])
    assert.equal(remoteActionBody(receipt, action, options, build, state, UUID), null);
  assert.equal(remoteInstalledCanConfigure(receipt, options, build, state), false);
  assert.equal(remoteInstalledSettingsPacket({ deployment_id: receipt.deployment_id }, receipt, state, UUID), null);
});

test("product UUID packets and settled observations preserve ordinary SSH request shape", () => {
  const receipt = fixture();
  assert.deepEqual(remoteActionBody(receipt, "prepare-setup", options, build, state, UUID), { deployment_id: UUID, action: "prepare-setup", action_request_id: UUID });
  assert.equal(remoteActionBody(receipt, "prepare-setup", options, build, state, "not-a-uuid"), null);
  const body = { deployment_id: UUID, settings: { API_KEY: "inert-private-value" } };
  assert.equal(remoteInstalledSettingsPacket(body, receipt, state), null);
  assert.deepEqual(remoteInstalledSettingsPacket(body, receipt, state, UUID), { ...body, action_request_id: UUID });
  receipt.product_control.action_request = { action_request_id: UUID, guide_id: state.id, deployment_id: UUID, action: "configure", phase: "settled", outcome: "returned" };
  assert.ok(remoteProductActionSettled(receipt, state, UUID, "configure"));
  assert.equal(remoteProductActionSettled(receipt, state, UUID, "finish-setup"), null);
  const ordinary = { ...receipt, product_runtime: false, product_scope: undefined, product_control: undefined,
    status: "stopped", worker_live: false, cleanup_confirmed: true, can_restart: true };
  assert.deepEqual(remoteActionBody(ordinary, "restart", options, build, state), { deployment_id: UUID, action: "restart" });
  assert.equal(remoteInstalledSettingsPacket(body, ordinary, state), body);
});

test("Stop may preserve an older running source; setup and restart require current source", () => {
  const receipt = fixture(); receipt.source_binding_current = false; receipt.status = "running"; receipt.can_stop = true;
  receipt.product_control.factory_status.status = "running"; receipt.product_control.factory_status.process_live = true;
  assert.equal(remoteActionAvailable(receipt, "stop", options, build, state, true), true);
  assert.deepEqual(remoteActionBody(receipt, "stop", options, build, state, UUID, true), { deployment_id: UUID, action: "stop", action_request_id: UUID });
  assert.equal(remoteActionAvailable(receipt, "restart", options, build, state), false);
  assert.equal(remoteApplicationSetupKey(receipt, options, build, state), null);
});

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const compiled = await bundle({ entryPoints: [fileURLToPath(new URL("remote-application-setup.jsx", import.meta.url))], bundle: true,
  write: false, format: "esm", platform: "node", logLevel: "silent", plugins: [{ name: "shared-mounted-react", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { RemoteApplicationSetup } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`);

test("remote wrapper mounts the actual setup card, reads only, and delegates explicit action once", async () => {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const receipt = fixture(), actions = [], calls = []; let root;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: async (url, settings = {}) => {
      calls.push({ url, method: settings.method || "GET" }); assert.equal(settings.body, undefined);
      return { ok: true, json: async () => readback(receipt) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  try {
    for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
    const { createRoot } = await import("react-dom/client"); root = createRoot(document.getElementById("root"));
    const render = () => act(async () => root.render(React.createElement(RemoteApplicationSetup,
      { receipt: structuredClone(receipt), options, build, state, confirmed: true, onAction: (action) => actions.push(action) })));
    await render();
    assert.ok(document.querySelector('[aria-label="Application setup"]'));
    const prepare = [...document.querySelectorAll("button")].find((button) => button.textContent === "Prepare application setup");
    assert.ok(prepare); assert.equal(prepare.disabled, false); await act(async () => prepare.click());
    assert.deepEqual(actions, ["prepare-setup"]); assert.equal(calls.length, 1); assert.equal(calls[0].method, "GET");
    assert.match(calls[0].url, /remote-deployment\/application-setup\?deployment_id=/);
    receipt.pending_operation = { id: UUID, action: "prepare-setup" }; await render();
    assert.match(document.body.textContent, /Setup will not be sent again automatically/);
    assert.equal([...document.querySelectorAll("button")].some((button) => button.textContent === "Prepare application setup"), false);
    assert.deepEqual(actions, ["prepare-setup"]); assert.equal(calls.length, 1);
  } finally {
    if (root) await act(async () => root.unmount()); dom.window.close();
    for (const [key, value] of previous) { if (value) Object.defineProperty(globalThis, key, value); else delete globalThis[key]; }
  }
});

const compiledSettings = await bundle({ entryPoints: [fileURLToPath(new URL("remote-private-settings.jsx", import.meta.url))], bundle: true,
  write: false, format: "esm", platform: "node", logLevel: "silent", plugins: [{ name: "shared-mounted-react", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { RemotePrivateSettings } = await import(`data:text/javascript;base64,${Buffer.from(compiledSettings.outputFiles[0].text).toString("base64")}`);

async function rejectedSaveJourney(response, knownNotSent) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const receipt = fixture(), posts = []; let root;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: async (_url, settings = {}) => {
      if (settings.method !== "POST") return { ok: true, json: async () => metadata(receipt) };
      const packet = JSON.parse(settings.body); posts.push(packet);
      if (posts.length === 1) return { ok: false, status: 409, json: async () => response(packet) };
      const next = structuredClone(receipt);
      next.configuration_saved = true;
      next.product_control.action_request = { action_request_id: packet.action_request_id, guide_id: state.id,
        deployment_id: receipt.deployment_id, action: "configure", phase: "settled", outcome: "returned" };
      return { ok: true, json: async () => next };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const handler = (element, event) => element[Object.keys(element).find((key) => key.startsWith("__reactProps$"))][event];
  const button = (text) => [...document.querySelectorAll("button")].find((item) => item.textContent === text);
  try {
    for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
    const { createRoot } = await import("react-dom/client"); root = createRoot(document.getElementById("root"));
    await act(async () => root.render(React.createElement(RemotePrivateSettings,
      { receipt: structuredClone(receipt), state, options, build, openRequest: {}, onBusy: () => true,
        renderConfiguration: ({ values, onChange }) => React.createElement("input", { "aria-label": "Synthetic provider key", value: values.API_KEY || "",
          onChange: (event) => onChange("API_KEY", event.target.value) }) })));
    await act(async () => handler(document.querySelector("input"), "onChange")({ target: { value: "inert-unsent-value" } }));
    await act(async () => handler(document.querySelector("form"), "onSubmit")({ preventDefault() {} }));
    assert.equal(posts.length, 1);
    assert.equal(document.querySelector("input").value, "inert-unsent-value");
    if (knownNotSent) {
      assert.match(document.body.textContent, /This save was not sent/);
      assert.equal(document.body.textContent.includes("Forge could not confirm this save"), false);
    } else assert.match(document.body.textContent, /Forge could not confirm this save/);
    await act(async () => button("Reload private settings").click());
    assert.equal(posts.length, 1, "Reload never repeats the mutation");
    assert.equal(document.querySelector("input").value, "inert-unsent-value");
    const save = button("Save private settings"); assert.ok(save);
    assert.equal(save.disabled, !knownNotSent);
    if (knownNotSent) {
      await act(async () => save.click());
      assert.equal(posts.length, 2);
      assert.notEqual(posts[0].action_request_id, posts[1].action_request_id);
      assert.match(document.body.textContent, /Private settings saved/);
      assert.equal(document.querySelector("form") === null, true);
      assert.equal(document.body.textContent.includes("inert-unsent-value"), false);
    }
  } finally {
    if (root) await act(async () => root.unmount()); dom.window.close();
    for (const [key, value] of previous) { if (value) Object.defineProperty(globalThis, key, value); else delete globalThis[key]; }
  }
}

test("an exact server rejection keeps unsent values and permits a checked manual save without automatic redelivery", async () => {
  await rejectedSaveJourney((packet) => ({ detail: "Inert known pre-delivery rejection", code: "remote_product_action_not_sent",
    action: "configure", action_request_id: packet.action_request_id }), true);
});

test("generic conflicts and foreign action rejection identities cannot unblock an uncertain save", async () => {
  for (const response of [
    () => ({ detail: "Inert generic conflict" }),
    (packet) => ({ code: "remote_product_action_not_sent", action: "configure", action_request_id:
      packet.action_request_id.slice(0, -1) + (packet.action_request_id.endsWith("1") ? "2" : "1") }),
    (packet) => ({ code: "remote_product_action_not_sent", action: "finish-setup", action_request_id: packet.action_request_id }),
  ]) await rejectedSaveJourney(response, false);
});

test("a lost product save reply waits for the same settled UUID without another POST", async () => {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const receipt = fixture(), posts = []; let root;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator, HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: async (_url, settings = {}) => {
      if (settings.method === "POST") { posts.push(JSON.parse(settings.body)); throw new Error("inert lost response"); }
      return { ok: true, json: async () => metadata(receipt) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const openRequest = {};
  const handler = (element, event) => element[Object.keys(element).find((key) => key.startsWith("__reactProps$"))][event];
  try {
    for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
    const { createRoot } = await import("react-dom/client"); root = createRoot(document.getElementById("root"));
    const render = () => act(async () => root.render(React.createElement(RemotePrivateSettings,
      { receipt: structuredClone(receipt), state, options, build, openRequest, onBusy: () => true,
        renderConfiguration: ({ values, onChange }) => React.createElement("input", { "aria-label": "Synthetic provider key", value: values.API_KEY || "",
          onChange: (event) => onChange("API_KEY", event.target.value) }) })));
    await render();
    const input = document.querySelector('input[aria-label="Synthetic provider key"]'); assert.ok(input);
    await act(async () => handler(input, "onChange")({ target: { value: "inert-synthetic-secret" } }));
    await act(async () => handler(document.querySelector("form"), "onSubmit")({ preventDefault() {} }));
    assert.equal(posts.length, 1); assert.match(posts[0].action_request_id, /^[a-f0-9-]{36}$/);
    assert.match(document.body.textContent, /without sending the save again/);
    await render(); assert.equal(posts.length, 1);
    receipt.product_control.action_request = { action_request_id: posts[0].action_request_id, guide_id: state.id,
      deployment_id: receipt.deployment_id, action: "configure", phase: "settled", outcome: "returned" };
    await render();
    assert.match(document.body.textContent, /original private settings save is confirmed/);
    assert.equal(document.querySelector("form") === null, true); assert.equal(posts.length, 1);
    assert.equal(document.body.textContent.includes("inert-synthetic-secret"), false);
  } finally {
    if (root) await act(async () => root.unmount()); dom.window.close();
    for (const [key, value] of previous) { if (value) Object.defineProperty(globalThis, key, value); else delete globalThis[key]; }
  }
});
