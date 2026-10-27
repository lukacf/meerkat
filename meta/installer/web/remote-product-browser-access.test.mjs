import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { remoteBrowserQualified, remoteBrowserBinding, remoteBrowserOwnedBinding,
  remoteBrowserReplyOwned, remoteBrowserReplyCurrent, remoteBrowserConsole } from "./remote-browser-access.mjs";
import { remoteApplicationExperiences, remoteApplicationReplyOwned, remoteApplicationReplyCurrent,
  remoteApplicationLink, remoteApplicationBinding } from "./remote-application-access.mjs";

// These public specimens follow the existing actor, native evidence and access
// binding contracts. Mounted production components are real; HTTP, native and
// listener facts are inert. This establishes no native, SSH or useful-task result.
const guide = { id: "11111111-1111-4111-8111-111111111111", version: 4, proposal: { target: "linux" } };
const build = { guide_id: guide.id, guide_version: 4, build_id: "synthetic-accepted-build", source_digest: "a".repeat(64),
  status: "completed", current: true, ready_to_package: true, acceptance_current: true,
  acceptance_trusted: true, acceptance: { status: "pass" } };
const options = { build_id: build.build_id, guide_version: 4, current: true, supported: false, target: "linux", host_target: "mac" };
const id = (n) => `${String(n).repeat(8)}-${String(n).repeat(4)}-4${String(n).repeat(3)}-8${String(n).repeat(3)}-${String(n).repeat(12)}`;
const now = Date.parse("2026-10-12T06:00:00Z"), consoleUrl = "http://127.0.0.1:8791/console";
const runtimeKeys = ["runtime_pid", "startup_attempt_id", "interactive_lifecycle_id", "execution_mode", "service_instance_id", "service_supervisor_pid"];
function fixture(mode = "interactive") {
  const receipt = { protocol: 1, guide_id: guide.id, guide_version: 4, build_id: build.build_id,
    deployment_id: id(2), accepted_source_digest: build.source_digest, package_sha256: "b".repeat(64),
    transferred_source_digest: "c".repeat(64), controller_sha256: "d".repeat(64), destination: "/home/fixture/app",
    endpoint: { host: "synthetic-linux.local", user: "fixture", ssh_port: 22 },
    host: { uid: 1001, home: "/home/fixture", destination: "/home/fixture/app" }, host_key_fingerprint: "SHA256:" + "A".repeat(43),
    status: "running", ready: true, source_current: true, source_binding_current: true, current_guide_version: 4,
    remote_admission_requested: true, admitted: true, observation: { available: true }, worker_live: true,
    cleanup_confirmed: false, service: {}, service_owned: false, service_observed: false,
    product_runtime: true, product_scope: "retained-original-product-owner", install_operation_id: id(3),
    product_control_manifest_sha256: "e".repeat(64) };
  const binding = { protocol: 1, install_operation_id: receipt.install_operation_id,
    control_manifest_sha256: receipt.product_control_manifest_sha256, initial_install_sha256: "f".repeat(64),
    binding: Object.fromEntries(["guide_id", "guide_version", "build_id", "accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"].map((key) => [key, receipt[key]])),
    worker: { pid: 100, uid: 1001, start_ticks: "100", boot_id: "synthetic-boot" },
    command: { pid: 101, uid: 1001, start_ticks: "101", boot_id: "synthetic-boot" } };
  receipt.product_control = { owner_live: true, initial_install_complete: true, binding,
    factory_status: { guide_id: guide.id, guide_version: 4, build_id: build.build_id, deployment_id: receipt.deployment_id,
      package_sha256: receipt.package_sha256, destination: receipt.destination, source_verified: true,
      source_digest: build.source_digest, target: "local", status: "running", phase: "ready", process_live: true, runtime_pid: 61234,
      execution_mode: mode, startup_attempt_id: id(4), interactive_lifecycle_id: mode === "interactive" ? id(5) : null,
      service_instance_id: mode === "background" ? id(6) : null, service_supervisor_pid: mode === "background" ? 61233 : null,
      console_url: consoleUrl, health: "ready", health_checked_at: new Date(now).toISOString(),
      bootstrap: { complete: true, ready: true, counts: { broken: 0 } } },
    native_evidence: { owned_source_verified: true, native_runtime_available: true,
      deployment_id: receipt.deployment_id, build_id: build.build_id, source_digest: build.source_digest } };
  receipt.application_experiences = { protocol: 1, supported: true, status: "resolved",
    source_digest: build.source_digest, installed_source_digest: receipt.transferred_source_digest, metadata_sha256: "9".repeat(64),
    entries: ["workspace", "reports"].map((name) => ({ id: name, label: name === "workspace" ? "Workspace" : "Reports",
      path: "/" + name, destination_port: 8792, listener_owned: true, listener_token: "8".repeat(64), reason: null, verified: false })) };
  return receipt;
}
function reply(receipt, entry = null, status = "closed") {
  const actual = receipt.product_control.factory_status;
  const value = { protocol: 1, ...Object.fromEntries(["guide_id", "guide_version", "build_id", "deployment_id", "accepted_source_digest", "package_sha256", "transferred_source_digest", "controller_sha256"].map((key) => [key, receipt[key]])),
    access_id: status === "ready" ? id(entry?.id === "reports" ? 8 : 7) : null, entry_id: entry ? "application:" + entry.id : "native-console",
    status, native_ready: status === "ready", cleanup_confirmed: status !== "ready", console_url: actual.console_url,
    local_url: status !== "ready" ? null : entry ? `http://127.0.0.1:${entry.destination_port}${entry.path}` : actual.console_url,
    service_name: null, service_source_digest: null, observed_at: new Date(now).toISOString(),
    product_control_binding: structuredClone(receipt.product_control.binding),
    product_runtime_binding: Object.fromEntries(runtimeKeys.map((key) => [key, actual[key] ?? null])),
    scope: { authentication: "local-password", health_scope: entry ? "declared-application-transport" : "native-bootstrap-and-console",
      requested_interfaces: "Native and application UI", custom_interfaces_verified: false } };
  if (entry) Object.assign(value, { metadata_sha256: receipt.application_experiences.metadata_sha256,
    entry_path: entry.path, destination_port: entry.destination_port, listener_token: entry.listener_token,
    transport_reachable: status === "ready", http_status: status === "ready" ? 200 : null,
    application_verified: false, authentication_verified: false, workflow_verified: false });
  return value;
}
for (const mode of ["interactive", "background"]) test(`idle ${mode} product uses its real instance with a live controller and no legacy service`, () => {
  const receipt = fixture(mode), ready = reply(receipt, null, "ready");
  assert.equal(remoteBrowserQualified(receipt, options, build, guide), true);
  assert.equal(remoteBrowserReplyCurrent(ready, receipt, guide, build), true);
  assert.equal(remoteBrowserConsole(ready, receipt, guide, build, now), consoleUrl);
  assert.equal(receipt.worker_live, true); assert.deepEqual(receipt.service, {});
  const entry = receipt.application_experiences.entries[0], experiences = remoteApplicationExperiences(receipt), app = reply(receipt, entry, "ready");
  assert.equal(remoteApplicationReplyCurrent(app, receipt, guide, build, experiences, entry), true);
  assert.equal(remoteApplicationLink(app, receipt, guide, build, experiences, entry, now), app.local_url);
});
test("partial, retired, pending, mutating and broken-native observations never qualify product opening", () => {
  const original = fixture();
  const mutations = [
    r => { delete r.product_runtime; }, r => { r.product_control.owner_live = false; },
    r => { r.product_control.binding.command.start_ticks = "foreign"; r.product_control.binding.binding.controller_sha256 = "0".repeat(64); },
    r => { r.product_control.factory_status.guide_version++; }, r => { r.product_control.factory_status.target = "gcp"; },
    r => { r.product_control.factory_status.status = "starting"; }, r => { r.product_control.factory_status.source_digest = "0".repeat(64); },
    r => { r.pending_operation = { action: "stop" }; },
    r => { r.product_control.action_request = { action_request_id: id(9), action: "stop", phase: "pending", guide_id: guide.id, deployment_id: r.deployment_id }; },
    r => { r.product_control.action_request = { action_request_id: id(9), action: "stop", phase: "unknown", guide_id: guide.id, deployment_id: r.deployment_id }; },
    r => { r.product_control.factory_status.phase = "update"; }, r => { r.product_control.factory_status.update = { status: "verifying" }; },
    r => { r.product_control.factory_status.bootstrap.counts.broken = 1; }, r => { r.product_control.native_evidence.native_runtime_available = false; },
    r => { r.product_control.factory_status.startup_attempt_id = null; }, r => { r.product_control.factory_status.interactive_lifecycle_id = null; },
    r => { r.observation.available = false; }, r => { r.worker_live = false; }, r => { r.service = { ready: true }; },
    r => { r.source_current = false; }, r => { r.source_binding_current = false; }
  ];
  for (const mutate of mutations) {
    const receipt = structuredClone(original); mutate(receipt);
    assert.equal(remoteBrowserQualified(receipt, options, build, guide), false);
    assert.equal(remoteBrowserConsole(reply(original, null, "ready"), receipt, guide, build, now), null);
  }
});
test("saved forward ownership survives observation loss while source and lifetime changes invalidate opening replies", () => {
  const receipt = fixture(), ready = reply(receipt, null, "ready"), lost = structuredClone(receipt);
  lost.observation.available = false; lost.product_control = null; lost.status = "observation-unavailable";
  lost.product_scope = "byte-install-stopped";
  assert.equal(remoteBrowserOwnedBinding(lost, guide), remoteBrowserOwnedBinding(receipt, guide));
  assert.equal(remoteBrowserReplyOwned(ready, lost, guide), true);
  assert.equal(remoteBrowserReplyCurrent(ready, lost, guide, build), false);
  for (const mutate of [r => { r.product_control.factory_status.startup_attempt_id = id(9); },
      r => { r.product_control.factory_status.interactive_lifecycle_id = id(9); },
      r => { r.product_control.factory_status.runtime_pid++; }, r => { r.product_control.factory_status.console_url = "http://127.0.0.1:8793/console"; },
      r => { r.product_control.binding.worker.start_ticks = "new-birth"; }]) {
    const changed = structuredClone(receipt); mutate(changed);
    assert.notEqual(remoteBrowserBinding(changed, guide, build), remoteBrowserBinding(receipt, guide, build));
    assert.equal(remoteBrowserReplyOwned(ready, changed, guide), true);
    assert.equal(remoteBrowserReplyCurrent(ready, changed, guide, build), false);
  }
  for (const mutate of [r => { r.controller_sha256 = "0".repeat(64); }, r => { r.package_sha256 = "0".repeat(64); },
      r => { r.endpoint.host = "foreign.local"; }, r => { r.host.uid++; }]) {
    const changed = structuredClone(receipt); mutate(changed);
    assert.notEqual(remoteBrowserOwnedBinding(changed, guide), remoteBrowserOwnedBinding(receipt, guide));
  }
});
test("custom ownership preserves only the original cleanup and current listener and instance join governs its link", () => {
  const receipt = fixture(), experiences = remoteApplicationExperiences(receipt), entry = experiences.entries[0], ready = reply(receipt, entry, "ready");
  const changed = { ...entry, destination_port: 8793, listener_token: "0".repeat(64) };
  assert.equal(remoteApplicationReplyOwned(ready, receipt, guide, build, experiences, changed), true);
  assert.equal(remoteApplicationReplyCurrent(ready, receipt, guide, build, experiences, changed), false);
  assert.notEqual(remoteApplicationBinding(receipt, guide, build, experiences, changed), remoteApplicationBinding(receipt, guide, build, experiences, entry));
  for (const patch of [{ product_control_binding: null }, { product_runtime_binding: {} }, { service_name: "pretend.service" },
      { service_source_digest: receipt.transferred_source_digest }, { local_url: "http://user:password@127.0.0.1:8792/workspace" },
      { application_verified: true }, { authentication_verified: true }, { workflow_verified: true }])
    assert.equal(remoteApplicationReplyOwned({ ...ready, ...patch }, receipt, guide, build, experiences, entry), false);
});

