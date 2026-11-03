import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { remoteBrowserUrl, remoteBrowserQualified, remoteBrowserBinding, remoteBrowserReplyCurrent,
  remoteBrowserOwned, remoteBrowserOwnedBinding, remoteBrowserReplyOwned,
  remoteBrowserConsole, remoteBrowserCanOpen, remoteBrowserCanClose, remoteBrowserObservedTime,
  REMOTE_BROWSER_FRESH_MS } from "./remote-browser-access.mjs";

const guide = { id: "synthetic-browser-guide", version: 4, proposal: { target: "linux" } };
const build = { guide_id: guide.id, guide_version: guide.version, build_id: "synthetic-accepted-build",
  source_digest: "a".repeat(64), status: "completed", current: true, ready_to_package: true,
  acceptance_current: true, acceptance_trusted: true, acceptance: { status: "pass" } };
const options = { build_id: build.build_id, guide_version: guide.version, current: true,
  supported: false, target: "linux", host_target: "mac" };
const accessId = "11111111-1111-4111-8111-111111111111";
const otherId = "22222222-2222-4222-8222-222222222222";
const consoleUrl = "http://127.0.0.1:8791/console";
const initialTime = Date.parse("2026-10-02T05:00:00Z");
const receipt = { protocol: 1, deployment_id: "33333333-3333-4333-8333-333333333333", guide_id: guide.id,
  guide_version: guide.version, build_id: build.build_id, accepted_source_digest: build.source_digest,
  package_sha256: "b".repeat(64), transferred_source_digest: "c".repeat(64), controller_sha256: "d".repeat(64),
  source_binding_current: true, source_current: true, status: "running", ready: true,
  remote_admission_requested: true, admitted: true, worker_live: false,
  service_owned: true, service_observed: true, observation: { available: true },
  endpoint: { host: "synthetic-linux.local", user: "fixture-user", ssh_port: 22 },
  destination: "/synthetic-owned/src/workshop",
  service: { service: `mobkit-launch.${otherId}.service`, status: "running", ready: true,
    scope: "native-bootstrap-and-console", source_digest: "c".repeat(64), source_current: true,
    definition_verified: true, console_url: consoleUrl } };
const closed = { protocol: 1, guide_id: guide.id, guide_version: guide.version, build_id: build.build_id,
  deployment_id: receipt.deployment_id, access_id: null, entry_id: "native-console", status: "closed",
  local_url: null, console_url: consoleUrl, native_ready: false, cleanup_confirmed: true,
  observed_at: new Date(initialTime).toISOString(), accepted_source_digest: receipt.accepted_source_digest,
  package_sha256: receipt.package_sha256, transferred_source_digest: receipt.transferred_source_digest,
  controller_sha256: receipt.controller_sha256, service_name: receipt.service.service,
  service_source_digest: receipt.service.source_digest,
  scope: { authentication: "local-password", health_scope: "native-bootstrap-and-console",
    requested_interfaces: "Native admin UI", custom_interfaces_verified: false } };
const ready = { ...closed, access_id: accessId, status: "ready", local_url: consoleUrl,
  native_ready: true, cleanup_confirmed: false };

