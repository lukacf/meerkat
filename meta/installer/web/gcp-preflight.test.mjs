import assert from "node:assert/strict";
import test from "node:test";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { createHash, webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import { createGcpPreflightGate, gcpPreflightAvailable, gcpPreflightScope, gcpPreflightUrl, gcpBindingMatches,
  gcpOptionsValid, gcpProjectsValid, gcpCheckValid, gcpCheckFresh, GCP_PREFLIGHT_FRESH_MS,
  gcpProjectFeedbackCurrent, gcpProjectBlueprint, gcpProjectBlueprintMatches } from "./gcp-preflight.mjs";

const state = { id: "synthetic-cloud-guide", version: 4, proposal: { target: "gcp", project: { name: "Observatory", slug: "observatory" },
  gcp: { project: "agent-project", zone: "europe-north1-a", machine_type: "e2-medium", disk_gb: 30 },
  agent_classes: [{ id: "astronomer", context: "Private observing plans" }], connections: [] } };
const build = { guide_id: state.id, guide_version: 3, current_guide_version: 4, build_id: "synthetic-cloud-build",
  status: "completed", current: true, ready_to_package: true, acceptance_current: true, acceptance_trusted: true,
  acceptance: { status: "pass" }, source_digest: "a".repeat(64), blueprint_hash: "b".repeat(64) };
const packageBytes = Buffer.from("Synthetic accepted cloud package bytes");
const binding = { guide_id: state.id, guide_version: state.version, build_id: build.build_id,
  source_digest: build.source_digest, blueprint_hash: build.blueprint_hash, package_sha256: createHash("sha256").update(packageBytes).digest("hex") };
const destination = { ...state.proposal.gcp, instance: "observatory", console_port: 8089 };
const check = { id: "compute", label: "Compute Engine", status: "checked", required: true, detail: "Synthetic verified access." };
const base = () => ({ protocol: 1, binding: structuredClone(binding), destination: structuredClone(destination), checked_at: new Date().toISOString(), checks: [structuredClone(check)] });
const accounts = [{ id: "first@example.test", label: "First account", active: true }, { id: "second@example.test", label: "Second account", active: false }];
const options = () => ({ ...base(), cli: { available: true, status: "checked", detail: "Synthetic CLI present." }, accounts: structuredClone(accounts), truncated: false });
const projects = (account = accounts[0].id) => ({ ...base(), account, projects: [{ id: "agent-project", label: "Observatory" }, { id: "other-project", label: "Another project" }], truncated: false });
const result = (account = accounts[0].id, project = "agent-project") => ({ ...base(), account, project, ready_for_plan: true, provisioning_verified: false,
  plan: { title: "Deploy Observatory", account, destination: structuredClone(destination), resources: ["One owned virtual machine"],
    access: ["IAP access"], mutations: ["Create the proposed computer"], commands: ["synthetic-command"], limitations: ["Application startup needs live verification."] } });

test("cloud preflight requires the current independently accepted GCP source", () => {
  assert.equal(gcpPreflightAvailable(state, build), true);
  for (const change of [{ current: false }, { acceptance_trusted: false }, { acceptance_current: false }, { status: "running" },
    { source_digest: null }, { blueprint_hash: "invalid" }, { cleanup_pending: true }, { current_guide_version: 3 }])
    assert.equal(gcpPreflightAvailable(state, { ...build, ...change }), false);
  assert.equal(gcpPreflightAvailable(state, build, true), false);
  assert.equal(gcpPreflightAvailable({ ...state, proposal: { ...state.proposal, target: "linux" } }, build), false);
  assert.equal(gcpPreflightUrl("guide /1", "check"), "/api/guide/guide%20%2F1/gcp-preflight/check");
});

test("every cloud observation is bound to transcript, accepted source and exact package", () => {
  assert.equal(gcpBindingMatches(options(), state, build), true);
  for (const key of Object.keys(binding)) {
    const value = options(); value.binding[key] = key === "guide_version" ? 3 : "d".repeat(64);
    assert.equal(gcpBindingMatches(value, state, build, binding), false, key);
  }
  assert.equal(gcpBindingMatches({ ...options(), protocol: 2 }, state, build), false);
  assert.equal(gcpBindingMatches({ ...options(), checked_at: "unobserved" }, state, build), false);
  assert.equal(gcpBindingMatches(options(), state, { ...build, package_sha256: "d".repeat(64) }), false);
  assert.notEqual(gcpPreflightScope(state, build), gcpPreflightScope(state, { ...build, source_digest: "d".repeat(64) }));
  assert.notEqual(gcpPreflightScope(state, build), gcpPreflightScope({ ...state, proposal: { ...state.proposal, gcp: { ...state.proposal.gcp, project: "other-project" } } }, build));
});

test("returning to the same cloud source cannot revive an old request", () => {
  const gate = createGcpPreflightGate(); gate.observe("A"); const token = gate.capture();
  gate.observe("B"); gate.observe("A"); assert.equal(gate.current(token), false);
  const selection = gate.capture(); gate.invalidate(); gate.invalidate(); assert.equal(gate.current(selection), false);
  const latest = gate.capture(); gate.close(); assert.equal(gate.current(latest), false);
});

test("blocked and unknown checks remain observations and never become a ready plan", () => {
  assert.equal(gcpOptionsValid(options()), true); assert.equal(gcpProjectsValid(projects(), accounts[0].id), true);
  for (const status of ["blocked", "unknown"]) {
    const value = { ...result(), checks: [{ ...check, status, action: "Ask the project administrator." }], ready_for_plan: false, plan: null };
    assert.equal(gcpCheckValid(value, accounts[0].id, "agent-project"), true);
    assert.equal(gcpCheckValid({ ...value, ready_for_plan: true, plan: result().plan }, accounts[0].id, "agent-project"), false);
  }
  assert.equal(gcpCheckValid(result(), accounts[0].id, "agent-project"), true);
  assert.equal(gcpCheckValid({ ...result(), provisioning_verified: true }, accounts[0].id, "agent-project"), false);
  assert.equal(gcpCheckValid({ ...result(), checks: [check, check] }, accounts[0].id, "agent-project"), false);
  assert.equal(gcpProjectsValid({ ...projects(), projects: [{ id: "-unsafe-project", label: "Invalid" }] }, accounts[0].id), false);
});

test("a cloud project choice changes only gcp.project and retains all agent context", () => {
  const before = structuredClone(state.proposal), changed = gcpProjectBlueprint(before, "other-project");
  assert.equal(changed.gcp.project, "other-project"); assert.equal(before.gcp.project, "agent-project");
  assert.deepEqual(changed.agent_classes, before.agent_classes);
  assert.equal(gcpProjectBlueprintMatches(before, changed, "other-project"), true);
  assert.equal(gcpProjectBlueprintMatches(before, { ...changed, connections: ["unexpected"] }, "other-project"), false);
  assert.equal(gcpProjectBlueprint(before, "-unsafe-project"), null);
  const expected = { scope: gcpPreflightScope(state, build), project: "other-project" };
  assert.equal(gcpProjectFeedbackCurrent(expected, state, build), true);
  assert.equal(gcpProjectFeedbackCurrent(expected, state, { ...build, source_digest: "d".repeat(64) }), false);
  assert.equal(gcpProjectFeedbackCurrent({ ...expected, project: "agent-project" }, state, build), false);
});

test("cloud readiness expires after five minutes and future observations are not fresh", () => {
  const now = Date.now(), value = { ...result(), checked_at: new Date(now).toISOString() };
  assert.equal(gcpCheckFresh(value, now + GCP_PREFLIGHT_FRESH_MS - 1), true);
  assert.equal(gcpCheckFresh(value, now + GCP_PREFLIGHT_FRESH_MS), false);
  assert.equal(gcpCheckFresh({ ...value, checked_at: new Date(now + 60000).toISOString() }, now), false);
});

let compiled;
async function component() {
  if (compiled) return compiled;
  const require = createRequire(import.meta.url);
  const built = await bundle({ stdin: { contents: 'export { GcpPreflight } from "./gcp-preflight.jsx"; export { DeploymentPanel } from "./deployment-panel.jsx";',
    resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "synthetic-cloud-component-entry.mjs" },
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
    } }] });
  compiled = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  return compiled;
}

