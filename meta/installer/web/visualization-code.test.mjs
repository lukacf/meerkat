import assert from "node:assert/strict";
import test from "node:test";
import { createSimulation } from "./simulation-state.mjs";
import { createSimulationScene } from "./simulation-scene.mjs";
import { applyVisualizationLayout } from "./visualization-code.mjs";

function fixture(count = 1) {
  const role = (id, instances = 1) => ({ id, name: id, description: `Own ${id}`, instructions: `Take continuing responsibility for ${id}.`, instances, members: [], tools: { memory: true, notes: "read", mcp: [] } });
  const plan = { agent_classes: [role("owner"), role("specialist", count)], topology: { mode: "hub", entrypoint: "owner:main", edges: [] }, connections: [], knowledge: { mode: "none" }, flows: [] };
  const model = createSimulation(plan), base = createSimulationScene(model, plan);
  const layout = { positions: model.nodes.map((node, index) => ({ id: node.id, x: 40 + (index % 40) * 20, y: 40 + Math.floor(index / 40) * 20 })), groups: [] };
  return { model, base, layout };
}

test("generated layout requires every exact identity once and finite coordinates inside the canvas", () => {
  const { model, base, layout } = fixture();
  for (const mutate of [
    (data) => data.positions.pop(),
    (data) => data.positions.push({ ...data.positions[0] }),
    (data) => data.positions.push({ id: "invented", x: 50, y: 50 }),
    (data) => data.positions[0].x = NaN,
    (data) => data.positions[0].y = Infinity,
    (data) => data.positions[0].x = 14,
    (data) => data.positions[0].x = 986,
    (data) => data.positions[0].y = 23,
    (data) => data.positions[0].y = 526,
  ]) {
    const bad = structuredClone(layout); mutate(bad);
    assert.throws(() => applyVisualizationLayout(base, bad, model));
  }
  const exact = applyVisualizationLayout(base, layout, model);
  assert.deepEqual(new Set(exact.nodes.filter((node) => node.kind !== "text").map((node) => node.id)), new Set(model.nodes.map((node) => node.id)));
});

test("groups may contain only existing agents, with no duplicate membership", () => {
  const { model, base, layout } = fixture();
  for (const groups of [
    [{ label: "Invented", memberIds: ["fake:main"] }],
    [{ label: "Resource", memberIds: ["resource:local:notes"] }],
    [{ label: "Empty", memberIds: [] }],
    [{ label: "Duplicate", memberIds: ["owner:main", "owner:main"] }],
    [{ label: "First", memberIds: ["owner:main"] }, { label: "Second", memberIds: ["owner:main"] }],
  ]) assert.throws(() => applyVisualizationLayout(base, { ...layout, groups }, model));
  const result = applyVisualizationLayout(base, { ...layout, groups: [{ label: "Owners", memberIds: ["owner:main", "specialist:main"], count: 800 }] }, model);
  assert.equal(result.groups[0].count, 2);
  assert.equal(result.groups[0].members[1].name, "specialist");
});

test("500+ identities and all exact edges survive generated grouping", () => {
  const { model, base, layout } = fixture(500);
  const ids = model.nodes.filter((node) => node.classId === "specialist").map((node) => node.id);
  layout.groups = [{ label: "Specialists", memberIds: ids, shape: "ring" }];
  const result = applyVisualizationLayout(base, layout, model);
  assert.equal(result.nodes.filter((node) => node.semanticKind === "agent").length, 501);
  assert.equal(result.groups[0].count, 500);
  assert.equal(result.groups[0].representedCount, 500);
  assert.equal(result.population.count, 501);
  assert.deepEqual(result.edges, base.edges);
  assert.deepEqual(result.edgeGroups, base.edgeGroups);
  assert.equal(result.nodes.filter((node) => node.dense).length, 500);
  assert.ok(result.nodes.filter((node) => node.dense).every((node) => node.kind === "agent" && node.durable && node.mem));
  for (const position of layout.positions) {
    assert.equal(result.nodeIndex[position.id].x, position.x);
    assert.equal(result.nodeIndex[position.id].y, position.y);
  }
});

test("generated choreography may use only an active edge in the named phase", () => {
  const { model, base, layout } = fixture();
  const peerPhase = model.phases.find((phase) => phase.kind === "permissions");
  const edgeId = peerPhase.activeEdgeIds[0];
  const event = { phaseId: peerPhase.id, edgeId, offset: 1.25 };
  for (const bad of [
    { ...event, phaseId: "invented" }, { ...event, edgeId: "invented" },
    { ...event, phaseId: model.phases[0].id }, { ...event, offset: -1 },
    { ...event, offset: 0.09 }, { ...event, offset: 2.61 }, { ...event, offset: NaN },
  ]) assert.throws(() => applyVisualizationLayout(base, { ...layout, events: [bad] }, model));
  assert.throws(() => applyVisualizationLayout(base, { ...layout, events: Array.from({ length: 4097 }, () => event) }, model));
  const result = applyVisualizationLayout(base, { ...layout, events: [event] }, model);
  const animation = result.script.find((item) => item.msg);
  assert.equal(animation.t, model.phases.indexOf(peerPhase) * 4.2 + 1.25);
  assert.equal(animation.c, "mid");
  assert.equal(animation.phaseId, peerPhase.id);
  assert.equal(animation.edgeId, edgeId);
  assert.deepEqual(animation.msg, [model.edges.find((edge) => edge.id === edgeId).source, model.edges.find((edge) => edge.id === edgeId).target]);
  assert.deepEqual(result.script.filter((item) => !item.msg), base.script.filter((item) => !item.msg));
});

test("scene return and validation do not mutate or lend nested inputs", () => {
  const { model, base, layout } = fixture(5);
  layout.groups = [{ label: "Specialists", memberIds: model.nodes.filter((node) => node.classId === "specialist").map((node) => node.id) }];
  const before = JSON.stringify({ model, base, layout });
  const result = applyVisualizationLayout(base, layout, model);
  result.groups[0].memberIds.push("invented");
  result.edges[0][2].kind = "invented";
  result.script[0].on.push("invented");
  assert.equal(JSON.stringify({ model, base, layout }), before);
});
