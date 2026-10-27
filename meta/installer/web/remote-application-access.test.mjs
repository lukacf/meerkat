import assert from "node:assert/strict";
import test from "node:test";
import { existsSync } from "node:fs";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
const webDir = existsSync(new URL("package.json", import.meta.url)) ? new URL("./", import.meta.url) : new URL("../../../web/", import.meta.url);
import { remoteApplicationExperiences, remoteApplicationPath, remoteApplicationEntryId, remoteApplicationUrl,
  remoteApplicationBinding, remoteApplicationAddress, remoteApplicationReplyCurrent, remoteApplicationLink,
  remoteApplicationCanOpen, remoteApplicationCanClose } from "./remote-application-access.mjs";

import { remoteInstalledUseBinding, remoteInstalledUseReadKey, remoteInstalledUseUrl } from "./remote-installed-use.mjs";

const guide = { id: "66666666-6666-4666-8666-666666666666", version: 4, proposal: { target: "linux" } };
const build = { guide_id: guide.id, guide_version: 4, build_id: "synthetic-accepted-build",
  source_digest: "a".repeat(64), status: "completed", current: true, ready_to_package: true,
  acceptance_current: true, acceptance_trusted: true, acceptance: { status: "pass" } };
const options = { build_id: build.build_id, guide_version: 4, current: true,
  supported: false, target: "linux", host_target: "mac" };
const ids = ["11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222",
  "33333333-3333-4333-8333-333333333333", "44444444-4444-4444-8444-444444444444"];
const consoleUrl = "http://127.0.0.1:8791/console";
const initialTime = Date.parse("2026-10-02T05:00:00Z");
const entries = [
  { id: "workshop", label: "Workshop", path: "/", destination_port: 8792,
    listener_owned: true, listener_token: "f".repeat(64), reason: null, verified: false },
  { id: "admin", label: "Workshop settings", path: "/admin", destination_port: 8792,
    listener_owned: true, listener_token: "f".repeat(64), reason: null, verified: false },
  { id: "reports", label: "Reports", path: "/reports/daily", destination_port: 12007,
    listener_owned: true, listener_token: "9".repeat(64), reason: null, verified: false },
  { id: "native-console", label: "Declared control page", path: "/custom", destination_port: 8791,
    listener_owned: true, listener_token: "8".repeat(64), reason: null, verified: false },
];
const receipt = { protocol: 1, deployment_id: "55555555-5555-4555-8555-555555555555", guide_id: guide.id,
  guide_version: 4, build_id: build.build_id, accepted_source_digest: build.source_digest,
  package_sha256: "b".repeat(64), transferred_source_digest: "c".repeat(64), controller_sha256: "d".repeat(64),
  source_binding_current: true, source_current: true, status: "running", ready: true,
  remote_admission_requested: true, admitted: true, worker_live: false,
  service_owned: true, service_observed: true, observation: { available: true },
  endpoint: { host: "synthetic-linux.local", user: "fixture-user", ssh_port: 22 },
  destination: "/synthetic-owned/src/workshop",
  host: { uid: 1001, home: "/synthetic-owned", destination: "/synthetic-owned/src/workshop" },
  host_key_fingerprint: "SHA256:" + "A".repeat(43),
  service: { service: "mobkit-launch." + ids[3] + ".service", status: "running", ready: true,
    scope: "native-bootstrap-and-console", source_digest: "c".repeat(64), source_current: true,
    definition_verified: true, console_url: consoleUrl },
  application_experiences: { protocol: 1, supported: true, status: "resolved", source_digest: build.source_digest,
    installed_source_digest: "c".repeat(64), metadata_sha256: "e".repeat(64), entries },
};
const experiences = remoteApplicationExperiences(receipt);
const closed = (entry, patch = {}) => ({ protocol: 1, guide_id: guide.id, guide_version: 4,
  build_id: build.build_id, deployment_id: receipt.deployment_id, access_id: null,
  entry_id: remoteApplicationEntryId(entry), status: "closed", local_url: null, console_url: consoleUrl,
  native_ready: false, cleanup_confirmed: true, observed_at: new Date(initialTime).toISOString(),
  accepted_source_digest: receipt.accepted_source_digest, package_sha256: receipt.package_sha256,
  transferred_source_digest: receipt.transferred_source_digest, controller_sha256: receipt.controller_sha256,
  service_name: receipt.service.service, service_source_digest: receipt.service.source_digest,
  metadata_sha256: experiences.metadata_sha256, entry_path: entry.path, destination_port: entry.destination_port,
  listener_token: entry.listener_token, transport_reachable: false, http_status: null,
  application_verified: false, authentication_verified: false, workflow_verified: false,
  scope: { health_scope: "declared-application-transport", custom_interfaces_verified: false }, ...patch });
