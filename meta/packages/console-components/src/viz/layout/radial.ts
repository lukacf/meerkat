// The radial layout (the agent ego graph, design spec B9): the agent at the
// centre, its peers on a ring, grouped by relation in sectors with a gap
// between groups, starting at the top and running clockwise. Labels sit
// outward along each peer's angle (the renderer reads `angles`).

import type { GraphLayout, LayoutEdge, LayoutNode, LayoutOptions } from "./types";
import { compareNodes, emptyLayout } from "./types";
import type { VizNodeBox, VizRelation } from "../graph/types";

export const RADIAL = {
  minRadius: 56,
  maxRadius: 220,
  /** Room the outward labels take beside the ring. */
  labelRoom: 72,
  /** Room above and below the ring. */
  vRoom: 24,
  /** Arc length one peer needs on the ring. */
  slot: 22,
  /** A second, inner ring for crowded egos: this much further in. */
  innerStep: 26,
} as const;

export const RELATION_ORDER: readonly VizRelation[] = ["wired", "traffic", "route", "lineage", "cross", "blocks", "parent", "related"];

/** The group a peer sits in: its lane, else the relation that ties it to the centre. */
export function radialGroupOf(node: LayoutNode, centre: string, edges: readonly LayoutEdge[]): string {
  if (node.lane) return node.lane;
  let best: VizRelation | null = null;
  for (const edge of edges) {
    if (!((edge.from === centre && edge.to === node.id) || (edge.to === centre && edge.from === node.id))) continue;
    if (best === null || RELATION_ORDER.indexOf(edge.relation) < RELATION_ORDER.indexOf(best)) best = edge.relation;
  }
  return best ?? "other";
}

function groupRank(group: string): number {
  const index = RELATION_ORDER.indexOf(group as VizRelation);
  return index >= 0 ? index : group === "other" ? RELATION_ORDER.length + 1 : RELATION_ORDER.length;
}

export function layoutRadial(
  nodes: readonly LayoutNode[],
  edges: readonly LayoutEdge[],
  center: string | undefined,
  options: LayoutOptions,
): GraphLayout {
  const layout = emptyLayout("radial");
  if (nodes.length === 0) return layout;
  const degree = new Map<string, number>();
  for (const edge of edges) {
    degree.set(edge.from, (degree.get(edge.from) ?? 0) + 1);
    degree.set(edge.to, (degree.get(edge.to) ?? 0) + 1);
  }
  const centreNode = (center !== undefined ? nodes.find((node) => node.id === center) : undefined)
    ?? [...nodes].sort((a, b) => (degree.get(b.id) ?? 0) - (degree.get(a.id) ?? 0) || compareNodes(a, b))[0];
  const peers = nodes.filter((node) => node.id !== centreNode.id);
  const groups = new Map<string, LayoutNode[]>();
  for (const peer of peers) {
    const key = radialGroupOf(peer, centreNode.id, edges);
    const list = groups.get(key);
    if (list) list.push(peer);
    else groups.set(key, [peer]);
  }
  const ordered = [...groups.entries()]
    .sort(([a], [b]) => groupRank(a) - groupRank(b) || (a < b ? -1 : a > b ? 1 : 0))
    .map(([key, list]) => [key, list.sort(compareNodes)] as const);
  const gaps = ordered.length > 1 ? ordered.length : 0;
  const slots = Math.max(1, peers.length + gaps);
  const room = Math.min(
    (options.width > 0 ? options.width : 480) / 2 - RADIAL.labelRoom,
    (options.height > 0 ? options.height : 360) / 2 - RADIAL.vRoom,
  );
  const needed = (slots * RADIAL.slot) / (2 * Math.PI);
  const radius = Math.max(RADIAL.minRadius, Math.min(RADIAL.maxRadius, Math.max(room, Math.min(needed, RADIAL.maxRadius))));
  // Crowded: alternate peers between the ring and a ring further in.
  const twoRings = needed > radius * 1.05;
  const cx = radius + RADIAL.labelRoom;
  const cy = radius + RADIAL.vRoom;
  const boxes = new Map<string, VizNodeBox>();
  const angles = new Map<string, number>();
  boxes.set(centreNode.id, { id: centreNode.id, x: cx, y: cy, w: centreNode.w, h: centreNode.h });
  const step = (2 * Math.PI) / slots;
  let slot = 0;
  let count = 0;
  for (const [, list] of ordered) {
    for (const peer of list) {
      const angle = -Math.PI / 2 + (slot + 0.5) * step;
      const r = twoRings && count % 2 === 1 ? radius - RADIAL.innerStep : radius;
      boxes.set(peer.id, { id: peer.id, x: cx + Math.cos(angle) * r, y: cy + Math.sin(angle) * r, w: peer.w, h: peer.h });
      angles.set(peer.id, angle);
      slot += 1;
      count += 1;
    }
    if (gaps > 0) slot += 1;
  }
  layout.nodes = boxes;
  layout.angles = angles;
  layout.bounds = { x: 0, y: 0, w: 2 * cx, h: 2 * cy };
  return layout;
}
