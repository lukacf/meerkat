import { describe, expect, it } from "vitest";

import {
  CLUSTERS,
  LANES,
  LAYERED,
  RADIAL,
  computeGraphLayout,
  effectiveLayoutKind,
  framesFor,
  layoutClusters,
  layoutLanes,
  layoutLayered,
  layoutRadial,
  orderRanks,
  rankItems,
  ringSlots,
  runForce,
  seedForce,
  toLayoutNodes,
} from "./index";
import type { LayoutEdge, LayoutNode } from "./index";
import type { VizEdge, VizNode } from "../graph/types";

const options = { width: 1000, height: 600, density: "comfortable" as const };

function mark(id: string, extra: Partial<LayoutNode> = {}): LayoutNode {
  return { id, kind: "agent", label: id, w: 12, h: 12, labelWidth: 40, ...extra };
}

function card(id: string, extra: Partial<LayoutNode> = {}): LayoutNode {
  return { id, kind: "item", label: id, w: 216, h: 48, labelWidth: 0, ...extra };
}

function blocks(from: string, to: string): LayoutEdge {
  return { id: `${from}>${to}`, from, to, relation: "blocks" };
}

describe("lanes layout", () => {
  it("draws one band per lane in lane order, skipping empty lanes, with unlisted nodes in a last band", () => {
    const layout = layoutLanes(
      [mark("a", { lane: "domains" }), mark("b", { lane: "coordinators" }), mark("c", { lane: "nowhere" })],
      [],
      [
        { id: "channels", label: "Channels" },
        { id: "coordinators", label: "Coordinators" },
        { id: "domains", label: "Domains" },
      ],
      options,
    );
    expect(layout.lanes.map((lane) => lane.id)).toEqual(["coordinators", "domains", "__other"]);
    expect(layout.lanes[0].y).toBe(0);
    for (const lane of layout.lanes) expect(lane.h).toBeGreaterThanOrEqual(LANES.minHeight.comfortable);
    const b = layout.nodes.get("b")!;
    const a = layout.nodes.get("a")!;
    const c = layout.nodes.get("c")!;
    expect(b.y).toBeLessThan(a.y);
    expect(a.y).toBeLessThan(c.y);
  });

  it("orders a band by manual order, then label, and spreads it across the width", () => {
    const layout = layoutLanes(
      [mark("zeta", { lane: "l" }), mark("alpha", { lane: "l" }), mark("mid", { lane: "l", order: 0 })],
      [],
      [{ id: "l", label: "Lane" }],
      options,
    );
    const xs = ["mid", "alpha", "zeta"].map((id) => layout.nodes.get(id)!.x);
    expect(xs[0]).toBeLessThan(xs[1]);
    expect(xs[1]).toBeLessThan(xs[2]);
    // Spread: the row uses the available width, not the minimum slots.
    expect(xs[2] - xs[0]).toBeGreaterThan(2 * LANES.minSlot);
    expect(layout.bounds.w).toBeGreaterThanOrEqual(options.width - 1);
    expect(layout.gutter).toBeGreaterThanOrEqual(LANES.minGutter);
  });

  it("keeps nodes at least max(96, label + 24) apart and wraps a band above 12 nodes", () => {
    const nodes = Array.from({ length: 15 }, (_, i) => mark(`n${String(i).padStart(2, "0")}`, { lane: "l", labelWidth: 100 }));
    const layout = layoutLanes(nodes, [], [{ id: "l", label: "Lane" }], { ...options, width: 0 });
    const rows = new Map<number, number[]>();
    for (const node of nodes) {
      const box = layout.nodes.get(node.id)!;
      rows.set(box.y, [...(rows.get(box.y) ?? []), box.x]);
    }
    expect([...rows.values()].map((row) => row.length).sort((a, b) => b - a)).toEqual([12, 3]);
    for (const row of rows.values()) {
      const sorted = [...row].sort((a, b) => a - b);
      for (let i = 1; i < sorted.length; i += 1) expect(sorted[i] - sorted[i - 1]).toBeGreaterThanOrEqual(124 - 0.001);
    }
  });

  it("hangs helpers off their host, right of it and 36 px down", () => {
    const layout = layoutLanes(
      [mark("host", { lane: "l" }), mark("h1", { lane: "l", kind: "helper", anchor: "host" }), mark("h2", { lane: "l", kind: "helper", anchor: "host" })],
      [],
      [{ id: "l", label: "Lane" }],
      options,
    );
    const host = layout.nodes.get("host")!;
    const h1 = layout.nodes.get("h1")!;
    const h2 = layout.nodes.get("h2")!;
    expect(h1.x - host.x).toBe(LANES.helperDx);
    expect(h1.y - host.y).toBe(LANES.helperDy);
    expect(h2.x - h1.x).toBe(LANES.helperStep);
    // The helpers fit inside their band.
    const band = layout.lanes[0];
    expect(h1.y).toBeLessThan(band.y + band.h);
  });
});

