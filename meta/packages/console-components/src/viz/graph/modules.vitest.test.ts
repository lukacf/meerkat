import { describe, expect, it } from "vitest";

import { bowAroundMarks, layoutLanes, sameRowArcs } from "../layout/lanes";
import type { LayoutNode } from "../layout/types";
import { VIZ_TOKEN_FALLBACKS } from "../tokens";
import { drawCanvasScene, nodeAtPoint } from "./canvas-renderer";
import { angleAt, edgeLength, edgePath, edgeShape, pointAt, trafficWidth } from "./edge-geometry";
import { layoutGoalHeader } from "./goal-header";
import { nodeToward, readingOrder } from "./keyboard";
import { fitText, outwardSides, placeLabels } from "./labels";
import { buildGraphModel, edgeIsDirected, highlightFor } from "./model";
import { PARTICLES, admitMessages, easeTravel, travelMs, trimSeen } from "./particles";
import { POSITION_CACHE_PREFIX, readMinimapHidden, readPositionCache, writeMinimapHidden, writePositionCache } from "./position-cache";
import type { VizEdge, VizNode, VizNodeBox } from "./types";
import { fitViewport, needsMinimap, revealViewport, zoomAt, zoomTo } from "./zoom-pan";

const node = (id: string, extra: Partial<VizNode> = {}): VizNode => ({ id, kind: "agent", label: id, ...extra });
const edge = (from: string, to: string, relation: VizEdge["relation"], extra: Partial<VizEdge> = {}): VizEdge => ({ id: `${from}-${to}-${relation}`, from, to, relation, ...extra });

describe("the view model", () => {
  it("drops retired nodes, dangling and duplicate edges, and counts toggled-off relations for the legend", () => {
    const model = buildGraphModel({
      nodes: [node("a"), node("b"), node("old", { retired: true }), node("a")],
      edges: [edge("a", "b", "wired"), edge("a", "b", "wired"), edge("a", "old", "wired"), edge("a", "ghost", "wired"), edge("b", "a", "traffic")],
      hiddenRelations: new Set(["traffic"]),
    });
    expect(model.nodes.map((n) => n.id)).toEqual(["a", "b"]);
    expect(model.edges.map((e) => e.id)).toEqual(["a-b-wired"]);
    expect([...model.relations]).toEqual([["wired", 1], ["traffic", 1]]);
    expect(model.degree.get("a")).toBe(1);
  });

  it("offsets parallel edges of one pair either side of the line and marks hubs", () => {
    const nodes = ["hub", "a", "b", "c", "d", "e", "f"].map((id) => node(id));
    const edges = [edge("hub", "a", "wired"), edge("hub", "a", "traffic"), edge("hub", "b", "wired"), edge("hub", "c", "wired"), edge("hub", "d", "wired"), edge("e", "f", "wired")];
    const model = buildGraphModel({ nodes, edges });
    const offsets = [model.offsets.get("hub-a-wired"), model.offsets.get("hub-a-traffic")];
    expect(offsets[0]).toBe(-offsets[1]!);
    expect(Math.abs(offsets[0]! - offsets[1]!)).toBe(6);
    expect(model.offsets.has("hub-b-wired")).toBe(false);
    expect(model.hubs.has("hub")).toBe(true);
    expect(model.hubs.has("e")).toBe(false);
  });

  it("draws only the focused node's neighbourhood in focus mode", () => {
    const model = buildGraphModel({
      nodes: ["a", "b", "c", "d"].map((id) => node(id)),
      edges: [edge("a", "b", "wired"), edge("b", "c", "wired"), edge("c", "d", "wired")],
      focus: "b",
    });
    expect(model.nodes.map((n) => n.id).sort()).toEqual(["a", "b", "c"]);
    expect(model.edges).toHaveLength(2);
  });

  it("directs route, lineage and blocks by default, and highlights a whole chain upstream and downstream", () => {
    expect(edgeIsDirected(edge("a", "b", "route"))).toBe(true);
    expect(edgeIsDirected(edge("a", "b", "wired"))).toBe(false);
    expect(edgeIsDirected(edge("a", "b", "traffic", { directed: true }))).toBe(true);
    const model = buildGraphModel({
      nodes: ["a", "b", "c", "d", "x"].map((id) => node(id, { kind: "item" })),
      edges: [edge("a", "b", "blocks"), edge("b", "c", "blocks"), edge("c", "d", "blocks"), edge("x", "d", "related")],
    });
    const chain = highlightFor(model, "b", "chain");
    expect([...chain.nodes].sort()).toEqual(["a", "b", "c", "d"]);
    expect([...chain.edges]).toEqual(["a-b-blocks"]);
    expect([...chain.downstream].sort()).toEqual(["c", "d"]);
    expect([...chain.downstreamEdges].sort()).toEqual(["b-c-blocks", "c-d-blocks"]);
    const near = highlightFor(model, "d", "neighbours");
    expect([...near.nodes].sort()).toEqual(["c", "d", "x"]);
  });
});

