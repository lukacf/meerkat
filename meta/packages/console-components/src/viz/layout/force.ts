// The force layout (design spec A3.9): d3-force with link distance 72,
// charge -180, collide 18 plus half the label width, a pull towards each
// group's anchor, alpha decay 0.03, run in idle chunks off the typing path.
// The picture starts from a deterministic seed (or the cached positions) and
// eases to the settled one. d3-force loads on first use: the console
// resolves it (vite alias, vitest alias, build.cjs nodePaths), and the kit's
// node test bundles, which cannot, keep the import for run time.

import type { GraphLayout, LayoutEdge, LayoutNode, LayoutPoint } from "./types";
import { boundsOf, compareNodes, emptyLayout } from "./types";
import type { VizNodeBox } from "../graph/types";

export const FORCE = {
  link: 72,
  charge: -180,
  collide: 18,
  groupPull: 0.1,
  alphaDecay: 0.03,
  alphaMin: 0.001,
  /** A refresh with most positions cached settles from a low alpha. */
  warmAlpha: 0.25,
  chunkMs: 8,
  maxTicks: 600,
  /** Above this many nodes `force` draws `clusters`. */
  maxNodes: 150,
  pad: 48,
} as const;

const GOLDEN = Math.PI * (3 - Math.sqrt(5));

/** Each group's anchor on a circle, in group order (first group at the top). */
export function forceAnchors(groups: readonly string[], nodeCount: number): Map<string, LayoutPoint> {
  const anchors = new Map<string, LayoutPoint>();
  const ring = groups.length > 1 ? Math.max(120, Math.sqrt(nodeCount) * 30) : 0;
  const centre = ring + 160;
  groups.forEach((group, index) => {
    const angle = -Math.PI / 2 + (2 * Math.PI * index) / Math.max(1, groups.length);
    anchors.set(group, { x: centre + Math.cos(angle) * ring, y: centre + Math.sin(angle) * ring });
  });
  return anchors;
}

export function groupOrder(nodes: readonly LayoutNode[], laneOrder: readonly string[]): string[] {
  const seen = new Set<string>();
  const order: string[] = [];
  for (const lane of laneOrder) {
    if (!seen.has(lane) && nodes.some((node) => node.lane === lane)) {
      seen.add(lane);
      order.push(lane);
    }
  }
  for (const node of [...nodes].sort(compareNodes)) {
    const group = node.lane ?? "";
    if (!seen.has(group)) {
      seen.add(group);
      order.push(group);
    }
  }
  return order;
}

/**
 * The seed: cached positions where known; others spiral out from their
 * group's anchor (or from the mean of their group's cached members).
 */
export function seedForce(
  nodes: readonly LayoutNode[],
  laneOrder: readonly string[],
  cached?: ReadonlyMap<string, LayoutPoint>,
): { layout: GraphLayout; anchors: Map<string, LayoutPoint> } {
  const layout = emptyLayout("force");
  if (nodes.length === 0) return { layout, anchors: new Map() };
  const groups = groupOrder(nodes, laneOrder);
  const anchors = forceAnchors(groups, nodes.length);
  // A group whose members were cached grows around where they are now.
  for (const group of groups) {
    let sx = 0;
    let sy = 0;
    let n = 0;
    for (const node of nodes) {
      const point = (node.lane ?? "") === group ? cached?.get(node.id) : undefined;
      if (!point) continue;
      sx += point.x;
      sy += point.y;
      n += 1;
    }
    if (n > 0) anchors.set(group, { x: sx / n, y: sy / n });
  }
  const counters = new Map<string, number>();
  const boxes = new Map<string, VizNodeBox>();
  for (const node of [...nodes].sort(compareNodes)) {
    const point = cached?.get(node.id);
    if (point) {
      boxes.set(node.id, { id: node.id, x: point.x, y: point.y, w: node.w, h: node.h });
      continue;
    }
    const group = node.lane ?? "";
    const anchor = anchors.get(group) ?? { x: 0, y: 0 };
    const index = counters.get(group) ?? 0;
    counters.set(group, index + 1);
    const r = 26 * Math.sqrt(index + (cached && cached.size > 0 ? 1.5 : 0.5));
    const angle = index * GOLDEN;
    boxes.set(node.id, { id: node.id, x: anchor.x + Math.cos(angle) * r, y: anchor.y + Math.sin(angle) * r, w: node.w, h: node.h });
  }
  layout.nodes = boxes;
  layout.bounds = boundsOf(boxes.values(), FORCE.pad);
  return { layout, anchors };
}

interface SimNode {
  id: string;
  group: string;
  labelWidth: number;
  x: number;
  y: number;
  fx?: number | null;
  fy?: number | null;
}

/** The slice of d3-force this layout uses (typed here, so no type package is needed). */
interface D3Force {
  forceSimulation(nodes: SimNode[]): D3Simulation;
  forceLink(links: { source: string; target: string }[]): D3ForceLink;
  forceManyBody(): { strength(value: number): { distanceMax(value: number): unknown } };
  forceCollide(): { radius(fn: (node: SimNode) => number): { strength(value: number): unknown } };
  forceX(fn: (node: SimNode) => number): { strength(value: number): unknown };
  forceY(fn: (node: SimNode) => number): { strength(value: number): unknown };
}
interface D3ForceLink {
  id(fn: (node: SimNode) => string): { distance(value: number): unknown };
}
interface D3Simulation {
  stop(): D3Simulation;
  alpha(): number;
  alpha(value: number): D3Simulation;
  alphaDecay(value: number): D3Simulation;
  alphaMin(value: number): D3Simulation;
  force(name: string, force: unknown): D3Simulation;
  tick(): void;
}

