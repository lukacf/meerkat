// The layered layout (WorkGraph, design spec A3.9 and B2): a DAG laid out
// left to right inside frames. Rank is the longest path over `blocks` edges
// among the items of one frame; two barycentre sweeps order each rank; rows
// follow the predecessors so chains run straight. Frames stack vertically,
// nest up to depth 2 (their child frames sit below their own items) and
// collapse to their header. A node's `rank` pins its column (fixed columns).

import { frameHeaderHeight, itemCardSize } from "./geometry";
import type {
  EdgeRoute,
  GraphLayout,
  LayoutEdge,
  LayoutFrameInput,
  LayoutNode,
  LayoutOptions,
  LayoutPoint,
  PlacedFrame,
} from "./types";
import { compareNodes, emptyLayout } from "./types";
import type { VizNodeBox } from "../graph/types";

export const LAYERED = {
  rankGap: 64,
  rowGap: 12,
  padX: 16,
  padBottom: 16,
  frameGap: 16,
  nestedGap: 12,
  pad: 32,
  maxDepth: 2,
  /** A mark's label to its right, in fixed-column layouts (Routing). */
  markLabelGap: 8,
  minMarkRow: 24,
} as const;

interface Block {
  /** Centres relative to the block's top-left. */
  centres: Map<string, LayoutPoint>;
  w: number;
  h: number;
}

/** Longest-path ranks over `blocks` edges; pinned ranks stay; cycles fall back to their placed predecessors. */
export function rankItems(items: readonly LayoutNode[], edges: readonly LayoutEdge[]): Map<string, number> {
  const ids = new Set(items.map((node) => node.id));
  const preds = new Map<string, string[]>(items.map((node) => [node.id, []]));
  const succs = new Map<string, string[]>(items.map((node) => [node.id, []]));
  const seen = new Set<string>();
  for (const edge of edges) {
    if (edge.relation !== "blocks" || edge.from === edge.to) continue;
    if (!ids.has(edge.from) || !ids.has(edge.to)) continue;
    const key = `${edge.from}\u0000${edge.to}`;
    if (seen.has(key)) continue;
    seen.add(key);
    preds.get(edge.to)!.push(edge.from);
    succs.get(edge.from)!.push(edge.to);
  }
  const pinned = new Map<string, number>();
  for (const node of items) {
    if (typeof node.rank === "number" && Number.isFinite(node.rank) && node.rank >= 0) pinned.set(node.id, Math.floor(node.rank));
  }
  const rank = new Map<string, number>();
  const remaining = new Map<string, number>(items.map((node) => [node.id, preds.get(node.id)!.length]));
  const queue = [...items].sort(compareNodes).filter((node) => remaining.get(node.id) === 0).map((node) => node.id);
  const order = new Map(items.map((node, index) => [node.id, index]));
  for (let index = 0; index < queue.length; index += 1) {
    const id = queue[index];
    let value = pinned.get(id);
    if (value === undefined) {
      value = 0;
      for (const pred of preds.get(id)!) value = Math.max(value, (rank.get(pred) ?? 0) + 1);
    }
    rank.set(id, value);
    for (const next of succs.get(id)!) {
      const left = remaining.get(next)! - 1;
      remaining.set(next, left);
      if (left === 0) queue.push(next);
    }
  }
  // Cycles: rank after the predecessors already placed, in input order.
  const unplaced = items.filter((node) => !rank.has(node.id)).sort((a, b) => order.get(a.id)! - order.get(b.id)!);
  for (const node of unplaced) {
    let value = pinned.get(node.id);
    if (value === undefined) {
      value = 0;
      for (const pred of preds.get(node.id)!) {
        const placed = rank.get(pred);
        if (placed !== undefined) value = Math.max(value, placed + 1);
      }
    }
    rank.set(node.id, value);
  }
  return rank;
}

