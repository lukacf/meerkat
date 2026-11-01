import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import React, { act } from "react";
import { loadTestModule } from "./test-support/bundle.mjs";
import { readGuideLink } from "./guide-link.mjs";
import { GUIDE_KEY } from "./guided-state.mjs";
import { GUIDE_CREATION_KEY, inputKey } from "./wizard-state.mjs";

const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const { GuidedSetup } = await loadTestModule("guided.jsx", {
  loader: { ".css": "empty" },
  nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
  plugins: [{ name: "inert-animation-only", setup(builder) {
    builder.onResolve({ filter: /proposal-simulation\.jsx$/ }, () => ({ path: "preview", namespace: "inert" }));
    builder.onLoad({ filter: /.*/, namespace: "inert" }, () => ({
      contents: "export function ProposalSimulation(){ return null; }", loader: "js" }));
  } }],
});
const A = "00000000-0000-4000-8000-000000000001", B = "00000000-0000-4000-8000-000000000002";
const guide = id => ({ id, version: 3, status: "question", stage: "purpose", completed_stages: [], answers: [],
  messages: [], brief: { facts: {}, requirements: [] }, capabilities: { available: true },
  question: { stage: "purpose", title: id === A ? "Saved conversation A" : "Linked conversation B",
    description: "Keep the original question.", options: [], allow_text: true } });
const delay = () => new Promise(resolve => setTimeout(resolve, 10));

async function mounted(search, read, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/" + search, pretendToBeVisual: true });
  const drafts = [A, B].map(id => ({ key: inputKey(guide(id)), draft: "  Unsent " + id + "\n", selected: [], mode: "chat" }));
  dom.window.localStorage.setItem(GUIDE_KEY, A);
  for (const draft of drafts) dom.window.localStorage.setItem(draft.key, JSON.stringify(draft));
  const pendingCreation = "00000000-0000-4000-8000-000000000003";
  dom.window.sessionStorage.setItem(GUIDE_CREATION_KEY, pendingCreation);
  const calls = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true, requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    fetch: async (url, options = {}) => {
      const method = options.method || "GET";
      calls.push({ url, method });
      assert.equal(method, "GET", "Opening or retrying a link must never send, start or resume work");
      const value = await read(url) ?? (url === "/api/guide/list" ? { setups: [] } :
        url.includes("/timeline") ? { frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true } : null);
      assert.ok(value, "Unexpected fixture read: " + url);
      return { ok: !value.httpError, status: value.httpError || 200,
        json: async () => value.httpError ? { detail: value.detail || "Fixture unavailable" } : structuredClone(value) };
    } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  for (const name of ["scrollTo", "scrollIntoView", "focus"]) dom.window.HTMLElement.prototype[name] = function() {};
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  try {
    await act(async () => { root.render(React.createElement(GuidedSetup, { active: true, bp: null, Icon: () => null,
      onBlueprint() {}, config: { architect: { available: true } } })); });
    await act(delay);
    const click = async label => {
      const button = [...dom.window.document.querySelectorAll("button")].find(node => node.textContent === label);
      assert.ok(button, label);
      await act(async () => { button.click(); await delay(); });
    };
    await verify({ window: dom.window, calls, drafts, click });
    for (const draft of drafts) assert.deepEqual(JSON.parse(dom.window.localStorage.getItem(draft.key)), draft);
    assert.equal(dom.window.sessionStorage.getItem(GUIDE_CREATION_KEY), pendingCreation);
    assert.ok(calls.every(call => call.method === "GET"));
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test("an exact guide link overrides the saved selection without consuming either draft or pending creation", async () => {
  await mounted("?guide=" + B, url => url === "/api/guide/" + B ? guide(B) : null, async ({ window, calls, drafts }) => {
    assert.match(window.document.body.textContent, /Linked conversation B/);
    assert.equal(window.document.querySelector("#setup-message").value, drafts[1].draft);
    assert.equal(window.localStorage.getItem(GUIDE_KEY), B);
    assert.equal(calls.some(call => call.url === "/api/guide/" + A || call.url === "/api/guide/new"), false);
  });
});

test("a missing linked guide stays explicit on retry and preserves the previous selection", async () => {
  await mounted("?guide=" + B, url => url === "/api/guide/" + B ? { httpError: 404 } :
    url === "/api/guide/" + A ? guide(A) : null, async ({ window, calls, click }) => {
    assert.match(window.document.body.textContent, /linked setup was not found in this workspace/);
    await click("Try again");
    assert.deepEqual(calls.map(call => call.url), ["/api/guide/" + B, "/api/guide/" + B]);
    assert.equal(window.localStorage.getItem(GUIDE_KEY), A);
    await click("Open Forge");
    assert.match(window.document.body.textContent, /Saved conversation A/);
    assert.equal(window.location.search, "");
  });
});

for (const query of ["?guide=", "?guide=latest", "?guide=" + B + "&guide=" + A, "?guide=" + B + "%0A"])
  test("an invalid guide link never falls back or sends a request: " + query, async () => {
    assert.ok(readGuideLink(query).error);
    await mounted(query, () => { throw new Error("No request expected"); }, async ({ window, calls, click }) => {
      assert.match(window.document.body.textContent, /setup link is invalid/);
      await click("Try again");
      assert.deepEqual(calls, []);
      assert.equal(window.localStorage.getItem(GUIDE_KEY), A);
    });
  });

test("an unrelated response cannot substitute for the exact linked guide", async () => {
  await mounted("?guide=" + B, url => url === "/api/guide/" + B ? guide(A) : null, async ({ window, calls }) => {
    assert.match(window.document.body.textContent, /linked setup could not be verified/);
    assert.equal(window.localStorage.getItem(GUIDE_KEY), A);
    assert.equal(calls.length, 1);
  });
});

test("an existing running command remains read-only when its guide is opened by link", async () => {
  const pending = { ...guide(B), operation: { status: "running", request_id: "a".repeat(64),
    version: 3, started_at: "2026-10-26T22:00:00Z", request: { version: 3, action: "chat", text: "Original request" } } };
  const before = structuredClone(pending);
  await mounted("?guide=" + B, url => url === "/api/guide/" + B ? pending : null, async ({ window, calls, drafts }) => {
    assert.equal(window.localStorage.getItem(GUIDE_KEY), B);
    assert.equal(window.document.querySelector("#setup-message").value, drafts[1].draft);
    assert.equal(window.document.querySelector("#setup-send").disabled, true);
    assert.ok(calls.every(call => call.method === "GET"));
    assert.deepEqual(pending, before);
  });
});

test("authentication failure preserves the exact link and previous saved selection", async () => {
  await mounted("?guide=" + B, url => url === "/api/guide/" + B ? { httpError: 401 } : null,
    async ({ window, calls, click }) => {
      await click("Try again");
      assert.deepEqual(calls.map(call => call.url), ["/api/guide/" + B, "/api/guide/" + B]);
      assert.equal(window.localStorage.getItem(GUIDE_KEY), A);
      assert.equal(window.location.search, "?guide=" + B);
    });
});

test("without a guide parameter the existing saved selection still opens", async () => {
  assert.equal(readGuideLink("?other=value"), null);
  await mounted("?other=value", url => url === "/api/guide/" + A ? guide(A) : null, async ({ window, calls }) => {
    assert.match(window.document.body.textContent, /Saved conversation A/);
    assert.equal(calls[0].url, "/api/guide/" + A);
    assert.equal(window.location.search, "?other=value");
  });
});