async function mounted(verify, integrated = false) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], feedback = [], downloads = [], timers = new Map(), savedPackages = [], urls = new Map();
  const originalSetTimeout = globalThis.setTimeout, originalClearTimeout = globalThis.clearTimeout;
  let handlers = { options, projects: (body) => projects(body.account), check: (body) => result(body.account, body.project) };
  let packageHandler = () => new Blob([packageBytes]);
  let digestHandler = (algorithm, bytes) => webcrypto.subtle.digest(algorithm, bytes);
  let selectedState = state, selectedBuild = build, blocked = false, nextTimer = 0;
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    localStorage: dom.window.localStorage,
    crypto: { randomUUID: () => "33333333-3333-4333-8333-333333333333", subtle: { digest: (...args) => digestHandler(...args) } },
    setTimeout(callback, delay, ...args) {
      // Held: the polls, the ordinary 30 s read deadline and every timer of 100 s or more.
      if (delay < 100000 && delay !== 1500 && delay !== 2000 && delay !== 30000) return originalSetTimeout(callback, delay, ...args);
      const token = { cloudFreshness: ++nextTimer }; timers.set(token, { callback: () => callback(...args), delay }); return token;
    },
    clearTimeout(token) { if (!timers.delete(token)) originalClearTimeout(token); },
    fetch: async (target, init = {}) => {
      const method = init.method || "GET", body = init.body ? JSON.parse(init.body) : null;
      calls.push({ target, method, body });
      if (integrated && target === `/api/guide/${state.id}/implementation/package?build_id=${build.build_id}`)
        return { ok: true, blob: async () => packageHandler() };
      if (integrated && target === `/api/guide/${state.id}/deployment`) return { ok: false, status: 404, json: async () => ({ detail: "No saved installation." }) };
      if (integrated && target === `/api/guide/${state.id}/remote-deployment`) return { ok: false, status: 404, json: async () => ({ detail: "No saved remote installation." }) };
      if (integrated && target === `/api/guide/${state.id}/gcp-deployment`) return { ok: true, json: async () => null };
      if (integrated && target === `/api/guide/${state.id}/deployment/options?build_id=${build.build_id}`) return { ok: true, json: async () => ({
        build_id: build.build_id, guide_version: build.guide_version, current_guide_version: state.version,
        current: true, target: "gcp", host_target: "mac", supported: false,
        handoff: { mode: "remote-package", status: "not-connected", steps: [{ title: "Manual deployment", description: "Synthetic manual instruction.", where: "This computer", commands: ["synthetic-manual-command"] }], limitations: ["Live startup is unverified."],
          package_url: `/api/guide/${state.id}/implementation/package?build_id=${build.build_id}` },
      }) };
      const action = target.match(/gcp-preflight\/(options|projects|check-guided|check)/)?.[1];
      assert.ok(action, `Unexpected synthetic cloud request: ${method} ${target}`);
      const value = await handlers[action === "check-guided" ? "check" : action](body);
      if (action === "check-guided") value.guided_supported = true;
      return { ok: true, json: async () => structuredClone(value) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const previousObjectUrl = URL.createObjectURL, previousRevokeUrl = URL.revokeObjectURL;
  URL.createObjectURL = (blob) => { const url = `blob:synthetic-${urls.size}`; urls.set(url, blob); return url; };
  URL.revokeObjectURL = (url) => urls.delete(url);
  dom.window.HTMLAnchorElement.prototype.click = function () { savedPackages.push(urls.get(this.href)); };
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client"), Components = await component();
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(integrated ? React.createElement(Components.DeploymentPanel, {
    state: selectedState, build: selectedBuild, plan: selectedState.proposal, designBusy: blocked, pendingManual: false,
    Icon: () => React.createElement("span"), onChooseCloudProject: async (value) => feedback.push(value),
  }) : React.createElement(Components.GcpPreflight, { state: selectedState, build: selectedBuild, blocked,
    onChooseProject: async (value) => feedback.push(value), onDownload: (value) => downloads.push(value) })));
  const document = dom.window.document;
  const button = (label) => [...document.querySelectorAll("button")].find((node) => node.textContent === label || node.querySelector("strong")?.textContent === label);
  const click = (label) => act(async () => { assert.ok(button(label), `Missing cloud action: ${label}`); button(label).click(); });
  const pendingClick = (label) => act(() => { assert.ok(button(label)); button(label).click(); });
  const manualDownload = () => act(async () => {
    const control = [...document.querySelectorAll(".remote-manual-deployment button")].find(node => node.textContent.includes("Download accepted GCP package"));
    assert.ok(control, "The optional Advanced download must exist."); control.click();
  });
  async function waitFor(predicate) {
    for (let tries = 0; tries < 100 && !predicate(); tries++)
      await act(async () => new Promise((done) => originalSetTimeout(done, 5)));
    assert.ok(predicate(), "The controlled download reached its observable terminal result.");
  }
  async function enter(value) {
    const input = document.querySelector('[name="gcp_project_id"]'); assert.ok(input);
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set;
    await act(async () => { setter.call(input, value); input.dispatchEvent(new dom.window.Event("input", { bubbles: true })); });
  }
  try {
    await render();
    await verify({ document, button, click, pendingClick, manualDownload, enter, calls, feedback, downloads, timers, savedPackages, waitFor,
      setHandler: (name, handler) => { handlers[name] = handler; },
      setPackageHandler: (handler) => { packageHandler = handler; },
      setDigestHandler: (handler) => { digestHandler = handler; },
      change: async (nextState = state, nextBuild = build, nextBlocked = false) => { selectedState = nextState; selectedBuild = nextBuild; blocked = nextBlocked; await render(); } });
  } finally {
    await act(async () => root.unmount()); assert.equal(timers.size, 0, "Unmount clears the readiness expiry timer.");
    dom.window.close(); for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
    URL.createObjectURL = previousObjectUrl; URL.revokeObjectURL = previousRevokeUrl;
  }
}

