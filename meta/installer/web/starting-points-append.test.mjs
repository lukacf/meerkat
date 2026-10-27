import { after, test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const webRoot = process.env.FORGE_TEST_WEB_ROOT || fileURLToPath(new URL(".", import.meta.url));
const sourcePath = process.env.FORGE_STARTING_POINTS_SOURCE || join(webRoot, "guided.jsx");
const require = createRequire(join(webRoot, "package.json"));
const React = require("react"), { act, useState } = React;
const { build } = require("esbuild");
const { JSDOM } = createRequire(resolve(webRoot, "../../console/package.json"))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-starting-points-append-"));
const modulePath = join(directory, "guided.mjs");
after(() => rm(directory, { recursive: true, force: true }));
const built = await build({
  stdin: { contents: await readFile(sourcePath, "utf8"), sourcefile: sourcePath, resolveDir: webRoot, loader: "jsx" },
  bundle: true, write: false, platform: "node", format: "esm", jsx: "automatic",
  outfile: modulePath, sourcemap: "linked", sourcesContent: true, logLevel: "silent",
  loader: { ".css": "empty" }, nodePaths: [join(webRoot, "node_modules")],
  alias: { "@console-core": resolve(webRoot, "../../packages/console-core/src") },
  plugins: [{ name: "shared-real-react", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
for (const output of built.outputFiles) {
  await writeFile(output.path, output.contents);
  if (output.path.endsWith(".map") && process.env.FORGE_STARTING_POINTS_MAP)
    await writeFile(process.env.FORGE_STARTING_POINTS_MAP, output.contents);
}
const { SetupStartingPoints } = await import(pathToFileURL(modulePath).href);

test("starting-point inspiration appends only deliberately to the latest exact draft and disabled or Keep actions preserve it", async () => {
  const prefix = " \tMy own idea\n\u00e5\u{1f9ed}  ";
  const dom = new JSDOM('<div id="root"></div>', { url: "http://offline.test/", pretendToBeVisual: true });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    fetch() { assert.fail("Inspiration must not admit or send guide work."); } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  const choices = [];
  let owner, root;
  function DraftOwner() {
    const [draft, setDraft] = useState(prefix), [disabled, setDisabled] = useState(false);
    owner = { setDisabled };
    return React.createElement("div", null,
      React.createElement("textarea", { id: "owned-idea", value: draft, onChange: event => setDraft(event.target.value) }),
      React.createElement(SetupStartingPoints, { draft, disabled, onChoose(text) { choices.push(text); setDraft(text); } }));
  }
  const document = dom.window.document;
  const button = text => {
    const found = [...document.querySelectorAll("button")].find(item => item.textContent === text);
    assert.ok(found, `Missing deliberate action: ${text}`);
    return found;
  };
  const example = index => document.querySelectorAll(".setup-starting-points > button")[index];
  const click = value => act(async () => value.click());
  const edit = value => act(async () => {
    const textarea = document.getElementById("owned-idea");
    Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set.call(textarea, value);
    textarea.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    textarea.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
  });
  const invokeDisabled = value => {
    const props = Object.keys(value).find(key => key.startsWith("__reactProps$"));
    assert.ok(props, "The mounted button must expose its real React event handler.");
    value[props].onClick();
  };
  try {
    const { createRoot } = require("react-dom/client");
    root = createRoot(document.getElementById("root"));
    await act(async () => root.render(React.createElement(DraftOwner)));
    await click(example(0));
    const firstText = document.querySelector("details > p").textContent;
    assert.deepEqual(choices, []);
    assert.equal(document.getElementById("owned-idea").value, prefix);
    await click(button("Add inspiration to my idea"));
    assert.deepEqual(choices, [prefix + "\n\n" + firstText]);
    assert.equal(document.getElementById("owned-idea").value, prefix + "\n\n" + firstText);
    assert.equal(document.querySelector("details") === null, true);

    const revised = "\n  A revised direction\n with exact trailing spaces  ";
    await edit(revised);
    await click(example(1));
    const secondText = document.querySelector("details > p").textContent;
    const latest = " \tLatest edit while inspiration is open\n\u00f6  ";
    await edit(latest);
    await act(async () => owner.setDisabled(true));
    const add = button("Add inspiration to my idea"), keep = button("Keep my idea"), other = example(2);
    assert.equal(add.disabled, true); assert.equal(keep.disabled, true); assert.equal(other.disabled, true);
    await click(add); await click(keep); await click(other);
    await act(async () => { invokeDisabled(add); invokeDisabled(keep); invokeDisabled(other); });
    assert.deepEqual(choices, [prefix + "\n\n" + firstText]);
    assert.equal(document.getElementById("owned-idea").value, latest);
    assert.equal(document.querySelector("details > p").textContent, secondText);
    await act(async () => owner.setDisabled(false));
    await click(button("Keep my idea"));
    assert.equal(document.querySelector("details") === null, true);
    assert.equal(document.getElementById("owned-idea").value, latest);
    assert.equal(choices.length, 1);

    await click(example(1));
    const newest = "\n Latest draft after reopening\t  ";
    await edit(newest);
    await click(button("Add inspiration to my idea"));
    assert.deepEqual(choices, [prefix + "\n\n" + firstText, newest + "\n\n" + secondText]);
    assert.equal(document.getElementById("owned-idea").value, newest + "\n\n" + secondText);
    assert.equal(document.querySelector("details") === null, true);
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
});
