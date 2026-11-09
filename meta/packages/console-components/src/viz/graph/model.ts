// The view model a GraphView draws: what is visible (retired nodes, legend
// toggles, focus mode), adjacency and degree, the hubs that keep their
// labels, each edge's direction and its offset among parallel edges.

import { RELATION_ORDER } from "../layout/radial";
import type { VizEdge, VizNode, VizNodeKind, VizRelation } from "./types";

/** Relations that point from one end to the other unless an edge says otherwise. */
const DIRECTED_BY_DEFAULT: Readonly<Record<VizRelation, boolean>> = {
  wired: false,
  traffic: false,
  route: true,
  lineage: true,
  cross: false,
  blocks: true,
  parent: false,
  related: false,
};

/** Screen pixels between two parallel edges of one pair (A3.9: offset by +-3 px). */
export const PARALLEL_STEP = 6;

export interface GraphModel {
  nodes: VizNode[];
  edges: VizEdge[];
  byId: Map<string, VizNode>;
  edgeById: Map<string, VizEdge>;
  degree: Map<string, number>;
  neighbours: Map<string, Set<string>>;
  incident: Map<string, string[]>;
  hubs: Set<string>;
  /** Perpendicular offset (screen px) of an edge among parallel edges of its pair. */
  offsets: Map<string, number>;
  directed: Map<string, boolean>;
  /** Every relation drawn or toggled off, with its count (the legend lists both). */
  relations: Map<VizRelation, number>;
  kinds: Map<VizNodeKind, number>;
}

export interface GraphModelInput {
  nodes: readonly VizNode[];
  edges: readonly VizEdge[];
  showRetired?: boolean;
  hiddenRelations?: ReadonlySet<VizRelation>;
  /** Focus mode: draw only this node and its 1-hop neighbourhood. */
  focus?: string | null;
}

export function edgeIsDirected(edge: VizEdge): boolean {
  return edge.directed ?? DIRECTED_BY_DEFAULT[edge.relation] ?? false;
}

function pairKey(a: string, b: string): string {
  return a < b ? `${a}\u0000${b}` : `${b}\u0000${a}`;
}