test("browser qualification joins current acceptance with admitted native service ownership", () => {
  assert.equal(remoteBrowserQualified(receipt, options, build, guide), true);
  for (const changed of [{ admitted: false }, { remote_admission_requested: false }, { worker_live: true },
    { pending_operation: { action: "stop" } }, { service_owned: false }, { service_observed: false },
    { source_current: false }, { ready: false }, { source_binding_current: false },
    { observation: { available: false } }, { controller_sha256: undefined }, { transferred_source_digest: "bad" }, { guide_version: 3 }])
    assert.equal(remoteBrowserQualified({ ...receipt, ...changed }, options, build, guide), false);
  for (const changed of [{ ready: false }, { status: "stopped" }, { scope: "custom-ui" },
    { definition_verified: false }, { source_current: false }, { source_digest: "e".repeat(64) },
    { service: 1 }, { service: {} }, { service: true }, { service: [] },
    { service: "mobkit-launch.------------------------------------.service" }, { console_url: "http://localhost:8791/console" }])
    assert.equal(remoteBrowserQualified({ ...receipt, service: { ...receipt.service, ...changed } }, options, build, guide), false);
  assert.equal(remoteBrowserQualified(receipt, options, build, guide, true), false);
  assert.equal(remoteBrowserQualified(receipt, options, build, guide, false, true), false);
  assert.equal(remoteBrowserQualified(receipt, options, { ...build, acceptance_trusted: false }, guide), false);
  assert.notEqual(remoteBrowserBinding(receipt, guide, build), remoteBrowserBinding({ ...receipt,
    service: { ...receipt.service, service: `mobkit-launch.${accessId}.service` } }, guide, build));
  assert.notEqual(remoteBrowserBinding(receipt, guide, build), remoteBrowserBinding({ ...receipt,
    endpoint: { ...receipt.endpoint, host: "another.local" } }, guide, build));
});

test("ready replies permit only the exact credential-free observed loopback Console", () => {
  assert.equal(remoteBrowserReplyCurrent(closed, receipt, guide, build), true);
  assert.equal(remoteBrowserReplyCurrent(ready, receipt, guide, build), true);
  for (const changed of [{ guide_id: "another" }, { guide_version: 5 }, { build_id: "another" },
    { deployment_id: otherId }, { entry_id: "custom-ui" }, { accepted_source_digest: "e".repeat(64) },
    { package_sha256: "e".repeat(64) }, { transferred_source_digest: "e".repeat(64) },
    { controller_sha256: "e".repeat(64) }, { service_name: `mobkit-launch.${accessId}.service` },
    { service_source_digest: "e".repeat(64) }, { access_id: null }, { access_id: "bad" },
    { native_ready: false }, { cleanup_confirmed: true }, { observed_at: "2026-10-02 05:00:00" },
    { scope: { ...closed.scope, authentication: "unknown" } },
    { scope: { ...closed.scope, custom_interfaces_verified: true } },
    { scope: { ...closed.scope, requested_interfaces: null } }])
    assert.equal(remoteBrowserReplyCurrent({ ...ready, ...changed }, receipt, guide, build), false);
  for (const local_url of ["http://127.0.0.1:8792/console", "http://localhost:8791/console",
    "https://127.0.0.1:8791/console", "http://127.0.0.1:8791/console/", `${consoleUrl}?token=synthetic-url-canary`,
    `${consoleUrl}#private`, "http://user:password@127.0.0.1:8791/console", "http://127.0.0.1:80/console"])
    assert.equal(remoteBrowserReplyCurrent({ ...ready, local_url, console_url: local_url }, receipt, guide, build), false);
  assert.equal(remoteBrowserReplyCurrent({ ...closed, local_url: consoleUrl }, receipt, guide, build), false);
  assert.equal(remoteBrowserUrl("guide /1", "deployment /2"),
    "/api/guide/guide%20%2F1/remote-deployment/browser-access?deployment_id=deployment%20%2F2");
});