/** Orders each rank by two barycentre sweeps (down, then up) to cut crossings. */
export function orderRanks(
  items: readonly LayoutNode[],
  edges: readonly LayoutEdge[],
  rank: ReadonlyMap<string, number>,
): string[][] {
  const maxRank = Math.max(0, ...rank.values());
  const layers: LayoutNode[][] = Array.from({ length: maxRank + 1 }, () => []);
  for (const node of items) layers[rank.get(node.id) ?? 0].push(node);
  for (const layer of layers) layer.sort(compareNodes);
  const ids = new Set(items.map((node) => node.id));
  const preds = new Map<string, string[]>();
  const succs = new Map<string, string[]>();
  for (const edge of edges) {
    if (edge.from === edge.to || !ids.has(edge.from) || !ids.has(edge.to)) continue;
    const fromRank = rank.get(edge.from) ?? 0;
    const toRank = rank.get(edge.to) ?? 0;
    if (fromRank === toRank) continue;
    const [lo, hi] = fromRank < toRank ? [edge.from, edge.to] : [edge.to, edge.from];
    (preds.get(hi) ?? preds.set(hi, []).get(hi)!).push(lo);
    (succs.get(lo) ?? succs.set(lo, []).get(lo)!).push(hi);
  }
  const position = new Map<string, number>();
  const index = () => {
    for (const layer of layers) layer.forEach((node, i) => position.set(node.id, i / Math.max(1, layer.length)));
  };
  index();
  const sweep = (layer: LayoutNode[], neighbours: Map<string, string[]>) => {
    const keyed = layer.map((node, i) => {
      const list = neighbours.get(node.id) ?? [];
      const bary = list.length > 0
        ? list.reduce((sum, id) => sum + (position.get(id) ?? 0), 0) / list.length
        : i / Math.max(1, layer.length);
      return { node, bary, i };
    });
    keyed.sort((a, b) => a.bary - b.bary || a.i - b.i);
    return keyed.map((entry) => entry.node);
  };
  for (let r = 1; r < layers.length; r += 1) {
    layers[r] = sweep(layers[r], preds);
    index();
  }
  for (let r = layers.length - 2; r >= 0; r -= 1) {
    layers[r] = sweep(layers[r], succs);
    index();
  }
  return layers.map((layer) => layer.map((node) => node.id));
}

function layoutBlock(items: readonly LayoutNode[], edges: readonly LayoutEdge[]): Block {
  if (items.length === 0) return { centres: new Map(), w: 0, h: 0 };
  const rank = rankItems(items, edges);
  const layers = orderRanks(items, edges, rank);
  const byId = new Map(items.map((node) => [node.id, node]));
  const preds = new Map<string, string[]>();
  for (const edge of edges) {
    if (!byId.has(edge.from) || !byId.has(edge.to) || edge.from === edge.to) continue;
    const a = rank.get(edge.from) ?? 0;
    const b = rank.get(edge.to) ?? 0;
    if (a === b) continue;
    const [lo, hi] = a < b ? [edge.from, edge.to] : [edge.to, edge.from];
    (preds.get(hi) ?? preds.set(hi, []).get(hi)!).push(lo);
  }
  // Rows: follow the mean row of the predecessors, keeping the rank's order.
  const row = new Map<string, number>();
  for (const layer of layers) {
    let previous = -1;
    for (const id of layer) {
      const placed = (preds.get(id) ?? []).map((pred) => row.get(pred)).filter((value): value is number => value !== undefined);
      const desired = placed.length > 0 ? Math.round(placed.reduce((sum, value) => sum + value, 0) / placed.length) : previous + 1;
      const value = Math.max(desired, previous + 1);
      row.set(id, value);
      previous = value;
    }
  }
  const isCard = (node: LayoutNode) => node.kind === "item" || node.kind === "goal";
  const colWidth = layers.map((layer) =>
    Math.max(0, ...layer.map((id) => {
      const node = byId.get(id)!;
      return isCard(node) ? node.w : node.w + LAYERED.markLabelGap + node.labelWidth;
    })),
  );
  const rowHeight = Math.max(LAYERED.minMarkRow, ...items.map((node) => node.h));
  const colX: number[] = [];
  let x = 0;
  for (let r = 0; r < layers.length; r += 1) {
    colX.push(x);
    x += colWidth[r] + (r < layers.length - 1 ? LAYERED.rankGap : 0);
  }
  const centres = new Map<string, LayoutPoint>();
  let maxRow = 0;
  layers.forEach((layer, r) => {
    for (const id of layer) {
      const node = byId.get(id)!;
      const rowIndex = row.get(id) ?? 0;
      maxRow = Math.max(maxRow, rowIndex);
      centres.set(id, {
        x: colX[r] + node.w / 2,
        y: rowIndex * (rowHeight + LAYERED.rowGap) + rowHeight / 2,
      });
    }
  });
  return { centres, w: x, h: (maxRow + 1) * rowHeight + maxRow * LAYERED.rowGap };
}