const ready = (entry, patch = {}) => closed(entry, { access_id: ids[entries.findIndex((row) => row.id === entry.id)],
  status: "ready", local_url: remoteApplicationAddress(entry), native_ready: true, cleanup_confirmed: false,
  transport_reachable: true, http_status: 200, ...patch });
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };

test("metadata joins accepted and installed source and validates all declared entries", () => {
  assert.equal(experiences.status, "resolved"); assert.equal(experiences.entries.length, 4);
  assert.equal(remoteApplicationExperiences({ ...receipt, application_experiences: undefined }).supported, false);
  assert.equal(remoteApplicationExperiences({ ...receipt, application_experiences: {
    ...receipt.application_experiences, supported: false, status: "unavailable",
    metadata_sha256: null, entries: [] } }).supported, false);
  assert.equal(remoteApplicationExperiences({ ...receipt, application_experiences: {
    ...receipt.application_experiences, status: "none", entries: [], metadata_sha256: null } }).status, "none");
  for (const changed of [{ source_digest: "0".repeat(64) }, { installed_source_digest: "0".repeat(64) },
    { metadata_sha256: null }, { metadata_sha256: { toString: null } }, { entries: [] },
    { entries: [...entries, entries[0]] }, { entries: [{ ...entries[0], id: { toString: null } }] },
    { entries: [{ ...entries[0], destination_port: "8792" }] },
    { entries: [{ ...entries[0], listener_token: { toString: null } }] },
    { entries: [{ ...entries[0], verified: true }] }, { entries: [{ ...entries[0], path: "/../other" }] }])
    assert.equal(remoteApplicationExperiences({ ...receipt, application_experiences: {
      ...receipt.application_experiences, ...changed } }).status, "unavailable");
});

test("canonical declared paths and ports refuse credentials, queries and traversal", () => {
  assert.equal(remoteApplicationAddress(entries[2]), "http://127.0.0.1:12007/reports/daily");
  assert.equal(remoteApplicationAddress({ ...entries[0], destination_port: 1024 }), "http://127.0.0.1:1024/");
  for (const path of ["/觀測/översikt", "/%E8%A7%80%E6%B8%AC/%C3%B6versikt"])
    assert.equal(remoteApplicationAddress({ ...entries[2], path }),
      "http://127.0.0.1:12007/%E8%A7%80%E6%B8%AC/%C3%B6versikt");
  for (const path of ["https://outside.example/", "//outside.example/", "/../other", "/%2e%2e/other",
    "/%252e%252e/other", "/%25252e%25252e/other", "/path?token=synthetic-url-canary", "/#private",
    "/path%3Fsecret", "/a\\b", "/path with spaces", "/%zz", "/%00"])
    assert.equal(remoteApplicationPath(path), false, path);
  for (const destination_port of [0, 80, 1023, 65536, "8792", null, 1.1])
    assert.equal(remoteApplicationAddress({ ...entries[0], destination_port }), null);
  assert.equal(remoteApplicationEntryId(entries[3]), "application:native-console");
  assert.equal(remoteApplicationUrl("guide /1", "deployment /2", entries[3]),
    "/api/guide/guide%20%2F1/remote-deployment/browser-access?deployment_id=deployment+%2F2&entry_id=application%3Anative-console");
});

test("ready replies require exact entry, listener custody, service and source evidence", () => {
  const entry = entries[2], value = ready(entry);
  assert.equal(remoteApplicationReplyCurrent(closed(entry), receipt, guide, build, experiences, entry), true);
  assert.equal(remoteApplicationReplyCurrent(value, receipt, guide, build, experiences, entry), true);
  for (const changed of [{ guide_id: "foreign" }, { guide_version: 5 }, { build_id: "foreign" },
    { deployment_id: ids[0] }, { entry_id: "native-console" }, { entry_id: "application:admin" },
    { accepted_source_digest: "0".repeat(64) }, { transferred_source_digest: "0".repeat(64) },
    { package_sha256: "0".repeat(64) }, { controller_sha256: "0".repeat(64) },
    { service_name: "mobkit-launch." + ids[0] + ".service" }, { service_source_digest: "0".repeat(64) },
    { metadata_sha256: "0".repeat(64) }, { entry_path: "/different" }, { destination_port: 8792 },
    { listener_token: "0".repeat(64) }, { listener_token: { toString: null } },
    { native_ready: false }, { transport_reachable: false }, { cleanup_confirmed: true },
    { http_status: 500 }, { http_status: 199 }, { http_status: "200" }, { http_status: null },
    { application_verified: true }, { authentication_verified: true }, { workflow_verified: true },
    { observed_at: "2026-10-02 05:00:00" }, { access_id: null }, { access_id: { toString: null } },
    { local_url: "http://localhost:12007/reports/daily" }, { local_url: "http://127.0.0.1:8792/reports/daily" },
    { local_url: "http://127.0.0.1:12007/reports/daily?token=synthetic-url-canary" },
    { local_url: "http://user:password@127.0.0.1:12007/reports/daily" },
    { console_url: "http://127.0.0.1:8792/console" },
    { scope: { health_scope: "native-bootstrap-and-console", custom_interfaces_verified: false } }])
    assert.equal(remoteApplicationReplyCurrent({ ...value, ...changed }, receipt, guide, build, experiences, entry), false);
  for (const http_status of [200, 302, 399, 401, 403])
    assert.equal(remoteApplicationReplyCurrent({ ...value, http_status }, receipt, guide, build, experiences, entry), true);
});