async function chooseDestination(ui) { await ui.click("Check cloud access"); await ui.click("First account"); await ui.click("Observatory"); }

test("the cloud flow reads explicit account/project choices and exposes a bound preview plus package download", async () => {
  await mounted(async (ui) => {
    assert.equal(ui.calls.length, 0); await chooseDestination(ui); await ui.click("Check destination");
    assert.match(ui.document.body.textContent, /Your destination plan is ready/);
    assert.match(ui.document.body.textContent, /Provisioning and application startup remain unverified/);
    assert.equal(ui.document.querySelector(".gcp-preflight-plan details").open, false);
    assert.deepEqual(ui.calls.map(({ method, body }) => ({ method, body })), [{ method: "GET", body: null },
      { method: "POST", body: { version: 4, build_id: build.build_id, account: accounts[0].id } },
      { method: "POST", body: { version: 4, build_id: build.build_id, account: accounts[0].id, project: "agent-project" } }]);
    await ui.click("Download accepted GCP package"); assert.equal(ui.downloads.length, 1);
    assert.deepEqual(ui.downloads[0].binding, binding); assert.equal(ui.downloads[0].isCurrent(), true);
    assert.equal(ui.document.defaultView.localStorage.length, 0); assert.equal(ui.document.defaultView.sessionStorage.length, 0);
  });
});