interface FrameTree {
  frame: LayoutFrameInput;
  depth: number;
  children: FrameTree[];
  items: LayoutNode[];
}

interface FrameBox {
  w: number;
  h: number;
  /** Lays the frame out with its top-left at (x, y) and the given width. */
  place: (x: number, y: number, width: number) => void;
}

/** A cubic route between two boxes: side to side when apart, else bottom to top. */
export function routeBetween(from: VizNodeBox, to: VizNodeBox): EdgeRoute {
  const fx0 = from.x - from.w / 2;
  const fx1 = from.x + from.w / 2;
  const tx0 = to.x - to.w / 2;
  const tx1 = to.x + to.w / 2;
  let start: LayoutPoint;
  let end: LayoutPoint;
  if (tx0 > fx1) {
    start = { x: fx1, y: from.y };
    end = { x: tx0, y: to.y };
  } else if (tx1 < fx0) {
    start = { x: fx0, y: from.y };
    end = { x: tx1, y: to.y };
  } else {
    // Overlapping columns: leave through the bottom (or top) at a shared x.
    const x = Math.min(Math.min(fx1, tx1), Math.max(Math.max(fx0, tx0), (from.x + to.x) / 2));
    const down = to.y >= from.y;
    start = { x, y: from.y + (down ? from.h / 2 : -from.h / 2) };
    end = { x, y: to.y + (down ? -to.h / 2 : to.h / 2) };
  }
  const horizontal = Math.abs(end.x - start.x) >= Math.abs(end.y - start.y) && (tx0 > fx1 || tx1 < fx0);
  const span = horizontal ? Math.abs(end.x - start.x) : Math.abs(end.y - start.y);
  const bend = Math.max(24, span * 0.4);
  const sx = Math.sign(end.x - start.x) || 1;
  const sy = Math.sign(end.y - start.y) || 1;
  const c1 = horizontal ? { x: start.x + sx * bend, y: start.y } : { x: start.x, y: start.y + sy * bend };
  const c2 = horizontal ? { x: end.x - sx * bend, y: end.y } : { x: end.x, y: end.y - sy * bend };
  return [start, c1, c2, end];
}