test("ordinary service qualification and reply contracts retain their original service and worker rules", () => {
  const receipt = fixture();
  for (const key of ["product_runtime", "product_scope", "product_control", "install_operation_id", "product_control_manifest_sha256"]) delete receipt[key];
  receipt.worker_live = false; receipt.service_owned = receipt.service_observed = true;
  receipt.service = { service: "mobkit-launch." + id(3) + ".service", source_digest: receipt.transferred_source_digest,
    status: "running", ready: true, source_current: true, definition_verified: true,
    scope: "native-bootstrap-and-console", console_url: consoleUrl };
  const original = fixture(), ready = reply(original, null, "ready");
  delete ready.product_control_binding; delete ready.product_runtime_binding;
  ready.service_name = receipt.service.service; ready.service_source_digest = receipt.service.source_digest;
  assert.equal(remoteBrowserQualified(receipt, options, build, guide), true);
  assert.equal(remoteBrowserReplyCurrent(ready, receipt, guide, build), true);
  assert.equal(remoteBrowserConsole(ready, receipt, guide, build, now), consoleUrl);
  assert.equal(remoteBrowserQualified({ ...receipt, worker_live: true }, options, build, guide), false);
  assert.equal(remoteBrowserQualified({ ...receipt, service_owned: false }, options, build, guide), false);
  assert.equal(remoteBrowserReplyCurrent({ ...ready, service_source_digest: "0".repeat(64) }, receipt, guide, build), false);
  const entry = receipt.application_experiences.entries[0], app = reply(original, entry, "ready");
  delete app.product_control_binding; delete app.product_runtime_binding;
  app.service_name = receipt.service.service; app.service_source_digest = receipt.service.source_digest;
  assert.equal(remoteApplicationReplyCurrent(app, receipt, guide, build, remoteApplicationExperiences(receipt), entry), true);
});