test("owned tunnel observation binds installed bytes independently of the current draft", () => {
  const changedGuide = { ...guide, version: 5, proposal: { target: "mac" } };
  const staleReceipt = { ...receipt, source_binding_current: false, source_current: false };
  assert.equal(remoteBrowserOwned(staleReceipt, changedGuide), true);
  assert.equal(remoteBrowserOwnedBinding(staleReceipt, changedGuide), remoteBrowserOwnedBinding(receipt, guide));
  assert.equal(remoteBrowserReplyOwned(ready, staleReceipt, changedGuide), true);
  assert.equal(remoteBrowserReplyCurrent(ready, staleReceipt, changedGuide, build), false);
  assert.equal(remoteBrowserOwned(receipt, { ...guide, id: "another-guide" }), false);
  for (const changed of [{ deployment_id: otherId }, { package_sha256: "e".repeat(64) },
    { endpoint: { ...receipt.endpoint, host: "another.local" } }, { controller_sha256: "e".repeat(64) },
    { service: { ...receipt.service, service: `mobkit-launch.${accessId}.service` } }])
    assert.notEqual(remoteBrowserOwnedBinding({ ...receipt, ...changed }, guide), remoteBrowserOwnedBinding(receipt, guide));
  for (const changed of [{ controller_sha256: "bad" }, { transferred_source_digest: "bad" },
    { service: { ...receipt.service, definition_verified: false } },
    { service: { ...receipt.service, source_digest: "e".repeat(64) } }])
    assert.equal(remoteBrowserOwnedBinding({ ...receipt, ...changed }, guide), "");
  for (const changed of [{ guide_version: 5 }, { build_id: "another" }, { deployment_id: otherId },
    { accepted_source_digest: "e".repeat(64) }, { package_sha256: "e".repeat(64) },
    { transferred_source_digest: "e".repeat(64) }, { controller_sha256: "e".repeat(64) },
    { service_name: `mobkit-launch.${accessId}.service` }, { service_source_digest: "e".repeat(64) }])
    assert.equal(remoteBrowserReplyOwned({ ...ready, ...changed }, staleReceipt, changedGuide), false);
});

test("readiness expires after fifteen seconds and uncertain cleanup never permits another connection", () => {
  assert.equal(remoteBrowserConsole(ready, receipt, guide, build, initialTime + REMOTE_BROWSER_FRESH_MS), consoleUrl);
  assert.equal(remoteBrowserConsole(ready, receipt, guide, build, initialTime + REMOTE_BROWSER_FRESH_MS + 1), null);
  assert.equal(remoteBrowserConsole({ ...ready, observed_at: initialTime + 3001 }, receipt, guide, build, initialTime), null);
  assert.equal(remoteBrowserObservedTime(initialTime / 1000), initialTime);
  assert.equal(remoteBrowserObservedTime("2026-10-02T05:00:00.000000+00:00"), initialTime);
  assert.equal(remoteBrowserObservedTime(NaN), null);
  assert.equal(remoteBrowserCanOpen(closed), true);
  for (const status of ["blocked", "unavailable"]) {
    assert.equal(remoteBrowserCanOpen({ ...closed, status, access_id: accessId }), true);
    assert.equal(remoteBrowserCanOpen({ ...closed, status, cleanup_confirmed: false }), false);
  }
  assert.equal(remoteBrowserCanOpen(null), false);
  assert.equal(remoteBrowserCanOpen({ ...closed, status: "cleanup-needed" }), false);
  assert.equal(remoteBrowserCanClose(ready), true);
  assert.equal(remoteBrowserCanClose({ ...ready, status: "cleanup-needed", local_url: null }), true);
  assert.equal(remoteBrowserCanClose({ ...closed, access_id: accessId }), false);
});

test("an old ready Console reply cannot reopen a removed cloud installation", () => {
  const cloudReceipt = { ...receipt, target: "gcp", cloud_operation_id: otherId,
    endpoint: { transport: "gcp-iap", operation_id: otherId, account: "fixture@example.test", project: "fixture-project",
      zone: "europe-west1-b", instance: "fixture-app", instance_id: "1234", ownership_token: "e".repeat(32),
      host: "compute.1234", user: "mobkit", ssh_port: 22 } };
  for (const cloud_status of ["removing", "removal-required", "removed"]) {
    const removed = { ...cloudReceipt, cloud_status };
    assert.equal(remoteBrowserOwned(removed, guide), true, "Owned connection cleanup remains selectable.");
    assert.equal(remoteBrowserQualified(removed, options, build, guide), false);
    assert.equal(remoteBrowserConsole(ready, removed, guide, build, initialTime), null);
    assert.equal(remoteBrowserCanClose({ ...ready, status: "cleanup-needed", local_url: null }), true);
  }
});

let componentPromise;
async function productionComponent() {
  if (!componentPromise) componentPromise = (async () => {
    const require = createRequire(import.meta.url);
    const built = await bundle({ stdin: { contents: 'export { RemoteBrowserAccess } from "./remote-browser-access.jsx";',
      resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "synthetic-browser-entry.mjs" },
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
      plugins: [{ name: "shared-mounted-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
      } }] });
    return import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  })();
  return componentPromise;
}

