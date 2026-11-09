import { test } from "node:test";
import assert from "node:assert/strict";
import {
  identities,
  resolvedEdges,
  resizeClass,
  classInstanceLimit,
  PLANNING_LIMITS,
  removeClass,
  removeConnection,
  removeStep,
  hasCycle,
  blueprintDiff,
  buildTopologyNodes,
  flowLayout,
  dedupeEdges,
  slugify,
} from "./state.mjs";
function fixture() {
  return {
    schema_version: 2,
    project: { name: "Team", slug: "team", description: "A useful team" },
    provider: "openai",
    model: "gpt-5.5",
    target: "mac",
    gcp: {},
    advanced: { port: 4110 },
    agent_classes: [
      {
        id: "lead",
        name: "Lead",
        description: "Coordination",
        instances: 1,
        tools: {
          workspace: false,
          notes: "read",
          builtins: false,
          knowledge: false,
          mcp: [{ connection_id: "search", tool_names: [] }],
        },
      },
      {
        id: "research",
        name: "Research",
        description: "Research",
        instances: 2,
        tools: {
          workspace: true,
          notes: "write",
          builtins: false,
          knowledge: false,
          mcp: [{ connection_id: "search", tool_names: ["search"] }],
        },
      },
    ],
    connections: [{ id: "search" }],
    topology: { mode: "hub", entrypoint: "lead:main", edges: [] },
    flows: [
      {
        id: "report",
        steps: [
          { id: "one", agent: "research:1", depends_on: [] },
          { id: "two", agent: "research:2", depends_on: [] },
          { id: "finish", agent: "lead:main", depends_on: ["one", "two"] },
        ],
      },
    ],
    knowledge: { mode: "none" },
  };
}
test("instances map to stable class identities and native topology presets", () => {
  const b = fixture();
  assert.deepEqual(
    identities(b).map((a) => a.id),
    ["lead:main", "research:1", "research:2"],
  );
  assert.equal(resolvedEdges(b).length, 2);
  b.topology.mode = "mesh";
  assert.equal(resolvedEdges(b).length, 3);
  b.topology.mode = "pipeline";
  assert.deepEqual(resolvedEdges(b), [
    { a: "lead:main", b: "research:1" },
    { a: "research:1", b: "research:2" },
  ]);
});
test("changing class population rewrites flow and manual edge references without duplicates", () => {
  const b = fixture();
  b.topology = {
    mode: "manual",
    entrypoint: "research:2",
    edges: [
      { a: "lead:main", b: "research:1" },
      { a: "lead:main", b: "research:2" },
      { a: "research:1", b: "research:2" },
    ],
  };
  const next = resizeClass(b, "research", 1);
  assert.equal(next.topology.entrypoint, "research:main");
  assert.deepEqual(next.topology.edges, [
    { a: "lead:main", b: "research:main" },
  ]);
  assert.equal(next.flows[0].steps[1].agent, "research:main");
  assert.equal(b.agent_classes[1].instances, 2);
  const grown = resizeClass(next, "research", 3);
  assert.equal(grown.flows[0].steps[1].agent, "research:1");
  assert.equal(grown.topology.entrypoint, "research:1");
});
test("deleting a class removes dangling steps, dependencies, edges and entrypoint", () => {
  const b = fixture();
  b.topology.edges = [{ a: "lead:main", b: "research:1" }];
  const next = removeClass(b, "research");
  assert.deepEqual(next.topology.edges, []);
  assert.deepEqual(
    next.flows[0].steps.map((s) => s.id),
    ["finish"],
  );
  assert.deepEqual(next.flows[0].steps[0].depends_on, []);
  assert.equal(removeClass(next, "lead").agent_classes.length, 1);
});

test("manual population editing retains 500+ identities and respects shared capacity", () => {
  const b = fixture();
  const grown = resizeClass(b, "research", 512);
  assert.equal(identities(grown).length, 513);
  assert.equal(grown.agent_classes[1].instances, 512);
  assert.equal(identities(grown).at(-1).id, "research:512");
  assert.equal(resolvedEdges(grown).length, 512);
  assert.equal(b.agent_classes[1].instances, 2);
  assert.equal(resizeClass(grown, "research", 999).agent_classes[1].instances, PLANNING_LIMITS.maxInstancesPerClass);
  assert.deepEqual(resizeClass(grown, "research", NaN), grown);
  assert.deepEqual(resizeClass(grown, "research", 3.5), grown);
  grown.agent_classes = Array.from({ length: 8 }, (_, i) => ({ ...grown.agent_classes[1], id: `class${i}`, instances: 512 }));
  grown.agent_classes[7].instances = 510;
  assert.equal(classInstanceLimit(grown, "class7"), 512);
  assert.equal(identities(resizeClass(grown, "class7", 999)).length, PLANNING_LIMITS.maxIdentities);
  grown.agent_classes.push({ ...grown.agent_classes[0], id: "extra", instances: 1 });
  assert.equal(classInstanceLimit(grown, "class7"), 511);
  assert.equal(resizeClass(grown, "class7", 999).agent_classes[7].instances, 511);
});