export function buildGraphModel(input: GraphModelInput): GraphModel {
  const seenIds = new Set<string>();
  let nodes = input.nodes.filter((node) => {
    if (!node.id || seenIds.has(node.id)) return false;
    seenIds.add(node.id);
    return input.showRetired || !node.retired;
  });
  let byId = new Map(nodes.map((node) => [node.id, node]));
  const seenEdges = new Set<string>();
  const relations = new Map<VizRelation, number>();
  let candidates = input.edges.filter((edge) => {
    if (!edge.id || seenEdges.has(edge.id) || edge.from === edge.to) return false;
    if (!byId.has(edge.from) || !byId.has(edge.to)) return false;
    seenEdges.add(edge.id);
    return true;
  });

  if (input.focus && byId.has(input.focus)) {
    const keep = new Set([input.focus]);
    for (const edge of candidates) {
      if (input.hiddenRelations?.has(edge.relation)) continue;
      if (edge.from === input.focus) keep.add(edge.to);
      if (edge.to === input.focus) keep.add(edge.from);
    }
    nodes = nodes.filter((node) => keep.has(node.id));
    byId = new Map(nodes.map((node) => [node.id, node]));
    candidates = candidates.filter((edge) => keep.has(edge.from) && keep.has(edge.to));
  }

  for (const edge of candidates) relations.set(edge.relation, (relations.get(edge.relation) ?? 0) + 1);
  const edges = candidates.filter((edge) => !input.hiddenRelations?.has(edge.relation));
  const edgeById = new Map(edges.map((edge) => [edge.id, edge]));

  const degree = new Map<string, number>();
  const neighbours = new Map<string, Set<string>>();
  const incident = new Map<string, string[]>();
  for (const node of nodes) {
    degree.set(node.id, 0);
    neighbours.set(node.id, new Set());
    incident.set(node.id, []);
  }
  const pairs = new Map<string, VizEdge[]>();
  const directed = new Map<string, boolean>();
  for (const edge of edges) {
    degree.set(edge.from, (degree.get(edge.from) ?? 0) + 1);
    degree.set(edge.to, (degree.get(edge.to) ?? 0) + 1);
    neighbours.get(edge.from)!.add(edge.to);
    neighbours.get(edge.to)!.add(edge.from);
    incident.get(edge.from)!.push(edge.id);
    incident.get(edge.to)!.push(edge.id);
    directed.set(edge.id, edgeIsDirected(edge));
    const key = pairKey(edge.from, edge.to);
    const list = pairs.get(key);
    if (list) list.push(edge);
    else pairs.set(key, [edge]);
  }

  // Parallel edges of one pair: offsets centred on the line, in relation order.
  const offsets = new Map<string, number>();
  for (const list of pairs.values()) {
    if (list.length < 2) continue;
    list.sort((a, b) => RELATION_ORDER.indexOf(a.relation) - RELATION_ORDER.indexOf(b.relation) || (a.id < b.id ? -1 : 1));
    list.forEach((edge, index) => {
      // The offset is perpendicular to the pair's canonical direction (low id to high id).
      const sign = edge.from < edge.to ? 1 : -1;
      offsets.set(edge.id, sign * (index - (list.length - 1) / 2) * PARALLEL_STEP);
    });
  }

  // Hubs: the top 15 % by degree (at least degree 2), and nodes marked hub.
  const hubs = new Set<string>();
  const ranked = nodes.filter((node) => (degree.get(node.id) ?? 0) >= 2).sort((a, b) => (degree.get(b.id) ?? 0) - (degree.get(a.id) ?? 0));
  const hubCount = Math.ceil(nodes.length * 0.15);
  ranked.slice(0, hubCount).forEach((node) => hubs.add(node.id));
  for (const node of nodes) if (node.hub) hubs.add(node.id);

  const kinds = new Map<VizNodeKind, number>();
  for (const node of nodes) kinds.set(node.kind, (kinds.get(node.kind) ?? 0) + 1);

  return { nodes, edges, byId, edgeById, degree, neighbours, incident, hubs, offsets, directed, relations, kinds };
}

export interface Highlight {
  /** Nodes kept at full ink. */
  nodes: Set<string>;
  /** Edges raised to the strong stroke. */
  edges: Set<string>;
  /** Chain mode: what lies downstream (drawn mid rather than strong). */
  downstream: Set<string>;
  downstreamEdges: Set<string>;
}

/** What a hover keeps at full ink: the node and its neighbours, or its whole chain. */
export function highlightFor(model: GraphModel, id: string, mode: "neighbours" | "chain"): Highlight {
  const nodes = new Set([id]);
  const edges = new Set<string>();
  const downstream = new Set<string>();
  const downstreamEdges = new Set<string>();
  if (mode === "neighbours") {
    for (const edgeId of model.incident.get(id) ?? []) {
      const edge = model.edgeById.get(edgeId);
      if (!edge) continue;
      edges.add(edgeId);
      nodes.add(edge.from === id ? edge.to : edge.from);
    }
    return { nodes, edges, downstream, downstreamEdges };
  }
  // Chain: walk directed edges upstream (into the node) and downstream (out of it).
  const walk = (start: string, forward: boolean, into: Set<string>) => {
    const queue = [start];
    const seen = new Set([start]);
    while (queue.length > 0) {
      const current = queue.shift()!;
      for (const edgeId of model.incident.get(current) ?? []) {
        const edge = model.edgeById.get(edgeId);
        if (!edge || !model.directed.get(edgeId)) continue;
        const next = forward ? (edge.from === current ? edge.to : null) : edge.to === current ? edge.from : null;
        if (next === null) continue;
        if (forward) {
          downstreamEdges.add(edgeId);
          into.add(next);
        } else {
          edges.add(edgeId);
          nodes.add(next);
        }
        if (!seen.has(next)) {
          seen.add(next);
          queue.push(next);
        }
      }
    }
  };
  walk(id, false, nodes);
  walk(id, true, downstream);
  for (const node of downstream) nodes.add(node);
  return { nodes, edges, downstream, downstreamEdges };
}
