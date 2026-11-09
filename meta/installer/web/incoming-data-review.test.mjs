import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";

const result = await build({
  stdin: { contents: 'export {IncomingDataReview} from "./incoming-data-review.jsx"; export {default as React} from "react"; export {renderToStaticMarkup} from "react-dom/server";',
    resolveDir: fileURLToPath(new URL(".", import.meta.url)), sourcefile: "incoming-review.mjs" },
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
  banner: {js: `import {createRequire} from "node:module"; const require=createRequire(${JSON.stringify(import.meta.url)});`},
});
const {IncomingDataReview, React, renderToStaticMarkup} = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}`);
const source = {id: "project", requirement_id: "updates", name: "Project changes", scope: "Selected project spaces",
  purpose: "Notice decisions that need attention", audience: "Project members", history: "new_only", routes: [
    {recipient: "owner:main", delivery: "direct"}, {recipient: "triage:main", delivery: "triage"}]};
const plan = {agent_classes: [{id: "owner", name: "Project owner", instances: 1}, {id: "triage", name: "Intake specialist", instances: 1}]};
function props(patch = {}, disposition = "open") {
  return {plan, brief: {incoming_data: [{...source, ...patch}], requirements: [{id: "updates", disposition,
    extension: "Connect and verify the selected sources"}]}};
}
const render = value => renderToStaticMarkup(React.createElement(IncomingDataReview, value));

test("empty briefs do not add an opening checklist", () => {
  assert.equal(render({}), "");
  assert.equal(render({brief: {incoming_data: []}}), "");
});

test("source review explains desired recipients and keeps access and implementation outstanding", () => {
  const value = props(), before = structuredClone(value), html = render(value);
  for (const label of ["Project owner", "receives matching updates directly", "Intake specialist", "judges relevance",
    "Project members", "Start with new changes", "Collection and access still need", "Connect and verify"])
    assert.ok(html.includes(label), `Render ${label}`);
  assert.equal(html.includes("<details open"), false);
  assert.deepEqual(value, before);
});


test("a zero-start class is explicit about future members without claiming a connected population", () => {
  const value = props({routes: [{recipient: "owner", recipient_kind: "class", delivery: "direct"}]});
  value.plan = {agent_classes: [{id: "owner", name: "Project specialists", instances: 0}]};
  const before = structuredClone(value), html = render(value);
  assert.ok(html.includes("Project specialists - all current and future members"));
  assert.ok(html.includes("Includes an agent class"));
  assert.ok(html.includes("Collection and access still need their setup and checks"));
  assert.equal(html.includes("1 recipient"), false);
  assert.deepEqual(value, before);
});

test("unknown scope decisions remain visible and deferred sources are not presented as active", () => {
  const html = render(props({audience: "", routes: [], history: "undecided"}, "deferred"));
  assert.ok(html.includes("Outside this version"));
  assert.ok(html.includes("Recipient to decide") === false);
  assert.ok(html.includes("Still to decide"));
  assert.ok(html.includes("Kept here as part of your scope decisions"));
});

test("provider text is escaped and a named person's identity remains readable", () => {
  const value = props({name: "<img src=x onerror=bad()>", routes: [{recipient: "owner:alice", delivery: "conversation"}],
    history: "bounded_history", history_window: "Only this week's project history"});
  value.plan = {agent_classes: [{id: "owner", instances: 2, members: [{key: "alice", name: "Alice's assistant"}]}]};
  const html = render(value);
  assert.ok(html.includes("&lt;img"));
  assert.equal(html.includes("<img"), false);
  assert.ok(html.includes("Alice&#x27;s assistant"));
  assert.ok(html.includes("participates in the conversation"));
  assert.ok(html.includes("Only this week&#x27;s project history"));
});

test("refinement dispatches only a conversation request, preserving the source intent", () => {
  const sent = [], value = props(), before = structuredClone(value);
  const tree = IncomingDataReview({...value, onRequestChange: text => sent.push(text)});
  function visit(node) {
    if (!node || typeof node !== "object") return;
    if (node.type === "button") node.props.onClick();
    const children = node.props?.children;
    for (const child of Array.isArray(children) ? children.flat(Infinity) : [children]) visit(child);
  }
  visit(tree);
  assert.equal(sent.length, 1);
  assert.ok(sent[0].includes("Project changes (project)"));
  assert.ok(sent[0].includes("multiple-choice question"));
  assert.deepEqual(value, before);
  assert.ok(render({...value, disabled: true, onRequestChange() {}}).includes('disabled=""'));
});