let componentPromise;
async function components() {
  if (!componentPromise) componentPromise = (async () => {
    const require = createRequire(import.meta.url);
    const built = await bundle({ stdin: { contents: 'export { RemoteBrowserAccess } from "./remote-browser-access.jsx"; export { RemoteApplicationAccess } from "./remote-application-access.jsx";',
      resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "synthetic-product-access.mjs" },
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", plugins: [{ name: "shared-mounted-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
      } }] });
    return import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  })();
  return componentPromise;
}
async function mounted(kind, initial, verify) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  let props = { state: guide, build, options, receipt: fixture() }, sequence = 0;
  const calls = [], polls = new Map(), clocks = new Map(), responses = new Map();
  for (const [key, value] of Object.entries(initial)) responses.set(key, structuredClone(value));
  let read, open, close;
  const globals = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const timers = { setTimeout, clearTimeout, setInterval, clearInterval }, originalNow = Date.now;
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  globalThis.setTimeout = (callback, delay, ...args) => { if (delay !== 2000) return timers.setTimeout(callback, delay, ...args); const key = ++sequence; polls.set(key, () => callback(...args)); return key; };
  globalThis.clearTimeout = key => { if (!polls.delete(key)) timers.clearTimeout(key); };
  globalThis.setInterval = (callback, delay, ...args) => { if (delay !== 1000) return timers.setInterval(callback, delay, ...args); const key = ++sequence; clocks.set(key, () => callback(...args)); return key; };
  globalThis.clearInterval = key => { if (!clocks.delete(key)) timers.clearInterval(key); };
  const fetchDescriptor = Object.getOwnPropertyDescriptor(globalThis, "fetch");
  globalThis.fetch = async (target, init = {}) => {
    const method = init.method || "GET", body = init.body ? JSON.parse(init.body) : null;
    const entry = method === "GET" ? new URL(target, "http://localhost").searchParams.get("entry_id") || "native-console" : body.entry_id;
    calls.push({ target, method, body });
    let value;
    if (method === "GET") value = read ? await read(entry) : responses.get(entry);
    else if (target.endsWith("/open")) {
      value = open ? await open(body) : assert.fail("No synthetic opening selected");
      responses.set(value.entry_id, structuredClone(value));
    } else if (target.endsWith("/close")) {
      value = close ? await close(body) : assert.fail("No synthetic closing selected");
      responses.set(value.entry_id, structuredClone(value));
    }
    else assert.fail("Unexpected access request");
    return { ok: true, json: async () => structuredClone(value) };
  };
  Date.now = () => now;
  const { createRoot } = await import("react-dom/client"), exported = await components();
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(kind === "console" ? exported.RemoteBrowserAccess : exported.RemoteApplicationAccess, props)));
  const document = dom.window.document, button = label => [...document.querySelectorAll("button")].find(node => node.textContent === label);
  try {
    await render();
    await verify({ document, calls, button, links: () => [...document.querySelectorAll("a[href]")],
      setRead: handler => { read = handler; }, setOpen: handler => { open = handler; }, setClose: handler => { close = handler; },
      async rerender(patch) { props = { ...props, ...patch }; await render(); },
      async poll() { const pending = [...polls.values()]; polls.clear(); await act(async () => { for (const callback of pending) callback(); }); }
    });
  } finally {
    await act(async () => root.unmount()); dom.window.close(); Date.now = originalNow;
    Object.assign(globalThis, timers);
    if (fetchDescriptor) Object.defineProperty(globalThis, "fetch", fetchDescriptor); else delete globalThis.fetch;
    for (const [key, descriptor] of previous) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; }
    assert.equal(polls.size, 0); assert.equal(clocks.size, 0);
  }
}
for (const kind of ["console", "custom"]) test(`mounted ${kind} opens only on deliberate connect using original product bindings`, async () => {
  const receipt = fixture(), entry = kind === "custom" ? receipt.application_experiences.entries[0] : null;
  const initial = kind === "console" ? { "native-console": reply(receipt) } : Object.fromEntries(receipt.application_experiences.entries.map(e => ["application:" + e.id, reply(receipt, e)]));
  await mounted(kind, initial, async ({ document, calls, button, setOpen, links }) => {
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    assert.equal(links().length, 0);
    setOpen(() => reply(receipt, entry, "ready"));
    await act(async () => button(kind === "console" ? "Connect browser to native Console" : "Connect to Workspace").click());
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.equal(links()[0].href, reply(receipt, entry, "ready").local_url);
    assert.equal(links()[0].target, "_blank"); assert.equal(links()[0].rel, "noopener noreferrer");
    assert.match(document.body.textContent, /sign in|Sign in/);
    assert.equal(receipt.worker_live, true); assert.deepEqual(receipt.service, {});
  });
});
for (const kind of ["console", "custom"]) test(`mounted ${kind} retains original Close through observation and metadata withdrawal without a link or reconnect`, async () => {
  const receipt = fixture(), entry = kind === "custom" ? receipt.application_experiences.entries[0] : null, ready = reply(receipt, entry, "ready");
  const initial = kind === "console" ? { "native-console": ready } : { "application:workspace": ready, "application:reports": reply(receipt, receipt.application_experiences.entries[1]) };
  await mounted(kind, initial, async ({ calls, button, links, rerender, setRead, setClose }) => {
    assert.ok(links().length);
    const lost = structuredClone(receipt); lost.status = "observation-unavailable"; lost.observation.available = false;
    lost.product_control = null; lost.product_scope = "byte-install-stopped";
    lost.application_experiences = { supported: true, status: "unavailable" };
    setRead(() => { throw Error("synthetic unavailable observation"); });
    await rerender({ receipt: lost, observationUnavailable: true });
    assert.equal(links().length, 0); assert.ok(button(kind === "console" ? "Close browser connection" : "Close connection"));
    assert.equal(button(kind === "console" ? "Connect browser to native Console" : "Connect to Workspace") === undefined, true);
    setClose(body => { assert.equal(body.access_id, ready.access_id); return { ...ready, status: "closed", local_url: null, native_ready: false, cleanup_confirmed: true }; });
    await act(async () => button(kind === "console" ? "Close browser connection" : "Close connection").click());
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.equal(links().length, 0);
  });
});
for (const kind of ["console", "custom"]) test(`mounted ${kind} same-port restart and late opening reply cannot renew the old instance`, async () => {
  const receipt = fixture(), entry = kind === "custom" ? receipt.application_experiences.entries[0] : null;
  const initial = kind === "console" ? { "native-console": reply(receipt) } : Object.fromEntries(receipt.application_experiences.entries.map(e => ["application:" + e.id, reply(receipt, e)]));
  await mounted(kind, initial, async ({ calls, button, links, rerender, setOpen }) => {
    let deliver; const result = new Promise(resolve => { deliver = resolve; }); setOpen(() => result);
    let clicked; act(() => { clicked = button(kind === "console" ? "Connect browser to native Console" : "Connect to Workspace").click(); });
    const restarted = structuredClone(receipt); restarted.product_control.factory_status.startup_attempt_id = id(9);
    restarted.product_control.factory_status.interactive_lifecycle_id = id(9);
    await rerender({ receipt: restarted });
    await act(async () => { deliver(reply(receipt, entry, "ready")); await clicked; });
    assert.equal(links().length, 0); assert.equal(calls.filter(call => call.method === "POST").length, 1);
  });
});
test("mounted custom shared-port cleanup stays visible when native observation is withdrawn", async () => {
  const receipt = fixture(), entries = receipt.application_experiences.entries;
  const initial = Object.fromEntries(entries.map(e => ["application:" + e.id, reply(receipt, e, "ready")]));
  await mounted("custom", initial, async ({ document, calls, button, links, rerender, setClose }) => {
    assert.equal(links().length, 2);
    const mutating = structuredClone(receipt); mutating.product_control.factory_status.phase = "update";
    await rerender({ receipt: mutating }); assert.equal(links().length, 0);
    assert.equal(document.querySelectorAll('button').length > 0, true);
    setClose(body => { const index = entries.findIndex(e => reply(receipt, e, "ready").access_id === body.access_id);
      return { ...reply(receipt, entries[index], "ready"), status: "cleanup-needed", local_url: null, native_ready: false, cleanup_confirmed: false }; });
    await act(async () => button("Close connection").click());
    assert.equal(links().length, 0); assert.ok(button("Close connection"));
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  });
});

