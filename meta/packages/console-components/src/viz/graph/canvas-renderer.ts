// The canvas renderer (above 300 nodes or 800 edges, design spec A3.9):
// edges and marks on a 2D canvas, painted with resolved tokens (never hex).
// Labels, rings and the accessibility proxy stay in the SVG overlay.

import type { EdgeRoute } from "../layout/types";
import { markGeometry } from "../layout/geometry";
import type { VizTokens } from "../tokens";
import type { GraphModel, Highlight } from "./model";
import type { VizNode, VizNodeBox, VizViewport } from "./types";

export interface CanvasScene {
  width: number;
  height: number;
  dpr: number;
  viewport: VizViewport;
  model: GraphModel;
  boxes: ReadonlyMap<string, VizNodeBox>;
  routes: ReadonlyMap<string, EdgeRoute>;
  /** Cluster members drawn as dots of this radius (geometric). */
  dots: ReadonlyMap<string, number>;
  /** Clusters: only the highlighted edges are drawn between members. */
  clusterMode: boolean;
  tokens: VizTokens;
  highlight: Highlight | null;
  selected: ReadonlySet<string>;
  flashes: ReadonlySet<string>;
  live: ReadonlySet<string>;
  accentNodes: ReadonlySet<string>;
  accentEdges: ReadonlySet<string>;
  /** Pulse phase in [0, 1); null under reduced motion (static double ring). */
  phase: number | null;
}

const TAU = Math.PI * 2;

function withAlpha(colour: string, alpha: number): string {
  const match = /^rgba?\(([^)]+)\)$/.exec(colour.trim());
  if (!match) return colour;
  const [r, g, b, a] = match[1].split(",").map((part) => part.trim());
  const base = a === undefined ? 1 : Number(a);
  return `rgba(${r}, ${g}, ${b}, ${Math.max(0, Math.min(1, base * alpha))})`;
}

const DASHES: Record<string, number[]> = {
  route: [4, 3],
  lineage: [1, 3],
  parent: [1, 3],
  related: [1, 3],
  pending: [3, 3],
  failed: [4, 3],
  declared: [3, 3],
};

