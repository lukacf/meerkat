import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";

const bundled = await build({
  stdin: { contents: 'export { SetupRequirementGroups, EvolvingPlan } from "./evolving-plan.jsx";\nexport { default as React } from "react";\nexport { renderToStaticMarkup } from "react-dom/server";',
    resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "requirement-group-render.mjs" },
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
  banner: { js: `import { createRequire as createNodeRequire } from "node:module"; const require = createNodeRequire(${JSON.stringify(import.meta.url)});` },
  loader: { ".css": "empty" },
  nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
});
const { React, renderToStaticMarkup, SetupRequirementGroups, EvolvingPlan } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text + "\n//# sourceURL=requirement-group-render.mjs").toString("base64")}`);
const previousMatchMedia = globalThis.matchMedia;
test.before(() => { globalThis.matchMedia = () => ({ matches: false }); });
test.after(() => {
  if (previousMatchMedia === undefined) delete globalThis.matchMedia;
  else globalThis.matchMedia = previousMatchMedia;
});

const state = {
  id: "grouped-proposal", version: 5, status: "refining",
  readiness: { ready: false, context_ready: true, decision_blockers: ["Choose the operating owner"],
    implementation_blockers: ["Build the requested staff page"] },
  brief: { facts: {}, requirements: [
    { id: "dependency", disposition: "prerequisite", outcome: "Configure local credentials" },
    { id: "later", disposition: "deferred", outcome: "A later channel", extension: "Build the later adapter" },
    { id: "removed", disposition: "cancelled", outcome: "A removed integration" },
  ] },
  warnings: ["Verify the actual connection"],
};

const render = (Component, props) => renderToStaticMarkup(React.createElement(Component, props));

test("shared requirement component renders separate decision, application and other-check groups", () => {
  const before = structuredClone(state), html = render(SetupRequirementGroups, { state });
  for (const label of ["Decisions to make", "Application work to build", "Other checks"])
    assert.ok(html.includes(`aria-label="${label}"`));
  assert.ok(html.indexOf("Choose the operating owner") < html.indexOf("Application work to build"));
  assert.ok(html.indexOf("Build the requested staff page") < html.indexOf("Other checks"));
  assert.ok(html.includes("Configure local credentials") && html.includes("Verify the actual connection"));
  assert.ok(!html.includes("A later channel") && !html.includes("A removed integration"));
  assert.ok(!html.includes("ready to package") && !html.includes("running"));
  assert.deepEqual(state, before);
});

test("empty groups render nothing and source strings remain escaped", () => {
  assert.equal(render(SetupRequirementGroups, { state: { readiness: {
    decision_blockers: [], implementation_blockers: [] } } }), "");
  const html = render(SetupRequirementGroups, { groups: {
    decisions: ["<script>wrong()</script>"], implementation: [], checks: [] } });
  assert.ok(html.includes("&lt;script&gt;wrong()&lt;/script&gt;"));
  assert.ok(!html.includes("<script>"));
  assert.ok(!html.includes("Application work to build") && !html.includes("Other checks"));
});

test("evolving plan keeps its grouped assessment alongside scope decisions and simulation", () => {
  const plan = { project: { name: "Shared proposal", description: "A proposed staff helper." },
    target: "mac", agent_classes: [], connections: [], flows: [], topology: { mode: "manual", entrypoint: "", edges: [] },
    knowledge: { mode: "none" }, storage: "sqlite", advanced: { port: 4110 } };
  const html = render(EvolvingPlan, { state, plan });
  assert.ok(html.includes("Animated simulation of your current proposal"));
  assert.ok(html.includes("Your scope decisions") && html.includes("A later channel") && html.includes("A removed integration"));
  assert.ok(html.includes("Decisions to make") && html.includes("Application work to build") && html.includes("Other checks"));
  assert.ok(!html.includes("Accepted plan - ready to package"));
});

test("later application progress preserves historical design-assessment wording", () => {
  const plan = { project: { name: "Shared proposal" }, target: "mac", agent_classes: [], connections: [], flows: [],
    topology: { mode: "manual", entrypoint: "", edges: [] }, knowledge: { mode: "none" }, advanced: { port: 4110 } };
  const html = render(EvolvingPlan, { state, plan, applicationProgress: { title: "Current source needs a fresh check" } });
  assert.ok(html.includes("Guide&#x27;s last design assessment"));
  assert.ok(html.includes("These are design requirements. Use the build and installation checks to see what is implemented and verified."));
  assert.ok(!html.includes("checks show the later results"));
  assert.ok(html.includes("Current source needs a fresh check"));
  assert.ok(html.includes("Application work to build") && html.includes("Decisions to make"));
});

test("an early proposal without a plan renders the same typed groups", () => {
  const html = render(EvolvingPlan, { state });
  assert.ok(html.includes("Decisions to make") && html.includes("Application work to build") && html.includes("Other checks"));
  assert.ok(!html.includes("To work out together"));
});

test("untouched starter does not turn missing core facts into an opening checklist", () => {
  const opening = { id: "new-guide", version: 1, status: "question", stage: "purpose",
    blueprint: null, proposal: null, brief: { facts: {}, requirements: [] }, feature_gaps: [], warnings: [],
    messages: [{ role: "assistant", text: "Describe what you want your system to do." }],
    readiness: { ready: false, context_ready: false, implementation_blockers: [], decision_blockers: [
      "Understand a concrete first job and useful output before designing the final team.",
      ...["audience", "interfaces", "ownership", "workflow", "inputs", "knowledge", "authority", "destination"]
        .map(topic => `Resolve ${topic} or show an explicit proposed default.`),
    ] } };
  const before = structuredClone(opening), html = render(EvolvingPlan, { state: opening });
  assert.ok(html.includes("Describe your idea") && html.includes("Your idea, brought to life."));
  assert.ok(!html.includes("Decisions to make") && !html.includes("Application work to build") && !html.includes("Other checks"));
  assert.ok(!html.includes("Resolve audience") && !html.includes("Understand a concrete first job"));
  assert.deepEqual(opening, before);
  const understood = { ...opening, brief: { facts: { outcome: {
    summary: "Compare supplied offers with cited evidence.", status: "confirmed", source_message_ids: ["user-request"],
  } }, requirements: [] }, messages: [...opening.messages, { role: "user", id: "user-request", text: "Compare these offers." }] };
  const contextualHtml = render(EvolvingPlan, { state: understood });
  assert.ok(contextualHtml.includes("Decisions to make") && contextualHtml.includes("Resolve audience"));
});

test("pre-build progress frames requirements without claiming later check results", () => {
  const plan = { project: { name: "Shared proposal" }, target: "mac", agent_classes: [], connections: [], flows: [],
    topology: { mode: "manual", entrypoint: "", edges: [] }, knowledge: { mode: "none" }, advanced: { port: 4110 } };
  const html = render(EvolvingPlan, { state, plan, applicationProgress: { title: "You can start building" } });
  assert.ok(html.includes("You can start building") && html.includes("Guide&#x27;s last design assessment"));
  assert.ok(html.includes("These are design requirements. Use the build and installation checks to see what is implemented and verified."));
  assert.ok(!html.includes("checks show the later results") && !html.includes("Accepted plan - ready to package"));
});