test("mounted custom port change withdraws the old link but closes its captured access before offering the new port", async () => {
  const receipt = fixture(), entry = receipt.application_experiences.entries[0], original = reply(receipt, entry, "ready");
  await mounted("custom", { "application:workspace": original,
    "application:reports": reply(receipt, receipt.application_experiences.entries[1]) }, async ({ button, links, rerender, setClose, setRead, poll, calls }) => {
    const changed = structuredClone(receipt), changedEntry = changed.application_experiences.entries[0];
    changedEntry.destination_port = 8793; changedEntry.listener_token = "0".repeat(64);
    await rerender({ receipt: changed });
    assert.equal(links().length, 0); assert.ok(button("Close connection"));
    assert.equal(button("Connect to Workspace") === undefined, true);
    setClose(body => { assert.equal(body.access_id, original.access_id);
      return { ...original, status: "closed", local_url: null, native_ready: false, cleanup_confirmed: true }; });
    await act(async () => button("Close connection").click());
    setRead(key => reply(changed, changed.application_experiences.entries.find(e => key === "application:" + e.id)));
    await poll();
    assert.equal(links().length, 0); assert.ok(button("Connect to Workspace"));
    assert.equal(calls.filter(call => call.method === "POST").length, 1, "No automatic connection to the successor port");
  });
});

