// Edge geometry: straight lines between mark centres (offset among parallel
// edges), routed cubics in the layered layout, the point and tangent at any
// t for particles, chevrons and midpoint marks.

import type { EdgeRoute, LayoutPoint } from "../layout/types";
import type { VizNodeBox } from "./types";

export interface EdgeShape {
  /** Cubic control points; a straight edge has c1 and c2 on the line. */
  points: EdgeRoute;
  straight: boolean;
}

function lerp(a: LayoutPoint, b: LayoutPoint, t: number): LayoutPoint {
  return { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
}

/** The shape of an edge: its route when the layout routed it, else centre to centre. */
export function edgeShape(from: VizNodeBox, to: VizNodeBox, route?: EdgeRoute): EdgeShape {
  if (route) return { points: route, straight: false };
  const a = { x: from.x, y: from.y };
  const b = { x: to.x, y: to.y };
  return { points: [a, lerp(a, b, 1 / 3), lerp(a, b, 2 / 3), b], straight: true };
}

export function pointAt(shape: EdgeShape, t: number): LayoutPoint {
  const [p0, p1, p2, p3] = shape.points;
  if (shape.straight) return lerp(p0, p3, t);
  const u = 1 - t;
  return {
    x: u * u * u * p0.x + 3 * u * u * t * p1.x + 3 * u * t * t * p2.x + t * t * t * p3.x,
    y: u * u * u * p0.y + 3 * u * u * t * p1.y + 3 * u * t * t * p2.y + t * t * t * p3.y,
  };
}

/** The direction of travel at t, in radians. */
export function angleAt(shape: EdgeShape, t: number): number {
  const [p0, p1, p2, p3] = shape.points;
  if (shape.straight) return Math.atan2(p3.y - p0.y, p3.x - p0.x);
  const u = 1 - t;
  const dx = 3 * u * u * (p1.x - p0.x) + 6 * u * t * (p2.x - p1.x) + 3 * t * t * (p3.x - p2.x);
  const dy = 3 * u * u * (p1.y - p0.y) + 6 * u * t * (p2.y - p1.y) + 3 * t * t * (p3.y - p2.y);
  if (dx === 0 && dy === 0) return Math.atan2(p3.y - p0.y, p3.x - p0.x);
  return Math.atan2(dy, dx);
}

/** The path data: "M x y L x y" for straight edges, a cubic otherwise. */
export function edgePath(shape: EdgeShape): string {
  const [p0, p1, p2, p3] = shape.points;
  if (shape.straight) return `M${r(p0.x)} ${r(p0.y)}L${r(p3.x)} ${r(p3.y)}`;
  return `M${r(p0.x)} ${r(p0.y)}C${r(p1.x)} ${r(p1.y)} ${r(p2.x)} ${r(p2.y)} ${r(p3.x)} ${r(p3.y)}`;
}

/** Arc length, approximated in 12 steps (curves) or exact (lines). */
export function edgeLength(shape: EdgeShape): number {
  if (shape.straight) {
    const [p0, , , p3] = shape.points;
    return Math.hypot(p3.x - p0.x, p3.y - p0.y);
  }
  let length = 0;
  let previous = pointAt(shape, 0);
  for (let i = 1; i <= 12; i += 1) {
    const next = pointAt(shape, i / 12);
    length += Math.hypot(next.x - previous.x, next.y - previous.y);
    previous = next;
  }
  return length;
}

/** The unit normal of a straight edge (left of travel), for parallel offsets. */
export function edgeNormal(from: LayoutPoint, to: LayoutPoint): LayoutPoint {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy) || 1;
  return { x: -dy / length, y: dx / length };
}

function r(value: number): number {
  return Math.round(value * 10) / 10;
}

/** Traffic stroke width by count (A3.9): 1-2 thin, 3-9 medium, 10+ thick. */
export function trafficWidth(weight: number | undefined): number {
  const count = weight ?? 1;
  if (count >= 10) return 2;
  if (count >= 3) return 1.5;
  return 1;
}

/** Bundle stroke width by count (clusters): 1 px for one edge, about 2.5 px for a hundred. */
export function bundleWidth(count: number): number {
  return Math.min(3, 1 + Math.log10(Math.max(1, count)) * 0.75);
}