describe("labels", () => {
  it("places below first, moves around a neighbour, drops what still collides unless forced", () => {
    const at = (id: string, priority: number, force = false) => ({ id, x: 0, y: 0, width: 60, r: 6, priority, force });
    // Five labels at one spot: below, right, left and above are taken in priority order.
    const placed = placeLabels(
      [at("p1", 5), at("p2", 4), at("p3", 3), at("p4", 2), at("p5", 1), at("p6", 0, true)],
      [{ id: "p1", x: 0, y: 0, r: 6 }],
    );
    expect(["p1", "p2", "p3", "p4"].map((id) => placed.get(id)?.side)).toEqual(["below", "right", "left", "above"]);
    expect(placed.has("p5")).toBe(false);
    expect(placed.get("p6")?.side).toBe("below");
    // A mark in the way of "below" sends the label to the right.
    const moved = placeLabels([at("a", 1)], [{ id: "a", x: 0, y: 0, r: 6 }, { id: "b", x: 0, y: 18, r: 6 }]);
    expect(moved.get("a")?.side).toBe("right");
  });

  it("tries outward first around a radial centre and fits text with an ellipsis", () => {
    expect(outwardSides(0)[0]).toBe("right");
    expect(outwardSides(Math.PI)[0]).toBe("left");
    expect(outwardSides(-Math.PI / 2)[0]).toBe("above");
    expect(outwardSides(Math.PI / 2)[0]).toBe("below");
    const measure = (text: string) => text.length * 10;
    expect(fitText("Billing", 100, measure)).toBe("Billing");
    const fitted = fitText("Publish the release", 100, measure);
    expect(fitted.endsWith("…")).toBe(true);
    expect(fitted.startsWith("Publish")).toBe(true);
    expect(measure(fitted)).toBeLessThanOrEqual(100);
  });
});

describe("zoom and pan", () => {
  it("keeps the world point under the pointer while zooming, within 0.25x to 3x", () => {
    const view = { tx: 10, ty: 20, scale: 1 };
    const zoomed = zoomAt(view, 2, 110, 120);
    expect(zoomed.scale).toBe(2);
    expect((110 - zoomed.tx) / zoomed.scale).toBeCloseTo((110 - view.tx) / view.scale);
    expect(zoomAt(view, 100, 0, 0).scale).toBe(3);
    expect(zoomAt(view, 0.001, 0, 0).scale).toBe(0.25);
    expect(zoomTo(view, 1, 50, 50)).toBe(view);
  });

  it("fits inside the padding, centres what fits, pins what does not, and never exceeds the limits", () => {
    const limits = { minScale: 0.5, maxScale: 1 };
    const small = fitViewport({ x: 0, y: 0, w: 200, h: 100 }, 800, 600, limits);
    expect(small.scale).toBe(1);
    expect(small.tx).toBe(300);
    expect(small.ty).toBe(250);
    const tall = fitViewport({ x: 0, y: 0, w: 400, h: 4000 }, 800, 600, limits);
    expect(tall.scale).toBe(0.5);
    expect(tall.ty).toBe(32);
    const padded = fitViewport({ x: 0, y: 0, w: 200, h: 100 }, 800, 600, limits, { top: 32, right: 32, bottom: 200, left: 32 });
    expect(padded.ty).toBe(32 + (600 - 32 - 200 - 100) / 2);
    const top = fitViewport({ x: 32, y: 32, w: 200, h: 100 }, 800, 600, { ...limits, alignTop: true }, 24);
    expect(top.ty).toBe(24 - 32);
    expect(top.tx).toBe(24 + (800 - 48 - 200) / 2 - 32);
  });

  it("reveals a box with the smallest pan, and shows the minimap only when part of the picture is out of view", () => {
    const view = { tx: 0, ty: 0, scale: 1 };
    expect(revealViewport(view, { x: 100, y: 100, w: 20, h: 20 }, 800, 600)).toBe(view);
    const moved = revealViewport(view, { x: 900, y: 100, w: 20, h: 20 }, 800, 600);
    expect(moved.tx).toBe(800 - 24 - 910);
    const bounds = { x: 0, y: 0, w: 700, h: 500 };
    expect(needsMinimap(bounds, { tx: 50, ty: 50, scale: 1 }, 800, 600)).toBe(false);
    expect(needsMinimap(bounds, { tx: 50, ty: 50, scale: 1.4 }, 800, 600)).toBe(true);
    expect(needsMinimap(bounds, { tx: -300, ty: 50, scale: 1 }, 800, 600)).toBe(true);
  });
});

