import assert from "node:assert/strict";
import { test } from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import React, { act, useCallback, useState } from "react";
import * as startup from "./guide-startup.mjs";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const compiled = await build({
  entryPoints: [fileURLToPath(new URL("provider-setup.jsx", import.meta.url))],
  bundle: true, write: false, platform: "node", format: "esm", logLevel: "silent",
  loader: { ".css": "empty" }, outfile: "forge-startup-progress.mjs",
  plugins: [{ name: "actual-react", setup(bundler) {
    bundler.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
const javascript = compiled.outputFiles.find(file => file.path.endsWith(".mjs"));
const { ProviderSetup } = await import(`data:text/javascript;base64,${Buffer.from(javascript.contents).toString("base64")}`);

const idea = "  Help our observing club plan nights.\nKeep each member's observing history private.  ";
const privateDiagnostic = "PRIVATE_PROVIDER_SECRET_NEVER_RENDER";
const phases = ["storage_preparation", "gateway_preparation", "native_bootstrap", "identity_warmup", "readiness_check"];
const errorCases = [
  { kind: "storage_preparation", phase: "storage_preparation", meaning: /(?:storage|saved.*(?:data|history|guide)|prepar)/i },
  { kind: "startup_timeout", phase: "identity_warmup", meaning: /(?:time(?:d)?\s*out|longer|too long)/i },
  { kind: "native_refusal", phase: "native_bootstrap", meaning: /(?:refus|reject|runtime.*(?:stop|declin))/i },
  { kind: "startup_failure", phase: "readiness_check", meaning: /(?:could not|fail|not.*ready)/i },
];
const hostStatus = (status, diagnostics = {}) => ({
  status, available: true, startup_retry_required: status === "failed",
  startup_phase: null, startup_error_kind: null, startup_error_code: null, ...diagnostics,
});
const settings = ({ active = false, status = active ? "ready" : "stopped" } = {}) => ({
  providers: ["openai", "anthropic", "gemini"].map(provider => ({
    provider, configured: provider === "openai", source: provider === "openai" ? "saved" : null, editable: !active,
  })),
  effective_provider: "openai", runtime_active: active, runtime_status: status,
  work_active: false, load_error: null, provider_binding_error: null,
});
const ok = value => ({ ok: true, json: async () => value });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};
const detailFor = value => {
  assert.equal(typeof startup.guideStartupDetail, "function", "Startup details use the shared host-observation projection");
  return startup.guideStartupDetail(startup.guideStartupObservation(value));
};

// The real component and request adapter run against controlled responses. Only
// observation clocks are advanced; a Start POST has no synthetic deadline.
async function mounted(read, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://offline.test/" });
  const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
  const realSetInterval = globalThis.setInterval, realClearInterval = globalThis.clearInterval;
  const intervals = new Map(), deadlines = new Map(), calls = [], runtimes = [];
  let root, closes = 0;
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    setTimeout(callback, delay, ...args) {
      if (delay !== 30000) return realSetTimeout(callback, delay, ...args);
      const token = {};
      deadlines.set(token, () => callback(...args));
      return token;
    },
    clearTimeout(token) {
      if (deadlines.has(token)) deadlines.delete(token);
      else realClearTimeout(token);
    },
    setInterval(callback, delay, ...args) {
      if (delay !== 4000) return realSetInterval(callback, delay, ...args);
      const token = {};
      intervals.set(token, () => callback(...args));
      return token;
    },
    clearInterval(token) {
      if (intervals.has(token)) intervals.delete(token);
      else realClearInterval(token);
    },
    fetch: async (url, options = {}) => {
      const call = { url, method: options.method || "GET", signal: options.signal,
        body: options.body ? JSON.parse(options.body) : undefined };
      calls.push(call);
      return read(call);
    },
  };
  const names = [...Object.keys(replacements), "localStorage", "sessionStorage"];
  const previous = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  for (const [name, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, name, { configurable: true, value });
  for (const name of ["localStorage", "sessionStorage"])
    Object.defineProperty(globalThis, name, { configurable: true, get() { throw new Error("Startup progress does not store credentials or drafts."); } });
  const document = dom.window.document;
  const button = label => {
    const node = [...document.querySelectorAll("button")].find(item => item.textContent === label);
    assert.ok(node, `Expected action: ${label}`);
    return node;
  };
  const settle = async () => act(async () => new Promise(done => realSetTimeout(done, 5)));
  const click = async label => { await act(async () => button(label).click()); await settle(); };
  const pollStart = async () => {
    await act(async () => { for (const callback of [...intervals.values()]) callback(); });
    await settle();
  };
  const unmount = async () => {
    await act(async () => root.unmount()); root = null; await settle();
  };
  function Host() {
    const [ready, setReady] = useState(false);
    const onRuntime = useCallback(runtime => { runtimes.push(runtime); setReady(runtime.status === "ready"); }, []);
    return React.createElement(ProviderSetup, { firstRun: true, idea, guideReady: ready,
      onRuntime, onClose: () => { closes += 1; } });
  }
  try {
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root"));
    await act(async () => root.render(React.createElement(Host)));
    await settle();
    await verify({ document, calls, runtimes, button, click, pollStart, settle, unmount,
      deadlines, closes: () => closes });
    assert.equal(dom.window.localStorage.length, 0);
    assert.equal(dom.window.sessionStorage.length, 0);
  } finally {
    if (root) await act(async () => root.unmount());
    for (const token of [...deadlines.keys()]) replacements.clearTimeout(token);
    for (const token of [...intervals.keys()]) replacements.clearInterval(token);
    dom.window.close();
    for (const [name, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
}

function progress(document) {
  const node = document.querySelector(".setup-provider-startup");
  assert.ok(node, "Current startup observation is visible in the provider handoff");
  assert.equal(node.getAttribute("role"), "status");
  assert.ok(node.querySelector("strong")?.textContent.trim());
  assert.ok(node.querySelector("p")?.textContent.trim());
  assert.doesNotMatch(node.textContent, new RegExp(privateDiagnostic));
  return node;
}
function assertDraftAndAdmission(document, calls) {
  assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea,
    "Observation retains the exact unsent draft, including whitespace and line breaks");
  const mutations = calls.filter(call => call.method !== "GET");
  assert.equal(mutations.length, 1, "No status observation repeats Start or sends the idea");
  assert.equal(mutations[0].url, "/api/architect/start");
  assert.equal(mutations[0].method, "POST");
  assert.equal(mutations[0].body, undefined);
  assert.equal(mutations[0].signal, undefined, "The original Start retains its mutation lifetime");
}

test("a pending first Start exposes successive host phases without releasing its action or sending the draft", async () => {
  const original = deferred();
  let observed = hostStatus("starting"), active = false;
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings({ active, status: observed.status }));
    if (call.method === "POST") return { ok: true, json: () => original.promise };
    assert.equal(call.url, "/api/architect/status");
    return ok(observed);
  }, async ({ document, calls, button, click, pollStart, settle, deadlines, closes }) => {
    await click("Start guide");
    assert.equal(deadlines.size, 0, "Start has no observation deadline");
    const labels = new Set();
    for (const phase of phases) {
      observed = hostStatus("starting", { startup_phase: phase });
      await pollStart();
      const block = progress(document), projected = detailFor(observed);
      assert.equal(projected.phase, phase);
      assert.equal(projected.errorKind, null);
      assert.equal(block.querySelector("strong").textContent, projected.title);
      assert.equal(block.querySelector("p").textContent, projected.detail);
      labels.add(projected.title);
      assert.equal(button("Back to my idea").disabled, true);
      assert.equal(document.querySelector(".setup-provider-return") === null, true);
      assertDraftAndAdmission(document, calls);
    }
    assert.equal(labels.size, phases.length, "Different observed preparation stages are distinguishable");
    observed = hostStatus("ready"); active = true;
    await act(async () => original.resolve(observed)); await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(document.querySelector(".setup-provider-startup") === null, true, "Finished startup does not leave an obsolete phase block");
    assert.equal(button("Back to my draft").disabled, false);
    assert.equal(closes(), 0, "Readiness does not navigate or send the draft automatically");
    assertDraftAndAdmission(document, calls);
  });
});

for (const { kind, phase, meaning } of errorCases) {
  test(`a fresh ${kind} startup diagnosis is useful while the original Start remains pending`, async () => {
    const original = deferred();
    const failed = hostStatus("failed", { startup_phase: phase, startup_error_kind: kind,
      startup_error_code: kind === "native_refusal" ? -32000 : null,
      detail: privateDiagnostic, proposal: { private: privateDiagnostic } });
    await mounted(call => {
      if (call.url === "/api/provider-settings") return ok(settings());
      if (call.method === "POST") return { ok: true, json: () => original.promise };
      assert.equal(call.url, "/api/architect/status"); return ok(failed);
    }, async ({ document, calls, button, click, pollStart, settle }) => {
      await click("Start guide"); await pollStart();
      const block = progress(document), projected = detailFor(failed);
      assert.equal(document.querySelector("h2").textContent, "Your guide needs attention",
        "A confirmed failure does not keep the working-startup heading");
      assert.equal(projected.errorKind, kind);
      assert.equal(projected.phase, phase);
      assert.match(block.textContent, meaning);
      assert.doesNotMatch(document.body.textContent, new RegExp(privateDiagnostic));
      assert.equal(button("Back to my idea").disabled, true);
      assert.equal(button("Check guide status").disabled, false);
      assertDraftAndAdmission(document, calls);
      await click("Check guide status");
      assert.equal(progress(document).querySelector("strong").textContent, projected.title,
        "An explicit failed status check retains its current diagnosis");
      assert.equal(button("Starting guide...").disabled, true);
      await click("Starting guide...");
      assertDraftAndAdmission(document, calls);
      await act(async () => original.reject(new Error(privateDiagnostic))); await settle();
      assert.doesNotMatch(document.body.textContent, new RegExp(privateDiagnostic));
      assertDraftAndAdmission(document, calls);
    });
  });
}

test("unknown and malformed startup diagnostics never become user text or readiness", async () => {
  const original = deferred();
  let observed = hostStatus("starting");
  const malformed = [
    { startup_phase: privateDiagnostic, startup_error_kind: privateDiagnostic, startup_error_code: privateDiagnostic },
    { startup_phase: { secret: privateDiagnostic }, startup_error_kind: [privateDiagnostic], startup_error_code: {} },
    { startup_phase: null, startup_error_kind: null },
  ];
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings());
    if (call.method === "POST") return { ok: true, json: () => original.promise };
    assert.equal(call.url, "/api/architect/status"); return ok(observed);
  }, async ({ document, calls, button, click, pollStart, settle }) => {
    await click("Start guide");
    for (const diagnostics of malformed) {
      observed = hostStatus("starting", { ...diagnostics, detail: privateDiagnostic });
      const projected = detailFor(observed);
      if (projected) {
        assert.equal(projected.phase, null); assert.equal(projected.errorKind, null);
        assert.doesNotMatch(JSON.stringify(projected), new RegExp(privateDiagnostic));
      }
      await pollStart();
      assert.doesNotMatch(document.body.textContent, new RegExp(privateDiagnostic));
      assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "starting");
      assert.equal(button("Back to my idea").disabled, true);
      assert.equal(document.querySelector(".setup-provider-return") === null, true);
      assertDraftAndAdmission(document, calls);
    }
    assert.equal(detailFor({ status: "starting", startup_retry_required: "false", startup_phase: "identity_warmup" }), null,
      "A malformed host observation cannot authorize a phase detail");
    assert.equal(detailFor(hostStatus("ready", { startup_phase: "identity_warmup", startup_error_kind: "startup_timeout" })), null,
      "Ready status cannot retain an earlier startup failure");
    await act(async () => original.reject(new Error(privateDiagnostic))); await settle();
    assert.doesNotMatch(document.body.textContent, new RegExp(privateDiagnostic));
  });
});