test("freshness withdraws links while changed listeners retain exact cleanup custody", () => {
  const entry = entries[0], value = ready(entry), changed = { ...entry, listener_token: "0".repeat(64) };
  assert.equal(remoteApplicationLink(value, receipt, guide, build, experiences, entry, initialTime + 15_000), value.local_url);
  assert.equal(remoteApplicationLink(value, receipt, guide, build, experiences, entry, initialTime + 15_001), null);
  assert.equal(remoteApplicationLink({ ...value, observed_at: initialTime + 3001 }, receipt,
    guide, build, experiences, entry, initialTime), null);
  assert.equal(remoteApplicationBinding(receipt, guide, build, experiences, entry),
    remoteApplicationBinding(receipt, guide, build, experiences, changed));
  assert.equal(remoteApplicationLink(value, receipt, guide, build, experiences, changed, initialTime), null);
  const cleanup = { ...value, status: "cleanup-needed", local_url: null, native_ready: false };
  assert.equal(remoteApplicationReplyCurrent(cleanup, receipt, guide, build, experiences, changed), true);
  assert.equal(remoteApplicationCanClose(cleanup), true); assert.equal(remoteApplicationCanOpen(cleanup, entry), false);
  for (const status of ["closed", "blocked", "unavailable"]) {
    assert.equal(remoteApplicationCanOpen(closed(entry, { status, access_id: ids[0] }), entry), true);
    assert.equal(remoteApplicationCanOpen(closed(entry, { status, cleanup_confirmed: false }), entry), false);
  }
  assert.equal(remoteApplicationCanClose({ access_id: { toString: null } }), false);
});

let componentPromise;
async function productionComponent() {
  if (!componentPromise) componentPromise = (async () => {
    const require = createRequire(import.meta.url);
    const built = await bundle({ stdin: { contents: 'export { RemoteApplicationAccess } from "./remote-application-access.jsx"; export { RemoteDeployment } from "./remote-deployment.jsx";',
      resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "synthetic-application-entry.mjs" },
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
      plugins: [{ name: "shared-mounted-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
      } }] });
    return import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
  })();
  return componentPromise;
}

