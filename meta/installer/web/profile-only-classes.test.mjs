import test from "node:test";
import assert from "node:assert/strict";
import { canResizeClass, classInstanceLimit, identities, removeClass, resizeClass } from "./state.mjs";
import { createSimulation, simulationSourceKey } from "./simulation-state.mjs";
import { createSimulationScene } from "./simulation-scene.mjs";
import { classCountRequest, planGraph } from "./evolving-plan-state.mjs";

function agent(id, extra = {}) {
  return {id, name: id, description: "Continuing domain responsibility for " + id,
    instructions: "Own the actual adopted context and retain its evidence.",
    provider: "openai", model: "gpt-6.1-sol", instances: 1, members: [],
    tools: {workspace: false, notes: "none", builtins: false, shell: false, memory: true,
      workgraph: false, schedule: false, knowledge: false, mcp: []}, ...extra};
}

function plan(extra = {}) {
  return {project: {name: "Actual context owners"}, agent_classes: [
    agent("lead"), agent("resident", {instances: 0})],
    topology: {mode: "manual", entrypoint: "lead:main", edges: []},
    connections: [], knowledge: {mode: "none"}, flows: [], ...extra};
}

test("zero classes remain exact cloned profile intent, never agent nodes or activity", () => {
  const source = plan(), before = structuredClone(source);
  const model = createSimulation(source, "__overview__");
  assert.deepEqual(identities(source).map(item => item.id), ["lead:main"]);
  assert.deepEqual(model.profile_only_classes, [source.agent_classes[1]]);
  assert.equal(model.nodes.filter(node => node.kind === "agent").length, 1);
  assert.ok(!model.nodes.some(node => node.id.startsWith("resident:")));
  assert.ok(!model.edges.some(edge => edge.source.startsWith("resident:") || edge.target.startsWith("resident:")));
  assert.ok(model.phases.every(phase => phase.activeNodeIds.every(id => !id.startsWith("resident:"))));
  const scene = createSimulationScene(model, source);
  assert.equal(scene.population.count, 1);
  assert.deepEqual(scene.profileOnlyClasses, model.profile_only_classes);
  assert.ok(!scene.groups.some(group => group.classId === "resident"));
  model.profile_only_classes[0].tools.shell = true;
  scene.profileOnlyClasses[0].instructions = "Changed local presentation";
  assert.deepEqual(source, before);
});

test("profile-only grant and context edits change the scene request without changing identities", () => {
  const before = plan(), after = structuredClone(before);
  after.agent_classes[1].tools.notes = "read";
  after.agent_classes[1].instructions += " Only that actual owner's preferences.";
  const first = createSimulation(before), second = createSimulation(after);
  assert.notEqual(simulationSourceKey(first, before), simulationSourceKey(second, after));
  assert.deepEqual(second.nodes, first.nodes);
  assert.deepEqual(second.edges, first.edges);
  assert.deepEqual(second.phases, first.phases);
  assert.equal(second.profile_only_classes[0].tools.notes, "read");
  const legacy = plan({agent_classes: [agent("lead")]});
  assert.equal("profile_only_classes" in createSimulation(legacy), false);
});

test("a non-referenced class can reach zero without retargeting unrelated work", () => {
  const source = plan({agent_classes: [agent("lead"), agent("resident", {members: [
    {key: "alice", name: "Alice", instructions: "Own Alice's actual context only."}]}), agent("review")]});
  source.topology.edges = [{a: "lead:main", b: "resident:alice"}, {a: "lead:main", b: "review:main"}];
  source.flows = [{id: "review", steps: [{id: "check", agent: "review:main", depends_on: []}]}];
  const before = structuredClone(source);
  assert.equal(canResizeClass(source, "resident", 0), true);
  const next = resizeClass(source, "resident", 0);
  assert.equal(next.agent_classes[1].instances, 0);
  assert.deepEqual(next.agent_classes[1].members, []);
  assert.deepEqual(next.topology.edges, [{a: "lead:main", b: "review:main"}]);
  assert.deepEqual(next.flows, source.flows);
  assert.equal(next.topology.entrypoint, "lead:main");
  assert.deepEqual(source, before);
});

