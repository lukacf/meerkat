import type { WorkGraphWireEdge, WorkGraphWireItem } from "../types";

/// Deterministic layered-DAG layout for the WorkGraph graph view.
///
/// Column = depth from parent edges (same semantics as
/// `buildWorkGraphPanelTree`: parent edges run child→parent, placement is
/// first-parent-wins, children of unknown parents are roots). Graphs without
/// a rendered parent edge instead advance columns along blocks dependencies,
/// so parallel prerequisites converge on the dependent. Row = stable index
/// within the column, ordered by created_at then id; dependency columns are
/// centered. No crossing minimization - the guardrails are determinism and
/// the node cap.

export const WORKGRAPH_GRAPH_COL_WIDTH = 220;
export const WORKGRAPH_GRAPH_ROW_HEIGHT = 64;
export const WORKGRAPH_GRAPH_NODE_WIDTH = 168;
export const WORKGRAPH_GRAPH_NODE_HEIGHT = 44;
/// Hard cap on rendered nodes; the surplus is reported, never silently cut.
export const WORKGRAPH_GRAPH_NODE_CAP = 200;
const PAD = 24;

export interface WorkGraphLayoutPoint {
  x: number;
  y: number;
}

export interface WorkGraphLayoutNode {
  itemId: string;
  x: number;
  y: number;
  w: number;
  h: number;
  status: string;
  title: string;
  priority?: string;
  ownerLabel: string;
  blocked: boolean;
  /// Titles (or ids) of parents beyond the placing one - the card's
  /// "also under X, Y" fold rule, kept as text instead of extra edges.
  alsoUnder: string[];
}

export interface WorkGraphLayoutEdge {
  kind: string;
  fromId: string;
  toId: string;
  /// Cubic bezier control points: [start, c1, c2, end].
  points: [WorkGraphLayoutPoint, WorkGraphLayoutPoint, WorkGraphLayoutPoint, WorkGraphLayoutPoint];
}

export interface WorkGraphLayout {
  nodes: WorkGraphLayoutNode[];
  edges: WorkGraphLayoutEdge[];
  width: number;
  height: number;
  /// Input items not drawn: past WORKGRAPH_GRAPH_NODE_CAP, id-less, or
  /// shadowed by a duplicate id. Always items.length - nodes.length.
  overflowCount: number;
}

/// Owner-label precedence shared by the tree rows, the graph nodes, and the
/// graph selection footer: display name, then key id, then the claim's.
export function workGraphItemOwnerLabel(item: WorkGraphWireItem): string {
  return item.owner?.display_name
    || item.owner?.key?.id
    || item.claim?.owner?.display_name
    || item.claim?.owner?.key?.id
    || "";
}

/// created_at then id - the tree panel's comparator, so both views agree.
function compareIds(byId: Map<string, WorkGraphWireItem>, left: string, right: string): number {
  const leftKey = byId.get(left)?.created_at || "";
  const rightKey = byId.get(right)?.created_at || "";
  if (leftKey !== rightKey) return leftKey < rightKey ? -1 : 1;
  return left < right ? -1 : left === right ? 0 : 1;
}

function edgePoints(
  from: { x: number; y: number; w: number; h: number },
  to: { x: number; y: number; w: number; h: number },
): WorkGraphLayoutEdge["points"] {
  let start: WorkGraphLayoutPoint;
  let end: WorkGraphLayoutPoint;
  if (to.x + to.w < from.x) {
    // Target strictly left of source (the common child→parent direction).
    start = { x: from.x, y: from.y + from.h / 2 };
    end = { x: to.x + to.w, y: to.y + to.h / 2 };
  } else if (to.x > from.x + from.w) {
    start = { x: from.x + from.w, y: from.y + from.h / 2 };
    end = { x: to.x, y: to.y + to.h / 2 };
  } else if (to.y > from.y) {
    // Same column: leave through the bottom/top edges.
    start = { x: from.x + from.w / 2, y: from.y + from.h };
    end = { x: to.x + to.w / 2, y: to.y };
  } else {
    start = { x: from.x + from.w / 2, y: from.y };
    end = { x: to.x + to.w / 2, y: to.y + to.h };
  }
  const horizontal = Math.abs(end.x - start.x) >= Math.abs(end.y - start.y);
  const bend = horizontal
    ? Math.max(24, Math.abs(end.x - start.x) * 0.4)
    : Math.max(24, Math.abs(end.y - start.y) * 0.4);
  const c1 = horizontal
    ? { x: start.x + Math.sign(end.x - start.x) * bend, y: start.y }
    : { x: start.x, y: start.y + Math.sign(end.y - start.y) * bend };
  const c2 = horizontal
    ? { x: end.x - Math.sign(end.x - start.x) * bend, y: end.y }
    : { x: end.x, y: end.y - Math.sign(end.y - start.y) * bend };
  return [start, c1, c2, end];
}

