// The lanes layout (Topology's default, design spec A3.9): one horizontal
// band per group, the label at the band's left edge, nodes spread across the
// width like the brand's household picture. Within a band the order is the
// manual order, else the label; a band wraps to another row after 12 nodes
// or where the width runs out, and bands share spare height (up to twice
// their minimum); helpers hang off their host, right of its label and 36 px
// down. An edge between two nodes of one row arcs above the row (its height
// by the span), so it never runs through the marks between them.

import { estimateLaneLabelWidth } from "./geometry";
import type { EdgeRoute, GraphLayout, LayoutEdge, LayoutLaneInput, LayoutNode, LayoutOptions, PlacedLane } from "./types";
import { compareNodes, emptyLayout } from "./types";
import type { VizNodeBox } from "../graph/types";

export const LANES = {
  /** The graph canvas's inner padding. */
  pad: 32,
  /** Left inset of the band label. */
  labelInset: 16,
  minSlot: 96,
  slotLabelPad: 24,
  rowCap: 12,
  helperDx: 28,
  helperStep: 22,
  helperDy: 36,
  rowPitch: { comfortable: 60, compact: 48 },
  minHeight: { comfortable: 88, compact: 64 },
  /** Where the mark sits in its row: the label hangs below it. */
  markOffset: { comfortable: 22, compact: 18 },
  minGutter: 112,
  maxGutter: 220,
  /** Same-row arcs: control height base + span share, capped; parallels stack outward. */
  arcBase: 12,
  arcShare: 0.1,
  arcCap: 44,
  arcStep: 10,
} as const;

/** The band for nodes in no listed lane. */
export const OTHER_LANE = "__other";

interface LaneRowMember {
  node: LayoutNode;
  helpers: LayoutNode[];
  slot: number;
  helperDx: number;
}

