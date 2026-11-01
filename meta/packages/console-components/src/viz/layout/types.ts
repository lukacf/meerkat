// Layout model shared by every layout. Coordinates are layout space: CSS
// pixels at zoom 1, y down. A placed node's x and y are its centre.

import type { VizLayoutKind, VizNodeBox, VizNodeKind, VizRelation } from "../graph/types";

/** A node as the layouts see it: identity, grouping and the room it needs. */
export interface LayoutNode {
  id: string;
  kind: VizNodeKind;
  label: string;
  lane?: string;
  frame?: string;
  order?: number;
  rank?: number;
  anchor?: string;
  live?: boolean;
  /** Box (cards) or mark (diameter) size. */
  w: number;
  h: number;
  /** The label's width at 11 px; spacing and collision use it. */
  labelWidth: number;
}

export interface LayoutEdge {
  id: string;
  from: string;
  to: string;
  relation: VizRelation;
  weight?: number;
}

export interface LayoutLaneInput {
  id: string;
  label: string;
}

export interface LayoutFrameInput {
  id: string;
  label: string;
  meta?: string;
  parent?: string;
  order?: number;
  collapsed?: boolean;
}

/** A lane band: y is its top edge, h its height (layout space). */
export interface PlacedLane {
  id: string;
  label: string;
  y: number;
  h: number;
  /** The y of the band's first row of marks: the label aligns with it. */
  labelY: number;
}

/** A frame rectangle (x, y top-left) with its header strip. */
export interface PlacedFrame {
  id: string;
  label: string;
  meta?: string;
  parent?: string;
  depth: number;
  collapsed: boolean;
  x: number;
  y: number;
  w: number;
  h: number;
  /** Height of the header strip at the top. */
  header: number;
  /** Items the frame holds, at any depth (collapsed frames included). */
  itemCount: number;
}

/** One group of the clusters layout: concentric rings around (cx, cy). */
export interface PlacedCluster {
  id: string;
  label: string;
  cx: number;
  cy: number;
  /** Outer radius of the drawn rings. */
  r: number;
  /** Ring radii, inner to outer. */
  rings: number[];
  count: number;
  /** The coordinator drawn at the centre, at full size. */
  lead?: string;
}

/**
 * A bundle's line between two groups' rings: a quadratic from (x0, y0) to
 * (x1, y1) through the control point (cx, cy); straight unless it bows
 * around a group that sits on the way.
 */
export interface BundleRoute {
  x0: number;
  y0: number;
  cx: number;
  cy: number;
  x1: number;
  y1: number;
  bowed: boolean;
}

/** Inter-group edges of the clusters layout, one line per group pair. */
export interface ClusterBundle {
  id: string;
  from: string;
  to: string;
  count: number;
  relations: VizRelation[];
  route: BundleRoute;
}

export interface LayoutPoint {
  x: number;
  y: number;
}

/** A routed edge: cubic Bezier control points [start, c1, c2, end]. */
export type EdgeRoute = readonly [LayoutPoint, LayoutPoint, LayoutPoint, LayoutPoint];

export interface LayoutBounds {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface GraphLayout {
  /** The layout drawn (force above the node limit draws clusters). */
  kind: VizLayoutKind;
  nodes: Map<string, VizNodeBox>;
  lanes: PlacedLane[];
  frames: PlacedFrame[];
  clusters: PlacedCluster[];
  bundles: ClusterBundle[];
  /** Routed edges (layered); other edges are straight centre to centre. */
  routes: Map<string, EdgeRoute>;
  /** Radial: the centre node and each peer's angle (labels sit outward). */
  angles: Map<string, number>;
  bounds: LayoutBounds;
  /** Lanes: the label gutter at the left of the bands. */
  gutter: number;
}

export interface LayoutOptions {
  /** The viewport width the layout may use (lanes spread across it, clusters wrap in it). */
  width: number;
  height: number;
  density: "comfortable" | "compact";
}

export function emptyLayout(kind: VizLayoutKind): GraphLayout {
  return {
    kind,
    nodes: new Map(),
    lanes: [],
    frames: [],
    clusters: [],
    bundles: [],
    routes: new Map(),
    angles: new Map(),
    bounds: { x: 0, y: 0, w: 0, h: 0 },
    gutter: 0,
  };
}

/** The bounding box of placed nodes (with each node's own extent), padded. */
export function boundsOf(boxes: Iterable<VizNodeBox>, pad = 0): LayoutBounds {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const box of boxes) {
    x0 = Math.min(x0, box.x - box.w / 2);
    y0 = Math.min(y0, box.y - box.h / 2);
    x1 = Math.max(x1, box.x + box.w / 2);
    y1 = Math.max(y1, box.y + box.h / 2);
  }
  if (!Number.isFinite(x0)) return { x: 0, y: 0, w: 0, h: 0 };
  return { x: x0 - pad, y: y0 - pad, w: x1 - x0 + 2 * pad, h: y1 - y0 + 2 * pad };
}

export function unionBounds(a: LayoutBounds, b: LayoutBounds): LayoutBounds {
  if (a.w <= 0 && a.h <= 0) return b;
  if (b.w <= 0 && b.h <= 0) return a;
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, w: Math.max(a.x + a.w, b.x + b.w) - x, h: Math.max(a.y + a.h, b.y + b.h) - y };
}

/** One collator: localeCompare with options builds one per call. */
const LABELS = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

/** Compares labels the way people read them ("Agent 2" before "Agent 10", case aside). */
export function compareLabels(a: string, b: string): number {
  return LABELS.compare(a, b);
}

/** Manual order first, then the label, then the id: every layout sorts alike. */
export function compareNodes(a: LayoutNode, b: LayoutNode): number {
  const ao = a.order ?? Number.POSITIVE_INFINITY;
  const bo = b.order ?? Number.POSITIVE_INFINITY;
  if (ao !== bo) return ao < bo ? -1 : 1;
  const byLabel = LABELS.compare(a.label, b.label);
  if (byLabel !== 0) return byLabel;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}