async function mountedAccess(initial, verify) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], polls = new Map(), clocks = new Map();
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  const originalSetInterval = globalThis.setInterval, originalClearInterval = globalThis.clearInterval;
  const originalNow = Date.now;
  let access = structuredClone(initial), now = initialTime, nextTimer = 0, readHandler;
  let openHandler = () => structuredClone(ready), closeHandler = () => ({ ...closed, access_id: accessId });
  let props = { state: guide, build, options, receipt };
  const response = (value) => ({ ok: true, json: async () => structuredClone(value) });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true,
    setTimeout(callback, delay, ...args) {
      if (delay !== 2000) return originalSetTimeout(callback, delay, ...args);
      const token = { poll: ++nextTimer }; polls.set(token, () => callback(...args)); return token;
    },
    clearTimeout(token) { if (!polls.delete(token)) originalClearTimeout(token); },
    setInterval(callback, delay, ...args) {
      if (delay !== 1000) return originalSetInterval(callback, delay, ...args);
      const token = { clock: ++nextTimer }; clocks.set(token, () => callback(...args)); return token;
    },
    clearInterval(token) { if (!clocks.delete(token)) originalClearInterval(token); },
    fetch: async (target, init = {}) => {
      const method = init.method || "GET", body = init.body ? JSON.parse(init.body) : null;
      calls.push({ target, method, body });
      if (method === "GET" && target.includes("/browser-access?"))
        return response(readHandler ? await readHandler(target) : access);
      if (method === "POST" && target.endsWith("/browser-access/open")) {
        const value = await openHandler(body); access = structuredClone(value); return response(value);
      }
      if (method === "POST" && target.endsWith("/browser-access/close")) {
        const value = await closeHandler(body); access = structuredClone(value); return response(value);
      }
      assert.fail(`Unexpected synthetic browser request: ${method} ${target}`);
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  Date.now = () => now;
  const { createRoot } = await import("react-dom/client");
  const { RemoteBrowserAccess } = await productionComponent();
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(RemoteBrowserAccess, props)));
  const document = dom.window.document;
  const button = (label) => [...document.querySelectorAll("button")].find((node) => node.textContent === label);
  const link = () => document.querySelector('a[href]');
  try {
    await render();
    await verify({ document, window: dom.window, calls, button, link,
      setRead(handler) { readHandler = handler; }, setAccess(value) { access = structuredClone(value); },
      setOpen(handler) { openHandler = handler; }, setClose(handler) { closeHandler = handler; },
      async rerender(patch) { props = { ...props, ...patch }; await render(); },
      async poll(value) {
        if (value) access = structuredClone(value);
        const first = polls.entries().next().value;
        assert.ok(first, "Production browser observation polling must be scheduled.");
        polls.delete(first[0]); await act(async () => { first[1](); });
      },
      async tick(milliseconds) { now += milliseconds; await act(async () => { for (const callback of clocks.values()) callback(); }); },
    });
  } finally {
    await act(async () => root.unmount());
    const remainingPolls = polls.size, remainingClocks = clocks.size;
    dom.window.close(); Date.now = originalNow;
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
    assert.equal(remainingPolls, 0, "Unmount cancels status polling.");
    assert.equal(remainingClocks, 0, "Unmount cancels freshness clock.");
  }
}