test("unknown destination checks show useful blockers without a plan or download", async () => {
  await mounted(async (ui) => {
    ui.setHandler("check", (body) => ({ ...result(body.account), ready_for_plan: false, plan: null,
      checks: [{ ...check, status: "unknown", detail: "Billing could not be observed.", action: "Ask the project owner to check billing." }] }));
    await chooseDestination(ui); await ui.click("Check destination");
    assert.match(ui.document.body.textContent, /Billing could not be observed/); assert.match(ui.document.body.textContent, /Not verified/);
    assert.match(ui.document.body.textContent, /Ask the project owner/); assert.equal(ui.document.querySelector(".gcp-preflight-plan") === null, true);
    assert.equal(ui.button("Download accepted GCP package") === undefined, true); assert.equal(ui.document.querySelector('[role="alert"]') === null, true);
  });
});

test("a CLI missing at Launch startup keeps the required restart step visible after a page retry", async () => {
  await mounted(async (ui) => {
    ui.setHandler("options", () => ({ ...options(), cli: { available: false, status: "blocked", detail: "Google Cloud CLI was not detected when Launch started." }, accounts: [],
      checks: [{ ...check, status: "blocked", label: "Cloud CLI", detail: "Google Cloud CLI was not detected when Launch started.",
        action: "Install Google Cloud CLI on this computer and sign in, then restart Launch and reopen this saved setup before checking cloud access again." }] }));
    await ui.click("Check cloud access"); assert.match(ui.document.body.textContent, /Install Google Cloud CLI/);
    assert.match(ui.document.body.textContent, /restart Launch and reopen this saved setup/);
    assert.equal(ui.button("First account") === undefined, true);
    await ui.click("Check cloud access again");
    assert.equal(ui.button("First account") === undefined, true);
    assert.match(ui.document.body.textContent, /restart Launch and reopen this saved setup/);
    assert.equal(ui.calls.filter((call) => call.target.includes("gcp-preflight/options")).length, 2);
  });
});