describe("layered layout", () => {
  it("ranks by the longest path over blocks edges and keeps a chain on one row", () => {
    const items = ["a", "b", "c", "d"].map((id) => card(id));
    const edges = [blocks("a", "b"), blocks("b", "c"), blocks("c", "d"), blocks("a", "d")];
    expect(Object.fromEntries(rankItems(items, edges))).toEqual({ a: 0, b: 1, c: 2, d: 3 });
    const layout = layoutLayered(items, edges, [], options);
    const ys = items.map((item) => layout.nodes.get(item.id)!.y);
    expect(new Set(ys).size).toBe(1);
    const xs = items.map((item) => layout.nodes.get(item.id)!.x);
    expect(xs[1] - xs[0]).toBe(216 + LAYERED.rankGap);
  });

  it("ignores non-blocks edges for ranks, pins a node's rank, and survives cycles", () => {
    const items = [card("a"), card("b"), card("c", { rank: 3 })];
    const ranks = rankItems(items, [{ id: "r", from: "a", to: "b", relation: "related" }, blocks("a", "c")]);
    expect(ranks.get("b")).toBe(0);
    expect(ranks.get("c")).toBe(3);
    const cyclic = rankItems([card("x"), card("y")], [blocks("x", "y"), blocks("y", "x")]);
    expect([...cyclic.values()].every((rank) => rank >= 0 && rank <= 1)).toBe(true);
  });

  it("uncrosses a rank with barycentre sweeps", () => {
    // a1 -> b2 and a2 -> b1 cross when b keeps label order.
    const items = [card("a1", { order: 0 }), card("a2", { order: 1 }), card("b1", { order: 0 }), card("b2", { order: 1 })];
    const edges = [blocks("a1", "b2"), blocks("a2", "b1")];
    const ranks = rankItems(items, edges);
    const layers = orderRanks(items, edges, ranks);
    expect(layers[1]).toEqual(["b2", "b1"]);
  });

  it("stacks frames by order, nests a child frame inside its parent, and makes the goal node the header", () => {
    const nodes: LayoutNode[] = [
      card("g1", { kind: "goal", h: 34 }),
      card("g2", { kind: "goal", h: 34 }),
      card("g3", { kind: "goal", h: 34 }),
      card("i1", { frame: "g1" }),
      card("i2", { frame: "g2" }),
      card("i3", { frame: "g3" }),
    ];
    const layout = layoutLayered(nodes, [], [
      { id: "g1", label: "Second", order: 2 },
      { id: "g2", label: "First", order: 1 },
      { id: "g3", label: "Nested", parent: "g1" },
    ], options);
    const frame = (id: string) => layout.frames.find((placed) => placed.id === id)!;
    expect(frame("g2").y).toBeLessThan(frame("g1").y);
    expect(frame("g3").depth).toBe(2);
    expect(frame("g3").x).toBeGreaterThan(frame("g1").x);
    expect(frame("g3").y).toBeGreaterThan(frame("g1").y);
    expect(frame("g3").y + frame("g3").h).toBeLessThanOrEqual(frame("g1").y + frame("g1").h);
    // Items sit inside their frames, below the header.
    const i3 = layout.nodes.get("i3")!;
    expect(i3.y - i3.h / 2).toBeGreaterThanOrEqual(frame("g3").y + frame("g3").header);
    // The goal's node is its frame's header strip.
    const header = layout.nodes.get("g1")!;
    expect(header.w).toBe(frame("g1").w);
    expect(header.y).toBe(frame("g1").y + frame("g1").header / 2);
    // Top-level frames share one width.
    expect(frame("g1").w).toBe(frame("g2").w);
  });

  it("collapses a frame to its header and routes edges into it to the header", () => {
    const nodes: LayoutNode[] = [card("done", { kind: "goal", h: 34 }), card("x", { frame: "done" }), card("y")];
    const layout = layoutLayered(nodes, [blocks("x", "y")], [{ id: "done", label: "Done goal", collapsed: true }], options);
    expect(layout.nodes.has("x")).toBe(false);
    const frame = layout.frames[0];
    expect(frame.collapsed).toBe(true);
    expect(frame.h).toBe(frame.header);
    expect(frame.itemCount).toBe(1);
    const route = layout.routes.get("x>y")!;
    expect(route).toBeDefined();
    const header = layout.nodes.get("done")!;
    expect(Math.abs(route[0].y - header.y)).toBeLessThanOrEqual(header.h / 2 + 0.001);
  });

  it("caps nesting at depth 2: a deeper frame's items join its depth-2 ancestor", () => {
    const nodes = [card("i", { frame: "c" })];
    const layout = layoutLayered(nodes, [], [
      { id: "a", label: "A" },
      { id: "b", label: "B", parent: "a" },
      { id: "c", label: "C", parent: "b" },
    ], options);
    expect(layout.frames.map((frame) => frame.id).sort()).toEqual(["a", "b"]);
    const b = layout.frames.find((frame) => frame.id === "b")!;
    const i = layout.nodes.get("i")!;
    expect(i.y).toBeGreaterThan(b.y);
    expect(i.y).toBeLessThan(b.y + b.h);
  });

  it("treats a cycle of frame parents as roots", () => {
    const layout = layoutLayered([card("i", { frame: "a" })], [], [
      { id: "a", label: "A", parent: "b" },
      { id: "b", label: "B", parent: "a" },
    ], options);
    expect(layout.frames).toHaveLength(2);
  });

  it("routes side to side when apart and bottom to top in one column", () => {
    const layout = layoutLayered([card("a"), card("b"), card("c")], [blocks("a", "b"), { id: "rel", from: "a", to: "c", relation: "related" }], [], options);
    const forward = layout.routes.get("a>b")!;
    const a = layout.nodes.get("a")!;
    expect(forward[0].x).toBe(a.x + a.w / 2);
    const down = layout.routes.get("rel")!;
    const c = layout.nodes.get("c")!;
    expect(down[0].y).toBe(a.y + a.h / 2);
    expect(down[3].y).toBe(c.y - c.h / 2);
  });
});

