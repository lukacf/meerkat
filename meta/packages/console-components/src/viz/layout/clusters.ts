// The clusters layout (large teams, design spec A3.9; the brand's engineering
// picture): each group drawn as concentric rings of member dots around its
// lead. Long-lived members fill the inner rings, helpers the outer ones; the
// groups pack in rows (as many per row as lets them be drawn largest in the
// frame); edges between groups bundle into one line per pair.

import { CLUSTER_DOT } from "./geometry";
import type { ClusterBundle, GraphLayout, LayoutEdge, LayoutLaneInput, LayoutNode, LayoutOptions, PlacedCluster } from "./types";
import { compareNodes, emptyLayout } from "./types";
import type { VizNodeBox, VizRelation } from "../graph/types";

export const CLUSTERS = {
  /** Arc length between neighbouring dots on a ring. */
  spacing: 11,
  ringGap: 10.5,
  /** First ring radius around a lead at the centre, and without one. */
  aroundLead: 19,
  aroundNothing: 10,
  groupGap: 72,
  /** Room under each cluster for its label and count. */
  labelRoom: 30,
  pad: 40,
} as const;

/** The group nodes cluster by: their lane. */
export const UNGROUPED = "__ungrouped";

export interface ClusterRingSlot {
  id: string;
  ring: number;
  angle: number;
}

/** Concentric ring slots for `count` members: ring k holds about 2 pi r_k / spacing. */
export function ringSlots(count: number, hasLead: boolean): { radii: number[]; slots: { ring: number; angle: number }[] } {
  const radii: number[] = [];
  const slots: { ring: number; angle: number }[] = [];
  // A lone member without a lead sits at the centre.
  if (!hasLead && count === 1) return { radii: [0], slots: [{ ring: 0, angle: 0 }] };
  let placed = 0;
  let ring = 0;
  while (placed < count) {
    const r = (hasLead ? CLUSTERS.aroundLead : CLUSTERS.aroundNothing) + ring * CLUSTERS.ringGap;
    const capacity = Math.max(ring === 0 && !hasLead ? 4 : 6, Math.floor((2 * Math.PI * r) / CLUSTERS.spacing));
    const take = Math.min(capacity, count - placed);
    radii.push(r);
    const offset = -Math.PI / 2 + (ring % 2 === 1 ? Math.PI / Math.max(1, take) : 0);
    for (let i = 0; i < take; i += 1) slots.push({ ring, angle: offset + (2 * Math.PI * i) / take });
    placed += take;
    ring += 1;
  }
  return { radii, slots };
}

const LONG_LIVED_ORDER: Record<string, number> = { coordinator: 0, agent: 1, external: 2, unknown: 3, person: 4, source: 5, store: 6, item: 7, goal: 8 };

