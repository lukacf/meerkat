import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { canSubmitAnswer } from "./guided-state.mjs";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const bundle = await build({ entryPoints: [fileURLToPath(new URL("guided.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", jsx: "automatic",
  loader: { ".css": "empty" }, nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
  plugins: [{ name: "shared-real-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
const { SetupGuideChoices } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);
const question = { id: "history", stage: "knowledge", title: "Which histories matter?", multiple: false,
  options: [{ id: "personal", label: "Personal context", description: "Each person's own ongoing history." },
    { id: "shared", label: "Shared context", description: "The group's shared record." }] };

async function mounted(overrides, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/" });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: () => { throw new Error("Choice rendering must not admit native work."); } };
  const prior = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const root = createRoot(dom.window.document.getElementById("root")), answers = [], combinations = [];
  let props = { question, selected: [], draft: "With separate visibility", combineOptions: false, disabled: false,
    Icon: () => React.createElement("span", null), onAnswer: value => answers.push(value), onAsk: () => {},
    onSelect: value => { props.selected = value; }, onCombine: value => { combinations.push(value); props.combineOptions = value; },
    ...overrides };
  const render = (changes = {}) => act(async () => { props = { ...props, ...changes }; root.render(React.createElement(SetupGuideChoices, props)); });
  const click = async text => {
    const button = [...dom.window.document.querySelectorAll("button")].find(item => item.textContent.includes(text));
    assert.ok(button, `Missing button ${text}`);
    await act(async () => button.click());
    await render();
  };
  try { await render(); await verify({ document: dom.window.document, answers, combinations, click, render, selected: () => props.selected }); }
  finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test("single cards select, replace and deselect a pending answer without submitting", async () => {
  await mounted({ showCaption: false }, async ({ document, click, answers, combinations }) => {
    const selected = () => [...document.querySelectorAll('.setup-choice.selected')].map(button => button.textContent);
    await click("Personal context");
    assert.equal(selected().length, 1);
    assert.match(selected()[0], /Personal context/);
    await click("Shared context");
    assert.equal(selected().length, 1);
    assert.match(selected()[0], /Shared context/);
    await click("Shared context");
    assert.deepEqual(selected(), []);
    assert.deepEqual(answers, []);
    assert.deepEqual(combinations, []);
    assert.equal(question.multiple, false);
    assert.match(document.body.textContent, /use Send answer below/);
  });
});

test("an explicit combined answer selects without submitting and requires two choices for the composer", async () => {
  const original = structuredClone(question);
  await mounted({}, async ({ document, click, answers, combinations, selected }) => {
    await click("More than one fits");
    assert.deepEqual(combinations, [true]);
    assert.match(document.body.textContent, /guide will check how it fits together/);
    assert.equal([...document.querySelectorAll("button")].some(item => item.textContent.includes("Continue")), false);
    assert.match(document.body.textContent, /use Send answer below/);
    assert.equal(canSubmitAnswer(question, selected(), "With separate visibility", true), false);
    await click("Personal context");
    assert.equal(canSubmitAnswer(question, selected(), "With separate visibility", true), false);
    assert.equal(answers.length, 0);
    await click("Shared context");
    assert.equal(canSubmitAnswer(question, selected(), "With separate visibility", true), true);
    assert.equal(answers.length, 0);
    assert.deepEqual(selected(), ["personal", "shared"]);
    assert.deepEqual(question, original);
    await click("Choose one instead");
    assert.deepEqual(combinations, [true, false]);
  });
});

test("published multiple choice does not need the override and blocked controls admit nothing", async () => {
  await mounted({ question: { ...question, multiple: true }, draft: "" }, async ({ document, click, answers, selected }) => {
    assert.ok(!document.body.textContent.includes("More than one fits"));
    await click("Personal context"); await click("Shared context");
    assert.deepEqual(selected(), ["personal", "shared"]);
    assert.equal(canSubmitAnswer({ ...question, multiple: true }, selected(), ""), true);
    assert.equal([...document.querySelectorAll("button")].some(item => item.textContent.includes("Continue")), false);
    assert.match(document.body.textContent, /use Send answer below/);
    assert.deepEqual(answers, []);
  });
  await mounted({ disabled: true }, async ({ document, click, answers, combinations }) => {
    assert.ok([...document.querySelectorAll("button")].every(button => button.disabled));
    await click("More than one fits"); await click("Personal context");
    assert.deepEqual(answers, []); assert.deepEqual(combinations, []);
  });
});

test("combining cannot submit stale, duplicate, one-card or non-choice answers", () => {
  assert.equal(canSubmitAnswer(question, ["personal", "shared"], "", true), true);
  for (const values of [[], ["personal"], ["personal", "personal"], ["personal", "unknown"]]) {
    assert.equal(canSubmitAnswer(question, values, "Extra context", true), false);
  }
  assert.equal(canSubmitAnswer(question, ["personal", "shared"], "", false), false);
  assert.equal(canSubmitAnswer({ ...question, multiple: true }, ["personal", "shared"], "", true), false);
  assert.equal(canSubmitAnswer({ ...question, options: [] }, [], "My idea", true), false);
});