describe("radial layout", () => {
  it("puts the centre in the middle and the peers on a ring in relation sectors from the top", () => {
    const nodes = [mark("me"), mark("w1"), mark("w2"), mark("t1"), mark("r1")];
    const edges: LayoutEdge[] = [
      { id: "1", from: "me", to: "t1", relation: "traffic" },
      { id: "2", from: "me", to: "w2", relation: "wired" },
      { id: "3", from: "w1", to: "me", relation: "wired" },
      { id: "4", from: "r1", to: "me", relation: "route" },
    ];
    const layout = layoutRadial(nodes, edges, "me", { width: 480, height: 240, density: "comfortable" });
    const centre = layout.nodes.get("me")!;
    const distances = ["w1", "w2", "t1", "r1"].map((id) => Math.hypot(layout.nodes.get(id)!.x - centre.x, layout.nodes.get(id)!.y - centre.y));
    for (const distance of distances) expect(distance).toBeCloseTo(distances[0], 6);
    expect(distances[0]).toBeGreaterThanOrEqual(RADIAL.minRadius);
    // Wired first (from the top, clockwise), then traffic, then route.
    const angle = (id: string) => layout.angles.get(id)!;
    expect(angle("w1")).toBeLessThan(angle("w2"));
    expect(angle("w2")).toBeLessThan(angle("t1"));
    expect(angle("t1")).toBeLessThan(angle("r1"));
    expect(angle("w1")).toBeGreaterThan(-Math.PI / 2);
    expect(layout.bounds.w).toBeGreaterThan(0);
  });

  it("takes the busiest node as the centre when none is named", () => {
    const layout = layoutRadial([mark("a"), mark("hub"), mark("b")], [
      { id: "1", from: "hub", to: "a", relation: "wired" },
      { id: "2", from: "hub", to: "b", relation: "wired" },
    ], undefined, options);
    expect(layout.angles.has("hub")).toBe(false);
  });
});