/** Draws the scene; returns the number of nodes drawn (culled ones excluded). */
export function drawCanvasScene(ctx: CanvasRenderingContext2D, scene: CanvasScene): number {
  const { viewport: v, tokens: t, model } = scene;
  ctx.setTransform(scene.dpr, 0, 0, scene.dpr, 0, 0);
  ctx.clearRect(0, 0, scene.width, scene.height);
  const sx = (x: number) => x * v.scale + v.tx;
  const sy = (y: number) => y * v.scale + v.ty;
  const margin = 24;
  const inView = (x: number, y: number) => x > -margin && y > -margin && x < scene.width + margin && y < scene.height + margin;
  const hovering = scene.highlight !== null;
  const lit = (id: string) => !hovering || scene.highlight!.nodes.has(id);

  // Edges, batched by stroke.
  const batches = new Map<string, { colour: string; width: number; dash: number[]; alpha: number; segments: (() => void)[] }>();
  const add = (key: string, colour: string, width: number, dash: number[], alpha: number, draw: () => void) => {
    const batch = batches.get(key);
    if (batch) batch.segments.push(draw);
    else batches.set(key, { colour, width, dash, alpha, segments: [draw] });
  };
  for (const edge of model.edges) {
    const a = scene.boxes.get(edge.from);
    const b = scene.boxes.get(edge.to);
    if (!a || !b) continue;
    const strong = scene.highlight?.edges.has(edge.id) ?? false;
    const downstream = scene.highlight?.downstreamEdges.has(edge.id) ?? false;
    if (scene.clusterMode && !strong && !downstream && !scene.accentEdges.has(edge.id)) continue;
    const x0 = sx(a.x);
    const y0 = sy(a.y);
    const x1 = sx(b.x);
    const y1 = sy(b.y);
    if (!inView(x0, y0) && !inView(x1, y1) && !scene.routes.has(edge.id)) {
      // Both ends off screen: skip unless the line crosses the view (cheap box test).
      if (Math.max(x0, x1) < 0 || Math.min(x0, x1) > scene.width || Math.max(y0, y1) < 0 || Math.min(y0, y1) > scene.height) continue;
    }
    const state = edge.state ?? "normal";
    const accent = scene.accentEdges.has(edge.id);
    let colour = t.edge;
    let width = 1;
    if (edge.relation === "traffic") {
      colour = withAlpha(t.edgeStrong, 0.7);
      const count = edge.weight ?? 1;
      width = count >= 10 ? 2 : count >= 3 ? 1.5 : 1;
    } else if (edge.relation === "blocks") {
      colour = t.edgeStrong;
      width = 1.25;
    } else if (edge.relation === "route" || edge.relation === "lineage" || edge.relation === "cross") {
      colour = t.mid;
      width = edge.relation === "lineage" ? 1.2 : 1;
    }
    if (state === "pending") colour = t.live;
    if (state === "failed") colour = t.negative;
    if (state === "declared") colour = t.faint;
    if (strong) {
      colour = t.edgeStrong;
      width += 0.5;
    } else if (downstream) {
      colour = t.mid;
    }
    if (accent) {
      colour = t.live;
      width = 2;
    }
    const dash = state !== "normal" ? DASHES[state] ?? [] : DASHES[edge.relation] ?? [];
    const alpha = hovering && !strong && !downstream ? 0.22 : 1;
    const route = scene.routes.get(edge.id);
    const key = `${colour}|${width}|${dash.join(",")}|${alpha}`;
    add(key, colour, width, dash, alpha, () => {
      ctx.moveTo(x0, y0);
      if (route) {
        ctx.bezierCurveTo(sx(route[1].x), sy(route[1].y), sx(route[2].x), sy(route[2].y), sx(route[3].x), sy(route[3].y));
      } else {
        ctx.lineTo(x1, y1);
      }
    });
  }
  ctx.lineCap = "round";
  for (const batch of batches.values()) {
    ctx.globalAlpha = batch.alpha;
    ctx.strokeStyle = batch.colour;
    ctx.lineWidth = batch.width;
    ctx.setLineDash(batch.dash);
    ctx.beginPath();
    for (const draw of batch.segments) draw();
    ctx.stroke();
  }
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;

  // Marks: dots batched by look; full marks one by one.
  let drawn = 0;
  const dotBatches = new Map<string, { fill: string | null; stroke: string | null; alpha: number; dots: [number, number, number][] }>();
  const full: VizNode[] = [];
  for (const node of model.nodes) {
    const box = scene.boxes.get(node.id);
    if (!box) continue;
    const x = sx(box.x);
    const y = sy(box.y);
    if (!inView(x, y)) continue;
    drawn += 1;
    const dot = scene.dots.get(node.id);
    if (dot === undefined) {
      full.push(node);
      continue;
    }
    const hot = scene.live.has(node.id) || scene.flashes.has(node.id);
    const alpha = lit(node.id) ? (node.retired ? 0.4 : 1) : 0.22;
    const r = dot * v.scale;
    const hollow = node.kind === "helper" || node.retired;
    const fill = hollow ? t.plate : hot ? t.live : node.kind === "external" || node.kind === "unknown" ? t.mid : t.ink;
    const stroke = hollow ? (hot ? t.live : t.ink) : null;
    const key = `${fill}|${stroke}|${alpha}`;
    const batch = dotBatches.get(key);
    if (batch) batch.dots.push([x, y, Math.max(0.8, r)]);
    else dotBatches.set(key, { fill, stroke, alpha, dots: [[x, y, Math.max(0.8, r)]] });
  }
  for (const batch of dotBatches.values()) {
    ctx.globalAlpha = batch.alpha;
    ctx.beginPath();
    for (const [x, y, r] of batch.dots) {
      ctx.moveTo(x + r, y);
      ctx.arc(x, y, r, 0, TAU);
    }
    if (batch.fill) {
      ctx.fillStyle = batch.fill;
      ctx.fill();
    }
    if (batch.stroke) {
      ctx.strokeStyle = batch.stroke;
      ctx.lineWidth = 1;
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;

  for (const node of full) {
    const box = scene.boxes.get(node.id)!;
    const x = sx(box.x);
    const y = sy(box.y);
    const { r, outer } = markGeometry(node.kind, node.size);
    const hot = scene.live.has(node.id) || scene.flashes.has(node.id);
    ctx.globalAlpha = lit(node.id) ? (node.retired ? 0.4 : 1) : 0.22;
    if (scene.live.has(node.id)) {
      ctx.strokeStyle = withAlpha(t.live, 0.35);
      ctx.lineWidth = 1;
      if (scene.phase === null) {
        ctx.beginPath();
        ctx.arc(x, y, r + 4, 0, TAU);
        ctx.moveTo(x + r + 6.5, y);
        ctx.arc(x, y, r + 6.5, 0, TAU);
        ctx.stroke();
      } else {
        ctx.beginPath();
        ctx.arc(x, y, r + 5 + Math.sin(scene.phase * TAU), 0, TAU);
        ctx.stroke();
      }
    }
    drawMark(ctx, node, x, y, r, hot, t);
    if (scene.selected.has(node.id)) {
      ctx.strokeStyle = t.live;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(x, y, outer + 5, 0, TAU);
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;
  return drawn;
}

function drawMark(ctx: CanvasRenderingContext2D, node: VizNode, x: number, y: number, r: number, hot: boolean, t: VizTokens): void {
  const retired = Boolean(node.retired);
  switch (node.kind) {
    case "helper":
    case "unknown": {
      ctx.fillStyle = t.plate;
      ctx.strokeStyle = hot ? t.live : node.kind === "unknown" ? t.mid : t.ink;
      ctx.lineWidth = 1.3;
      if (node.kind === "unknown") ctx.setLineDash([2, 2]);
      ctx.beginPath();
      ctx.arc(x, y, r, 0, TAU);
      ctx.fill();
      ctx.stroke();
      ctx.setLineDash([]);
      return;
    }
    case "person": {
      ctx.fillStyle = t.plate;
      ctx.strokeStyle = hot ? t.live : t.mid;
      ctx.lineWidth = 1.4;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, TAU);
      ctx.fill();
      ctx.stroke();
      ctx.fillStyle = hot ? t.live : t.mid;
      ctx.beginPath();
      ctx.arc(x, y, Math.max(1.6, r * 0.36), 0, TAU);
      ctx.fill();
      return;
    }
    case "source": {
      ctx.fillStyle = hot ? withAlpha(t.live, 0.18) : t.plate;
      ctx.strokeStyle = hot ? t.live : t.mid;
      ctx.lineWidth = 1.2;
      ctx.fillRect(x - r, y - r, r * 2, r * 2);
      ctx.strokeRect(x - r, y - r, r * 2, r * 2);
      return;
    }
    case "store": {
      const hw = r;
      const e = r * 0.34;
      const top = y - r * 0.93 + e;
      const bottom = y + r * 0.93 - e;
      ctx.fillStyle = t.plate;
      ctx.strokeStyle = hot ? t.live : t.ink;
      ctx.lineWidth = 1.2;
      ctx.beginPath();
      ctx.ellipse(x, top, hw, e, 0, Math.PI, 0);
      ctx.lineTo(x + hw, bottom);
      ctx.ellipse(x, bottom, hw, e, 0, 0, Math.PI);
      ctx.closePath();
      ctx.fill();
      ctx.stroke();
      ctx.beginPath();
      ctx.ellipse(x, top, hw, e, 0, 0, Math.PI);
      ctx.stroke();
      return;
    }
    case "external": {
      ctx.fillStyle = hot ? t.live : t.mid;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, TAU);
      ctx.fill();
      ctx.strokeStyle = t.mid;
      ctx.lineWidth = 1;
      ctx.setLineDash([2, 2]);
      ctx.beginPath();
      ctx.arc(x, y, r + 3, 0, TAU);
      ctx.stroke();
      ctx.setLineDash([]);
      return;
    }
    case "coordinator":
    case "agent":
    default: {
      if (retired) {
        ctx.fillStyle = t.plate;
        ctx.strokeStyle = t.ink;
        ctx.lineWidth = 1.3;
        ctx.beginPath();
        ctx.arc(x, y, r, 0, TAU);
        ctx.fill();
        ctx.stroke();
      } else {
        ctx.fillStyle = hot ? t.live : t.ink;
        ctx.beginPath();
        ctx.arc(x, y, r, 0, TAU);
        ctx.fill();
      }
      if (node.kind === "coordinator") {
        ctx.strokeStyle = withAlpha(hot ? t.live : t.ink, 0.5);
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(x, y, r + 3, 0, TAU);
        ctx.stroke();
      }
    }
  }
}

/** Nodes near a screen point, nearest first, within `radius` px (canvas hit testing). */
export function nodeAtPoint(
  boxes: ReadonlyMap<string, VizNodeBox>,
  viewport: VizViewport,
  px: number,
  py: number,
  radius: number,
  dots: ReadonlyMap<string, number>,
): string | null {
  let best: string | null = null;
  let bestDistance = Infinity;
  for (const box of boxes.values()) {
    const x = box.x * viewport.scale + viewport.tx;
    const y = box.y * viewport.scale + viewport.ty;
    const dot = dots.get(box.id);
    const reach = Math.max(radius, dot !== undefined ? dot * viewport.scale + 3 : Math.max(box.w, box.h) / 2 + 3);
    const distance = Math.hypot(px - x, py - y);
    if (distance <= reach && distance < bestDistance) {
      best = box.id;
      bestDistance = distance;
    }
  }
  return best;
}
