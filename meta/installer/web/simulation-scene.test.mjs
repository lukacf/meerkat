import assert from "node:assert/strict";
import test from "node:test";
import { createSimulation } from "./simulation-state.mjs";
import { createSimulationScene } from "./simulation-scene.mjs";

const member = (id, name = id, tools = {}) => ({
  id,
  name,
  description: `Own ${name}`,
  instructions: `Maintain responsibility for ${name}.`,
  instances: 1,
  members: [],
  tools: { memory: true, mcp: [], ...tools },
});
const plan = (agentClasses, flows = [], extra = {}) => ({
  agent_classes: agentClasses,
  flows,
  topology: {
    mode: "hub",
    entrypoint: `${agentClasses[0].id}:main`,
    edges: [],
  },
  knowledge: { mode: "none" },
  connections: [],
  ...extra,
});

function checkReferences(scene, model) {
  const ids = new Set(scene.nodes.map((node) => node.id));
  assert.equal(ids.size, scene.nodes.length);
  assert.ok(model.nodes.every((node) => ids.has(node.id)));
  for (const node of scene.nodes) {
    assert.ok(Number.isFinite(node.x) && Number.isFinite(node.y), node.id);
    assert.ok(
      node.x >= 0 && node.x <= scene.W && node.y >= 0 && node.y <= scene.H,
      node.id,
    );
    assert.equal(node.born, undefined);
    assert.equal(node.die, undefined);
  }
  const original = new Map(model.edges.map((edge) => [edge.id, edge]));
  assert.equal(
    scene.edgeGroups.reduce((sum, group) => sum + group.count, 0),
    model.edges.length,
  );
  for (const group of scene.edgeGroups) {
    assert.ok(
      group.relationships.some(
        (edge) => edge.source === group.source && edge.target === group.target,
      ),
    );
    for (const edge of group.relationships) {
      assert.equal(original.get(edge.id).source, edge.source);
      assert.equal(original.get(edge.id).target, edge.target);
    }
  }
  for (const event of scene.script) {
    assert.ok(event.t < scene.duration);
    for (const id of event.on || []) assert.ok(ids.has(id));
    if (event.msg) {
      assert.ok(event.msg.every((id) => ids.has(id)));
      const exact = original.get(event.edgeId);
      assert.deepEqual(event.msg, [exact.source, exact.target]);
    }
  }
}

function schoolPlan() {
  const schools = ["Oak", "Birch", "Pine"];
  const classes = [
    member("lead", "Operations lead"),
    ...["assistant", "documents"].map((id) => ({
      ...member(
        id,
        id === "assistant" ? "School assistant" : "Document specialist",
        id === "documents"
          ? { mcp: [{ connection_id: "drive", tool_names: ["read_file"] }] }
          : { notes: "read" },
      ),
      instances: 3,
      members: schools.map((school) => ({
        key: school.toLowerCase(),
        name: `${school} ${id === "assistant" ? "assistant" : "document specialist"}`,
        instructions: `Own the ${school} responsibilities.`,
      })),
    })),
  ];
  const steps = [
    ...schools.map((school) => ({
      id: `read_${school.toLowerCase()}`,
      agent: `documents:${school.toLowerCase()}`,
      prompt: `Read ${school} source documents.`,
      depends_on: [],
    })),
    ...schools.map((school) => ({
      id: `assess_${school.toLowerCase()}`,
      agent: `assistant:${school.toLowerCase()}`,
      prompt: `Assess ${school} open cases.`,
      depends_on: [`read_${school.toLowerCase()}`],
    })),
    {
      id: "summary",
      agent: "lead:main",
      prompt: "Summarize all original evidence and school assessments.",
      depends_on: schools.flatMap((school) => [
        `read_${school.toLowerCase()}`,
        `assess_${school.toLowerCase()}`,
      ]),
    },
  ];
  return plan(classes, [{ id: "morning", name: "Morning review", steps }], {
    connections: [
      {
        id: "drive",
        name: "Google Drive",
        description: "Read school source files.",
        transport: "stdio",
      },
    ],
  });
}