describe("clusters layout", () => {
  it("fills rings by capacity and puts the lead at the centre", () => {
    const { radii, slots } = ringSlots(30, true);
    expect(slots).toHaveLength(30);
    expect(radii[0]).toBe(CLUSTERS.aroundLead);
    for (let ring = 0; ring < radii.length - 1; ring += 1) {
      expect(slots.filter((slot) => slot.ring === ring).length).toBe(Math.max(6, Math.floor((2 * Math.PI * radii[ring]) / CLUSTERS.spacing)));
    }
    const nodes = [
      mark("lead", { kind: "coordinator", lane: "g", w: 18, h: 18 }),
      ...Array.from({ length: 12 }, (_, i) => mark(`m${i}`, { lane: "g" })),
      ...Array.from({ length: 5 }, (_, i) => mark(`h${i}`, { lane: "g", kind: "helper" })),
    ];
    const layout = layoutClusters(nodes, [], [{ id: "g", label: "Group" }], options);
    const cluster = layout.clusters[0];
    expect(cluster.lead).toBe("lead");
    expect(layout.nodes.get("lead")!.x).toBe(cluster.cx);
    // Helpers sit on rings at least as far out as every long-lived member.
    const dist = (id: string) => Math.hypot(layout.nodes.get(id)!.x - cluster.cx, layout.nodes.get(id)!.y - cluster.cy);
    const innerMost = Math.min(...Array.from({ length: 5 }, (_, i) => dist(`h${i}`)));
    const outerLong = Math.max(...Array.from({ length: 12 }, (_, i) => dist(`m${i}`)));
    expect(innerMost).toBeGreaterThanOrEqual(outerLong - 0.001);
    expect(cluster.count).toBe(18);
  });

  it("bundles edges between groups into one line per pair, counted", () => {
    const nodes = [mark("a1", { lane: "a" }), mark("a2", { lane: "a" }), mark("b1", { lane: "b" })];
    const edges: LayoutEdge[] = [
      { id: "1", from: "a1", to: "b1", relation: "wired" },
      { id: "2", from: "b1", to: "a2", relation: "traffic" },
      { id: "3", from: "a1", to: "a2", relation: "wired" },
    ];
    const layout = layoutClusters(nodes, edges, [{ id: "a", label: "A" }, { id: "b", label: "B" }], options);
    expect(layout.bundles).toHaveLength(1);
    expect(layout.bundles[0].count).toBe(2);
    expect(layout.bundles[0].relations.sort()).toEqual(["traffic", "wired"]);
  });

  it("packs 300 nodes in 8 groups without overlap between clusters", () => {
    const nodes = Array.from({ length: 300 }, (_, i) => mark(`n${i}`, { lane: `g${i % 8}`, kind: i % 5 === 0 ? "helper" : "agent" }));
    const layout = layoutClusters(nodes, [], Array.from({ length: 8 }, (_, i) => ({ id: `g${i}`, label: `Group ${i}` })), { ...options, width: 1100 });
    expect(layout.nodes.size).toBe(300);
    for (let i = 0; i < layout.clusters.length; i += 1) {
      for (let j = i + 1; j < layout.clusters.length; j += 1) {
        const a = layout.clusters[i];
        const b = layout.clusters[j];
        expect(Math.hypot(a.cx - b.cx, a.cy - b.cy)).toBeGreaterThanOrEqual(a.r + b.r);
      }
    }
  });
});