test("mount and reload observe only; deliberate connect publishes the same-port safe new-tab link", async () => {
  await mountedAccess(closed, async ({ document, window, calls, button, link, setOpen, poll }) => {
    assert.equal(calls.length, 1); assert.equal(calls[0].method, "GET");
    assert.equal(link() === null, true); assert.ok(button("Connect browser to native Console"));
    assert.equal(document.querySelector("details").open, false);
    assert.match(document.body.textContent, /Keep Forge open while using the Console/);
    setOpen(() => ({ ...ready, status: "connecting", native_ready: false, local_url: null }));
    await act(async () => button("Connect browser to native Console").click());
    assert.equal(link() === null, true);
    assert.deepEqual(calls.filter((call) => call.method === "POST"), [{
      target: remoteBrowserUrl(guide.id, receipt.deployment_id, "open"), method: "POST",
      body: { version: guide.version, build_id: build.build_id, deployment_id: receipt.deployment_id, entry_id: "native-console" } }]);
    await poll(ready);
    assert.equal(link().getAttribute("href"), consoleUrl);
    assert.equal(link().target, "_blank"); assert.equal(link().rel, "noopener noreferrer");
    assert.match(document.body.textContent, /Sign in with the Console password you chose/);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
    assert.equal(document.querySelector('input[type="password"]') === null, true);
  });
  await mountedAccess(ready, async ({ calls, link }) => {
    assert.equal(calls.length, 1); assert.equal(calls[0].method, "GET"); assert.ok(link());
  });
});

test("a client removal intent immediately hides ready links but retains deliberate uncertain tunnel cleanup", async () => {
  await mountedAccess(ready, async ({ document, button, link, rerender, setClose, calls }) => {
    assert.ok(link());
    await rerender({ removalPending: true });
    assert.equal(link() === null, true);
    assert.equal(button("Connect browser to native Console") === undefined, true);
    assert.ok(button("Close browser connection"));
    assert.match(document.body.textContent, /Opening Console is withdrawn/);
    setClose(() => ({ ...closed, access_id: accessId, cleanup_confirmed: false, status: "cleanup-needed" }));
    await act(async () => button("Close browser connection").click());
    assert.equal(link() === null, true);
    assert.ok(button("Close browser connection"));
    assert.equal(button("Connect browser to native Console") === undefined, true);
    assert.deepEqual(calls.filter((call) => call.method === "POST"), [{
      target: remoteBrowserUrl(guide.id, receipt.deployment_id, "close"), method: "POST",
      body: { deployment_id: receipt.deployment_id, access_id: accessId },
    }]);
  });
});