test("mesh population editor enforces its edge budget before creating the graph", () => {
  const b = fixture();
  b.topology.mode = "mesh";
  const grown = resizeClass(b, "research", 512);
  assert.equal(grown.agent_classes[1].instances, 511);
  assert.equal(identities(grown).length, 512);
  assert.ok(resolvedEdges(grown).length <= PLANNING_LIMITS.maxPeerEdges);
});
test("removing connections revokes all class grants without changing the input", () => {
  const b = fixture(),
    next = removeConnection(b, "search");
  assert.equal(next.connections.length, 0);
  assert.ok(next.agent_classes.every((c) => c.tools.mcp.length === 0));
  assert.equal(b.agent_classes[0].tools.mcp.length, 1);
});
test("removing workflow steps also removes dependent references", () => {
  const next = removeStep(fixture().flows[0], "one");
  assert.deepEqual(next.steps[1].depends_on, ["two"]);
});
test("cycles are detected regardless of array ordering, parallel branches remain valid", () => {
  const steps = fixture().flows[0].steps;
  assert.equal(hasCycle(steps), false);
  assert.equal(hasCycle([...steps].reverse()), false);
  steps[0].depends_on = ["finish"];
  assert.equal(hasCycle(steps), true);
});
test("DAG canvas gives parallel steps one level and joins only after dependencies", () => {
  const layout = flowLayout(fixture().flows[0].steps);
  const nodes = Object.fromEntries(layout.nodes.map((n) => [n.step.id, n]));
  assert.equal(nodes.one.y, nodes.two.y);
  assert.notEqual(nodes.one.x, nodes.two.x);
  assert.ok(nodes.finish.y > nodes.one.y);
});
test("topology view uses blueprint identities and never claims running agents", () => {
  const b = fixture(),
    nodes = buildTopologyNodes(b);
  assert.equal(nodes[0].identity, "lead:main");
  assert.equal(nodes[0].state, "draft");
  assert.equal(nodes[0].addressable, false);
  assert.deepEqual(nodes[0].wired_to, ["research:1", "research:2"]);
});
test("architecture diff includes removed classes and tool grants", () => {
  const b = fixture(),
    next = removeClass(b, "research");
  next.agent_classes[0].tools.workspace = true;
  const diff = blueprintDiff(b, next);
  assert.ok(diff.includes("Remove Research"));
  assert.ok(diff.includes("Update Lead"));
  assert.ok(diff.includes("Workflows"));
});
test("edge normalization ignores direction duplicates and self edges", () => {
  assert.deepEqual(
    dedupeEdges([
      { a: "a", b: "b" },
      { a: "b", b: "a" },
      { a: "a", b: "a" },
    ]),
    [{ a: "a", b: "b" }],
  );
});
test("project slug normalization has a safe leading letter", () => {
  assert.equal(slugify("123 Team"), "mob-123-team");
  assert.equal(slugify("My Garden!"), "my-garden");
});
test("proposal fingerprint agrees with Python sorted JSON including Unicode", async () => {
  const { fingerprint } = await import("./state.mjs");
  assert.equal(
    await fingerprint({ z: "Zagreb", a: "Luka", city: "Zürich 🐘" }),
    "6616b979f42d4833776838dbace234e144ca03e293e22b7a4df31d3d5540b577",
  );
});

test("server object-key ordering does not invent manual changes", () => {
  const before = fixture();
  const reorder = (value) =>
    Array.isArray(value)
      ? value.map(reorder)
      : value && typeof value === "object"
        ? Object.fromEntries(
            Object.keys(value)
              .sort()
              .map((key) => [key, reorder(value[key])]),
          )
        : value;
  assert.deepEqual(blueprintDiff(before, reorder(before)), []);
  const after = reorder(before);
  after.agent_classes[0].instances = 2;
  assert.deepEqual(blueprintDiff(before, after), ["Update Lead"]);
});
