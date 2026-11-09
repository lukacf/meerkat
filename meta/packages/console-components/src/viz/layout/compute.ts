// One entry for every layout: maps the view's nodes to layout nodes (sizes,
// measured label widths), derives goal frames, and picks the layout drawn
// (force above 150 nodes draws clusters, the A3.9 level of detail).

import { layoutClusters } from "./clusters";
import { FORCE, seedForce } from "./force";
import { estimateLabelWidth, nodeBoxSize } from "./geometry";
import type { Density, TextMeasure } from "./geometry";
import { layoutLanes } from "./lanes";
import { layoutLayered } from "./layered";
import { layoutRadial } from "./radial";
import type { GraphLayout, LayoutEdge, LayoutFrameInput, LayoutNode, LayoutPoint } from "./types";
import type { VizEdge, VizFrame, VizLane, VizLayoutKind, VizNode } from "../graph/types";

export interface GraphLayoutInput {
  nodes: readonly VizNode[];
  edges: readonly VizEdge[];
  layout: VizLayoutKind;
  lanes?: readonly VizLane[];
  frames?: readonly VizFrame[];
  center?: string;
  density?: Density;
  /** The viewport the layout may use. */
  width?: number;
  height?: number;
  /** Measures an 11 px label; estimated when omitted. */
  measure?: TextMeasure;
  /** Force: positions to start from (the cache, or the last settled run). */
  positions?: ReadonlyMap<string, LayoutPoint>;
}

/** The layout a view draws: `force` above 150 nodes draws `clusters`. */
export function effectiveLayoutKind(kind: VizLayoutKind, nodeCount: number): VizLayoutKind {
  return kind === "force" && nodeCount > FORCE.maxNodes ? "clusters" : kind;
}

export function toLayoutNodes(nodes: readonly VizNode[], density: Density, measure: TextMeasure = estimateLabelWidth): LayoutNode[] {
  return nodes.map((node) => {
    const size = nodeBoxSize(node, density);
    return {
      id: node.id,
      kind: node.kind,
      label: node.label,
      lane: node.lane,
      frame: node.frame,
      order: node.order,
      rank: node.rank,
      anchor: node.anchor,
      live: node.live,
      w: size.w,
      h: size.h,
      labelWidth: node.kind === "item" || node.kind === "goal" ? 0 : measure(node.label),
    };
  });
}

export function toLayoutEdges(edges: readonly VizEdge[]): LayoutEdge[] {
  return edges.map((edge) => ({ id: edge.id, from: edge.from, to: edge.to, relation: edge.relation, weight: edge.weight }));
}

/**
 * The frames the layered layout draws: the given ones, plus one per goal
 * node that has none (label, sublabel as meta, its own frame as parent).
 */
export function framesFor(nodes: readonly VizNode[], frames: readonly VizFrame[] = []): LayoutFrameInput[] {
  const result: LayoutFrameInput[] = frames.map((frame) => ({
    id: frame.id,
    label: frame.label,
    meta: frame.meta,
    parent: frame.parent,
    order: frame.order,
    collapsed: frame.collapsed,
  }));
  const known = new Set(result.map((frame) => frame.id));
  const goals = new Map(nodes.filter((node) => node.kind === "goal").map((node) => [node.id, node]));
  for (const frame of result) {
    const goal = goals.get(frame.id);
    if (goal && frame.parent === undefined && goal.frame !== undefined && goal.frame !== goal.id) frame.parent = goal.frame;
  }
  for (const goal of goals.values()) {
    if (known.has(goal.id)) continue;
    known.add(goal.id);
    result.push({
      id: goal.id,
      label: goal.label,
      meta: goal.sublabel,
      parent: goal.frame !== undefined && goal.frame !== goal.id ? goal.frame : undefined,
      order: goal.order,
    });
  }
  return result;
}

export function computeGraphLayout(input: GraphLayoutInput): GraphLayout {
  const density = input.density ?? "comfortable";
  const nodes = toLayoutNodes(input.nodes, density, input.measure);
  const edges = toLayoutEdges(input.edges);
  const lanes = (input.lanes ?? []).map((lane) => ({ id: lane.id, label: lane.label }));
  const options = { width: input.width ?? 0, height: input.height ?? 0, density };
  const kind = effectiveLayoutKind(input.layout, nodes.length);
  switch (kind) {
    case "lanes":
      return layoutLanes(nodes, edges, lanes, options);
    case "layered":
      return layoutLayered(nodes, edges, framesFor(input.nodes, input.frames), options);
    case "radial":
      return layoutRadial(nodes, edges, input.center, options);
    case "clusters":
      return layoutClusters(nodes, edges, lanes, options);
    case "force":
    default:
      return seedForce(nodes, lanes.map((lane) => lane.id), input.positions).layout;
  }
}
