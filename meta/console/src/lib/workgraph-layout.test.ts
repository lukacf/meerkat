import assert from "node:assert/strict";
import test from "node:test";

import {
  WORKGRAPH_GRAPH_COL_WIDTH,
  WORKGRAPH_GRAPH_NODE_CAP,
  WORKGRAPH_GRAPH_ROW_HEIGHT,
  layoutWorkGraph,
  workGraphEdgeMidpoint,
  workGraphEdgePath,
} from "./workgraph-layout";
import type { WorkGraphWireItem } from "../types";

function item(id: string, createdAt: string, extra: Partial<WorkGraphWireItem> = {}): WorkGraphWireItem {
  return { id, title: id, created_at: createdAt, status: "open", revision: 1, ...extra };
}

test("workgraph layout places roots in column 0 and children one column deeper", () => {
  const layout = layoutWorkGraph(
    [
      item("root", "2026-07-08T08:00:00Z"),
      item("child", "2026-07-08T08:10:00Z"),
      item("grandchild", "2026-07-08T08:20:00Z"),
    ],
    [
      // Parent edges run child→parent.
      { kind: "parent", from_id: "child", to_id: "root" },
      { kind: "parent", from_id: "grandchild", to_id: "child" },
    ],
  );
  const byId = new Map(layout.nodes.map((node) => [node.itemId, node]));
  const rootNode = byId.get("root");
  const childNode = byId.get("child");
  const grandchildNode = byId.get("grandchild");
  assert.ok(rootNode && childNode && grandchildNode);
  assert.equal(childNode.x - rootNode.x, WORKGRAPH_GRAPH_COL_WIDTH);
  assert.equal(grandchildNode.x - childNode.x, WORKGRAPH_GRAPH_COL_WIDTH);
  // One structural parent edge per placed child.
  assert.deepEqual(
    layout.edges.map((edge) => [edge.kind, edge.fromId, edge.toId]).sort(),
    [["parent", "child", "root"], ["parent", "grandchild", "child"]],
  );
  assert.equal(layout.overflowCount, 0);
});

test("workgraph layout treats children of unknown parents as roots", () => {
  const layout = layoutWorkGraph(
    [item("orphan", "2026-07-08T08:00:00Z"), item("root", "2026-07-08T07:00:00Z")],
    [{ kind: "parent", from_id: "orphan", to_id: "missing-parent" }],
  );
  const byId = new Map(layout.nodes.map((node) => [node.itemId, node]));
  assert.equal(byId.get("orphan")?.x, byId.get("root")?.x, "orphan lands in the root column");
  // The dangling parent edge is never drawn.
  assert.deepEqual(layout.edges, []);
});

test("workgraph layout rows are deterministic: created_at then id within a column", () => {
  const layout = layoutWorkGraph(
    [
      item("b-late", "2026-07-08T09:00:00Z"),
      item("a-early", "2026-07-08T08:00:00Z"),
      item("c-tie", "2026-07-08T08:00:00Z"),
    ],
    [],
  );
  const rows = layout.nodes.map((node) => node.itemId);
  // Ties break on id, so a-early < c-tie < b-late.
  assert.deepEqual(rows, ["a-early", "c-tie", "b-late"]);
  assert.equal(layout.nodes[1].y - layout.nodes[0].y, WORKGRAPH_GRAPH_ROW_HEIGHT);
});

test("workgraph layout passes blocks edges through as geometry with kind intact", () => {
  const layout = layoutWorkGraph(
    [item("a", "2026-07-08T08:00:00Z"), item("b", "2026-07-08T08:10:00Z")],
    [
      { kind: "blocks", from_id: "a", to_id: "b" },
      // Edges touching unknown items are dropped, not guessed.
      { kind: "blocks", from_id: "a", to_id: "ghost" },
      // Self edges and duplicates get the parent loop's hygiene: a self
      // blocks edge would be a degenerate bezier through the node's own
      // body, a duplicate a stacked double-stroke path.
      { kind: "blocks", from_id: "a", to_id: "a" },
      { kind: "blocks", from_id: "a", to_id: "b" },
    ],
  );
  assert.equal(layout.edges.length, 1);
  const edge = layout.edges[0];
  assert.equal(edge.kind, "blocks");
  assert.equal(edge.fromId, "a");
  assert.equal(edge.toId, "b");
  assert.match(workGraphEdgePath(edge), /^M -?[\d.]+ -?[\d.]+ C /);
  const mid = workGraphEdgeMidpoint(edge);
  assert.ok(Number.isFinite(mid.x) && Number.isFinite(mid.y));
});