let loaded: Promise<D3Force | null> | null = null;

/** d3-force, loaded once; null where it cannot be resolved (the layout keeps its seed). */
export function loadD3Force(): Promise<D3Force | null> {
  if (!loaded) {
    loaded = (async () => {
      try {
        return (await import("d3-force")) as unknown as D3Force;
      } catch {
        return null;
      }
    })();
  }
  return loaded;
}

export interface ForceRun {
  /** Settled positions, or null when cancelled or d3-force is unavailable. */
  done: Promise<Map<string, LayoutPoint> | null>;
  cancel(): void;
}

type Scheduler = (task: () => void) => void;

function idleScheduler(): Scheduler {
  const idle = (globalThis as { requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number }).requestIdleCallback;
  if (typeof idle === "function") return (task) => void idle(task, { timeout: 80 });
  return (task) => void setTimeout(task, 0);
}

/**
 * Runs the simulation from the seed in idle chunks of about 8 ms. Nodes in
 * `pinned` keep their place (a refresh never reshuffles the picture); with
 * most nodes pinned the run starts warm.
 */
export function runForce(
  nodes: readonly LayoutNode[],
  edges: readonly LayoutEdge[],
  seed: ReadonlyMap<string, VizNodeBox>,
  anchors: ReadonlyMap<string, LayoutPoint>,
  options: { pinned?: ReadonlySet<string>; schedule?: Scheduler; now?: () => number } = {},
): ForceRun {
  let cancelled = false;
  const schedule = options.schedule ?? idleScheduler();
  const now = options.now ?? (() => (typeof performance !== "undefined" ? performance.now() : Date.now()));
  const done = (async () => {
    const d3 = await loadD3Force();
    if (!d3 || cancelled) return null;
    const ids = new Set(nodes.map((node) => node.id));
    const simNodes: SimNode[] = nodes.map((node) => {
      const box = seed.get(node.id);
      const sim: SimNode = { id: node.id, group: node.lane ?? "", labelWidth: node.labelWidth, x: box?.x ?? 0, y: box?.y ?? 0 };
      if (options.pinned?.has(node.id)) {
        sim.fx = sim.x;
        sim.fy = sim.y;
      }
      return sim;
    });
    const links = edges
      .filter((edge) => edge.from !== edge.to && ids.has(edge.from) && ids.has(edge.to))
      .map((edge) => ({ source: edge.from, target: edge.to }));
    const anchorOf = (node: SimNode) => anchors.get(node.group) ?? { x: 0, y: 0 };
    const warm = (options.pinned?.size ?? 0) * 2 > nodes.length;
    const simulation = d3
      .forceSimulation(simNodes)
      .stop()
      .alpha(warm ? FORCE.warmAlpha : 1)
      .alphaDecay(FORCE.alphaDecay)
      .alphaMin(FORCE.alphaMin)
      .force("link", d3.forceLink(links).id((node) => node.id).distance(FORCE.link))
      .force("charge", d3.forceManyBody().strength(FORCE.charge).distanceMax(600))
      .force("collide", d3.forceCollide().radius((node) => FORCE.collide + node.labelWidth / 2).strength(0.7))
      .force("x", d3.forceX((node) => anchorOf(node).x).strength(FORCE.groupPull))
      .force("y", d3.forceY((node) => anchorOf(node).y).strength(FORCE.groupPull));
    let ticks = 0;
    await new Promise<void>((resolve) => {
      const chunk = () => {
        if (cancelled) {
          resolve();
          return;
        }
        const end = now() + FORCE.chunkMs;
        while (simulation.alpha() > FORCE.alphaMin && ticks < FORCE.maxTicks) {
          simulation.tick();
          ticks += 1;
          if (now() >= end) break;
        }
        if (simulation.alpha() > FORCE.alphaMin && ticks < FORCE.maxTicks) schedule(chunk);
        else resolve();
      };
      schedule(chunk);
    });
    if (cancelled) return null;
    return new Map(simNodes.map((node) => [node.id, { x: node.x, y: node.y }]));
  })();
  return {
    done,
    cancel: () => {
      cancelled = true;
    },
  };
}

/** A force layout from settled (or seed) positions. */
export function forceLayoutFrom(nodes: readonly LayoutNode[], positions: ReadonlyMap<string, LayoutPoint>): GraphLayout {
  const layout = emptyLayout("force");
  const boxes = new Map<string, VizNodeBox>();
  for (const node of nodes) {
    const point = positions.get(node.id);
    if (point) boxes.set(node.id, { id: node.id, x: point.x, y: point.y, w: node.w, h: node.h });
  }
  layout.nodes = boxes;
  layout.bounds = boundsOf(boxes.values(), FORCE.pad);
  return layout;
}