test("a rejected Start clears an earlier working phase until status can be confirmed", async () => {
  const original = deferred();
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings());
    if (call.method === "POST") return { ok: true, json: () => original.promise };
    assert.equal(call.url, "/api/architect/status");
    return ok(hostStatus("starting", { startup_phase: "identity_warmup" }));
  }, async ({ document, calls, button, click, pollStart, settle }) => {
    await click("Start guide"); await pollStart();
    assert.equal(progress(document).dataset.startupPhase, "identity_warmup");
    await act(async () => original.reject(new Error(privateDiagnostic))); await settle();
    assert.equal(document.querySelector(".setup-provider-startup") === null, true,
      "A past working observation is not current progress after Start becomes unconfirmed");
    assert.match(document.body.textContent, /Starting was not confirmed/);
    assert.equal(document.querySelector(".setup-provider-return") === null, true);
    assert.equal(button("Check saved settings").disabled, false);
    assert.doesNotMatch(document.body.textContent, new RegExp(privateDiagnostic));
    assertDraftAndAdmission(document, calls);
  });
});

test("a failed manual status read clears the older working phase at check admission", async () => {
  const original = deferred(), currentRead = deferred();
  let statusReads = 0;
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings());
    if (call.method === "POST") return { ok: true, json: () => original.promise };
    assert.equal(call.url, "/api/architect/status");
    if (++statusReads === 1) return ok(hostStatus("starting", { startup_phase: "identity_warmup" }));
    return { ok: true, json: () => currentRead.promise };
  }, async ({ document, calls, button, click, pollStart, settle }) => {
    await click("Start guide"); await pollStart();
    assert.equal(progress(document).dataset.startupPhase, "identity_warmup");
    await click("Check guide status");
    assert.equal(document.querySelector(".setup-provider-startup") === null, true,
      "Checking current status removes the superseded working observation before the read settles");
    assert.equal(button("Checking guide status...").disabled, true);
    await act(async () => currentRead.reject(new Error(privateDiagnostic))); await settle();
    assert.equal(document.querySelector(".setup-provider-startup") === null, true);
    assert.match(document.body.textContent, /Guide status could not be confirmed/);
    assert.equal(button("Check guide status").disabled, false);
    assert.equal(button("Back to my idea").disabled, true, "The original Start is still entered");
    assertDraftAndAdmission(document, calls);
    await act(async () => original.resolve(hostStatus("starting", { startup_phase: "native_bootstrap" }))); await settle();
    assert.equal(document.querySelector(".setup-provider-startup") === null, true,
      "The late Start result cannot override the newer failed observation");
    assert.doesNotMatch(document.body.textContent, new RegExp(privateDiagnostic));
    assertDraftAndAdmission(document, calls);
  });
});