export function layoutLanes(
  nodes: readonly LayoutNode[],
  edges: readonly LayoutEdge[],
  lanes: readonly LayoutLaneInput[],
  options: LayoutOptions,
): GraphLayout {
  const layout = emptyLayout("lanes");
  if (nodes.length === 0) return layout;
  const density = options.density;
  const known = new Map(lanes.map((lane, index) => [lane.id, index]));
  const byLane = new Map<string, LayoutNode[]>();
  for (const node of nodes) {
    const key = node.lane !== undefined && known.has(node.lane) ? node.lane : OTHER_LANE;
    const list = byLane.get(key);
    if (list) list.push(node);
    else byLane.set(key, [node]);
  }
  const bands: LayoutLaneInput[] = lanes.filter((lane) => byLane.has(lane.id));
  if (byLane.has(OTHER_LANE)) bands.push({ id: OTHER_LANE, label: lanes.length > 0 ? "Other" : "" });

  const gutter = bands.some((band) => band.label)
    ? Math.min(LANES.maxGutter, Math.max(LANES.minGutter, LANES.labelInset + Math.max(...bands.map((band) => estimateLaneLabelWidth(band.label))) + 24))
    : LANES.pad;

  // Rows per band: helpers attach to a host in the same band.
  const bandRows = new Map<string, LaneRowMember[][]>();
  const available = options.width > 0 ? options.width - gutter - LANES.pad : Number.POSITIVE_INFINITY;
  let widest = 0;
  for (const band of bands) {
    const members = byLane.get(band.id) ?? [];
    const byId = new Map(members.map((node) => [node.id, node]));
    const attachable = (node: LayoutNode) =>
      node.kind === "helper" && node.anchor !== undefined && node.anchor !== node.id && byId.has(node.anchor);
    const helpersOf = new Map<string, LayoutNode[]>();
    const main: LayoutNode[] = [];
    for (const node of members) {
      // A helper attaches to a host that sits in the row itself; a helper's
      // helper stays in the row.
      const host = node.anchor !== undefined ? byId.get(node.anchor) : undefined;
      if (attachable(node) && host && !attachable(host)) {
        const list = helpersOf.get(host.id);
        if (list) list.push(node);
        else helpersOf.set(host.id, [node]);
      } else {
        main.push(node);
      }
    }
    main.sort(compareNodes);
    const sized = main.map((node): LaneRowMember => {
      const helpers = (helpersOf.get(node.id) ?? []).sort(compareNodes);
      // Helpers start right of the host's label, so their link clears it.
      const helperDx = Math.max(LANES.helperDx, node.labelWidth / 2 + 14);
      const helperReach = helpers.length > 0 ? helperDx + (helpers.length - 1) * LANES.helperStep + 18 : 0;
      const slot = Math.max(LANES.minSlot, node.labelWidth + LANES.slotLabelPad, helperReach * 2);
      return { node, helpers, slot, helperDx };
    });
    // A row holds at most 12 nodes, and wraps earlier where the width runs out.
    const rows: LaneRowMember[][] = [];
    let row: LaneRowMember[] = [];
    let used = 0;
    for (const member of sized) {
      if (row.length > 0 && (row.length >= LANES.rowCap || used + member.slot > available)) {
        rows.push(row);
        row = [];
        used = 0;
      }
      row.push(member);
      used += member.slot;
    }
    if (row.length > 0) rows.push(row);
    for (const line of rows) widest = Math.max(widest, line.reduce((sum, member) => sum + member.slot, 0));
    bandRows.set(band.id, rows);
  }

  const contentWidth = Math.max(widest, options.width - gutter - LANES.pad);
  const pitch = LANES.rowPitch[density];
  const markOffset = LANES.markOffset[density];
  const placed: PlacedLane[] = [];
  const boxes = new Map<string, VizNodeBox>();
  /** Row key -> its row members (id and x), for the same-row arcs. */
  const rowOf = new Map<string, { id: string; x: number }[]>();
  // Band heights: the rows they hold; spare frame height is shared out, a
  // band growing to twice the minimum at most (the bands fill the canvas).
  const blocks = bands.map((band) => {
    const rows = bandRows.get(band.id) ?? [];
    const hasHelpers = rows.length > 0 && rows[rows.length - 1].some((member) => member.helpers.length > 0);
    return rows.length * pitch + (hasHelpers ? LANES.helperDy : 0);
  });
  const heights = blocks.map((block) => Math.max(LANES.minHeight[density], block + 24));
  const spare = options.height > 0 ? options.height - heights.reduce((sum, value) => sum + value, 0) : 0;
  if (spare > 0 && heights.length > 0) {
    const share = spare / heights.length;
    const cap = LANES.minHeight[density] * 2;
    heights.forEach((height, index) => {
      heights[index] = height + Math.max(0, Math.min(share, cap - height));
    });
  }
  let top = 0;
  for (const [bandIndex, band] of bands.entries()) {
    const rows = bandRows.get(band.id) ?? [];
    const block = blocks[bandIndex];
    const height = heights[bandIndex];
    const rowTop = top + (height - block) / 2;
    rows.forEach((row, rowIndex) => {
      const used = row.reduce((sum, member) => sum + member.slot, 0);
      const extra = (contentWidth - used) / row.length;
      let x = gutter;
      const y = rowTop + rowIndex * pitch + markOffset;
      for (const member of row) {
        const slot = member.slot + extra;
        const cx = x + slot / 2;
        boxes.set(member.node.id, { id: member.node.id, x: cx, y, w: member.node.w, h: member.node.h });
        const key = `${band.id}\u0000${rowIndex}`;
        const list = rowOf.get(key);
        if (list) list.push({ id: member.node.id, x: cx });
        else rowOf.set(key, [{ id: member.node.id, x: cx }]);
        member.helpers.forEach((helper, index) => {
          boxes.set(helper.id, {
            id: helper.id,
            x: cx + member.helperDx + index * LANES.helperStep,
            y: y + LANES.helperDy,
            w: helper.w,
            h: helper.h,
          });
        });
        x += slot;
      }
    });
    placed.push({ id: band.id, label: band.label, y: top, h: height, labelY: rowTop + markOffset });
    top += height;
  }

  const routes = sameRowArcs(edges, boxes, rowOf);
  for (const [id, route] of bowAroundMarks(edges, boxes, routes)) routes.set(id, route);
  let apex = 0;
  for (const route of routes.values()) apex = Math.min(apex, route[1].y * 0.75 + route[0].y * 0.25 - 4);

  layout.nodes = boxes;
  layout.lanes = placed;
  layout.routes = routes;
  layout.gutter = gutter;
  layout.bounds = { x: 0, y: Math.min(0, apex), w: gutter + contentWidth + LANES.pad, h: top - Math.min(0, apex) };
  return layout;
}