test("school flow lays out document sources, parallel specialists, school owners, and summary in native order", () => {
  const model = createSimulation(schoolPlan());
  const scene = createSimulationScene(model);
  const node = (id) => scene.nodes.find((item) => item.id === id);
  for (const school of ["oak", "birch", "pine"]) {
    assert.ok(
      node("resource:connector:drive").x < node(`documents:${school}`).x,
    );
    assert.ok(node(`documents:${school}`).x < node(`assistant:${school}`).x);
    assert.ok(node(`assistant:${school}`).x < node("lead:main").x);
    assert.equal(node(`documents:${school}`).kind, "agent");
    assert.equal(node(`documents:${school}`).sub, `documents:${school}`);
  }
  assert.equal(
    new Set(
      ["oak", "birch", "pine"].map((school) => node(`documents:${school}`).x),
    ).size,
    1,
  );
  assert.equal(node("boundary:console:input").sub, "Human interaction");
  assert.equal(scene.edges.some(([source, target]) => [source, target].includes("boundary:console:input")), false);
  assert.equal(scene.script.some(event => event.msg?.includes("boundary:console:input")), false);
  assert.equal(node("resource:local:notes").kind, "mem");
  assert.ok(node("resource:local:notes").y > node("lead:main").y);
  assert.equal(node("boundary:console:output").kind, "ext");
  assert.equal(node("boundary:console:output").sub, "Native flow output");
  assert.equal(scene.clock, undefined);
  checkReferences(scene, model);
});

test("an everyday custom UI and separate native administrator do not animate an invented contact or response path", () => {
  const value = plan([member("operator", "Native administrator"), member("personal", "Personal context owner"), member("specialist", "Domain specialist")]);
  value.agent_classes[1].instructions = "Own the person's conversation and present the specialist's findings.";
  const fact = { status: "confirmed", summary: "People use their personal browser accounts. The native administrator Console is separate.", source_message_ids: ["custom-ui", "separate-admin"] };
  const brief = {facts: {interfaces: fact, ownership: {status: "confirmed",
    summary: "The personal owner receives questions and returns a coherent answer.", source_message_ids: ["owner-choice"]}}, requirements: []};
  const model = createSimulation(value, "__overview__", fact, brief);
  const before = JSON.stringify([value, model]);
  const scene = createSimulationScene(model, value);
  const input = scene.nodes.find(node => node.semanticKind === "input");
  const output = scene.nodes.find(node => node.semanticKind === "output");
  assert.equal(input.sub, "Human interaction");
  assert.equal(output.sub, "Intended result");
  assert.doesNotMatch(JSON.stringify([input, output]), /unresolved/);
  assert.deepEqual(model.design_context.facts.ownership, brief.facts.ownership);
  assert.doesNotMatch(JSON.stringify([input, output]), /MobKit console|Console answer/);
  assert.equal(scene.edges.some(([source, target]) => [source, target].some(id => id === input.id || id === output.id)), false);
  assert.equal(scene.script.some(event => event.msg?.some(id => id === input.id || id === output.id)), false);
  assert.ok(scene.edgeGroups.some(group => group.kind === "peer"));
  assert.equal(model.entrypoint, "operator:main");
  assert.equal(JSON.stringify([value, model]), before);
  checkReferences(scene, model);
});

test("script maps exact phases to4.2second ranges and preserves simultaneous work", () => {
  const model = createSimulation(schoolPlan());
  const scene = createSimulationScene(model);
  assert.equal(scene.duration, model.phases.length * 4.2);
  const parallel = model.phases.find(
    (phase) => phase.kind === "work" && phase.stepIds.length === 3,
  );
  const offset = scene.phaseOffsets.find((phase) => phase.id === parallel.id);
  const event = scene.script.find(
    (item) => item.phaseId === parallel.id && item.on,
  );
  assert.deepEqual(event.on, parallel.activeNodeIds);
  assert.equal(offset.start, offset.index * 4.2);
  assert.ok(Math.abs(offset.end - offset.start - 4.2) < 1e-9);
  assert.equal(
    scene.script.filter((item) => item.msg && item.phaseId === parallel.id)
      .length,
    0,
  );
  assert.ok(scene.script.some((item) => item.msg && item.c === "ink"));
  assert.ok(scene.script.some((item) => item.msg && item.c === "accent"));
  checkReferences(scene, model);
});