test("manual ready observation owns startup presentation over a late background phase and late Start failure", async () => {
  const original = deferred(), oldPoll = deferred();
  let active = false, statusReads = 0;
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings({ active }));
    if (call.method === "POST") return { ok: true, json: () => original.promise };
    assert.equal(call.url, "/api/architect/status");
    if (++statusReads === 1) return ok(hostStatus("starting", { startup_phase: "storage_preparation" }));
    if (statusReads === 2) return { ok: true, json: () => oldPoll.promise };
    return ok(hostStatus("ready"));
  }, async ({ document, calls, runtimes, button, click, pollStart, settle, closes }) => {
    await click("Start guide"); await pollStart(); progress(document);
    await pollStart(); active = true;
    await click("Check guide status");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(document.querySelector(".setup-provider-startup") === null, true);
    assert.equal(button("Stop setup team").disabled, true, "A readback does not replace the entered Start");
    const readCount = calls.length, runtimeCount = runtimes.length;
    await act(async () => oldPoll.resolve(hostStatus("failed", { startup_phase: "identity_warmup",
      startup_error_kind: "startup_timeout", detail: privateDiagnostic }))); await settle();
    await pollStart();
    assert.equal(calls.length, readCount, "Manual observation stops further background status reads");
    assert.equal(runtimes.length, runtimeCount, "An older phase observation is inert");
    await act(async () => original.reject(new Error(privateDiagnostic))); await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(document.querySelector(".setup-provider-startup") === null, true);
    assert.doesNotMatch(document.body.textContent, new RegExp(privateDiagnostic));
    assertDraftAndAdmission(document, calls);
    await click("Back to my draft"); assert.equal(closes(), 1);
    assertDraftAndAdmission(document, calls);
  });
});

test("an unmounted provider handoff cannot publish a delayed startup diagnosis", async () => {
  const original = deferred(), delayed = deferred();
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings());
    if (call.method === "POST") return { ok: true, json: () => original.promise };
    assert.equal(call.url, "/api/architect/status"); return { ok: true, json: () => delayed.promise };
  }, async ({ document, calls, runtimes, click, pollStart, settle, unmount }) => {
    await click("Start guide"); await pollStart();
    assertDraftAndAdmission(document, calls);
    await unmount();
    const count = runtimes.length;
    await act(async () => delayed.resolve(hostStatus("failed", { startup_phase: "storage_preparation",
      startup_error_kind: "storage_preparation", detail: privateDiagnostic }))); await settle();
    await act(async () => original.resolve(hostStatus("ready"))); await settle();
    assert.equal(runtimes.length, count);
    assert.equal(document.querySelector(".setup-provider-startup") === null, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  });
});