test("a deliberate close immediately withdraws the link and only closes the exact owned access id", async () => {
  await mountedAccess(ready, async ({ document, calls, button, link, setClose }) => {
    let resolveClose;
    setClose(() => new Promise((resolve) => { resolveClose = resolve; }));
    await act(async () => button("Close browser connection").click());
    assert.equal(link() === null, true); assert.match(document.body.textContent, /Closing your browser connection/);
    assert.deepEqual(calls.at(-1).body, { deployment_id: receipt.deployment_id, access_id: accessId });
    await act(async () => resolveClose({ ...closed, access_id: accessId }));
    assert.equal(link() === null, true); assert.ok(button("Connect browser to native Console"));
    assert.equal(button("Close browser connection") === undefined, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("a lost connect response observes admission without duplicate POST or stale pending-read rollback", async () => {
  await mountedAccess(closed, async ({ document, calls, button, link, setRead, setOpen, setAccess }) => {
    let resolveOld, reads = 0;
    setRead(() => ++reads === 1 ? new Promise((resolve) => { resolveOld = resolve; }) : ready);
    await act(async () => button("Recheck browser connection").click());
    setOpen(() => { setAccess(ready); throw new Error("Synthetic accepted response loss."); });
    await act(async () => button("Connect browser to native Console").click());
    assert.ok(link()); assert.doesNotMatch(document.body.textContent, /could not confirm the connection response/);
    await act(async () => resolveOld(closed));
    assert.ok(link()); assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("observation loss withdraws the link and retains exact close without asserting remote failure", async () => {
  await mountedAccess(ready, async ({ document, button, link, setRead, poll, calls }) => {
    setRead(() => { throw new Error("Synthetic observation transport loss."); });
    await poll();
    assert.equal(link() === null, true); assert.ok(button("Close browser connection"));
    assert.equal(button("Connect browser to native Console") === undefined, true);
    assert.match(document.body.textContent, /does not prove the remote application failed or stopped/);
    setRead(null); await poll(ready); assert.ok(link());
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("lost observation of a previously closed connection requires recheck before another deliberate open", async () => {
  await mountedAccess(closed, async ({ button, setRead, poll, calls }) => {
    setRead(() => { throw new Error("Synthetic browser status outage."); });
    await poll();
    assert.equal(button("Connect browser to native Console") === undefined, true);
    assert.ok(button("Recheck browser connection"));
    setRead(null); await poll(closed);
    assert.ok(button("Connect browser to native Console"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("the freshness clock hides an expired link even while a real observation request is stalled", async () => {
  await mountedAccess(ready, async ({ document, button, link, setRead, tick, calls }) => {
    let resolveRead;
    setRead(() => new Promise((resolve) => { resolveRead = resolve; }));
    await act(async () => button("Recheck browser connection").click());
    assert.ok(link()); await tick(15_001); assert.equal(link() === null, true);
    assert.equal(document.querySelector("h4").textContent, "Recheck your browser connection");
    assert.match(document.body.textContent, /last readiness observation is no longer fresh/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    await act(async () => resolveRead(ready)); assert.equal(link() === null, true);
  });
});

test("cleanup uncertainty permits exact close only; clean blocked attempts can be deliberately opened again", async () => {
  await mountedAccess({ ...ready, status: "cleanup-needed", local_url: null, native_ready: false },
    async ({ document, button, link, poll, calls }) => {
      assert.equal(link() === null, true); assert.ok(button("Close browser connection"));
      assert.equal(button("Connect browser to native Console") === undefined, true);
      assert.match(document.body.textContent, /cleanup is uncertain/);
      await poll({ ...closed, access_id: accessId, status: "blocked" });
      assert.ok(button("Connect browser to native Console")); assert.equal(button("Close browser connection") === undefined, true);
      assert.match(document.body.textContent, /port may already be in use/);
      await act(async () => button("Connect browser to native Console").click());
      assert.ok(link()); assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    });
});

test("foreign or credential-bearing replies never render a URL, credentials or raw response detail", async () => {
  await mountedAccess(ready, async ({ document, button, link, poll }) => {
    await poll({ ...ready, local_url: `${consoleUrl}?token=synthetic-url-canary`,
      detail: "synthetic-response-detail-canary" });
    assert.equal(link() === null, true); assert.ok(button("Close browser connection"));
    assert.doesNotMatch(document.body.innerHTML, /synthetic-url-canary|synthetic-response-detail-canary/);
    await poll({ ...ready, deployment_id: otherId }); assert.equal(link() === null, true);
    await poll(ready); assert.ok(link());
  });
});

test("late open replies cannot restore links while owned observation survives source eligibility changes", async () => {
  await mountedAccess(closed, async ({ calls, button, link, setOpen, rerender }) => {
    let resolveOpen;
    setOpen(() => new Promise((resolve) => { resolveOpen = resolve; }));
    await act(async () => button("Connect browser to native Console").click());
    await rerender({ state: { ...guide, version: 5 } }); assert.equal(link() === null, true);
    await act(async () => resolveOpen(ready)); assert.equal(link() === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
  await mountedAccess(ready, async ({ document, button, link, setRead, rerender }) => {
    let resolveRead;
    setRead(() => new Promise((resolve) => { resolveRead = resolve; }));
    await act(async () => button("Recheck browser connection").click());
    await rerender({ receipt: { ...receipt, source_current: false } });
    assert.ok(document.querySelector(".remote-browser-access"));
    assert.ok(button("Close browser connection"));
    assert.equal(button("Connect browser to native Console") === undefined, true);
    await act(async () => resolveRead(ready)); assert.equal(link() === null, true);
  });
  await mountedAccess(ready, async ({ link, rerender, setAccess }) => {
    setAccess(closed);
    await rerender({ receipt: { ...receipt, service: { ...receipt.service,
      service: `mobkit-launch.${accessId}.service` } } });
    assert.equal(link() === null, true);
  });
});

test("draft destination and application source selection changes retain only owned recheck and close", async () => {
  for (const patch of [
    { state: { ...guide, version: 5, proposal: { target: "mac" } }, options: { ...options, target: "mac", supported: true } },
    { options: null, blocked: true },
    { options: { ...options, source_kind: "native-team", build_id: null, source_id: "e".repeat(64) }, blocked: true },
    { build: { ...build, build_id: "another-build", source_digest: "e".repeat(64), acceptance_trusted: false } },
    { receipt: { ...receipt, source_binding_current: false }, observationUnavailable: true },
    { receipt: { ...receipt, status: "observation-unavailable", observation: { available: false } }, observationUnavailable: true },
    { receipt: { ...receipt, status: "stopping", pending_operation: { id: accessId, action: "stop" } } },
  ]) await mountedAccess(ready, async ({ document, calls, button, link, rerender, poll }) => {
    await rerender(patch);
    assert.equal(calls.length, 1, "Draft changes keep the same owned observer lifetime.");
    assert.equal(link() === null, true);
    assert.equal(button("Connect browser to native Console") === undefined, true);
    assert.ok(button("Close browser connection"));
    assert.ok(button("Recheck browser connection"));
    assert.match(document.body.textContent, /matching accepted application and current destination checks/);
    await poll(ready); assert.equal(link() === null, true);
    await act(async () => button("Close browser connection").click());
    assert.deepEqual(calls.filter((call) => call.method === "POST"), [{
      target: remoteBrowserUrl(guide.id, receipt.deployment_id, "close"), method: "POST",
      body: { deployment_id: receipt.deployment_id, access_id: accessId },
    }]);
    assert.equal(button("Close browser connection") === undefined, true);
    assert.equal(button("Connect browser to native Console") === undefined, true);
  });
});

test("foreign replies after a destination change preserve the prior owned close identity", async () => {
  await mountedAccess(ready, async ({ button, link, rerender, poll, calls }) => {
    await rerender({ state: { ...guide, version: 5, proposal: { target: "mac" } }, options: null });
    for (const changed of [{ guide_version: 5 }, { deployment_id: otherId }, { build_id: "another-build" },
      { controller_sha256: "e".repeat(64) }, { service_name: `mobkit-launch.${accessId}.service` }]) {
      await poll({ ...ready, ...changed, access_id: otherId });
      assert.equal(link() === null, true); assert.ok(button("Close browser connection"));
      assert.equal(button("Connect browser to native Console") === undefined, true);
    }
    await act(async () => button("Close browser connection").click());
    assert.deepEqual(calls.at(-1).body, { deployment_id: receipt.deployment_id, access_id: accessId });
  });
});

test("a draft round trip retires a pending open without retiring the owned observation", async () => {
  await mountedAccess(closed, async ({ calls, button, link, setOpen, rerender, poll }) => {
    let resolveOpen;
    setOpen(() => new Promise((resolve) => { resolveOpen = resolve; }));
    await act(async () => button("Connect browser to native Console").click());
    await rerender({ blocked: true, options: null });
    await rerender({ blocked: false, options });
    await act(async () => resolveOpen(ready));
    assert.equal(link() === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    await poll(ready); assert.ok(link()); assert.ok(button("Close browser connection"));
  });
});

test("an exact close completion remains usable when the draft changes during closing", async () => {
  await mountedAccess(ready, async ({ calls, button, link, setClose, rerender }) => {
    let resolveClose;
    setClose(() => new Promise((resolve) => { resolveClose = resolve; }));
    await act(async () => button("Close browser connection").click());
    await rerender({ state: { ...guide, version: 5, proposal: { target: "mac" } }, options: null });
    await act(async () => resolveClose({ ...closed, access_id: accessId }));
    assert.equal(link() === null, true); assert.equal(button("Close browser connection") === undefined, true);
    assert.equal(button("Connect browser to native Console") === undefined, true);
    assert.ok(button("Recheck browser connection"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("a late close of an earlier access cannot replace a newer access observed in the same service scope", async () => {
  await mountedAccess(ready, async ({ document, button, setClose, poll }) => {
    let resolveClose;
    setClose(() => new Promise((resolve) => { resolveClose = resolve; }));
    await act(async () => button("Close browser connection").click());
    await poll({ ...ready, access_id: otherId, observed_at: new Date(initialTime + 1).toISOString() });
    await act(async () => resolveClose({ ...closed, access_id: accessId }));
    assert.equal(document.querySelector(".remote-browser-access").dataset.browserStatus, "ready");
    assert.ok(button("Close browser connection"));
    assert.equal(button("Connect browser to native Console") === undefined, true);
  });
});

test("typed loopback-only packages use their actual authentication declaration without a password claim", async () => {
  await mountedAccess({ ...closed, scope: { ...closed.scope, authentication: "loopback-only" } },
    async ({ document }) => {
      assert.match(document.body.textContent, /does not ask for a Console password/);
      assert.doesNotMatch(document.body.textContent, /Sign in with the Console password you chose/);
      const details = document.querySelector("details"); assert.equal(details.open, false);
      assert.match(details.textContent, /Custom application UI, successful sign-in and useful application results still need separate verification/);
    });
});

test("malformed truthy service names withhold the mounted panel without a render crash or connection attempt", async () => {
  await mountedAccess(ready, async ({ document, calls, rerender }) => {
    for (const service of [1, {}, true, []]) {
      await rerender({ receipt: { ...receipt, service: { ...receipt.service, service } } });
      assert.equal(document.querySelector(".remote-browser-access") === null, true);
    }
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.equal(calls.length, 1, "Refused malformed service scopes start no additional observation request.");
  });
});


test("a refreshed native browser observation survives harmless chat without replacing its owned tunnel", async () => {
  await mountedAccess(ready, async ({ calls, link, rerender, poll }) => {
    assert.equal(link().getAttribute("href"), consoleUrl);
    const next = { ...guide, version: 5 }, currentBuild = { ...build, current_guide_version: 5 };
    const currentReceipt = { ...receipt, current_guide_version: 5 };
    await rerender({ state: next, build: currentBuild, receipt: currentReceipt,
      options: { ...options, current_guide_version: 5 } });
    assert.equal(link() === null, true, "An old ready reply does not attest the new chat snapshot.");
    await poll({ ...ready, current_guide_version: 5 });
    assert.equal(link().getAttribute("href"), consoleUrl);
    assert.equal(remoteBrowserOwnedBinding(currentReceipt, next), remoteBrowserOwnedBinding(receipt, guide));
    for (const patch of [{ current_guide_version: null }, { current_guide_version: "5" },
      { package_sha256: "e".repeat(64) }, { controller_sha256: "e".repeat(64) }]) {
      assert.equal(remoteBrowserReplyCurrent({ ...ready, current_guide_version: 5, ...patch },
        currentReceipt, next, currentBuild), false);
    }
    await rerender({ receipt: { ...currentReceipt, source_binding_current: false } });
    assert.equal(link() === null, true);
    assert.equal(calls.some(call => call.method === "POST"), false);
  });
});

test("native browser open after harmless chat submits the current version and retains connection provenance", async () => {
  await mountedAccess(closed, async ({ calls, link, rerender, poll, button, setOpen }) => {
    const next = { ...guide, version: 5 };
    await rerender({ state: next, build: { ...build, current_guide_version: 5 },
      options: { ...options, current_guide_version: 5 }, receipt: { ...receipt, current_guide_version: 5 } });
    assert.equal(button("Connect browser to native Console") === undefined, true);
    await poll({ ...closed, current_guide_version: 5 });
    setOpen(() => ({ ...ready, current_guide_version: 5 }));
    await act(async () => button("Connect browser to native Console").click());
    assert.equal(link().getAttribute("href"), consoleUrl);
    const posts = calls.filter(call => call.method === "POST"); assert.equal(posts.length, 1);
    assert.deepEqual(posts[0].body, { version: 5, build_id: build.build_id,
      deployment_id: receipt.deployment_id, entry_id: "native-console" });
  });
});
