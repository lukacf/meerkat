import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import React, { act } from "react";
import { createRoot } from "react-dom/client";
import {
  GUIDE_KEY,
  stageIndex,
  toggleAnswer,
  canSubmitAnswer,
  manualFeedback,
  guideMessageEntries,
  pendingManual,
} from "./guided-state.mjs";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const rendered = await build({ entryPoints: [fileURLToPath(new URL("guided.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", jsx: "automatic",
  loader: { ".css": "empty" }, nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
  plugins: [{ name: "shared-real-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
const { GuidedSetup, SetupStartingPoints } = await import(`data:text/javascript;base64,${Buffer.from(rendered.outputFiles[0].text).toString("base64")}`);

async function mountedStartingPoints(draft, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const replacements = { window: { value: dom.window }, document: { value: dom.window.document },
    navigator: { value: dom.window.navigator }, HTMLElement: { value: dom.window.HTMLElement },
    IS_REACT_ACT_ENVIRONMENT: { value: true },
    fetch: { value: () => { throw new Error("An example preview must not admit guide work."); } } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, descriptor] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, ...descriptor });
  const root = createRoot(dom.window.document.getElementById("root")), choices = [];
  const render = (value, disabled = false) => act(async () => root.render(React.createElement("div", null,
    React.createElement("textarea", { id: "owned-description", value, readOnly: true }),
    React.createElement(SetupStartingPoints, { draft: value, disabled, onChoose: text => choices.push(text) }))));
  try {
    await render(draft);
    await verify(dom.window.document, choices, render);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

async function mountedGuideRuntime({ busy = false, architect, status }, verify) {
  const guide = { id: "restored-runtime-guide", version: 5, status: "question", stage: "purpose",
    completed_stages: [], answers: [], brief: { facts: {}, requirements: [] }, capabilities: { available: true },
    console_url: "http://127.0.0.1:58124/console", messages: [{ role: "assistant", text: "Your saved plan is retained." }],
    question: { stage: "purpose", title: "Saved server question", multiple: false,
      options: [{ id: "continue", label: "Keep this choice", description: "A saved choice." }] },
    operation: { status: busy ? "running" : "completed", request_id: "a".repeat(64), version: 4,
      request: { version: 4, action: "chat", text: "Check the saved plan.", selected: [] } } };
  const before = structuredClone(guide), calls = [], intervals = [];
  let currentGuide = guide;
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  dom.window.localStorage.setItem(GUIDE_KEY, guide.id);
  const replacements = { window: { value: dom.window }, document: { value: dom.window.document },
    navigator: { value: dom.window.navigator }, HTMLElement: { value: dom.window.HTMLElement },
    localStorage: { value: dom.window.localStorage }, IS_REACT_ACT_ENVIRONMENT: { value: true },
    requestAnimationFrame: { value: dom.window.requestAnimationFrame.bind(dom.window) },
    cancelAnimationFrame: { value: dom.window.cancelAnimationFrame.bind(dom.window) },
    matchMedia: { value: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) },
    setInterval: { value: (callback, delay) => {
      const interval = { callback, delay, cancelled: false }; intervals.push(interval); return interval;
    } }, clearInterval: { value: interval => { interval.cancelled = true; } },
    fetch: { value: async (url, options = {}) => {
      calls.push({ url, method: options.method || "GET" });
      assert.equal(options.method || "GET", "GET", "Runtime projection tests cannot admit new work");
      const body = url === `/api/guide/${guide.id}` ? structuredClone(currentGuide)
        : url === "/api/guide/list" ? { setups: [] }
        : url.startsWith(`/api/guide/${guide.id}/timeline?`)
          ? { frames: [], bindings: [], next_cursor: null, latest_cursor: null, exhausted: true }
        : url === "/api/architect/status" && status ? await status() : null;
      assert.ok(body, `Unexpected offline fixture request: ${url}`);
      return { ok: true, json: async () => body };
    } } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, descriptor] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, ...descriptor });
  dom.window.HTMLElement.prototype.scrollTo = function() {};
  dom.window.HTMLElement.prototype.scrollIntoView = function() {};
  dom.window.HTMLElement.prototype.focus = function() {};
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = owner => act(async () => root.render(React.createElement(GuidedSetup,
    { active: true, bp: null, Icon: () => null, onBlueprint: () => {}, config: { architect: owner } })));
  const link = () => dom.window.document.querySelector('a[title="Inspect the live Forge setup team"]');
  const finish = async () => {
    currentGuide = { ...structuredClone(guide), operation: { ...guide.operation, status: "completed" } };
    await act(async () => intervals.find(interval => interval.delay === 2500 && !interval.cancelled).callback());
  };
  try {
    await render(architect);
    await verify({ render, link, calls, intervals, finish });
    assert.deepEqual(guide, before, "Current owner projection must preserve the saved guide and body");
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

const oldRuntimeOwner = { available: true, status: "ready", console_url: "http://127.0.0.1:58124/console",
  browser_console_url: "/api/owned-console/guide/11111111-1111-4111-8111-111111111111/console" };
const currentRuntimeOwner = { available: true, status: "ready", console_url: "http://127.0.0.1:51558/console",
  browser_console_url: "/api/owned-console/guide/22222222-2222-4222-8222-222222222222/console" };

test("Guide console follows the confirmed current owner while a restored guide is idle", async () => {
  await mountedGuideRuntime({ busy: true, architect: oldRuntimeOwner, status: () => oldRuntimeOwner },
    async ({ render, link, calls, intervals, finish }) => {
    await act(async () => intervals.find(interval => interval.delay === 4000).callback());
    assert.equal(link()?.href, new URL(oldRuntimeOwner.browser_console_url, "http://localhost/").href);
    assert.equal(link().textContent.trim(), "Guide console");
    assert.equal(link().getAttribute("aria-label"), "Open the Forge setup guide console in a new tab");
    assert.equal(link().target, "_blank");
    await finish();
    assert.ok(intervals.filter(interval => interval.delay === 4000).every(interval => interval.cancelled),
      "The guide is idle and its own runtime polling has stopped");
    const reads = calls.filter(call => call.url === "/api/architect/status").length;
    await render(currentRuntimeOwner);
    assert.equal(link()?.href, new URL(currentRuntimeOwner.browser_console_url, "http://localhost/").href);
    assert.equal(calls.filter(call => call.url === "/api/architect/status").length, reads,
      "An idle guide must receive the owner update without starting its own runtime polling");
  });
});

test("Guide console is removed when the confirmed owner explicitly has no Console URL", async () => {
  await mountedGuideRuntime({ busy: true, architect: oldRuntimeOwner, status: () => oldRuntimeOwner },
    async ({ render, link, intervals }) => {
      await act(async () => intervals.find(interval => interval.delay === 4000).callback());
      assert.equal(link()?.href, new URL(oldRuntimeOwner.browser_console_url, "http://localhost/").href);
      await render({ ...currentRuntimeOwner, browser_console_url: null });
      assert.equal(link() === null, true, "The old local and historical URLs cannot survive an owner-null update");
    });
});

test("Guide console respects an explicit null URL from the current runtime status read", async () => {
  await mountedGuideRuntime({ busy: true, architect: oldRuntimeOwner,
    status: () => ({ available: true, status: "stopped", console_url: null, browser_console_url: null }) }, async ({ link, intervals }) => {
      assert.equal(link()?.href, new URL(oldRuntimeOwner.browser_console_url, "http://localhost/").href);
      await act(async () => intervals.find(interval => interval.delay === 4000).callback());
      assert.equal(link() === null, true, "A confirmed null status must not fall back to an older config URL");
    });
});

test("Guide console rejects a delayed busy status response after a newer owner is confirmed", async () => {
  let completeOldRead;
  await mountedGuideRuntime({ busy: true, architect: oldRuntimeOwner,
    status: () => new Promise(resolve => { completeOldRead = resolve; }) }, async ({ render, link, calls, intervals }) => {
      const oldPoll = intervals.find(interval => interval.delay === 4000);
      await act(async () => { oldPoll.callback(); });
      assert.ok(completeOldRead, "The old status request is already in flight");
      await render(currentRuntimeOwner);
      assert.equal(oldPoll.cancelled, true);
      assert.equal(link()?.href, new URL(currentRuntimeOwner.browser_console_url, "http://localhost/").href);
      await act(async () => { completeOldRead(oldRuntimeOwner); });
      assert.equal(link()?.href, new URL(currentRuntimeOwner.browser_console_url, "http://localhost/").href,
        "The old delayed response must not replace the fresh owner");
      assert.equal(calls.filter(call => call.url === "/api/architect/status").length, 1);
    });
});

test("server question stage and option IDs control the guided selection", () => {
  assert.equal(
    stageIndex({ stage: "purpose", question: { stage: "knowledge" } }),
    2,
  );
  assert.deepEqual(toggleAnswer(["source_a"], "source_b", false), ["source_b"]);
  assert.deepEqual(toggleAnswer(["source_a"], "source_b", true), [
    "source_a",
    "source_b",
  ]);
  assert.deepEqual(toggleAnswer(["source_a", "source_b"], "source_a", true), [
    "source_b",
  ]);
  assert.deepEqual(toggleAnswer(["source_a"], "source_a", false), []);
});
test("old or invented choices cannot enable continue, free text remains supported", () => {
  const question = {
    options: [{ id: "personalized_choice" }],
    allow_text: true,
  };
  assert.equal(canSubmitAnswer(question, ["old_choice"], ""), false);
  assert.equal(canSubmitAnswer(question, ["old_choice", "personalized_choice"], ""), false);
  assert.equal(canSubmitAnswer(question, ["old_choice"], "My own idea"), false);
  assert.equal(canSubmitAnswer(question, ["personalized_choice"], ""), true);
  assert.equal(canSubmitAnswer(question, [], "   "), false);
  assert.equal(canSubmitAnswer(question, [], "My own idea"), true);
});
test("single-choice answers cannot submit multiple or duplicate choices, even with added text", () => {
  const question = { multiple: false, options: [{ id: "staff" }, { id: "guests" }], allow_text: true };
  assert.equal(canSubmitAnswer(question, ["staff", "guests"], "My context"), false);
  assert.equal(canSubmitAnswer(question, ["staff", "staff"], ""), false);
  assert.equal(canSubmitAnswer(question, ["staff"], "My context"), true);
  assert.equal(canSubmitAnswer(question, [], "My own idea"), true);
  assert.equal(canSubmitAnswer({ ...question, multiple: true }, ["staff", "guests"], ""), true);
  assert.equal(canSubmitAnswer({ ...question, multiple: true }, ["staff", "staff"], ""), false);
});
test("returning from advanced controls preserves a concrete snapshot and meaningful delta", () => {
  const before = {
    project: { name: "Example" },
    agent_classes: [],
    connections: [],
    topology: {},
    flows: [],
    knowledge: { mode: "none" },
    target: "mac",
    gcp: {},
    advanced: {},
  };
  assert.equal(manualFeedback(before, before), null);
  const after = structuredClone(before);
  after.target = "linux";
  const feedback = manualFeedback(before, after);
  assert.deepEqual(feedback.changes, ["Deployment settings"]);
  assert.equal(feedback.blueprint.target, "linux");
  assert.match(feedback.text, /Keep my choices/);
  after.target = "gcp";
  assert.equal(feedback.blueprint.target, "linux");
});
test("conversation entries are stable projections of saved guide messages", () => {
  const make = (role, text) => ({
    id: Math.random(),
    identity: { role },
    text,
  });
  const source = [
    { role: "user", text: "Research" },
    { role: "agent", text: "Let's choose sources." },
    { role: "system", text: "" },
  ];
  const first = guideMessageEntries(source, make),
    second = guideMessageEntries(source, make);
  assert.deepEqual(first, second);
  assert.equal(first.length, 2);
  assert.equal(first[1].identity.role, "assistant");
});

test("pending manual changes survive reload and cannot leak into another setup", () => {
  const feedback = { blueprint: { project: { name: "My manual choice" } },
    changes: ["Project and purpose"], text: "Keep my choice" };
  const storage = { getItem: () => JSON.stringify({ guideId: "setup-a", feedback }) };
  assert.deepEqual(pendingManual(storage, "setup-a"), feedback);
  assert.equal(pendingManual(storage, "setup-b"), null);
  assert.equal(pendingManual({ getItem: () => "broken" }, "setup-a"), null);
  assert.equal(pendingManual({ getItem: () => JSON.stringify({ guideId: "setup-a", feedback: {} }) }, "setup-a"), null);
});

test("optional examples seed an empty message for editing and never send guide work", async () => {
  await mountedStartingPoints("", async (document, choices, render) => {
    await act(async () => document.querySelector(".setup-starting-points > button").click());
    assert.equal(choices.length, 1);
    assert.match(choices[0], /which information it needs/);
    assert.equal(document.querySelector("details") === null, true);
    await render("", true);
    await act(async () => document.querySelector(".setup-starting-points > button").click());
    assert.equal(choices.length, 1);
  });
});

test("exploring several starting points previews examples while preserving every byte of an unsent description", async () => {
  for (const draft of ["  My own idea\n\u00e5\u{1f9ed}  ", " \n  "]) {
    await mountedStartingPoints(draft, async (document, choices) => {
      const buttons = document.querySelectorAll(".setup-starting-points > button");
      await act(async () => buttons[0].click());
      assert.deepEqual(choices, []);
      assert.equal(document.querySelector("#owned-description").value, draft);
      assert.equal(document.querySelector("details").open, true);
      assert.match(document.querySelector("details").textContent, /Example: Stay on top of my projects/);
      assert.match(document.querySelector("details").textContent, /Your description is still in the message box/);
      await act(async () => buttons[1].click());
      assert.deepEqual(choices, []);
      assert.equal(document.querySelector("#owned-description").value, draft);
      assert.match(document.querySelector("details").textContent, /Example: Keep our household in sync/);
      assert.doesNotMatch(document.querySelector("details").textContent, /Example: Stay on top of my projects/);
    });
  }
});

test("finding an idea seeds an editable discovery request and preserves an existing description", async () => {
  await mountedStartingPoints("", async (document, choices, render) => {
    const help = () => document.querySelector(".forge-starting-point-help button");
    await act(async () => help().click());
    assert.equal(choices.length, 1);
    assert.match(choices[0], /Ask me about my day-to-day work/);
    assert.match(choices[0], /before suggesting an agent system/);
    await render("  My exact idea\n\u00e5  ");
    await act(async () => help().click());
    assert.equal(choices.length, 1);
    assert.equal(document.querySelector("#owned-description").value, "  My exact idea\n\u00e5  ");
    assert.match(document.querySelector("details").textContent, /Find a starting point together/);
    await render("", true);
    await act(async () => help().click());
    assert.equal(choices.length, 1);
  });
});