async function mountedAccess(initial, verify, initialProps = {}) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", webDir))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], polls = new Map(), clocks = new Map(), reads = new Map();
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  const originalSetInterval = globalThis.setInterval, originalClearInterval = globalThis.clearInterval;
  const originalNow = Date.now;
  const access = new Map(entries.map((entry) => [entry.id, structuredClone(initial[entry.id] || closed(entry))]));
  let now = initialTime, nextTimer = 0;
  let openHandler = (body) => ready(entries.find((entry) => remoteApplicationEntryId(entry) === body.entry_id));
  let closeHandler = (body) => {
    const entry = entries.find((row) => access.get(row.id).access_id === body.access_id);
    for (const alias of entries.filter((row) => row.destination_port === entry.destination_port))
      access.set(alias.id, closed(alias, { access_id: access.get(alias.id).access_id }));
    return access.get(entry.id);
  };
  let props = { state: guide, build, options, receipt, ...initialProps };
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
      const entryId = method === "GET" ? new URL(target, "http://localhost").searchParams.get("entry_id") : body?.entry_id;
      calls.push({ target, method, body, entryId });
      if (initialProps.mountDeployment && method === "GET" && target.endsWith("/remote-deployment")) return response(props.receipt);
      if (initialProps.mountDeployment && method === "GET" && target === remoteInstalledUseUrl(guide.id, receipt.deployment_id))
        return response(remoteTaskMetadata(props.receipt));
      if (method === "GET" && target.includes("/browser-access?")) {
        if (initialProps.mountDeployment && entryId === "native-console") return response({ ...closed(entries[0]), entry_id,
          scope: { authentication: "loopback-only", health_scope: "native-bootstrap-and-console", custom_interfaces_verified: false,
            requested_interfaces: "native-console" } });
        assert.match(entryId, /^application:/);
        const id = entryId.slice("application:".length);
        return response(reads.has(id) ? await reads.get(id)(target) : access.get(id));
      }
      if (method === "POST" && target.endsWith("/browser-access/open")) {
        const value = await openHandler(body); access.set(entryId.slice("application:".length), structuredClone(value));
        return response(value);
      }
      if (method === "POST" && target.endsWith("/browser-access/close")) return response(await closeHandler(body));
      assert.fail("Unexpected synthetic application request: " + method + " " + target);
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  Date.now = () => now;
  const { createRoot } = await import("react-dom/client");
  const { RemoteApplicationAccess, RemoteDeployment } = await productionComponent();
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(initialProps.mountDeployment ? RemoteDeployment : RemoteApplicationAccess,
    { ...props, plan: { project: { name: "Workshop", slug: "workshop" } }, renderConfiguration: () => null })));
  const document = dom.window.document;
  const row = (id) => document.querySelector('[data-entry-id="' + id + '"]');
  const button = (id, label) => [...(row(id)?.querySelectorAll("button") || [])].find((node) => node.textContent === label);
  const link = (id) => row(id)?.querySelector("a[href]") || null;
  try {
    await render();
    await verify({ document, window: dom.window, calls, row, button, link,
      setRead(id, handler) { if (handler) reads.set(id, handler); else reads.delete(id); },
      setAccess(id, value) { access.set(id, structuredClone(value)); },
      setOpen(handler) { openHandler = handler; }, setClose(handler) { closeHandler = handler; },
      async rerender(patch) { props = { ...props, ...patch }; await render(); },
      async click(id, label) { const selected = button(id, label); assert.ok(selected, id + ": " + label);
        await act(async () => selected.click()); },
      async poll() {
        const pending = [...polls.entries()]; assert.ok(pending.length, "Production application polling must be scheduled.");
        await act(async () => { for (const [token, callback] of pending) { polls.delete(token); callback(); } });
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
    assert.equal(remainingPolls, 0, "Unmount cancels every entry's status polling.");
    assert.equal(remainingClocks, 0, "Unmount cancels every entry's freshness clock.");
  }
}

test("multiple entries mount and reload through GET only; explicit open publishes the declared safe path", async () => {
  await mountedAccess({}, async ({ document, window, calls, button, link, click }) => {
    assert.equal(calls.length, entries.length); assert.ok(calls.every((call) => call.method === "GET"));
    assert.equal(document.querySelector("h4").textContent, "Open my app");
    assert.equal(document.querySelector("details").open, false);
    for (const entry of entries) { assert.ok(button(entry.id, "Connect to " + entry.label)); assert.equal(link(entry.id) === null, true); }
    await click("reports", "Connect to Reports");
    assert.equal(link("reports").getAttribute("href"), "http://127.0.0.1:12007/reports/daily");
    assert.equal(link("reports").target, "_blank"); assert.equal(link("reports").rel, "noopener noreferrer");
    assert.deepEqual(calls.filter((call) => call.method === "POST"), [{
      target: remoteApplicationUrl(guide.id, receipt.deployment_id, entries[2], "open"), method: "POST",
      body: { version: 4, build_id: build.build_id, deployment_id: receipt.deployment_id,
        entry_id: "application:reports" }, entryId: "application:reports" }]);
    assert.match(document.body.textContent, /does not verify sign-in or a useful task result/);
    assert.match(document.body.textContent, /Keep Forge open while using the app/);
    assert.equal(window.localStorage.length, 0); assert.equal(window.sessionStorage.length, 0);
    assert.equal(document.querySelector("input") === null, true);
  });
  await mountedAccess({ reports: ready(entries[2]) }, async ({ calls, link }) => {
    assert.ok(link("reports")); assert.ok(calls.every((call) => call.method === "GET"));
  });
});

test("a declared native-console id stays namespaced independently from native Console access", async () => {
  await mountedAccess({}, async ({ click, calls, link }) => {
    await click("native-console", "Connect to Declared control page");
    assert.equal(calls.find((call) => call.method === "POST").body.entry_id, "application:native-console");
    assert.equal(link("native-console").getAttribute("href"), "http://127.0.0.1:8791/custom");
  });
});

test("Unicode and already escaped declared paths publish one encoding without changing entry authority", async () => {
  for (const path of ["/觀測/översikt", "/%E8%A7%80%E6%B8%AC/%C3%B6versikt"]) {
    const entry = { ...entries[2], path };
    await mountedAccess({ reports: ready(entry) }, async ({ link, calls }) => {
      assert.equal(link("reports").getAttribute("href"), "http://127.0.0.1:12007/%E8%A7%80%E6%B8%AC/%C3%B6versikt");
      assert.equal(calls.length, 1); assert.equal(calls[0].method, "GET");
    }, { receipt: { ...receipt, application_experiences: { ...receipt.application_experiences, entries: [entry] } } });
  }
});

test("older controllers and no-entry packages require no custom requests", async () => {
  for (const application_experiences of [undefined, { ...receipt.application_experiences,
    supported: false, status: "unavailable", metadata_sha256: null, entries: [] }, { ...receipt.application_experiences,
    status: "none", metadata_sha256: null, entries: [] }])
    await mountedAccess({}, async ({ document, calls }) => {
      assert.equal(document.querySelector(".remote-application-access") === null, true); assert.equal(calls.length, 0);
    }, { receipt: { ...receipt, application_experiences } });
});

test("malformed metadata and unowned listeners never offer connection, including non-string service names", async () => {
  await mountedAccess({}, async ({ document, calls, rerender }) => {
    await rerender({ receipt: { ...receipt, application_experiences: { ...receipt.application_experiences,
      metadata_sha256: { toString: null } } } });
    assert.equal(document.querySelectorAll("a, .remote-application-entry").length, 0);
    assert.match(document.body.textContent, /could not check this installation's application links/);
    for (const service of [1, {}, true, []]) {
      await rerender({ receipt: { ...receipt, service: { ...receipt.service, service } } });
      assert.equal(document.querySelector(".remote-application-access") === null, true);
    }
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
  const entry = { ...entries[0], listener_owned: false, listener_token: null,
    reason: "The declared listener is not currently owned by this service." };
  await mountedAccess({ workshop: closed(entry) }, async ({ document, button, link, calls }) => {
    assert.equal(button("workshop", "Connect to Workshop") === undefined, true); assert.equal(link("workshop") === null, true);
    assert.match(document.body.textContent, /declared listener is not currently owned/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  }, { receipt: { ...receipt, application_experiences: { ...receipt.application_experiences, entries: [entry] } } });
});

test("same-port close hides every alias immediately while unrelated port access remains available", async () => {
  await mountedAccess({ workshop: ready(entries[0]), admin: ready(entries[1]), reports: ready(entries[2]) },
    async ({ calls, click, link, button, setClose, setAccess, poll }) => {
      const pending = deferred();
      setClose((body) => {
        assert.equal(body.access_id, ids[0]);
        for (const entry of entries.slice(0, 2)) setAccess(entry.id, closed(entry, {
          access_id: ids[entries.indexOf(entry)], status: "cleanup-needed", cleanup_confirmed: false }));
        return pending.promise;
      });
      await click("workshop", "Close connection");
      assert.equal(link("workshop") === null, true); assert.equal(link("admin") === null, true); assert.ok(link("reports"));
      assert.equal(button("admin", "Connect to Workshop settings") === undefined, true);
      assert.deepEqual(calls.find((call) => call.method === "POST").body,
        { deployment_id: receipt.deployment_id, access_id: ids[0] });
      await act(async () => pending.resolve(closed(entries[0], {
        access_id: ids[0], status: "cleanup-needed", cleanup_confirmed: false })));
      assert.equal(link("admin") === null, true); assert.ok(button("admin", "Close connection"));
      for (const entry of entries.slice(0, 2)) setAccess(entry.id, closed(entry, { access_id: ids[entries.indexOf(entry)] }));
      await poll();
      assert.ok(button("workshop", "Connect to Workshop")); assert.ok(button("admin", "Connect to Workshop settings"));
      assert.ok(link("reports")); assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    });
});

test("a delayed alias open admitted before shared close cannot restore any closed-port link", async () => {
  await mountedAccess({ workshop: ready(entries[0]), reports: ready(entries[2]) },
    async ({ click, link, button, setOpen, setClose, setAccess, calls, poll }) => {
      const pendingOpen = deferred(), pendingClose = deferred();
      setOpen(() => pendingOpen.promise); await click("admin", "Connect to Workshop settings");
      setClose(() => {
        for (const entry of entries.slice(0, 2)) setAccess(entry.id, closed(entry, {
          access_id: ids[entries.indexOf(entry)], status: "cleanup-needed", cleanup_confirmed: false }));
        return pendingClose.promise;
      });
      await click("workshop", "Close connection");
      assert.equal(link("workshop") === null, true); assert.equal(link("admin") === null, true);
      await act(async () => pendingOpen.resolve(ready(entries[1])));
      assert.equal(link("admin") === null, true); assert.equal(button("admin", "Connect to Workshop settings") === undefined, true);
      assert.ok(link("reports"));
      setAccess("admin", closed(entries[1], { access_id: ids[1], status: "cleanup-needed", cleanup_confirmed: false }));
      await act(async () => pendingClose.resolve(closed(entries[0], { access_id: ids[0],
        status: "cleanup-needed", cleanup_confirmed: false })));
      await poll(); assert.equal(link("admin") === null, true);
      assert.equal(calls.filter((call) => call.method === "POST").length, 2);
    });
});

test("listener changes withdraw readiness while retaining exact admitted close custody", async () => {
  await mountedAccess({ workshop: ready(entries[0]) }, async ({ click, link, button, rerender, setClose, calls }) => {
    const changedEntries = entries.map((entry) => entry.destination_port === 8792 ? { ...entry, listener_token: "0".repeat(64) } : entry);
    await rerender({ receipt: { ...receipt, application_experiences: { ...receipt.application_experiences, entries: changedEntries } } });
    assert.equal(link("workshop") === null, true); assert.ok(button("workshop", "Close connection"));
    setClose(() => closed(entries[0], { access_id: ids[0], status: "cleanup-needed", cleanup_confirmed: false }));
    await click("workshop", "Close connection");
    assert.equal(calls.find((call) => call.method === "POST").body.access_id, ids[0]);
    assert.equal(link("workshop") === null, true); assert.ok(button("workshop", "Close connection"));
  });
});

test("observation loss hides ready links without asserting the Linux application stopped", async () => {
  await mountedAccess({ reports: ready(entries[2]) }, async ({ document, link, button, setRead, click, calls }) => {
    setRead("reports", () => { throw new Error("Synthetic observation loss."); });
    await click("reports", "Recheck connection");
    assert.equal(link("reports") === null, true); assert.ok(button("reports", "Close connection"));
    assert.equal(button("reports", "Connect to Reports") === undefined, true);
    assert.match(document.body.textContent, /Linux application may still be running/);
    setRead("reports", null); await click("reports", "Recheck connection"); assert.ok(link("reports"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
  await mountedAccess({}, async ({ button, setRead, click, calls }) => {
    setRead("reports", () => { throw new Error("Synthetic closed observation loss."); });
    await click("reports", "Recheck connection"); assert.equal(button("reports", "Connect to Reports") === undefined, true);
    setRead("reports", null); await click("reports", "Recheck connection"); assert.ok(button("reports", "Connect to Reports"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("freshness clocks hide links even when entry observations stall", async () => {
  await mountedAccess({ workshop: ready(entries[0]), reports: ready(entries[2]) },
    async ({ document, link, click, tick, setRead, calls }) => {
      const pending = deferred(); setRead("workshop", () => pending.promise);
      await click("workshop", "Recheck connection"); assert.ok(link("workshop"));
      await tick(15_001); assert.equal(link("workshop") === null, true); assert.equal(link("reports") === null, true);
      assert.match(document.body.textContent, /readiness needs another check/);
      await act(async () => pending.resolve(ready(entries[0]))); assert.equal(link("workshop") === null, true);
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    });
});

test("lost explicit open response discovers admission without duplicate open or an older GET rollback", async () => {
  await mountedAccess({}, async ({ link, click, setRead, setOpen, setAccess, calls }) => {
    const previous = deferred(); let reads = 0;
    setRead("reports", () => ++reads === 1 ? previous.promise : ready(entries[2]));
    await click("reports", "Recheck connection");
    setOpen(() => { setAccess("reports", ready(entries[2])); throw new Error("Synthetic admitted response loss."); });
    await click("reports", "Connect to Reports"); assert.ok(link("reports"));
    await act(async () => previous.resolve(closed(entries[2]))); assert.ok(link("reports"));
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("foreign, credential-bearing and false verification replies withhold links and raw detail", async () => {
  await mountedAccess({ reports: ready(entries[2]) }, async ({ document, link, click, setAccess, button, calls }) => {
    for (const changed of [{ entry_id: "application:admin" }, { metadata_sha256: "0".repeat(64) },
      { entry_path: "/another" }, { destination_port: 8792 }, { listener_token: "0".repeat(64) },
      { authentication_verified: true }, { http_status: 500 }, { access_id: { toString: null } },
      { local_url: "http://127.0.0.1:12007/reports/daily?token=synthetic-url-canary",
        detail: "synthetic-response-detail-canary" }]) {
      setAccess("reports", ready(entries[2], changed)); await click("reports", "Recheck connection");
      assert.equal(link("reports") === null, true); assert.ok(button("reports", "Close connection"));
      assert.doesNotMatch(document.body.innerHTML, /synthetic-url-canary|synthetic-response-detail-canary/);
    }
    setAccess("reports", ready(entries[2], { http_status: 401 })); await click("reports", "Recheck connection");
    assert.ok(link("reports")); assert.match(document.body.textContent, /sign in if it asks/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
  });
});

test("late open and GET replies cannot publish after source, guide or application declaration changes", async () => {
  await mountedAccess({}, async ({ document, click, link, setOpen, rerender, calls }) => {
    const pending = deferred(); setOpen(() => pending.promise);
    await click("reports", "Connect to Reports"); await rerender({ state: { ...guide, version: 5 } });
    await act(async () => pending.resolve(ready(entries[2])));
    assert.equal(link("reports") === null, true); assert.equal(document.querySelector(".remote-application-access") === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  });
  await mountedAccess({ reports: ready(entries[2]) }, async ({ document, click, link, setRead, rerender }) => {
    const pending = deferred(); setRead("reports", () => pending.promise);
    await click("reports", "Recheck connection"); await rerender({ receipt: { ...receipt, source_current: false } });
    await act(async () => pending.resolve(ready(entries[2])));
    assert.equal(link("reports") === null, true); assert.equal(document.querySelector(".remote-application-access") === null, true);
  });
  await mountedAccess({ reports: ready(entries[2]) }, async ({ click, link, setRead, rerender }) => {
    const pending = deferred(); setRead("reports", () => pending.promise); await click("reports", "Recheck connection");
    const changedEntries = entries.map((entry) => entry.id === "reports" ? { ...entry, path: "/other", destination_port: 12008 } : entry);
    await rerender({ receipt: { ...receipt, application_experiences: { ...receipt.application_experiences,
      metadata_sha256: "0".repeat(64), entries: changedEntries } } });
    await act(async () => pending.resolve(ready(entries[2]))); assert.equal(link("reports") === null, true);
  });
});


const draftProjection = (task, entryId = "workshop") => ({ readKey: remoteInstalledUseReadKey(receipt, guide, build),
  binding: remoteInstalledUseBinding(receipt, guide), entryId, task });
const handoffLink = (document, id = "workshop") => [...document.querySelectorAll('[data-entry-id="' + id + '"] a')]
  .find((node) => node.textContent === "Continue with this task") || null;
const linkedDraft = (node) => JSON.parse(decodeURIComponent(new URL(node.href).hash.slice("#mobkit-task=".length))).task;

test("remote handoff appends the current editable draft only to the exact existing ready owned tunnel", async () => {
  const task = "  Remote editable \u{1f9ed} draft\nwith exact whitespace.  ";
  await mountedAccess({ workshop: ready(entries[0], { http_status: 401 }), admin: ready(entries[1]) },
    async ({ document, calls, rerender, link }) => {
      const handoff = handoffLink(document);
      assert.ok(handoff);
      assert.equal(handoff.href.split("#")[0], link("workshop").href);
      assert.equal(linkedDraft(handoff), task);
      assert.equal(handoffLink(document, "admin") === null, true);
      assert.equal(handoff.getAttribute("referrerpolicy"), "no-referrer");
      assert.equal(handoff.rel, "noopener noreferrer");
      assert.equal(new URL(handoff.href).search, "");
      assert.match(document.body.textContent, /responding interface does not verify sign-in or a useful task result/);
      assert.match(document.body.textContent, /send and confirmation controls/);
      const edited = "Human edited this after the suggestion.";
      await rerender({ taskHandoff: draftProjection(edited) });
      assert.equal(linkedDraft(handoffLink(document)), edited);
      for (const taskHandoff of [{ ...draftProjection(edited), readKey: "stale" },
        { ...draftProjection(edited), binding: "another source" }, draftProjection(edited, "missing"), draftProjection("bad\ud800"), null]) {
        await rerender({ taskHandoff }); assert.equal(handoffLink(document) === null, true);
        assert.ok(link("workshop"), "The ordinary checked app link remains independent.");
      }
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    }, { taskHandoff: draftProjection(task) });
});

test("remote draft handoff is absent until explicit owned tunnel readiness and expires with its displayed link", async () => {
  const taskHandoff = draftProjection("Human draft");
  await mountedAccess({}, async ({ document, calls, click, tick }) => {
    assert.equal(handoffLink(document) === null, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    await click("workshop", "Connect to Workshop");
    assert.equal(linkedDraft(handoffLink(document)), "Human draft");
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
    assert.deepEqual(calls.find((call) => call.method === "POST").body.entry_id, "application:workshop");
    await tick(15001);
    assert.equal(handoffLink(document) === null, true);
  }, { taskHandoff });
});

test("remote handoff immediately withdraws for close, health observation loss and stale listener custody", async () => {
  const taskHandoff = draftProjection("Retained draft");
  await mountedAccess({ workshop: ready(entries[0]) }, async ({ document, click, rerender }) => {
    assert.ok(handoffLink(document));
    await click("workshop", "Close connection");
    assert.equal(handoffLink(document) === null, true);
    await rerender({ observationUnavailable: true });
    assert.equal(handoffLink(document) === null, true);
  }, { taskHandoff });
  await mountedAccess({ workshop: ready(entries[0]) }, async ({ document, rerender }) => {
    await rerender({ receipt: { ...receipt, application_experiences: { ...receipt.application_experiences,
      entries: entries.map((entry) => entry.id === "workshop" ? { ...entry, listener_token: "0".repeat(64) } : entry) } } });
    assert.equal(handoffLink(document) === null, true);
    await rerender({ receipt, blocked: true });
    assert.equal(handoffLink(document) === null, true);
  }, { taskHandoff });
});


function remoteTaskMetadata(r) {
  return { protocol: 1, guide_id: guide.id, version: guide.version, guide_version: r.guide_version,
    deployment_id: r.deployment_id, build_id: r.build_id, accepted_source_digest: r.accepted_source_digest,
    source_digest: r.accepted_source_digest, package_sha256: r.package_sha256,
    transferred_source_digest: r.transferred_source_digest, controller_sha256: r.controller_sha256,
    remote_binding: JSON.parse(remoteInstalledUseBinding(r, guide)), scope: "frozen-remote-package-source",
    current: true, status: r.status, project: { name: "Workshop" }, entrypoint: { identity: "lead:main", name: "Workshop owner" },
    first_use: { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64), entry_id: "workshop",
      tasks: [{ label: "Plan my first job", request: "Suggested editable job" }], detail: null },
    observation: { scope: "saved-destination-observation", status: r.status, available: true, freshness: "fresh",
      admitted: true, source_current: true }, installed_source_verified: true, native_runtime_available: true,
    application_outcomes_verified: false, continuation: { build_id: build.build_id, source_digest: build.source_digest,
      available: false, requires_acknowledgement: false } };
}

test("mounted RemoteDeployment joins its current draft to its sibling checked tunnel without an automatic send", async () => {
  await mountedAccess({ workshop: ready(entries[0]), admin: ready(entries[1]) }, async ({ document, window, calls, rerender }) => {
    assert.equal(handoffLink(document) === null, true);
    await act(async () => document.querySelector(".deployment-use-ideas button").click());
    assert.equal(linkedDraft(handoffLink(document)), "Suggested editable job");
    const task = "  The actual edited remote task \u{1f9ed}\nwith my limits.  ";
    const field = document.querySelector(".deployment-use-field textarea");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(field, task); field.dispatchEvent(new window.Event("input", { bubbles: true })); });
    assert.equal(linkedDraft(handoffLink(document)), task);
    assert.equal(handoffLink(document, "admin") === null, true);
    await rerender({ blocked: true });
    assert.equal(handoffLink(document) === null, true);
    assert.equal(field.value, task);
    await rerender({ blocked: false });
    assert.equal(linkedDraft(handoffLink(document)), task);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.equal(calls.some((call) => new URL(call.target, "http://localhost").searchParams.has("task")), false);
    assert.match(document.body.textContent, /application UI, sign-in, phone access and external workflows have not been verified/);
  }, { mountDeployment: true });
});


test("refreshed custom UI links survive harmless chat and retain exact listener custody", async () => {
  const entry = entries[2];
  await mountedAccess({ reports: ready(entry) }, async ({ calls, link, rerender, setAccess, poll }) => {
    assert.equal(link("reports").getAttribute("href"), "http://127.0.0.1:12007/reports/daily");
    const next = { ...guide, version: 5 }, currentBuild = { ...build, current_guide_version: 5 };
    const currentReceipt = { ...receipt, current_guide_version: 5 };
    await rerender({ state: next, build: currentBuild, receipt: currentReceipt,
      options: { ...options, current_guide_version: 5 } });
    assert.equal(link("reports") === null, true);
    setAccess("reports", { ...ready(entry), current_guide_version: 5 }); await poll();
    assert.equal(link("reports").getAttribute("href"), "http://127.0.0.1:12007/reports/daily");
    for (const patch of [{ current_guide_version: undefined }, { current_guide_version: null },
      { current_guide_version: "5" }, { listener_token: "1".repeat(64) }, { metadata_sha256: "1".repeat(64) },
      { package_sha256: "1".repeat(64) }, { guide_version: 5 }])
      assert.equal(remoteApplicationReplyCurrent({ ...ready(entry), current_guide_version: 5, ...patch },
        currentReceipt, next, currentBuild, experiences, entry), false);
    await rerender({ receipt: { ...currentReceipt, source_binding_current: false } });
    assert.equal(link("reports") === null, true);
    assert.equal(calls.some(call => call.method === "POST"), false);
  });
});