export function layoutWorkGraph(
  items: WorkGraphWireItem[],
  edges: WorkGraphWireEdge[],
): WorkGraphLayout {
  const byId = new Map<string, WorkGraphWireItem>();
  for (const item of items) {
    if (typeof item.id === "string" && item.id) byId.set(item.id, item);
  }

  // First parent wins the placement; the rest become "also under" notes.
  const parentOf = new Map<string, string>();
  const extraParents = new Map<string, string[]>();
  for (const edge of edges) {
    if (edge.kind !== "parent") continue;
    if (
      typeof edge.from_id !== "string" || !edge.from_id
      || typeof edge.to_id !== "string" || !edge.to_id
      || edge.from_id === edge.to_id
    ) continue;
    const placed = parentOf.get(edge.from_id);
    if (placed === undefined) {
      parentOf.set(edge.from_id, edge.to_id);
    } else if (placed !== edge.to_id) {
      const extras = extraParents.get(edge.from_id) || [];
      if (!extras.includes(edge.to_id)) extras.push(edge.to_id);
      extraParents.set(edge.from_id, extras);
    }
  }

  const sortedIds = [...byId.keys()].sort((left, right) => compareIds(byId, left, right));
  const keptIds = sortedIds.slice(0, WORKGRAPH_GRAPH_NODE_CAP);
  // Everything in keptIds renders, so this counts every input item that
  // does not: the capped tail plus id-less/duplicate-id rows.
  const overflowCount = items.length - keptIds.length;
  const kept = new Set(keptIds);

  // Depth from parent chains, cycle-guarded (a parent cycle degrades to
  // roots rather than recursing forever). Unknown/dropped parents = root.
  const depthOf = new Map<string, number>();
  const resolveDepth = (id: string, onPath: Set<string>): number => {
    const memo = depthOf.get(id);
    if (memo !== undefined) return memo;
    const parent = parentOf.get(id);
    let depth = 0;
    if (parent && kept.has(parent) && !onPath.has(parent)) {
      onPath.add(id);
      depth = resolveDepth(parent, onPath) + 1;
      onPath.delete(id);
    }
    depthOf.set(id, depth);
    return depth;
  };
  for (const id of keptIds) resolveDepth(id, new Set([id]));

  // Preserve the established structural tree when one is present. Otherwise
  // use only typed blocks edges for dependency ranks, never relates/unknown
  // kinds or targets that were omitted by the node cap.
  const hasRenderedParent = [...parentOf].some(([child, parent]) => kept.has(child) && kept.has(parent));
  let dependencyColumns = false;
  if (!hasRenderedParent) {
    const dependents = new Map(keptIds.map(id => [id, new Set<string>()]));
    const prerequisites = new Map(keptIds.map(id => [id, 0]));
    for (const edge of edges) {
      if (edge.kind !== "blocks" || !edge.from_id || !edge.to_id || edge.from_id === edge.to_id) continue;
      if (!kept.has(edge.from_id) || !kept.has(edge.to_id)) continue;
      const targets = dependents.get(edge.from_id)!;
      if (targets.has(edge.to_id)) continue;
      targets.add(edge.to_id);
      prerequisites.set(edge.to_id, prerequisites.get(edge.to_id)! + 1);
      dependencyColumns = true;
    }
    if (dependencyColumns) {
      const ready = keptIds.filter(id => prerequisites.get(id) === 0);
      const ranks = new Map(keptIds.map(id => [id, 0]));
      const resolved = new Set<string>();
      for (let index = 0; index < ready.length; index += 1) {
        const id = ready[index];
        resolved.add(id);
        for (const dependent of dependents.get(id)!) {
          ranks.set(dependent, Math.max(ranks.get(dependent)!, ranks.get(id)! + 1));
          const remaining = prerequisites.get(dependent)! - 1;
          prerequisites.set(dependent, remaining);
          if (remaining === 0) ready.push(dependent);
        }
      }
      // Cycles and their downstream nodes cannot be topologically placed;
      // retain them as roots rather than inventing an order or looping.
      for (const id of keptIds) depthOf.set(id, resolved.has(id) ? ranks.get(id)! : 0);
    }
  }
  const rowsPerColumn = new Map<number, number>();
  for (const id of keptIds) {
    const depth = depthOf.get(id) ?? 0;
    rowsPerColumn.set(depth, (rowsPerColumn.get(depth) ?? 0) + 1);
  }
  const tallestColumn = Math.max(0, ...rowsPerColumn.values());

  // Row = stable index within the column, following the global sort.
  const columnRows = new Map<number, number>();
  const rects = new Map<string, { x: number; y: number; w: number; h: number }>();
  const nodes: WorkGraphLayoutNode[] = [];
  let maxDepth = 0;
  let maxRows = 0;
  for (const id of keptIds) {
    const item = byId.get(id);
    if (!item) continue;
    const depth = depthOf.get(id) ?? 0;
    const row = columnRows.get(depth) ?? 0;
    columnRows.set(depth, row + 1);
    maxDepth = Math.max(maxDepth, depth);
    maxRows = Math.max(maxRows, row + 1);
    const rect = {
      x: PAD + depth * WORKGRAPH_GRAPH_COL_WIDTH,
      y: PAD + (row + (dependencyColumns ? (tallestColumn - rowsPerColumn.get(depth)!) / 2 : 0)) * WORKGRAPH_GRAPH_ROW_HEIGHT,
      w: WORKGRAPH_GRAPH_NODE_WIDTH,
      h: WORKGRAPH_GRAPH_NODE_HEIGHT,
    };
    rects.set(id, rect);
    const status = item.status || "open";
    nodes.push({
      itemId: id,
      ...rect,
      status,
      title: item.title || id,
      priority: item.priority,
      ownerLabel: workGraphItemOwnerLabel(item),
      blocked: status === "blocked",
      alsoUnder: (extraParents.get(id) || []).map(
        (parentId) => byId.get(parentId)?.title || parentId,
      ),
    });
  }

  const layoutEdges: WorkGraphLayoutEdge[] = [];
  // Structural parent edges: only the placing parent (extras are alsoUnder).
  for (const [child, parent] of parentOf) {
    const from = rects.get(child);
    const to = rects.get(parent);
    if (!from || !to) continue;
    layoutEdges.push({ kind: "parent", fromId: child, toId: parent, points: edgePoints(from, to) });
  }
  // Every other edge kind passes through as geometry when both ends render,
  // with the same hygiene the parent loop applies: no self edges (a
  // degenerate bezier through the node's own body), no duplicates (stacked
  // double-stroke paths).
  const seenEdges = new Set<string>();
  for (const edge of edges) {
    if (edge.kind === "parent" || typeof edge.kind !== "string" || !edge.kind) continue;
    if (typeof edge.from_id !== "string" || typeof edge.to_id !== "string") continue;
    if (edge.from_id === edge.to_id) continue;
    const dedupeKey = `${edge.kind} ${edge.from_id} ${edge.to_id}`;
    if (seenEdges.has(dedupeKey)) continue;
    seenEdges.add(dedupeKey);
    const from = rects.get(edge.from_id);
    const to = rects.get(edge.to_id);
    if (!from || !to) continue;
    layoutEdges.push({
      kind: edge.kind,
      fromId: edge.from_id,
      toId: edge.to_id,
      points: edgePoints(from, to),
    });
  }

  if (nodes.length === 0) {
    return { nodes, edges: layoutEdges, width: 0, height: 0, overflowCount };
  }
  return {
    nodes,
    edges: layoutEdges,
    width: PAD * 2 + maxDepth * WORKGRAPH_GRAPH_COL_WIDTH + WORKGRAPH_GRAPH_NODE_WIDTH,
    height: PAD * 2 + (maxRows - 1) * WORKGRAPH_GRAPH_ROW_HEIGHT + WORKGRAPH_GRAPH_NODE_HEIGHT,
    overflowCount,
  };
}

export function workGraphEdgePath(edge: WorkGraphLayoutEdge): string {
  const [p0, p1, p2, p3] = edge.points;
  return `M ${p0.x} ${p0.y} C ${p1.x} ${p1.y}, ${p2.x} ${p2.y}, ${p3.x} ${p3.y}`;
}

/// Bezier midpoint (t = 0.5) for edge-kind labels.
export function workGraphEdgeMidpoint(edge: WorkGraphLayoutEdge): WorkGraphLayoutPoint {
  const [p0, p1, p2, p3] = edge.points;
  return {
    x: (p0.x + 3 * p1.x + 3 * p2.x + p3.x) / 8,
    y: (p0.y + 3 * p1.y + 3 * p2.y + p3.y) / 8,
  };
}
