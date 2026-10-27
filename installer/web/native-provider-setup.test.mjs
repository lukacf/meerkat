import test from "node:test";
import assert from "node:assert/strict";
import React, { act } from "react";
import { createRequire } from "node:module";
import { loadTestModule } from "./test-support/bundle.mjs";
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const { ProviderSetup } = await loadTestModule("provider-setup.jsx", { loader: { ".css": "empty" } });
const { NativeProviderSetup, NativeProviderPrompt } = await loadTestModule("native-provider-setup.jsx", { loader: { ".css": "empty" } });
const snapshot = (prompt, id = "native-1") => ({ status: "prompt", prompt_id: id, prompt, joined: false });
const choices = snapshot({ kind: "choices", choices: [
  { choice_id: "offered", provider: "native-provider", model: "exact-native-model", auth: "native_oauth", selectable: true },
  { choice_id: "disabled", provider: "disabled-provider", model: "disabled-model", auth: "unsupported", selectable: false },
] });
const action = snapshot({ kind: "action", selection: choices.prompt.choices[0], credential: "ready", actions: ["use", "back"] }, "native-2");
const selected = { status: "selected", joined: true, selection: { provider: "native-provider", model: "exact-native-model", credential: "ready", live_entitlement: "unobserved" } };

async function mounted(read, check, { Component = NativeProviderSetup, props = {}, strict = false } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "https://forge.example.invalid/" });
  const names = ["window", "document", "navigator", "HTMLElement", "IS_REACT_ACT_ENVIRONMENT", "fetch", "localStorage", "sessionStorage"];
  const old = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  for (const name of ["window", "document", "navigator", "HTMLElement"])
    Object.defineProperty(globalThis, name, { configurable: true, value: dom.window[name] || dom.window });
  Object.defineProperty(globalThis, "IS_REACT_ACT_ENVIRONMENT", { configurable: true, value: true });
  for (const name of ["localStorage", "sessionStorage"])
    Object.defineProperty(globalThis, name, { configurable: true, get() { throw Error("No secret browser storage"); } });
  const calls = [], closed = [];
  Object.defineProperty(globalThis, "fetch", { configurable: true, value: async (url, options = {}) => {
    const call = { url, method: options.method || "GET", body: options.body && JSON.parse(options.body) };
    calls.push(call);
    const value = await read(call);
    if (value instanceof Error) throw value;
    return value?.httpError ? { ok: false, status: value.httpError, json: async () => ({ detail: "private-error-canary" }) }
      : { ok: true, json: async () => value };
  } });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root"));
  const tick = async () => act(async () => new Promise(resolve => setTimeout(resolve, 2)));
  const button = label => [...document.querySelectorAll("button")].find(node => node.textContent === label);
  const click = async label => { assert.ok(button(label), label); await act(async () => button(label).click()); await tick(); };
  const input = async (selector, value) => {
    const node = document.querySelector(selector); assert.ok(node, selector);
    const proto = node.tagName === "SELECT" ? dom.window.HTMLSelectElement.prototype : dom.window.HTMLInputElement.prototype;
    await act(async () => {
      Object.getOwnPropertyDescriptor(proto, "value").set.call(node, value);
      node.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
      node.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    });
  };
  try {
    const view = React.createElement(Component, { onClose: () => closed.push(true), firstRun: true, idea: "My unsent idea", ...props });
    await act(async () => root.render(strict ? React.createElement(React.StrictMode, null, view) : view));
    await tick(); await check({ calls, closed, button, click, input, tick, document: dom.window.document });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [name, descriptor] of old) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor); else delete globalThis[name];
    }
  }
}

test("server native mode projects only native choices and Continue never submits a draft or Build", async () => {
  let state = { status: "not_started", joined: false };
  await mounted(call => {
    if (call.url === "/api/provider-settings") return { native_provider_setup: true };
    if (call.url.endsWith("/begin")) state = choices;
    if (call.url.endsWith("/answer")) {
      assert.equal(call.body.prompt_id, state.prompt_id);
      if (state === choices) {
        assert.deepEqual(call.body.answer, { kind: "choice", choice_id: "offered" }); state = action;
      } else {
        assert.deepEqual(call.body.answer, { kind: "action", action: "use" }); state = selected;
      }
    }
    return state;
  }, async ({ calls, click, input, button, document, closed }) => {
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    await click("Choose model and sign-in");
    assert.match(document.body.textContent, /exact-native-model/);
    assert.doesNotMatch(document.body.textContent, /gpt-6|claude-sonnet/);
    assert.equal(document.querySelector('option[value="disabled"]').disabled, true);
    await input("select", "offered"); await click("Continue");
    assert.equal(button("Sign in"), undefined);
    await click("Use this model");
    assert.match(document.body.textContent, /Live model access has not been checked/);
    await click("Continue"); assert.equal(closed.length, 1);
    assert.equal(calls.filter(call => call.method === "POST").length, 3);
    assert.ok(calls.every(call => call.url.startsWith("/api/provider-setup") || call.url === "/api/provider-settings"));
    assert.ok(calls.every(call => !JSON.stringify(call.body || {}).includes("My unsent idea")));
  }, { Component: ProviderSetup });
});