test("workgraph layout folds extra parents into alsoUnder instead of extra edges", () => {
  const layout = layoutWorkGraph(
    [
      item("p1", "2026-07-08T08:00:00Z", { title: "First parent" }),
      item("p2", "2026-07-08T08:01:00Z", { title: "Second parent" }),
      item("kid", "2026-07-08T08:02:00Z"),
    ],
    [
      { kind: "parent", from_id: "kid", to_id: "p1" },
      { kind: "parent", from_id: "kid", to_id: "p2" },
    ],
  );
  const kid = layout.nodes.find((node) => node.itemId === "kid");
  assert.deepEqual(kid?.alsoUnder, ["Second parent"]);
  assert.equal(layout.edges.length, 1, "only the placing parent edge is drawn");
  assert.equal(layout.edges[0].toId, "p1");
});

test("workgraph layout caps nodes and reports the surplus", () => {
  const items = Array.from({ length: WORKGRAPH_GRAPH_NODE_CAP + 7 }, (_, index) =>
    item(`item-${String(index).padStart(3, "0")}`, "2026-07-08T08:00:00Z"));
  const layout = layoutWorkGraph(items, []);
  assert.equal(layout.nodes.length, WORKGRAPH_GRAPH_NODE_CAP);
  assert.equal(layout.overflowCount, 7);
  // Deterministic cut: the kept set is the sorted prefix.
  assert.equal(layout.nodes[0].itemId, "item-000");
});

test("workgraph layout counts undrawable items in the overflow, never silently", () => {
  // An id-less row and a duplicate id both fail to draw; the overflow count
  // owns them so overflowCount is always items.length - nodes.length.
  const layout = layoutWorkGraph(
    [
      item("a", "2026-07-08T08:00:00Z"),
      { title: "no id", created_at: "2026-07-08T08:01:00Z" },
      item("a", "2026-07-08T08:02:00Z", { title: "duplicate id" }),
    ],
    [],
  );
  assert.equal(layout.nodes.length, 1);
  assert.equal(layout.overflowCount, 2);
});

test("workgraph layout carries status, owner, priority, and blocked onto nodes", () => {
  const layout = layoutWorkGraph(
    [
      item("busy", "2026-07-08T08:00:00Z", {
        status: "in_progress",
        priority: "high",
        claim: { owner: { key: { kind: "agent", id: "helper" } } },
      }),
      item("stuck", "2026-07-08T08:01:00Z", { status: "blocked" }),
    ],
    [],
  );
  const busy = layout.nodes.find((node) => node.itemId === "busy");
  const stuck = layout.nodes.find((node) => node.itemId === "stuck");
  assert.equal(busy?.status, "in_progress");
  assert.equal(busy?.priority, "high");
  assert.equal(busy?.ownerLabel, "helper");
  assert.equal(busy?.blocked, false);
  assert.equal(stuck?.blocked, true);
});

test("workgraph layout of nothing is a zero-size empty layout", () => {
  const layout = layoutWorkGraph([], []);
  assert.deepEqual(layout, { nodes: [], edges: [], width: 0, height: 0, overflowCount: 0 });
});