export function layoutLayered(
  nodes: readonly LayoutNode[],
  edges: readonly LayoutEdge[],
  frames: readonly LayoutFrameInput[],
  options: LayoutOptions,
): GraphLayout {
  const layout = emptyLayout("layered");
  if (nodes.length === 0) return layout;
  const header = frameHeaderHeight(options.density);
  const card = itemCardSize(options.density);

  // The frame tree, with depth capped: a deeper frame merges into its
  // depth-2 ancestor, whose items it joins.
  const inputs = new Map<string, LayoutFrameInput>();
  frames.forEach((frame) => {
    if (!inputs.has(frame.id)) inputs.set(frame.id, frame);
  });
  const frameIndex = new Map([...inputs.keys()].map((id, index) => [id, index]));
  const declaredParent = (id: string): string | undefined => {
    const parent = inputs.get(id)?.parent;
    return parent !== undefined && parent !== id && inputs.has(parent) ? parent : undefined;
  };
  /** The parent, unless following parents comes back to `id` (a cycle makes it a root). */
  const parentOf = (id: string): string | undefined => {
    const parent = declaredParent(id);
    const seen = new Set([id]);
    for (let current = parent; current !== undefined; current = declaredParent(current)) {
      if (seen.has(current)) return current === id ? undefined : parent;
      seen.add(current);
    }
    return parent;
  };
  const chainOf = (id: string): string[] => {
    const chain = [id];
    const seen = new Set(chain);
    let current = parentOf(id);
    while (current !== undefined && !seen.has(current)) {
      chain.push(current);
      seen.add(current);
      current = parentOf(current);
    }
    return chain.reverse();
  };
  /** The drawn frame an id resolves to (itself, or its depth-2 ancestor). */
  const drawnFrame = (id: string): string => {
    const chain = chainOf(id);
    return chain[Math.min(chain.length, LAYERED.maxDepth) - 1];
  };
  const trees = new Map<string, FrameTree>();
  const roots: FrameTree[] = [];
  const sortFrames = (a: FrameTree, b: FrameTree) => {
    const ao = a.frame.order ?? Number.POSITIVE_INFINITY;
    const bo = b.frame.order ?? Number.POSITIVE_INFINITY;
    if (ao !== bo) return ao < bo ? -1 : 1;
    return frameIndex.get(a.frame.id)! - frameIndex.get(b.frame.id)!;
  };
  for (const id of inputs.keys()) {
    if (drawnFrame(id) !== id) continue;
    trees.set(id, { frame: inputs.get(id)!, depth: chainOf(id).length, children: [], items: [] });
  }
  for (const tree of trees.values()) {
    const parent = parentOf(tree.frame.id);
    const parentTree = parent !== undefined ? trees.get(parent) : undefined;
    if (parentTree && tree.depth > 1) parentTree.children.push(tree);
    else {
      tree.depth = 1;
      roots.push(tree);
    }
  }
  const fixDepth = (tree: FrameTree, depth: number) => {
    tree.depth = depth;
    tree.children.sort(sortFrames);
    for (const child of tree.children) fixDepth(child, depth + 1);
  };
  roots.sort(sortFrames);
  for (const tree of roots) fixDepth(tree, 1);

  // Items: goal nodes are frame headers; everything else sits in its frame.
  const rootItems: LayoutNode[] = [];
  const headers = new Map<string, LayoutNode>();
  for (const node of nodes) {
    if (node.kind === "goal" && trees.has(node.id)) {
      headers.set(node.id, node);
      continue;
    }
    const frameId = node.frame !== undefined && inputs.has(node.frame) ? drawnFrame(node.frame) : undefined;
    const tree = frameId !== undefined ? trees.get(frameId) : undefined;
    if (tree) tree.items.push(node);
    else rootItems.push(node);
  }

  const boxes = new Map<string, VizNodeBox>();
  const placedFrames: PlacedFrame[] = [];
  /** Items hidden in a collapsed frame map to the frame's header node, if it has one. */
  const hiddenTo = new Map<string, string | null>();

  const countItems = (tree: FrameTree): number =>
    tree.items.length + tree.children.reduce((sum, child) => sum + countItems(child), 0);
  const hideAll = (tree: FrameTree, header: string | null) => {
    for (const item of tree.items) hiddenTo.set(item.id, header);
    for (const child of tree.children) {
      if (headers.has(child.frame.id)) hiddenTo.set(child.frame.id, header);
      hideAll(child, header);
    }
  };

  const measureFrame = (tree: FrameTree): FrameBox => {
    const minWidth = card.w + 2 * LAYERED.padX;
    if (tree.frame.collapsed) {
      hideAll(tree, headers.has(tree.frame.id) ? tree.frame.id : null);
      return {
        w: minWidth,
        h: header,
        place: (x, y, width) => {
          placedFrames.push({
            id: tree.frame.id,
            label: tree.frame.label,
            meta: tree.frame.meta,
            parent: tree.depth > 1 ? parentOf(tree.frame.id) : undefined,
            depth: tree.depth,
            collapsed: true,
            x,
            y,
            w: width,
            h: header,
            header,
            itemCount: countItems(tree),
          });
        },
      };
    }
    const block = layoutBlock(tree.items, edges);
    const children = tree.children.map(measureFrame);
    const contentW = Math.max(block.w, ...children.map((child) => child.w), card.w);
    const childrenH = children.reduce((sum, child) => sum + child.h, 0) + Math.max(0, children.length - 1) * LAYERED.nestedGap;
    const gap = block.h > 0 && children.length > 0 ? LAYERED.nestedGap : 0;
    const contentH = block.h + gap + childrenH;
    const h = header + contentH + (contentH > 0 ? LAYERED.padBottom : 0);
    return {
      w: Math.max(minWidth, contentW + 2 * LAYERED.padX),
      h,
      place: (x, y, width) => {
        const innerX = x + LAYERED.padX;
        const innerW = width - 2 * LAYERED.padX;
        const top = y + header;
        const byId = new Map(tree.items.map((item) => [item.id, item]));
        for (const [id, centre] of block.centres) {
          const node = byId.get(id)!;
          boxes.set(id, { id, x: innerX + centre.x, y: top + centre.y, w: node.w, h: node.h });
        }
        let childY = top + block.h + gap;
        tree.children.forEach((_, index) => {
          children[index].place(innerX, childY, innerW);
          childY += children[index].h + LAYERED.nestedGap;
        });
        placedFrames.push({
          id: tree.frame.id,
          label: tree.frame.label,
          meta: tree.frame.meta,
          parent: tree.depth > 1 ? parentOf(tree.frame.id) : undefined,
          depth: tree.depth,
          collapsed: false,
          x,
          y,
          w: width,
          h,
          header,
          itemCount: countItems(tree),
        });
      },
    };
  };

  const rootBlock = layoutBlock(rootItems, edges);
  const measured = roots.map(measureFrame);
  // Frames fill the width the view gives them (items stay left-aligned in them).
  const width = Math.max(rootBlock.w, ...measured.map((frame) => frame.w), measured.length > 0 && options.width > 0 ? options.width : 0);
  let y = LAYERED.pad;
  const rootById = new Map(rootItems.map((item) => [item.id, item]));
  for (const [id, centre] of rootBlock.centres) {
    const node = rootById.get(id)!;
    boxes.set(id, { id, x: LAYERED.pad + centre.x, y: y + centre.y, w: node.w, h: node.h });
  }
  if (rootBlock.h > 0) y += rootBlock.h + (measured.length > 0 ? LAYERED.frameGap : 0);
  measured.forEach((frame, index) => {
    frame.place(LAYERED.pad, y, width);
    y += frame.h + (index < measured.length - 1 ? LAYERED.frameGap : 0);
  });
  // Frames are pushed child first; draw parents first.
  placedFrames.sort((a, b) => a.depth - b.depth);

  // Goal headers: the frame's header strip is the goal's node.
  for (const frame of placedFrames) {
    const node = headers.get(frame.id);
    if (!node) continue;
    boxes.set(node.id, { id: node.id, x: frame.x + frame.w / 2, y: frame.y + frame.header / 2, w: frame.w, h: frame.header });
  }

  // Routes: an end hidden in a collapsed frame goes to its header.
  const routes = new Map<string, EdgeRoute>();
  const seen = new Set<string>();
  for (const edge of edges) {
    const from = hiddenTo.has(edge.from) ? hiddenTo.get(edge.from) : edge.from;
    const to = hiddenTo.has(edge.to) ? hiddenTo.get(edge.to) : edge.to;
    if (!from || !to || from === to) continue;
    const a = boxes.get(from);
    const b = boxes.get(to);
    if (!a || !b) continue;
    const key = `${edge.relation}\u0000${from}\u0000${to}`;
    if (from !== edge.from || to !== edge.to) {
      if (seen.has(key)) continue;
      seen.add(key);
    }
    routes.set(edge.id, routeBetween(a, b));
  }

  layout.nodes = boxes;
  layout.frames = placedFrames;
  layout.routes = routes;
  // The picture itself: the view adds its own padding around it.
  layout.bounds = { x: LAYERED.pad, y: LAYERED.pad, w: width, h: y - LAYERED.pad };
  return layout;
}