/**
 * Arcs for edges whose ends share a row: a cubic that bows above the row,
 * its height by the span (capped); a second edge between the same pair bows
 * further out.
 */
export function sameRowArcs(
  edges: readonly LayoutEdge[],
  boxes: ReadonlyMap<string, { x: number; y: number }>,
  rowOf: ReadonlyMap<string, { id: string; x: number }[]>,
): Map<string, EdgeRoute> {
  const routes = new Map<string, EdgeRoute>();
  const pairCount = new Map<string, number>();
  const memberRow = new Map<string, string>();
  for (const [key, members] of rowOf) for (const member of members) memberRow.set(member.id, key);
  for (const edge of edges) {
    if (edge.from === edge.to) continue;
    const row = memberRow.get(edge.from);
    if (row === undefined || memberRow.get(edge.to) !== row) continue;
    const a = boxes.get(edge.from);
    const b = boxes.get(edge.to);
    if (!a || !b) continue;
    const lo = Math.min(a.x, b.x);
    const hi = Math.max(a.x, b.x);
    const pair = edge.from < edge.to ? `${edge.from}\u0000${edge.to}` : `${edge.to}\u0000${edge.from}`;
    const index = pairCount.get(pair) ?? 0;
    pairCount.set(pair, index + 1);
    const lift = Math.min(LANES.arcCap, LANES.arcBase + (hi - lo) * LANES.arcShare) + index * LANES.arcStep;
    const dx = b.x - a.x;
    routes.set(edge.id, [
      { x: a.x, y: a.y },
      { x: a.x + dx * 0.25, y: a.y - lift },
      { x: a.x + dx * 0.75, y: b.y - lift },
      { x: b.x, y: b.y },
    ]);
  }
  return routes;
}

/** How close (px) a straight edge may pass another mark before it bows around it. */
const MARK_CLEARANCE = 12;

/**
 * Straight edges that would run through another node's mark (a node lined
 * up between two lanes the edge joins) bow sideways around it instead.
 */
export function bowAroundMarks(
  edges: readonly LayoutEdge[],
  boxes: ReadonlyMap<string, { id: string; x: number; y: number; w: number; h: number }>,
  routed: ReadonlyMap<string, EdgeRoute>,
): Map<string, EdgeRoute> {
  const bowed = new Map<string, EdgeRoute>();
  const all = [...boxes.values()];
  for (const edge of edges) {
    if (routed.has(edge.id) || edge.from === edge.to) continue;
    const a = boxes.get(edge.from);
    const b = boxes.get(edge.to);
    if (!a || !b) continue;
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const length2 = dx * dx + dy * dy;
    if (length2 < 1) continue;
    const length = Math.sqrt(length2);
    let worst: { t: number; side: number; distance: number; reach: number } | null = null;
    for (const c of all) {
      if (c.id === a.id || c.id === b.id) continue;
      const t = ((c.x - a.x) * dx + (c.y - a.y) * dy) / length2;
      if (t <= 0.06 || t >= 0.94) continue;
      const cross = (dx * (c.y - a.y) - dy * (c.x - a.x)) / length;
      const reach = Math.max(c.w, c.h) / 2 + MARK_CLEARANCE;
      if (Math.abs(cross) >= reach) continue;
      if (!worst || reach - Math.abs(cross) > worst.reach - worst.distance) worst = { t, side: Math.sign(cross) || 1, distance: Math.abs(cross), reach };
    }
    if (!worst) continue;
    // Bow away from the mark: the curve's peak clears it by the clearance.
    const nx = -dy / length;
    const ny = dx / length;
    const away = -worst.side;
    const peak = (worst.reach + worst.distance) / 0.75;
    const offset = away * peak;
    bowed.set(edge.id, [
      { x: a.x, y: a.y },
      { x: a.x + dx * 0.3 + nx * offset, y: a.y + dy * 0.3 + ny * offset },
      { x: a.x + dx * 0.7 + nx * offset, y: a.y + dy * 0.7 + ny * offset },
      { x: b.x, y: b.y },
    ]);
  }
  return bowed;
}