describe("force layout", () => {
  it("seeds deterministically and keeps cached positions", () => {
    const nodes = Array.from({ length: 20 }, (_, i) => mark(`n${i}`, { lane: `g${i % 3}` }));
    const first = seedForce(nodes, ["g0", "g1", "g2"]).layout;
    const second = seedForce(nodes, ["g0", "g1", "g2"]).layout;
    expect([...first.nodes.values()]).toEqual([...second.nodes.values()]);
    const cached = new Map([["n4", { x: 12, y: 34 }]]);
    const seeded = seedForce(nodes, ["g0", "g1", "g2"], cached).layout;
    expect(seeded.nodes.get("n4")).toMatchObject({ x: 12, y: 34 });
  });

  it("settles with d3-force in chunks and leaves pinned nodes where they were", async () => {
    const nodes = Array.from({ length: 24 }, (_, i) => mark(`n${i}`, { lane: `g${i % 3}` }));
    const edges: LayoutEdge[] = nodes.slice(1).map((node, i) => ({ id: `e${i}`, from: nodes[i].id, to: node.id, relation: "wired" }));
    const { layout, anchors } = seedForce(nodes, ["g0", "g1", "g2"]);
    let chunks = 0;
    const run = runForce(nodes, edges, layout.nodes, anchors, {
      pinned: new Set(["n0"]),
      schedule: (task) => {
        chunks += 1;
        setTimeout(task, 0);
      },
    });
    const settled = await run.done;
    expect(settled).not.toBeNull();
    expect(chunks).toBeGreaterThan(0);
    const seed0 = layout.nodes.get("n0")!;
    expect(settled!.get("n0")).toEqual({ x: seed0.x, y: seed0.y });
    // Linked neighbours end up near the link distance, not stacked.
    for (let i = 1; i < 6; i += 1) {
      const a = settled!.get(nodes[i - 1].id)!;
      const b = settled!.get(nodes[i].id)!;
      expect(Math.hypot(a.x - b.x, a.y - b.y)).toBeGreaterThan(20);
    }
  });

  it("stops when cancelled", async () => {
    const nodes = Array.from({ length: 10 }, (_, i) => mark(`n${i}`));
    const { layout, anchors } = seedForce(nodes, []);
    const run = runForce(nodes, [], layout.nodes, anchors);
    run.cancel();
    expect(await run.done).toBeNull();
  });
});

describe("computeGraphLayout", () => {
  const viz = (id: string, extra: Partial<VizNode> = {}): VizNode => ({ id, kind: "agent", label: id, ...extra });

  it("draws clusters for a force graph above 150 nodes", () => {
    expect(effectiveLayoutKind("force", 150)).toBe("force");
    expect(effectiveLayoutKind("force", 151)).toBe("clusters");
    expect(effectiveLayoutKind("lanes", 1000)).toBe("lanes");
    const nodes = Array.from({ length: 151 }, (_, i) => viz(`n${i}`, { lane: `g${i % 4}` }));
    expect(computeGraphLayout({ nodes, edges: [], layout: "force" }).kind).toBe("clusters");
  });

  it("makes a frame of each goal node that has none, nested by the goal's frame", () => {
    const frames = framesFor([
      viz("g1", { kind: "goal", label: "Ship", sublabel: "1 of 5" }),
      viz("g2", { kind: "goal", label: "Sub", frame: "g1" }),
      viz("i", { kind: "item", frame: "g2" }),
    ], [{ id: "other", label: "Other work" }]);
    expect(frames).toEqual([
      { id: "other", label: "Other work", meta: undefined, parent: undefined, order: undefined, collapsed: undefined },
      { id: "g1", label: "Ship", meta: "1 of 5", parent: undefined, order: undefined },
      { id: "g2", label: "Sub", meta: undefined, parent: "g1", order: undefined },
    ]);
  });

  it("sizes item cards by density and measures mark labels", () => {
    const nodes = toLayoutNodes([viz("i", { kind: "item" }), viz("a", { label: "Billing" })], "compact", (text) => text.length * 10);
    expect(nodes[0]).toMatchObject({ w: 168, h: 36, labelWidth: 0 });
    expect(nodes[1].labelWidth).toBe(70);
  });

  it("lays out a layered graph of view nodes with goal frames", () => {
    const nodes: VizNode[] = [
      viz("g", { kind: "goal", label: "Goal" }),
      viz("a", { kind: "item", frame: "g" }),
      viz("b", { kind: "item", frame: "g" }),
    ];
    const edges: VizEdge[] = [{ id: "ab", from: "a", to: "b", relation: "blocks" }];
    const layout = computeGraphLayout({ nodes, edges, layout: "layered" });
    expect(layout.frames.map((frame) => frame.id)).toEqual(["g"]);
    expect(layout.nodes.get("b")!.x).toBeGreaterThan(layout.nodes.get("a")!.x);
  });
});