// The server bounds one whole cloud access observation at PROBE_TIMEOUT. It
// reads and checks the accepted package before and after the signed-in account
// reads, refuses another probe until this one ends, and past the bound answers
// with its own refusal. The page's read must stay open beyond it, yet stay bounded.
function serverCloudProbeMs() {
  const source = readFileSync(new URL("../launchkit/gcp_preflight.py", import.meta.url), "utf8");
  return 1000 * Number(source.match(/^PROBE_TIMEOUT = (\d+)$/m)?.[1]);
}

test("a slow cloud access check keeps its read open past the ordinary 30 s deadline until the server's bounded probe answers", async () => {
  const bound = serverCloudProbeMs();
  assert.equal(Number.isFinite(bound) && bound > 30000, true, `The server's cloud probe bound reads as ${bound} ms.`);
  await mounted(async (ui) => {
    let answer; ui.setHandler("options", () => new Promise((done) => { answer = done; }));
    ui.pendingClick("Check cloud access");
    try {
      const deadlines = [...ui.timers.values()].map(({ delay }) => delay);
      assert.equal(deadlines.length, 1, "The cloud access read holds one deadline.");
      assert.equal(deadlines[0] > bound, true, `The cloud access read deadline (${deadlines[0]} ms) outlasts the server's ${bound} ms probe bound.`);
      // The server's bound passes: every held deadline at or under it fires.
      await act(async () => { for (const [token, timer] of [...ui.timers]) if (timer.delay <= bound) { ui.timers.delete(token); timer.callback(); } });
      assert.equal(ui.document.querySelector('[role="alert"]') === null, true, "No read timeout is shown while the server's probe can still answer.");
      assert.equal(ui.button("Checking cloud access...") !== undefined, true, "The cloud access check is still waiting.");
    } finally { await act(async () => answer(options())); }
    assert.equal(ui.button("First account") !== undefined, true, "The late answer shows its accounts.");
    assert.equal(ui.document.querySelector('[role="alert"]') === null, true);
    assert.equal(ui.timers.size, 0, "The answered read leaves no deadline behind.");
  });
});

test("a cloud access read the server never answers still ends at its explicit deadline and can be checked again", async () => {
  await mounted(async (ui) => {
    let reads = 0; ui.setHandler("options", () => ++reads === 1 ? new Promise(() => {}) : options());
    ui.pendingClick("Check cloud access");
    const held = [...ui.timers];
    try {
      assert.equal(held.length, 1, "The cloud access read holds one deadline.");
      assert.equal(Number.isFinite(held[0][1].delay), true);
    } finally { await act(async () => { for (const [token, timer] of held) { ui.timers.delete(token); timer.callback(); } }); }
    assert.match(ui.document.querySelector('[role="alert"]')?.textContent ?? "", /taking too long to respond/);
    assert.equal(ui.button("Check cloud access")?.disabled, false, "The cloud access check can run again.");
    await ui.click("Check cloud access");
    assert.equal(reads, 2); assert.equal(ui.button("First account") !== undefined, true);
    assert.equal(ui.document.querySelector('[role="alert"]') === null, true, "The new read clears the timeout message.");
  });
});

test("an incomplete project list preserves exact project ID choice and the proposed destination", async () => {
  await mounted(async (ui) => {
    ui.setHandler("projects", (body) => ({ ...projects(body.account), projects: [], truncated: true, checks: [{ ...check, status: "unknown", detail: "Project listing is incomplete." }] }));
    await ui.click("Check cloud access"); await ui.click("First account");
    assert.ok(ui.button("Use the proposed project")); await ui.enter("other-project"); await ui.click("Choose this project");
    ui.setHandler("check", (body) => ({ ...result(body.account, body.project), ready_for_plan: false, plan: null,
      checks: [{ ...check, id: "project-match", status: "blocked", detail: "This package targets a different project." }] }));
    await ui.click("Check destination"); assert.match(ui.document.body.textContent, /requires a fresh accepted build/);
    await ui.click("Use this project"); assert.equal(ui.feedback.length, 1); assert.equal(ui.feedback[0].project, "other-project");
    assert.equal(Object.hasOwn(ui.feedback[0], "account"), false); assert.equal(ui.feedback[0].isCurrent(), true);
    await ui.click("Second account"); assert.equal(ui.feedback[0].isCurrent(), false);
  });
});