test("500 durable members are rendered in labelled rings around their actual owner with exact identities", () => {
  const value = plan([
    member("desk", "Desk owner"),
    {
      ...member("instrument", "Instrument specialist"),
      instances: 500,
      members: Array.from({ length: 500 }, (_, index) => ({
        key: `asset_${index}`,
        name: `Instrument ${index}`,
        instructions: `Track asset ${index}.`,
      })),
    },
  ]);
  const model = createSimulation(value);
  const scene = createSimulationScene(model);
  const group = scene.groups.find((item) => item.classId === "instrument");
  assert.equal(group.count, 500);
  assert.equal(group.renderedCount, 500);
  assert.equal(group.representedCount, 500);
  assert.equal(group.members.length, 500);
  assert.deepEqual(group.members[499], {
    id: "instrument:asset_499",
    name: "Instrument 499",
  });
  assert.equal(scene.nodes.filter((node) => node.dense).length, 500);
  assert.equal(
    scene.nodes.some(
      (node) => node.kind === "worker" || node.kind === "helper",
    ),
    false,
  );
  const owner = scene.nodes.find((node) => node.id === "desk:main");
  assert.equal(owner.kind, "coord");
  for (const node of scene.nodes.filter((item) => item.dense)) {
    assert.ok(
      Math.abs(
        Math.hypot(node.x - owner.x, node.y - owner.y) - node.ringRadius,
      ) < 1e-8,
    );
    assert.equal(node.kind, "agent");
    assert.equal(node.mem, true);
  }
  assert.equal(scene.population.sampled, false);
  assert.equal(scene.population.count, 501);
  assert.equal(scene.population.groupedEdges, true);
  assert.equal(
    scene.edgeGroups.find((item) => item.kind === "peer").count,
    500,
  );
  assert.equal(scene.edges.length, 1);
  assert.ok(scene.edgeGroups.every(group => group.kind === "peer"), 'Only actual peer relationships are drawn for this population');
  assert.ok(
    scene.nodes.some(
      (node) => node.kind === "text" && node.label.includes("500"),
    ),
  );
  assert.ok(
    scene.nodes
      .filter((node) => node.dense)
      .every((node) => node.durable === true && node.semanticKind === "agent"),
  );
  checkReferences(scene, model);
});

test("large mesh groups every real permission without inventing endpoints or losing counts", () => {
  const value = plan(
    [
      member("owner"),
      ...["alpha", "beta"].map((id) => ({ ...member(id), instances: 256 })),
    ],
    [],
    {
      topology: { mode: "mesh", entrypoint: "owner:main", edges: [] },
    },
  );
  const model = createSimulation(value);
  const scene = createSimulationScene(model);
  const peers = model.edges.filter((edge) => edge.kind === "peer");
  assert.equal(peers.length, (513 * 512) / 2);
  assert.equal(
    scene.edgeGroups
      .filter((group) => group.kind === "peer")
      .reduce((sum, group) => sum + group.count, 0),
    peers.length,
  );
  assert.ok(scene.edges.length < 10);
  assert.equal(
    scene.nodes.filter((node) => node.semanticKind === "agent").length,
    513,
  );
  const permission = scene.phaseOffsets.find(
    (phase) => phase.kind === "permissions",
  );
  assert.equal(permission.representedEdgeCount, peers.length);
  assert.ok(permission.animatedEdgeCount < 10);
  checkReferences(scene, model);
});

test("same identity appears once across several workflow steps and retains same-agent evidence", () => {
  const value = plan(
    [member("owner")],
    [
      {
        id: "two",
        name: "Two steps",
        steps: [
          {
            id: "read",
            agent: "owner:main",
            prompt: "Read evidence.",
            depends_on: [],
          },
          {
            id: "write",
            agent: "owner:main",
            prompt: "Write a report using the evidence.",
            depends_on: ["read"],
          },
        ],
      },
    ],
  );
  const model = createSimulation(value);
  const scene = createSimulationScene(model);
  assert.equal(
    scene.nodes.filter((node) => node.id === "owner:main").length,
    1,
  );
  assert.ok(
    scene.edgeGroups.some(
      (edge) =>
        edge.kind === "flow" &&
        edge.source === "owner:main" &&
        edge.target === "owner:main",
    ),
  );
  checkReferences(scene, model);
});

test("layout is deterministic and model data is neither mutated nor borrowed", () => {
  const model = createSimulation(schoolPlan());
  const before = JSON.stringify(model);
  const scene = createSimulationScene(model);
  assert.deepEqual(scene, createSimulationScene(model));
  scene.phaseOffsets[0].activeNodeIds.push("invented");
  scene.groups[0].members[0].name = "Changed";
  scene.edgeGroups[0].relationships[0].label = "Changed";
  assert.equal(JSON.stringify(model), before);
  const empty = createSimulationScene(createSimulation(null));
  assert.equal(empty.nodes.length, 0);
  assert.equal(empty.duration, 4.2);
  assert.equal(empty.population.count, 0);
});
