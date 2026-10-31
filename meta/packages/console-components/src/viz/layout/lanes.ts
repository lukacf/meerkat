// The lanes layout (Topology's default, design spec A3.9): one horizontal
// band per group, the label at the band's left edge, nodes spread across the
// width like the brand's household picture. Within a band the order is the
// manual order, else the label; a band with more than 12 nodes wraps to a
// second row; helpers hang off their host, right of it and 36 px down.

import { estimateLaneLabelWidth } from "./geometry";
import type { GraphLayout, LayoutEdge, LayoutLaneInput, LayoutNode, LayoutOptions, PlacedLane } from "./types";
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
} as const;

/** The band for nodes in no listed lane. */
export const OTHER_LANE = "__other";

interface LaneRowMember {
  node: LayoutNode;
  helpers: LayoutNode[];
  slot: number;
}

export function layoutLanes(
  nodes: readonly LayoutNode[],
  _edges: readonly LayoutEdge[],
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
    const rows: LaneRowMember[][] = [];
    for (let start = 0; start < main.length; start += LANES.rowCap) {
      const row = main.slice(start, start + LANES.rowCap).map((node) => {
        const helpers = (helpersOf.get(node.id) ?? []).sort(compareNodes);
        const helperReach = helpers.length > 0 ? LANES.helperDx + (helpers.length - 1) * LANES.helperStep + 18 : 0;
        const slot = Math.max(LANES.minSlot, node.labelWidth + LANES.slotLabelPad, helperReach * 2);
        return { node, helpers, slot };
      });
      rows.push(row);
      widest = Math.max(widest, row.reduce((sum, member) => sum + member.slot, 0));
    }
    bandRows.set(band.id, rows);
  }

  const contentWidth = Math.max(widest, options.width - gutter - LANES.pad);
  const pitch = LANES.rowPitch[density];
  const markOffset = LANES.markOffset[density];
  const placed: PlacedLane[] = [];
  const boxes = new Map<string, VizNodeBox>();
  let top = 0;
  for (const band of bands) {
    const rows = bandRows.get(band.id) ?? [];
    const hasHelpers = rows.length > 0 && rows[rows.length - 1].some((member) => member.helpers.length > 0);
    const block = rows.length * pitch + (hasHelpers ? LANES.helperDy : 0);
    const height = Math.max(LANES.minHeight[density], block + 24);
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
        member.helpers.forEach((helper, index) => {
          boxes.set(helper.id, {
            id: helper.id,
            x: cx + LANES.helperDx + index * LANES.helperStep,
            y: y + LANES.helperDy,
            w: helper.w,
            h: helper.h,
          });
        });
        x += slot;
      }
    });
    placed.push({ id: band.id, label: band.label, y: top, h: height });
    top += height;
  }

  layout.nodes = boxes;
  layout.lanes = placed;
  layout.gutter = gutter;
  layout.bounds = { x: 0, y: 0, w: gutter + contentWidth + LANES.pad, h: top };
  return layout;
}