test("changing project clears ready output synchronously and A-B-A cannot revive a late check", async () => {
  await mounted(async (ui) => {
    let resolve; ui.setHandler("check", () => new Promise((done) => { resolve = done; }));
    await chooseDestination(ui); ui.pendingClick("Check destination");
    await ui.click("Another project"); await ui.click("Observatory");
    await act(async () => resolve(result()));
    assert.equal(ui.document.querySelector(".gcp-preflight-result") === null, true); assert.ok(ui.button("Check destination"));
    ui.setHandler("check", (body) => result(body.account, body.project)); await ui.click("Check destination"); assert.ok(ui.document.querySelector(".gcp-preflight-plan"));
    ui.pendingClick("Another project"); assert.equal(ui.document.querySelector(".gcp-preflight-plan") === null, true);
  });
});

test("changing account twice rejects the first account's late project response", async () => {
  await mounted(async (ui) => {
    let resolve, reads = 0; ui.setHandler("projects", (body) => ++reads === 1 ? new Promise((done) => { resolve = done; }) : { ...projects(body.account), projects: [] });
    await ui.click("Check cloud access"); ui.pendingClick("First account"); await ui.click("Second account"); await ui.click("First account");
    await act(async () => resolve(projects()));
    assert.equal(ui.button("Observatory") === undefined, true); assert.ok(ui.button("Use the proposed project"));
  });
});

test("source or guide changes invalidate delayed cloud observations even after returning to the same source", async () => {
  for (const [nextState, nextBuild, blocked] of [[state, { ...build, source_digest: "d".repeat(64) }, false],
    [{ ...state, version: 5 }, { ...build, current_guide_version: 5 }, false], [state, build, true]]) {
    await mounted(async (ui) => {
      let resolve; ui.setHandler("check", () => new Promise((done) => { resolve = done; }));
      await chooseDestination(ui); ui.pendingClick("Check destination"); await ui.change(nextState, nextBuild, blocked); await ui.change();
      await act(async () => resolve(result()));
      assert.equal(ui.document.querySelector(".gcp-preflight-result") === null, true); assert.ok(ui.button("Check cloud access"));
    });
  }
});

test("a delayed failed read cannot replace the current account choice with an error", async () => {
  await mounted(async (ui) => {
    let reject; ui.setHandler("projects", (body) => body.account === accounts[0].id ? new Promise((_, fail) => { reject = fail; }) : projects(body.account));
    await ui.click("Check cloud access"); ui.pendingClick("First account"); await ui.click("Second account");
    await act(async () => reject(new Error("Old account request failed.")));
    assert.equal(ui.document.querySelector('[role="alert"]') === null, true); assert.ok(ui.button("Observatory"));
  });
});

test("a response for another accepted package is rejected and no plan is shown", async () => {
  await mounted(async (ui) => {
    ui.setHandler("check", () => ({ ...result(), binding: { ...binding, package_sha256: "d".repeat(64) } }));
    await chooseDestination(ui); await ui.click("Check destination");
    assert.match(ui.document.querySelector('[role="alert"]').textContent, /do not match this accepted package/);
    assert.equal(ui.document.querySelector(".gcp-preflight-plan") === null, true);
  });
});

test("expiry removes readiness while keeping the observed checks and their capture time", async () => {
  await mounted(async (ui) => {
    const checkedAt = Date.now(); ui.setHandler("check", () => ({ ...result(), checked_at: new Date(checkedAt).toISOString() }));
    await chooseDestination(ui); await ui.click("Check destination"); assert.ok(ui.document.querySelector(".gcp-preflight-plan"));
    const [token, timer] = ui.timers.entries().next().value; ui.timers.delete(token);
    const originalNow = Date.now;
    try { Date.now = () => checkedAt + GCP_PREFLIGHT_FRESH_MS; await act(async () => timer.callback());
      assert.equal(ui.document.querySelector(".gcp-preflight-plan") === null, true); assert.match(ui.document.body.textContent, /These checks need a refresh/);
      assert.match(ui.document.body.textContent, /Synthetic verified access/); assert.match(ui.document.body.textContent, /Checked/);
      assert.equal(ui.button("Download accepted GCP package") === undefined, true);
    } finally { Date.now = originalNow; }
  });
});