test("target review, offered sign-in, device code, notices and native refusal stay exact", async () => {
  let state = snapshot({ kind: "target_review", selection: choices.prompt.choices[0], routes: { exact: "native-route" }, agent_model_changed: false });
  await mounted(call => {
    if (call.method === "POST") {
      if (call.body.answer?.kind === "confirm") state = snapshot({ kind: "action", actions: ["sign_in"], credential: "missing" }, "sign-in");
      else if (call.body.answer?.action === "sign_in") state = snapshot({ kind: "device", user_code: "EXACT-CODE", verification_uri: "https://provider.invalid/device" }, "device");
    }
    return state;
  }, async ({ click, document, button }) => {
    assert.match(document.querySelector("pre").textContent, /native-route/);
    await click("Confirm model setup"); assert.equal(button("Enter API key"), undefined);
    await click("Sign in");
    assert.match(document.body.textContent, /EXACT-CODE/);
    assert.equal(document.querySelector("a").href, "https://provider.invalid/device");
    assert.equal(button("Continue"), undefined);
    state = { status: "failed", joined: true, notices: [{ kind: "target_saved", exact: "native-observation" }],
      terminal: { kind: "failed", reason: "native_refusal_or_unavailable", native_effects: "reobserve" } };
    await click("Check setup");
    assert.match(document.body.textContent, /native_refusal_or_unavailable/);
    assert.match(document.body.textContent, /native-observation/);
    assert.equal(document.querySelector("a"), null);
  });
});

for (const kind of ["key", "browser_manual"]) test(`${kind} clears secrets and prevents replay after a lost response`, async () => {
  const prompt = snapshot({ kind, authorization_url: "https://provider.invalid/authorize", redirect: "http://localhost:1455/auth/callback" });
  let state = prompt;
  await mounted(call => {
    if (call.url.endsWith("/answer")) return new Error("private-secret-canary");
    if (call.url.endsWith("/cancel")) state = { status: "cancelled", joined: true, terminal: { kind: "cancelled", native_effects: "reobserve" } };
    return state;
  }, async ({ click, input, document, calls, button }) => {
    await input("input", "private-secret-canary");
    const submit = kind === "key" ? "Save API key" : "Submit return URL";
    await click(submit);
    assert.equal(document.querySelector("input"), null);
    assert.doesNotMatch(document.body.textContent, /private-secret-canary/);
    await click("Check setup");
    assert.equal(document.querySelector("input").value, "");
    assert.equal(button(submit).disabled, true);
    await click("Cancel setup");
    assert.match(document.body.textContent, /cancelled and closed/);
    assert.equal(calls.filter(call => call.url.endsWith("/answer")).length, 1);
    assert.equal(button("Continue"), undefined);
  });
});

test("an obsolete read cannot resurrect a prompt after an accepted answer", async () => {
  let state = choices, resolve, wait = false;
  await mounted(call => {
    if (call.method === "GET" && wait) return new Promise(done => { resolve = done; });
    if (call.method === "POST") state = action;
    return state;
  }, async ({ click, input, document, tick }) => {
    await input("select", "offered"); wait = true;
    // Let the read remain outstanding while the user answers the same prompt.
    await act(async () => [...document.querySelectorAll("button")].find(b => b.textContent === "Check setup").click());
    await click("Continue");
    assert.match(document.body.textContent, /Use this model/);
    await act(async () => resolve(choices)); await tick();
    assert.equal(document.querySelector("select"), null);
  });
});

test("authentication failure hides private prompts and never echoes server errors", async () => {
  let failed = false;
  await mounted(() => failed ? { httpError: 403 } : snapshot({ kind: "device", user_code: "PRIVATE-CODE", verification_uri: "https://provider.invalid/" }),
    async ({ click, document }) => {
      failed = true; await click("Check setup");
      assert.doesNotMatch(document.body.textContent, /PRIVATE-CODE|private-error-canary/);
      assert.match(document.body.textContent, /Sign in again/);
      assert.equal(document.querySelector("a"), null);
    });
});

test("provider failure keeps polling until the original child has joined", async t => {
  let poll, reads = 0;
  t.mock.method(globalThis, "setInterval", callback => { poll = callback; return 1; });
  t.mock.method(globalThis, "clearInterval", () => {});
  await mounted(call => {
    assert.equal(call.method, "GET");
    return { status: "failed", joined: ++reads > 1, live_entitlement: "unobserved" };
  }, async ({ calls, button, tick, document }) => {
    assert.ok(button("Cancel setup"));
    assert.match(document.body.textContent, /Native cleanup is not confirmed yet/);
    await act(async () => poll()); await tick();
    assert.equal(reads, 2, "failure is not evidence that native cleanup finished");
    assert.equal(button("Cancel setup"), undefined);
    assert.doesNotMatch(document.body.textContent, /Native cleanup is not confirmed yet/);
    await act(async () => poll()); await tick();
    assert.equal(reads, 2, "polling ends after the same child is observed joined");
    assert.ok(calls.every(call => call.method === "GET"));
  });
});

test("StrictMode reads do not start native work, and unknown prompts offer no invented actions", async () => {
  await mounted(() => snapshot({ kind: "future_prompt", detail: "native-detail" }), async ({ document, calls, button }) => {
    assert.match(document.body.textContent, /not supported by this Forge version/);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    assert.equal(button("Continue"), undefined);
    assert.ok(button("Cancel setup"));
  }, { strict: true });
});

test("browser presentation refuses executable URLs and has no callback answer without manual transport", async () => {
  await mounted(() => { throw Error("No HTTP expected"); }, async ({ document, button }) => {
    assert.equal(document.querySelector("a"), null);
    assert.equal(document.querySelector("input"), null);
    assert.equal(button("Continue"), undefined);
    assert.match(document.body.textContent, /cannot be opened/);
  }, { Component: NativeProviderPrompt, props: { prompt: { kind: "browser", authorization_url: "javascript:alert(1)", redirect: "http://localhost/callback" } } });
});
