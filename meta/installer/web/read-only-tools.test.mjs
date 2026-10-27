import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import vm from "node:vm";
import { toolSummary } from "./evolving-plan-state.mjs";

const require = createRequire(import.meta.url);
const { transformSync } = require("esbuild");
const source = readFileSync(new URL("./app.jsx", import.meta.url), "utf8");
const start = source.indexOf("function ClassInspector(");
const end = source.indexOf("\nfunction Connections(", start);
assert.ok(start >= 0 && end > start);
// Render only the real inspector with a controlled virtual JSX fixture.
// No application bootstrap, browser, native runtime or provider is executed.
const jsx = transformSync(source.slice(start, end), { loader: "jsx", format: "cjs" }).code;
const Toggle = () => {}, Field = () => {}, Icon = () => {}, Button = () => {}, AllowlistInput = () => {}, NativeBundleSelection = () => {};
const ClassInspector = vm.runInNewContext(`${jsx}\nClassInspector`, {
  React: { createElement: (type, props, ...children) => ({ type, props: props || {}, children }), Fragment: "fragment" },
  useState: () => ["tools", () => {}], Toggle, Field, Icon, Button, AllowlistInput, NativeBundleSelection,
});

function grants() {
  return { workspace: true, notes: "write", builtins: true, shell: true, memory: true,
    workgraph: true, schedule: true, knowledge: true,
    mcp: [{ connection_id: "evidence", tool_names: ["read", "update"] }] };
}

function elements(tree) {
  if (Array.isArray(tree)) return tree.flatMap(elements);
  if (!tree || typeof tree !== "object") return [];
  return [tree, ...elements(tree.children)];
}

function inspector(tools, onChange) {
  const c = { id: "evidence", name: "Evidence", instances: 1, tools };
  return elements(ClassInspector({ c, bp: { knowledge: { mode: "remote" }, connections: [], agent_classes: [c] }, onChange }));
}

test("older tool summaries stay compatible and do not count policy as a category", () => {
  const old = grants();
  assert.deepEqual(toolSummary(old), toolSummary({ ...old, read_only: false }));
  assert.deepEqual(toolSummary({}), []);
  assert.deepEqual(toolSummary({ read_only: true }), ["Native read-only policy (declared)"]);
});

test("read-only summary distinguishes declaration from every retained grant", () => {
  const tools = { ...grants(), read_only: true }, before = structuredClone(tools);
  assert.deepEqual(toolSummary(tools), ["Native read-only policy (declared)", ...toolSummary(grants()).map(item => `Grant: ${item}`)]);
  assert.deepEqual(tools, before);
});

test("actual inspector toggle preserves mutable grants and makes no topology patch", () => {
  const original = grants(), before = structuredClone(original), changes = [];
  let tools = original;
  for (const value of [true, false]) {
    const toggle = inspector(tools, patch => changes.push(patch)).find(item => item.type === Toggle && item.props.title === "Native read-only policy");
    assert.ok(toggle);
    assert.equal(toggle.props.value, tools.read_only ?? false);
    toggle.props.onChange(value);
    const patch = JSON.parse(JSON.stringify(changes.at(-1)));
    assert.deepEqual(patch, { tools: { ...before, read_only: value } });
    tools = patch.tools;
  }
  assert.deepEqual(original, before);
});

test("editing a category under read-only retains its native policy and other grants", () => {
  const original = { ...grants(), read_only: true }, changes = [];
  const toggle = inspector(original, patch => changes.push(patch)).find(item => item.type === Toggle && item.props.title === "Shell execution");
  toggle.props.onChange(false);
  assert.deepEqual(JSON.parse(JSON.stringify(changes[0])), { tools: { ...original, shell: false } });
  assert.equal(original.shell, true);
});