test("the production deployment panel makes guided cloud setup primary and keeps manual commands collapsed", async () => {
  await mounted(async (ui) => {
    const manual = ui.document.querySelector(".remote-manual-deployment");
    assert.ok(manual); assert.equal(manual.open, false);
    assert.match(manual.querySelector("summary").textContent, /Manual Google Cloud.*advanced/);
    assert.ok(ui.button("Check cloud access"));
    assert.equal(ui.document.querySelector(".gcp-preflight-next") === null, true);
    await chooseDestination(ui); await ui.click("Check destination");
    const next = ui.document.querySelector(".gcp-preflight-next button");
    assert.equal(next.textContent, "Create this cloud computer"); assert.equal(next.closest("details") === null, true); assert.equal(next.disabled, false);
    assert.ok(ui.calls.some(call => call.target.endsWith("/check-guided")));
    assert.ok([...ui.document.querySelectorAll(".gcp-guided button")].every(node => !node.textContent.includes("Download accepted GCP package")));
    assert.equal([...manual.querySelectorAll("button")].filter(node => node.textContent.includes("Download accepted GCP package")).length, 1);
    assert.equal(manual.open, false);
  }, true);
});

test("an unavailable account inventory never claims no signed-in account exists", async () => {
  await mounted(async (ui) => {
    ui.setHandler("options", () => ({ ...options(), accounts: [], checks: [{ ...check, id: "accounts", status: "unknown", detail: "Account listing was unavailable." }] }));
    await ui.click("Check cloud access"); assert.match(ui.document.body.textContent, /Account listing was unavailable/);
    assert.doesNotMatch(ui.document.body.textContent, /No signed-in account/); assert.match(ui.document.body.textContent, /No account choices are available from this check/);
  });
});

test("rapid duplicate clicks share each explicit cloud read without retrying it", async () => {
  await mounted(async (ui) => {
    let releaseOptions, releaseProjects, releaseCheck;
    ui.setHandler("options", () => new Promise((done) => { releaseOptions = done; }));
    await act(() => { ui.button("Check cloud access").click(); ui.button("Check cloud access").click(); });
    assert.equal(ui.calls.length, 1); await act(async () => releaseOptions(options()));
    ui.setHandler("projects", () => new Promise((done) => { releaseProjects = done; }));
    await act(() => { ui.button("First account").click(); ui.button("First account").click(); });
    assert.equal(ui.calls.length, 2); await act(async () => releaseProjects(projects()));
    await ui.click("Observatory");
    ui.setHandler("check", () => new Promise((done) => { releaseCheck = done; }));
    await act(() => { ui.button("Check destination").click(); ui.button("Check destination").click(); });
    assert.equal(ui.calls.length, 3); await act(async () => releaseCheck(result()));
    assert.ok(ui.document.querySelector(".gcp-preflight-plan"));
  });
});

test("editing a project ID discards an old ready result before another project is submitted", async () => {
  await mounted(async (ui) => {
    await chooseDestination(ui); await ui.click("Check destination"); assert.ok(ui.document.querySelector(".gcp-preflight-plan"));
    await ui.enter("other-project"); assert.equal(ui.document.querySelector(".gcp-preflight-plan") === null, true);
    assert.equal(ui.button("Download accepted GCP package") === undefined, true); assert.equal(ui.button("Check destination") === undefined, true);
    assert.equal(ui.calls.length, 3); await ui.click("Choose this project"); assert.ok(ui.button("Check destination")); assert.equal(ui.calls.length, 3);
  });
});

test("a ready plan cannot claim a project or resource destination outside the accepted package", () => {
  assert.equal(gcpCheckValid({ ...result(), project: "other-project", plan: { ...result().plan, destination: { ...destination, project: "other-project" } } }, accounts[0].id, "other-project"), false);
  assert.equal(gcpCheckValid({ ...result(), plan: { ...result().plan, destination: { ...destination, zone: "different-zone" } } }, accounts[0].id, "agent-project"), false);
  assert.equal(gcpBindingMatches({ ...options(), destination: { ...destination, project: "other-project" } }, state, build), false);
});

test("the parent Advanced download uses the exact accepted build URL and preserves the returned package bytes", async () => {
  await mounted(async (ui) => {
    await ui.manualDownload();
    await ui.waitFor(() => ui.savedPackages.length === 1);
    assert.equal(createHash("sha256").update(Buffer.from(await ui.savedPackages[0].arrayBuffer())).digest("hex"), binding.package_sha256);
    assert.deepEqual(ui.calls.filter((call) => call.target.includes("/implementation/package?")), [{
      target: `/api/guide/${state.id}/implementation/package?build_id=${build.build_id}`, method: "GET", body: null,
    }]);
    assert.equal(ui.calls.filter(call => call.target.includes("/gcp-deployment/")).length, 0);
  }, true);
});