test("count transitions refuse to remove the entrypoint, last real identity or fixed flow target", () => {
  for (const source of [
    plan(),
    plan({agent_classes: [agent("lead")]}),
    plan({agent_classes: [agent("lead"), agent("resident")],
      flows: [{id: "job", steps: [{id: "step", agent: "resident:main", depends_on: []}]}]}),
  ]) {
    const classId = source.flows.length ? "resident" : "lead";
    assert.equal(canResizeClass(source, classId, 0), false);
    assert.deepEqual(resizeClass(source, classId, 0), source);
  }
  assert.deepEqual(removeClass(plan(), "lead"), plan());
});

test("shrinking named owners does not hand another owner's fixed work to a survivor", () => {
  const owners = agent("resident", {instances: 2, members: [
    {key: "alice", name: "Alice", instructions: "Own Alice's private context only."},
    {key: "bob", name: "Bob", instructions: "Own Bob's private context only."}]});
  const source = plan({agent_classes: [agent("lead"), owners],
    flows: [{id: "job", steps: [{id: "read", agent: "resident:bob", depends_on: []}]}]});
  assert.equal(canResizeClass(source, "resident", 1), false);
  assert.deepEqual(resizeClass(source, "resident", 1), source);
  source.flows = [];
  source.topology.entrypoint = "resident:bob";
  assert.equal(canResizeClass(source, "resident", 1), false);
  assert.deepEqual(resizeClass(source, "resident", 1), source);
});

test("a reusable class can gain a real initial identity without losing its class grants", () => {
  const source = plan(), before = structuredClone(source);
  source.agent_classes[1].tools.notes = "read";
  const next = resizeClass(source, "resident", 1);
  assert.equal(next.agent_classes[1].instances, 1);
  assert.equal(next.agent_classes[1].tools.notes, "read");
  assert.deepEqual(identities(next).map(item => item.id), ["lead:main", "resident:main"]);
  assert.equal("profile_only_classes" in createSimulation(next), false);
  assert.equal(before.agent_classes[1].instances, 0);
  for (const count of [-1, NaN, 0.5, Infinity]) {
    assert.equal(canResizeClass(source, "resident", count), false);
    assert.deepEqual(resizeClass(source, "resident", count), source);
  }
});

test("zero classes cannot acquire one identity beyond the existing shared capacity", () => {
  const source = plan({agent_classes: [
    ...Array.from({length: 8}, (_, index) => agent("c" + index, {instances: 512})),
    agent("resident", {instances: 0})],
    topology: {mode: "hub", entrypoint: "c0:1", edges: []}});
  assert.equal(classInstanceLimit(source, "resident"), 0);
  assert.equal(resizeClass(source, "resident", 1).agent_classes.at(-1).instances, 0);
  assert.equal(identities(resizeClass(source, "resident", 1)).length, 4096);
});

test("guided zero intent keeps the class without an unassigned owner or invented route", () => {
  const request = classCountRequest(plan().agent_classes[1], 0);
  assert.match(request, /reusable class with zero initial identities and empty members/);
  assert.match(request, /Preserve an actual native entrypoint and valid fixed workflow targets/);
  assert.match(request, /Do not replace real owners/);
  const graph = planGraph(plan());
  const profile = graph.nodes.find(node => node.classId === "resident");
  assert.equal(profile.kind, "profile");
  assert.equal(profile.count, 0);
  assert.ok(!graph.edges.some(edge => edge.kind === "communication" && (edge.a === profile.id || edge.b === profile.id)));
});


test("raw valid zero classes project their exact omitted-default source without invented fields", () => {
  const source = plan();
  source.agent_classes[1] = {id: "resident", name: "Future resident", description: "Future context owner",
    instructions: "Own one actual resident's continuing context.", instances: 0};
  const before = structuredClone(source), model = createSimulation(source);
  assert.deepEqual(model.profile_only_classes, [source.agent_classes[1]]);
  assert.equal("tools" in model.profile_only_classes[0], false);
  assert.equal("provider" in model.profile_only_classes[0], false);
  assert.deepEqual(model.nodes.filter(node => node.kind === "agent").map(node => node.id), ["lead:main"]);
  assert.deepEqual(source, before);
});