describe("particles", () => {
  it("travels at the brand's pace with an in-out ease", () => {
    expect(travelMs(10)).toBe(PARTICLES.minMs);
    expect(travelMs(260)).toBe(1000);
    expect(travelMs(5000)).toBe(PARTICLES.maxMs);
    expect(easeTravel(0)).toBe(0);
    expect(easeTravel(0.5)).toBe(0.5);
    expect(easeTravel(1)).toBe(1);
    expect(easeTravel(0.25)).toBeLessThan(0.25);
  });

  it("plays each message once, skips history, and overflows past 12 in flight", () => {
    const seen = new Set<string>();
    const now = 100_000;
    const messages = Array.from({ length: 15 }, (_, i) => ({ from: "a", to: "b", at: now - 1000 + i }));
    const first = admitMessages([...messages, { from: "a", to: "b", at: now - 60_000 }], seen, 0, now);
    expect(first.fly).toHaveLength(12);
    expect(first.overflow).toHaveLength(3);
    expect(admitMessages(messages, seen, 0, now).fly).toHaveLength(0);
    for (let i = 0; i < 10; i += 1) seen.add(`extra${i}`);
    trimSeen(seen, 5);
    expect(seen.size).toBe(5);
  });
});

describe("the position cache and the minimap toggle", () => {
  it("round-trips positions per key and ignores junk", () => {
    writePositionCache("topology:ops", new Map([["a", { x: 1.234, y: 5.678 }]]));
    expect(readPositionCache("topology:ops").get("a")).toEqual({ x: 1.2, y: 5.7 });
    sessionStorage.setItem(`${POSITION_CACHE_PREFIX}broken`, "{nope");
    expect(readPositionCache("broken").size).toBe(0);
    expect(readPositionCache("missing").size).toBe(0);
    writeMinimapHidden("topology", true);
    expect(readMinimapHidden("topology")).toBe(true);
    writeMinimapHidden("topology", false);
    expect(readMinimapHidden("topology")).toBe(false);
  });
});

describe("the goal header", () => {
  const measures = { title: (text: string) => text.length * 7, meta: (text: string) => text.length * 6 };
  const input = { title: "Ship the 2.4 release", meta: "1 of 5 done", progress: { done: 1, total: 5 }, trailing: { text: "Coordinator · Leading the work", avatar: "C" }, collapsed: false };

  it("lays out title, ring, meta and the right-aligned trailer when there is room", () => {
    const layout = layoutGoalHeader(800, input, measures);
    expect(layout.title).toBe("Ship the 2.4 release");
    expect(layout.ring!.cx).toBeGreaterThan(layout.titleX + 20 * 7);
    expect(layout.meta!.x).toBeGreaterThan(layout.ring!.cx);
    expect(layout.trailing!.right).toBe(800 - 12);
    expect(layout.trailing!.avatarCx).toBeLessThan(layout.trailing!.right - 30 * 6);
  });

  it("truncates the title first, then drops the meta, then the trailer", () => {
    const narrow = layoutGoalHeader(470, input, measures);
    expect(narrow.title.endsWith("…")).toBe(true);
    expect(narrow.meta).toBeDefined();
    expect(narrow.trailing).toBeDefined();
    const tighter = layoutGoalHeader(400, input, measures);
    expect(tighter.meta).toBeUndefined();
    expect(tighter.trailing).toBeDefined();
    const tightest = layoutGoalHeader(300, input, measures);
    expect(tightest.meta).toBeUndefined();
    expect(tightest.trailing).toBeUndefined();
    expect(tightest.title).toBe("Ship the 2.4 release");
  });
});