test("mounted RemoteDeployment retains the exact custom Close through producer observation loss, stopped recovery and pending cloud removal", async () => {
  const require = createRequire(import.meta.url);
  const built = await bundle({ stdin: { contents: 'export { RemoteDeployment } from "./remote-deployment.jsx";',
    resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "synthetic-product-parent.mjs" },
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    } }] });
  const { RemoteDeployment } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const original = fixture(), cloudOperation = id(9);
  original.target = "gcp"; original.cloud_operation_id = cloudOperation;
  original.endpoint = { transport: "gcp-iap", operation_id: cloudOperation, account: "fixture@example.test",
    project: "fixture-project", zone: "europe-west1-b", instance: "fixture-app", instance_id: "1234",
    ownership_token: "e".repeat(32), host: "compute.1234", user: "mobkit", ssh_port: 22 };
  const entry = original.application_experiences.entries[0], ready = reply(original, entry, "ready");
  let workspaceAccess = structuredClone(ready);
  const base = `/api/guide/${guide.id}/remote-deployment`;
  let observed = structuredClone(original), cloudRemoval = null, sequence = 0;
  const calls = [], polls = new Map(), clocks = new Map();
  const timers = { setTimeout, clearTimeout, setInterval, clearInterval }, originalNow = Date.now;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: async (target, init = {}) => {
      const url = new URL(target, "http://localhost"), method = init.method || "GET", body = init.body ? JSON.parse(init.body) : null;
      calls.push({ target, method, body });
      let value, ok = true;
      if (method === "GET" && url.pathname === base) value = observed;
      else if (method === "GET" && url.pathname === base + "/browser-access") {
        const selected = url.searchParams.get("entry_id") || "native-console";
        value = selected === "application:workspace" ? workspaceAccess : selected === "native-console" ? reply(original)
          : reply(original, original.application_experiences.entries[1]);
      } else if (method === "GET" && url.pathname === base + "/application-setup") value = { available: false };
      else if (method === "GET" && url.pathname === base + "/use") { value = { detail: "Synthetic metadata is unavailable" }; ok = false; }
      else if (method === "POST" && url.pathname === base + "/browser-access/close") {
        assert.deepEqual(body, { deployment_id: original.deployment_id, access_id: ready.access_id });
        workspaceAccess = { ...ready, status: "closed", cleanup_confirmed: true, native_ready: false, local_url: null };
        value = workspaceAccess;
      } else assert.fail(`Unexpected mounted parent request: ${method} ${target}`);
      return { ok, status: ok ? 200 : 503, json: async () => structuredClone(value) };
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  globalThis.setTimeout = (callback, delay, ...args) => { if (delay !== 2000) return timers.setTimeout(callback, delay, ...args); const key = ++sequence; polls.set(key, () => callback(...args)); return key; };
  globalThis.clearTimeout = key => { if (!polls.delete(key)) timers.clearTimeout(key); };
  globalThis.setInterval = (callback, delay, ...args) => { if (delay !== 1000) return timers.setInterval(callback, delay, ...args); const key = ++sequence; clocks.set(key, () => callback(...args)); return key; };
  globalThis.clearInterval = key => { if (!clocks.delete(key)) timers.clearInterval(key); };
  Date.now = () => now;
  const { createRoot } = await import("react-dom/client"), root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(RemoteDeployment, {
    state: { ...guide, proposal: { target: "gcp" } }, build, options: { ...options, target: "gcp" },
    plan: { project: { slug: "synthetic-app" }, target: "gcp" }, cloudRemoval })));
  const button = label => [...dom.window.document.querySelectorAll("button")].find(node => node.textContent === label);
  const panel = () => dom.window.document.querySelector('[aria-label="Custom application browser access"]');
  try {
    await render();
    const originalPanel = panel(); assert.ok(originalPanel);
    assert.equal(originalPanel.querySelector('a[href]')?.href, ready.local_url);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    // Exact unavailable()/public() shape: retain original installed bytes and
    // saved control data, withdraw observation and its freshness-derived scope.
    observed.status = "observation-unavailable"; observed.last_known_status = "running";
    observed.observation = { available: false };
    observed.product_scope = "byte-install-stopped";
    for (const key of ["can_stop", "can_restart", "can_configure", "can_prepare_setup", "can_finish_setup", "can_start_background", "private_settings_supported"]) observed[key] = false;
    await act(async () => button("Recheck remote status").click());
    assert.equal(panel() === originalPanel, true); assert.equal(panel().querySelector('a[href]') === null, true);
    assert.ok(button("Close connection")); assert.equal(button("Connect to Workspace") === undefined, true);
    assert.equal(dom.window.document.body.textContent.includes("Your application is running in interactive mode."), false);
    for (const status of ["stopped", "recovery-needed"]) {
      observed.status = status; observed.product_control = null; observed.observation = { available: true };
      await act(async () => button("Recheck remote status").click());
      assert.equal(panel() === originalPanel, true); assert.ok(button("Close connection"));
      assert.equal(panel().querySelector('a[href]') === null, true);
      assert.equal(dom.window.document.body.textContent.includes("Your application is running in interactive mode."), false);
    }
    // Pending origin removal withdraws opening before the receipt's cloud
    // status changes. It must not dispose of the admitted local forward.
    cloudRemoval = { guideId: guide.id, operationId: cloudOperation, pending: true, status: "removing" };
    await render();
    assert.equal(panel() === originalPanel, true); assert.ok(button("Close connection"));
    assert.equal(button("Connect to Workspace") === undefined, true);
    await act(async () => button("Close connection").click());
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.equal(panel().querySelector('a[href]') === null, true);
    assert.equal(button("Close connection") === undefined, true);
  } finally {
    await act(async () => root.unmount()); dom.window.close(); Date.now = originalNow;
    Object.assign(globalThis, timers);
    for (const [key, descriptor] of previous) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; }
    assert.equal(polls.size, 0); assert.equal(clocks.size, 0);
  }
});