test("parallel prerequisites stay on one rank and visibly converge on their dependent", () => {
  const items = [item("source-review", "1"), item("badge-review", "2"), item("publish", "3")];
  const edges = [
    { kind: "blocks", from_id: "source-review", to_id: "publish" },
    { kind: "blocks", from_id: "badge-review", to_id: "publish" },
  ];
  const layout = layoutWorkGraph(items, edges);
  const byId = new Map(layout.nodes.map(node => [node.itemId, node]));
  const source = byId.get("source-review")!;
  const badge = byId.get("badge-review")!;
  const publish = byId.get("publish")!;
  assert.equal(source.x, badge.x, "independent prerequisites share a rank");
  assert.equal(publish.x - source.x, WORKGRAPH_GRAPH_COL_WIDTH, "only the dependent advances a rank");
  assert.ok(source.y < publish.y && publish.y < badge.y, "dependent is centered between parallel prerequisites");
  assert.deepEqual(layout.edges.map(edge => [edge.kind, edge.fromId, edge.toId]), [
    ["blocks", "source-review", "publish"], ["blocks", "badge-review", "publish"],
  ]);
  const [first, second] = layout.edges;
  assert.notEqual(workGraphEdgePath(first), workGraphEdgePath(second));
  assert.notDeepEqual(workGraphEdgeMidpoint(first), workGraphEdgeMidpoint(second), "edge labels remain separate");
  for (const edge of layout.edges) {
    const [p0, p1, p2, p3] = edge.points;
    for (let step = 0; step <= 200; step += 1) {
      const t = step / 200; const u = 1 - t;
      const x = u ** 3 * p0.x + 3 * u ** 2 * t * p1.x + 3 * u * t ** 2 * p2.x + t ** 3 * p3.x;
      const y = u ** 3 * p0.y + 3 * u ** 2 * t * p1.y + 3 * u * t ** 2 * p2.y + t ** 3 * p3.y;
      for (const node of layout.nodes) {
        if (node.itemId === edge.fromId || node.itemId === edge.toId) continue;
        assert.ok(!(x > node.x && x < node.x + node.w && y > node.y && y < node.y + node.h), `${edge.fromId} -> ${edge.toId} crosses unrelated ${node.itemId}`);
      }
      assert.ok(x >= 0 && x <= layout.width && y >= 0 && y <= layout.height, "edges fit the advertised graph extent");
    }
  }
  assert.deepEqual(layoutWorkGraph(items.slice().reverse(), edges.slice().reverse()).nodes, layout.nodes, "dependency placement does not depend on input ordering");
});

test("dependency ranking follows the longest prerequisite path and ignores dangling or duplicate edges", () => {
  const layout = layoutWorkGraph([item("a", "1"), item("b", "2"), item("c", "3"), item("d", "4")], [
    { kind: "blocks", from_id: "a", to_id: "b" },
    { kind: "blocks", from_id: "a", to_id: "b" },
    { kind: "blocks", from_id: "a", to_id: "c" },
    { kind: "blocks", from_id: "b", to_id: "c" },
    { kind: "blocks", from_id: "c", to_id: "d" },
    { kind: "blocks", from_id: "ghost", to_id: "d" },
    { kind: "relates", from_id: "d", to_id: "a" },
  ]);
  assert.deepEqual(layout.nodes.map(node => node.x - layout.nodes[0].x), [0, 1, 2, 3].map(rank => rank * WORKGRAPH_GRAPH_COL_WIDTH));
  assert.equal(layout.edges.length, 5);
});

test("dependency edges preserve structural parent placement and cycles remain deterministic and bounded", () => {
  const items = [item("root", "1"), item("child", "2"), item("peer", "3")];
  const parents = [{ kind: "parent", from_id: "child", to_id: "root" }];
  const baseline = layoutWorkGraph(items, parents);
  assert.deepEqual(layoutWorkGraph(items, [...parents, { kind: "blocks", from_id: "child", to_id: "peer" }]).nodes, baseline.nodes);
  const cycle = [{ kind: "blocks", from_id: "root", to_id: "child" }, { kind: "blocks", from_id: "child", to_id: "root" }];
  const cyclic = layoutWorkGraph(items, cycle);
  assert.equal(cyclic.nodes.length, items.length);
  assert.equal(cyclic.edges.length, 2);
  assert.deepEqual(layoutWorkGraph(items.slice().reverse(), cycle.slice().reverse()).nodes, cyclic.nodes);
  assert.ok(cyclic.width < WORKGRAPH_GRAPH_COL_WIDTH * items.length && Number.isFinite(cyclic.height));
});