describe("keyboard movement", () => {
  const boxes = new Map<string, VizNodeBox>([
    ["a", { id: "a", x: 0, y: 0, w: 12, h: 12 }],
    ["b", { id: "b", x: 100, y: 0, w: 12, h: 12 }],
    ["c", { id: "c", x: 100, y: 100, w: 12, h: 12 }],
    ["d", { id: "d", x: 40, y: 100, w: 12, h: 12 }],
  ]);
  it("goes to the nearest node that way, keeping to the row or column", () => {
    expect(nodeToward(boxes, "a", "ArrowRight")).toBe("b");
    expect(nodeToward(boxes, "a", "ArrowDown")).toBe("d");
    expect(nodeToward(boxes, "b", "ArrowDown")).toBe("c");
    expect(nodeToward(boxes, "a", "ArrowLeft")).toBeNull();
    expect(nodeToward(boxes, null, "Home")).toBe("a");
    expect(nodeToward(boxes, null, "End")).toBe("c");
    expect(nodeToward(boxes, null, "End", true)).toBe("c");
    expect(readingOrder(boxes.values(), true).map((box) => box.id)).toEqual(["a", "d", "b", "c"]);
  });
});

describe("edge geometry", () => {
  it("runs straight between marks and along the route otherwise", () => {
    const a = { id: "a", x: 0, y: 0, w: 12, h: 12 };
    const b = { id: "b", x: 100, y: 0, w: 12, h: 12 };
    const straight = edgeShape(a, b);
    expect(edgePath(straight)).toBe("M0 0L100 0");
    expect(pointAt(straight, 0.5)).toEqual({ x: 50, y: 0 });
    expect(edgeLength(straight)).toBe(100);
    const routed = edgeShape(a, b, [{ x: 0, y: 0 }, { x: 25, y: -40 }, { x: 75, y: -40 }, { x: 100, y: 0 }]);
    expect(edgePath(routed)).toMatch(/^M0 0C/);
    expect(pointAt(routed, 0.5).y).toBeCloseTo(-30);
    expect(angleAt(routed, 1)).toBeGreaterThan(0);
    expect(edgeLength(routed)).toBeGreaterThan(100);
    expect([trafficWidth(1), trafficWidth(5), trafficWidth(12)]).toEqual([1, 1.5, 2]);
  });
});

describe("lane edges", () => {
  const mark = (id: string, lane: string, extra: Partial<LayoutNode> = {}): LayoutNode => ({ id, kind: "agent", label: id, lane, w: 12, h: 12, labelWidth: 40, ...extra });

  it("arcs above the row between two nodes of one row, higher for a longer span and for a second edge", () => {
    const nodes = ["a", "b", "c", "d"].map((id, order) => mark(id, "l", { order }));
    const layout = layoutLanes(nodes, [
      { id: "near", from: "a", to: "b", relation: "wired" },
      { id: "far", from: "a", to: "d", relation: "wired" },
      { id: "far2", from: "d", to: "a", relation: "traffic" },
    ], [{ id: "l", label: "Lane" }], { width: 900, height: 0, density: "comfortable" });
    const near = layout.routes.get("near")!;
    const far = layout.routes.get("far")!;
    const far2 = layout.routes.get("far2")!;
    const a = layout.nodes.get("a")!;
    expect(near[1].y).toBeLessThan(a.y);
    expect(far[1].y).toBeLessThan(near[1].y);
    expect(Math.min(far2[1].y, far2[2].y)).toBeLessThan(far[1].y);
    // A band at its minimum height: the highest arc reaches above it, and the bounds follow.
    expect(layout.bounds.y).toBeLessThan(0);
    expect(sameRowArcs([{ id: "x", from: "a", to: "zz", relation: "wired" }], layout.nodes, new Map()).size).toBe(0);
  });

  it("bows a straight edge around a mark it would run through", () => {
    const boxes = new Map([
      ["top", { id: "top", x: 100, y: 0, w: 12, h: 12 }],
      ["mid", { id: "mid", x: 101, y: 100, w: 12, h: 12 }],
      ["bottom", { id: "bottom", x: 102, y: 200, w: 12, h: 12 }],
      ["aside", { id: "aside", x: 300, y: 100, w: 12, h: 12 }],
    ]);
    const bowed = bowAroundMarks([
      { id: "through", from: "top", to: "bottom", relation: "cross" },
      { id: "clear", from: "top", to: "aside", relation: "wired" },
    ], boxes, new Map());
    expect(bowed.has("clear")).toBe(false);
    const route = bowed.get("through")!;
    const peak = pointAt({ points: route, straight: false }, 0.5);
    expect(Math.abs(peak.x - 101)).toBeGreaterThanOrEqual(6 + 12 - 1);
  });
});