test("the standalone checked callback retains the exact observed package digest and no private account", async () => {
  await mounted(async (ui) => {
    await chooseDestination(ui); await ui.click("Check destination"); await ui.click("Download accepted GCP package");
    assert.equal(ui.downloads.length, 1); assert.deepEqual(ui.downloads[0].binding, binding);
    assert.equal(ui.downloads[0].binding.package_sha256, binding.package_sha256);
    assert.equal(Object.hasOwn(ui.downloads[0], "account"), false);
    assert.equal(ui.downloads[0].isCurrent(), true);
    assert.equal(ui.savedPackages.length, 0, "A standalone callback is not evidence of a downloaded file.");
  });
});

test("account and project reversals retire the standalone checked callback even after returning to the same choices", async () => {
  for (const change of [async (ui) => { await ui.click("Second account"); await ui.click("First account"); },
    async (ui) => { await ui.click("Another project"); await ui.click("Observatory"); }]) {
    await mounted(async (ui) => {
      await chooseDestination(ui); await ui.click("Check destination"); await ui.click("Download accepted GCP package");
      const selected = ui.downloads[0]; assert.ok(selected); assert.equal(selected.isCurrent(), true);
      await change(ui);
      assert.equal(selected.isCurrent(), false); assert.deepEqual(selected.binding, binding);
      assert.equal(ui.savedPackages.length, 0); assert.equal(ui.document.querySelector(".gcp-preflight-plan") === null, true);
    });
  }
});

test("source reversals and check expiry retire the standalone checked callback without changing its original digest", async () => {
  for (const retire of [async (ui) => { await ui.change(state, { ...build, source_digest: "d".repeat(64) }); await ui.change(); },
    async (ui, checkedAt) => { const [token, timer] = ui.timers.entries().next().value; ui.timers.delete(token);
      Date.now = () => checkedAt + GCP_PREFLIGHT_FRESH_MS; await act(async () => timer.callback()); }]) {
    const originalNow = Date.now;
    try {
      await mounted(async (ui) => {
        const checkedAt = originalNow();
        ui.setHandler("check", () => ({ ...result(), checked_at: new Date(checkedAt).toISOString() }));
        await chooseDestination(ui); await ui.click("Check destination"); await ui.click("Download accepted GCP package");
        const selected = ui.downloads[0]; assert.ok(selected); assert.equal(selected.isCurrent(), true);
        await retire(ui, checkedAt);
        assert.equal(selected.isCurrent(), false); assert.deepEqual(selected.binding, binding);
        assert.equal(ui.savedPackages.length, 0);
      });
    } finally { Date.now = originalNow; }
  }
});

test("source reversal while the Advanced package is arriving prevents saving a retired accepted build", async () => {
  await mounted(async (ui) => {
    let release; ui.setPackageHandler(() => new Promise(done => { release = done; }));
    await ui.manualDownload(); assert.equal(typeof release, "function");
    await ui.change(state, { ...build, source_digest: "d".repeat(64) }); await ui.change();
    await act(async () => release(new Blob([packageBytes])));
    assert.equal(ui.savedPackages.length, 0);
    assert.equal(ui.calls.filter(call => call.target.includes("/implementation/package?")).length, 1);
  }, true);
});

test("the optional manual handoff download keeps its ordinary button event separate from checked binding", async () => {
  await mounted(async (ui) => {
    const manual = ui.document.querySelector(".remote-manual-deployment");
    const button = [...manual.querySelectorAll("button")].find((node) => node.textContent.includes("Download accepted GCP package"));
    assert.ok(button); await act(async () => button.click()); await ui.waitFor(() => ui.savedPackages.length === 1);
  }, true);
});

test("existing-host proposals cannot expose the new-VM provisioner or reuse its result", () => {
  const changed = structuredClone(state);
  changed.proposal.gcp = { ...changed.proposal.gcp, provisioning: "existing_host", existing_instance: "retained-vm" };
  assert.equal(gcpPreflightAvailable(changed, build), false);
  assert.equal(gcpBindingMatches(options(), changed, build), false);
  assert.notEqual(gcpPreflightScope(changed, build), gcpPreflightScope(state, build));
  changed.proposal.gcp.provisioning = "new_vm";
  assert.equal(gcpPreflightAvailable(changed, build), true);
});