export function layoutClusters(
  nodes: readonly LayoutNode[],
  edges: readonly LayoutEdge[],
  lanes: readonly LayoutLaneInput[],
  options: LayoutOptions,
): GraphLayout {
  const layout = emptyLayout("clusters");
  if (nodes.length === 0) return layout;
  const laneIndex = new Map(lanes.map((lane, index) => [lane.id, index]));
  const labels = new Map(lanes.map((lane) => [lane.id, lane.label]));
  const groups = new Map<string, LayoutNode[]>();
  for (const node of nodes) {
    const key = node.lane ?? UNGROUPED;
    const list = groups.get(key);
    if (list) list.push(node);
    else groups.set(key, [node]);
  }
  const order = [...groups.keys()].sort((a, b) => {
    const ia = laneIndex.get(a) ?? Number.POSITIVE_INFINITY;
    const ib = laneIndex.get(b) ?? Number.POSITIVE_INFINITY;
    if (ia !== ib) return ia < ib ? -1 : 1;
    const sa = groups.get(a)!.length;
    const sb = groups.get(b)!.length;
    if (sa !== sb) return sb - sa;
    return a < b ? -1 : a > b ? 1 : 0;
  });

  interface Shape {
    id: string;
    lead: LayoutNode | null;
    members: LayoutNode[];
    radii: number[];
    slots: { ring: number; angle: number }[];
    r: number;
  }
  const shapes: Shape[] = order.map((id) => {
    const members = [...groups.get(id)!];
    const coordinators = members.filter((node) => node.kind === "coordinator").sort(compareNodes);
    const lead = coordinators[0] ?? null;
    const rest = members
      .filter((node) => node !== lead)
      .sort((a, b) => {
        const ha = a.kind === "helper" ? 1 : 0;
        const hb = b.kind === "helper" ? 1 : 0;
        if (ha !== hb) return ha - hb;
        const ka = LONG_LIVED_ORDER[a.kind] ?? 9;
        const kb = LONG_LIVED_ORDER[b.kind] ?? 9;
        if (ka !== kb) return ka - kb;
        return compareNodes(a, b);
      });
    const { radii, slots } = ringSlots(rest.length, lead !== null);
    const outer = radii.length > 0 ? radii[radii.length - 1] : 0;
    const leadR = lead ? lead.w / 2 : 0;
    return { id, lead, members: rest, radii, slots, r: Math.max(leadR + 6, outer + CLUSTER_DOT.filled + 6) };
  });

  // Rows of clusters, as many per row as lets them be drawn largest in the
  // frame (its aspect), each row centred.
  const frameW = options.width > 0 ? options.width : 960;
  const frameH = options.height > 0 ? options.height : 560;
  const arrange = (columns: number): Shape[][] => {
    const result: Shape[][] = [];
    for (let start = 0; start < shapes.length; start += columns) result.push(shapes.slice(start, start + columns));
    return result;
  };
  const extent = (arranged: Shape[][]) => {
    const width = Math.max(...arranged.map((line) => line.reduce((sum, shape) => sum + shape.r * 2, 0) + (line.length - 1) * CLUSTERS.groupGap));
    const height = arranged.reduce((sum, line) => sum + Math.max(...line.map((shape) => shape.r * 2)) + CLUSTERS.labelRoom, 0) + (arranged.length - 1) * (CLUSTERS.groupGap / 2);
    return { width: width + 2 * CLUSTERS.pad, height: height + 2 * CLUSTERS.pad };
  };
  let rows = arrange(shapes.length);
  let best = -Infinity;
  for (let columns = 1; columns <= shapes.length; columns += 1) {
    const arranged = arrange(columns);
    const { width, height } = extent(arranged);
    const scale = Math.min(frameW / width, frameH / height);
    if (scale > best + 1e-6) {
      best = scale;
      rows = arranged;
    }
  }
  const usable = extent(rows).width - 2 * CLUSTERS.pad;

  const boxes = new Map<string, VizNodeBox>();
  const clusters: PlacedCluster[] = [];
  const groupOf = new Map<string, string>();
  let y = CLUSTERS.pad;
  let maxX = 0;
  for (const line of rows) {
    const height = Math.max(...line.map((shape) => shape.r * 2));
    const width = line.reduce((sum, shape) => sum + shape.r * 2, 0) + (line.length - 1) * CLUSTERS.groupGap;
    let x = CLUSTERS.pad + Math.max(0, (usable - width) / 2);
    for (const shape of line) {
      const cx = x + shape.r;
      const cy = y + height / 2;
      if (shape.lead) {
        boxes.set(shape.lead.id, { id: shape.lead.id, x: cx, y: cy, w: shape.lead.w, h: shape.lead.h });
        groupOf.set(shape.lead.id, shape.id);
      }
      shape.members.forEach((member, index) => {
        const slot = shape.slots[index];
        const r = shape.radii[slot.ring] ?? 0;
        const d = (member.kind === "helper" ? CLUSTER_DOT.hollow : CLUSTER_DOT.filled) * 2;
        boxes.set(member.id, { id: member.id, x: cx + Math.cos(slot.angle) * r, y: cy + Math.sin(slot.angle) * r, w: d, h: d });
        groupOf.set(member.id, shape.id);
      });
      clusters.push({
        id: shape.id,
        label: labels.get(shape.id) ?? (shape.id === UNGROUPED ? "" : shape.id),
        cx,
        cy,
        r: shape.r,
        rings: shape.radii,
        count: shape.members.length + (shape.lead ? 1 : 0),
        lead: shape.lead?.id,
      });
      x += shape.r * 2 + CLUSTERS.groupGap;
      maxX = Math.max(maxX, cx + shape.r);
    }
    y += height + CLUSTERS.labelRoom + CLUSTERS.groupGap / 2;
  }

  // One bundle per group pair, its width by count.
  const bundles = new Map<string, ClusterBundle>();
  for (const edge of edges) {
    const a = groupOf.get(edge.from);
    const b = groupOf.get(edge.to);
    if (a === undefined || b === undefined || a === b) continue;
    const [from, to] = a < b ? [a, b] : [b, a];
    const key = `${from}\u0000${to}`;
    const bundle = bundles.get(key);
    if (bundle) {
      bundle.count += 1;
      if (!bundle.relations.includes(edge.relation)) bundle.relations.push(edge.relation);
    } else {
      bundles.set(key, { id: `bundle:${from}:${to}`, from, to, count: 1, relations: [edge.relation as VizRelation] });
    }
  }

  layout.nodes = boxes;
  layout.clusters = clusters;
  layout.bundles = [...bundles.values()];
  layout.bounds = { x: 0, y: 0, w: Math.max(maxX + CLUSTERS.pad, usable + 2 * CLUSTERS.pad), h: y - CLUSTERS.groupGap / 2 + CLUSTERS.pad };
  return layout;
}