describe("the canvas renderer", () => {
  it("paints with the resolved tokens and hit-tests nodes by screen position", () => {
    const calls: string[] = [];
    const context = new Proxy({} as Record<string, unknown>, {
      get(target, key) {
        if (key in target) return target[key as string];
        return (...args: unknown[]) => {
          calls.push(String(key));
          return args;
        };
      },
      set(target, key, value) {
        target[key as string] = value;
        if (key === "fillStyle" || key === "strokeStyle") calls.push(`${String(key)}=${value}`);
        return true;
      },
    }) as unknown as CanvasRenderingContext2D;
    const model = buildGraphModel({ nodes: [node("a", { live: true }), node("b"), node("c", { kind: "helper" })], edges: [edge("a", "b", "wired")] });
    const boxes = new Map([
      ["a", { id: "a", x: 10, y: 10, w: 12, h: 12 }],
      ["b", { id: "b", x: 60, y: 10, w: 12, h: 12 }],
      ["c", { id: "c", x: 110, y: 10, w: 5, h: 5 }],
    ]);
    const drawn = drawCanvasScene(context, {
      width: 200, height: 100, dpr: 2, viewport: { tx: 0, ty: 0, scale: 1 }, model, boxes, routes: new Map(),
      dots: new Map([["c", 2.4]]), clusterMode: false, tokens: VIZ_TOKEN_FALLBACKS, highlight: null,
      selected: new Set(["b"]), flashes: new Set(), live: new Set(["a"]), accentNodes: new Set(), accentEdges: new Set(), phase: 0.25,
    });
    expect(drawn).toBe(3);
    expect(calls).toContain(`fillStyle=${VIZ_TOKEN_FALLBACKS.live}`);
    expect(calls).toContain(`strokeStyle=${VIZ_TOKEN_FALLBACKS.edge}`);
    expect(calls.filter((call) => call === "arc").length).toBeGreaterThanOrEqual(4);
    expect(nodeAtPoint(boxes, { tx: 0, ty: 0, scale: 1 }, 61, 11, 8, new Map())).toBe("b");
    expect(nodeAtPoint(boxes, { tx: 0, ty: 0, scale: 1 }, 160, 60, 8, new Map())).toBeNull();
  });
});

describe("lanes in the frame", () => {
  const mark = (id: string, lane: string, labelWidth = 80): LayoutNode => ({ id, kind: "agent", label: id, lane, w: 12, h: 12, labelWidth });

  it("wraps a band where the width runs out, before 12 nodes", () => {
    const nodes = Array.from({ length: 8 }, (_, i) => mark(`n${i}`, "l"));
    const layout = layoutLanes(nodes, [], [{ id: "l", label: "Lane" }], { width: 600, height: 0, density: "comfortable" });
    const rows = new Set(nodes.map((node) => layout.nodes.get(node.id)!.y));
    expect(rows.size).toBeGreaterThan(1);
    for (const node of nodes) expect(layout.nodes.get(node.id)!.x + 52).toBeLessThanOrEqual(layout.bounds.w);
  });

  it("shares spare height among the bands, up to twice the minimum", () => {
    const nodes = [mark("a", "one"), mark("b", "two")];
    const lanes = [{ id: "one", label: "One" }, { id: "two", label: "Two" }];
    const tight = layoutLanes(nodes, [], lanes, { width: 900, height: 0, density: "comfortable" });
    const roomy = layoutLanes(nodes, [], lanes, { width: 900, height: 300, density: "comfortable" });
    const huge = layoutLanes(nodes, [], lanes, { width: 900, height: 2000, density: "comfortable" });
    expect(tight.lanes.map((lane) => lane.h)).toEqual([88, 88]);
    expect(roomy.lanes.map((lane) => lane.h)).toEqual([150, 150]);
    expect(huge.lanes.map((lane) => lane.h)).toEqual([176, 176]);
    // The marks stay centred in their band.
    expect(roomy.nodes.get("a")!.y).toBeGreaterThan(tight.nodes.get("a")!.y);
  });
});
