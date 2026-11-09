"use strict";
const assert = require("node:assert/strict");
const test = require("node:test");
const { _oracles } = require("./real-images.cjs");
const accepted = { interaction_id: "requested", input_frame_id: "input-requested" };
const done = { kind: "interaction_complete", interaction_id: "requested", payload: { result: "Image operation complete" } };

test("a committed artifact and another interaction's success are not completion", () => {
  assert.equal(_oracles.interactionCompletion([{ kind: "assistant_image", payload: { blob_id: "blob" } }, { ...done, interaction_id: "other" }], accepted, "Image operation complete"), null);
});
test("an image followed by stopped continuation fails even if an old completion exists", () => {
  assert.throws(() => _oracles.interactionCompletion([done, { kind: "text_complete", interaction_id: "requested", payload: { content: "Acceptance scenario stopped: malformed generated result" } }], accepted, "Image operation complete"), /stopped/);
});
test("completion belongs to the accepted interaction and original input", () => {
  assert.throws(() => _oracles.interactionCompletion([{ kind: "user_input", id: accepted.input_frame_id, status: "delivery_failed" }, done], accepted, "Image operation complete"), /delivery_failed/);
});
test("matching instruction and image in different user messages cannot prove ingress", () => {
  const text = { type: "text", text: "inspect" }, image = { type: "image", media_type: "image/png", data: "abc" };
  assert.equal(_oracles.modelImageIngress([{ messages: [{ role: "user", content: [text] }, { role: "user", content: [image] }] }], "inspect", "abc"), null);
  assert(_oracles.modelImageIngress([{ model: "fixture", messages: [{ role: "user", content: [text, image] }] }], "inspect", "abc"));
});
test("tool use id in a call is not a returned image result", () => {
  assert.throws(() => _oracles.imageToolEvidence([{ messages: [{ role: "block_assistant", blocks: [{ block_type: "tool_use", data: { id: "call", name: "generate_image" } }] }] }], "call", "blob"), /result/);
});
test("a successful result for a different blob cannot satisfy image continuation", () => {
  const result = { tool_use_id: "call", is_error: false, content: [{ type: "text", text: JSON.stringify({ terminal: { terminal: "generated" }, images: [{ image_id: "image", blob_ref: { blob_id: "other", media_type: "image/png" } }] }) }] };
  assert.throws(() => _oracles.imageToolEvidence([{ messages: [{ role: "tool_results", results: [result] }] }], "call", "blob"), /blob/);
});
test("three open tasks and two reversed edges cannot satisfy dependency proof", () => {
  const items = ["Review diagram", "Review badge", "Publish report"].map((title, i) => ({ id: String(i), title, status: "open", machine_state: { claim_owner_key: null } }));
  const graph = { items, edges: [{ kind: "blocks", from_id: "2", to_id: "0" }, { kind: "blocks", from_id: "2", to_id: "1" }] };
  assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /prerequisite/);
  graph.edges.forEach(edge => { [edge.from_id, edge.to_id] = [edge.to_id, edge.from_id]; });
  assert.deepEqual(_oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }).map(item => item.id), ["0", "1", "2"]);
  items[0].machine_state.claim_owner_key = "some-owner";
  assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /unclaimed/);
});

test("unscheduled WorkGraph proof rejects explicit epoch and future scheduling in owner state", () => {
  const items = ["Review diagram", "Review badge", "Publish report"].map((title, i) => ({ id: String(i), title, status: "open", machine_state: { claim_owner_key: null } }));
  const graph = { items, edges: [{ kind: "blocks", from_id: "0", to_id: "2" }, { kind: "blocks", from_id: "1", to_id: "2" }] };
  for (const field of ["due_at", "not_before", "snoozed_until"]) {
    for (const value of ["1970-01-01T00:00:00Z", "9999-12-31T23:59:59Z"]) {
      items[0][field] = value;
      assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /unscheduled/, `${field}=${value} is an explicit date, not an absent schedule`);
    }
    delete items[0][field];
    items[0].machine_state[`${field}_utc_ms`] = 0;
    assert.throws(() => _oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }), /unscheduled/, "typed epoch millis must not be treated as falsy absence");
    items[0].machine_state[`${field}_utc_ms`] = null;
  }
  assert.equal(_oracles.assertOpenGraph(graph, { items: items.slice(0, 2) }).length, 3);
});

test("live model cannot silently introduce scheduling and clear it before final inspection", () => {
  const create = { payload: { name: "workgraph_create", args: { title: "Review diagram", realm_id: "mob.console-acceptance" } } };
  assert.doesNotThrow(() => _oracles.assertUnscheduledGraphCalls([create]));
  for (const name of ["workgraph_create", "workgraph_update"]) {
    for (const field of ["due_at", "not_before", "snoozed_until"]) {
      assert.throws(() => _oracles.assertUnscheduledGraphCalls([{ payload: { name, args: { [field]: "1970-01-01T00:00:00Z" } } }]), /unscheduled/);
    }
  }
});
