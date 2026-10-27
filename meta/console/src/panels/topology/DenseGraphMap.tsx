import React from "react";
import {
  colourForRole,
  edgeKey,
  roleIndexFor,
  type TopoAgent,
  type TopoActivity,
  type TopoGraph,
} from "./data";

interface DenseGraphMapProps {
  graph: TopoGraph;
  edgeMode?: "all" | "focus";
  activity?: TopoActivity;
}

interface LayoutNode {
  id: string;
  agent: TopoAgent;
  groupIndex: number;
  x: number;
  y: number;
  radius: number;
}

interface GroupAnchor {
  name: string;
  x: number;
  y: number;
  count: number;
  colour: string;
}

interface Layout {
  nodes: LayoutNode[];
  byId: Map<string, LayoutNode>;
  groups: GroupAnchor[];
  edgeById: Map<string, TopoGraph["edges"][number][]>;
  width: number;
  height: number;
}

interface Viewport {
  scale: number;
  x: number;
  y: number;
}

const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));
const EMPTY_ACTIVITY: TopoActivity = { active: {}, busy: {}, calls: {}, pulses: [] };
const ACTIVITY_LIFE_MS = 8000;
const SMALL_GRAPH_NODE_LIMIT = 16;
const SMALL_GRAPH_EDGE_LIMIT = 80;

function hash(value: string): number {
  let h = 2166136261;
  for (let i = 0; i < value.length; i++) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

function cssVar(el: HTMLElement, name: string, fallback: string): string {
  const value = getComputedStyle(el).getPropertyValue(name).trim();
  return value || fallback;
}

function groupPalette(index: number): string {
  const colours = [
    "hsl(188 74% 66%)",
    "hsl(156 68% 62%)",
    "hsl(260 54% 72%)",
    "hsl(35 70% 69%)",
    "hsl(214 76% 66%)",
    "hsl(335 58% 70%)",
  ];
  return colours[index % colours.length];
}

function withAlpha(colour: string, alpha: number): string {
  if (colour.startsWith("hsl(") && !colour.includes("/")) {
    return colour.replace(")", ` / ${alpha})`);
  }
  return colour;
}

function fitLayout(nodes: LayoutNode[], groups: GroupAnchor[], width: number, height: number): void {
  if (nodes.length === 0) return;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const node of nodes) {
    const x = node.x || 0;
    const y = node.y || 0;
    const outer = Math.max(14, node.radius + 12);
    minX = Math.min(minX, x - outer);
    minY = Math.min(minY, y - outer - 30);
    maxX = Math.max(maxX, x + outer);
    maxY = Math.max(maxY, y + outer + 34);
  }
  const padX = Math.max(54, Math.min(width, height) * 0.085);
  const padY = Math.max(62, Math.min(width, height) * 0.115);
  const graphW = Math.max(1, maxX - minX);
  const graphH = Math.max(1, maxY - minY);
  const scale = Math.min(
    (width - padX * 2) / graphW,
    (height - padY * 2) / graphH,
    1,
  );
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  for (const node of nodes) {
    node.x = ((node.x || 0) - cx) * scale + width / 2;
    node.y = ((node.y || 0) - cy) * scale + height / 2;
  }

  const grouped = new Map<number, { x: number; y: number; count: number }>();
  for (const node of nodes) {
    const entry = grouped.get(node.groupIndex) || { x: 0, y: 0, count: 0 };
    entry.x += node.x || 0;
    entry.y += node.y || 0;
    entry.count += 1;
    grouped.set(node.groupIndex, entry);
  }
  for (let index = 0; index < groups.length; index += 1) {
    const entry = grouped.get(index);
    if (!entry || entry.count === 0) continue;
    groups[index].x = entry.x / entry.count;
    groups[index].y = entry.y / entry.count;
  }
}

function isSmallGraph(graph: TopoGraph): boolean {
  return graph.agents.length <= SMALL_GRAPH_NODE_LIMIT && graph.edges.length <= SMALL_GRAPH_EDGE_LIMIT;
}

function buildLayout(graph: TopoGraph, width: number, height: number): Layout {
  const groupIndex = new Map<string, number>();
  graph.groups.forEach((group, index) => groupIndex.set(group, index));
  const cx = width / 2;
  const cy = height / 2;
  const groupCount = Math.max(1, graph.groups.length);
  const smallGraph = isSmallGraph(graph);
  const marginX = smallGraph ? Math.max(82, width * 0.12) : Math.max(155, width * 0.23);
  const marginY = smallGraph ? Math.max(76, height * 0.13) : Math.max(150, height * 0.3);
  const compactR = Math.max(72, Math.min(width, height) * 0.18);
  let explicitAnchors: Array<{ x: number; y: number }> = [];
  if (smallGraph && groupCount === 1) {
    explicitAnchors = [{ x: cx, y: cy }];
  } else if (smallGraph && groupCount === 2) {
    explicitAnchors = [{ x: cx - compactR, y: cy }, { x: cx + compactR, y: cy }];
  } else if (smallGraph && groupCount === 3) {
    explicitAnchors = [
      { x: cx, y: cy - compactR * 0.72 },
      { x: cx + compactR * 0.86, y: cy + compactR * 0.56 },
      { x: cx - compactR * 0.86, y: cy + compactR * 0.56 },
    ];
  } else if (smallGraph && groupCount === 4) {
    explicitAnchors = [
      { x: cx - compactR * 0.72, y: cy - compactR * 0.62 },
      { x: cx + compactR * 0.72, y: cy - compactR * 0.62 },
      { x: cx - compactR * 0.72, y: cy + compactR * 0.62 },
      { x: cx + compactR * 0.72, y: cy + compactR * 0.62 },
    ];
  } else if (groupCount === 1) {
    explicitAnchors = [{ x: cx, y: cy }];
  } else if (groupCount === 2) {
    explicitAnchors = [{ x: marginX, y: cy }, { x: width - marginX, y: cy }];
  } else if (groupCount === 3) {
    explicitAnchors = [
      { x: cx, y: marginY },
      { x: width - marginX, y: height - marginY },
      { x: marginX, y: height - marginY },
    ];
  } else if (groupCount === 4) {
    explicitAnchors = [
      { x: marginX, y: marginY },
      { x: width - marginX, y: marginY },
      { x: marginX, y: height - marginY },
      { x: width - marginX, y: height - marginY },
    ];
  }
  const rx = smallGraph ? compactR : Math.max(180, width * 0.34);
  const ry = smallGraph ? compactR * 0.78 : Math.max(130, height * 0.31);
  const groups = graph.groups.map((name, index) => {
    const fallbackT = (index / groupCount) * Math.PI * 2 - Math.PI / 2;
    const anchor = explicitAnchors[index] || {
      x: cx + Math.cos(fallbackT) * rx,
      y: cy + Math.sin(fallbackT) * ry,
    };
    return {
      name,
      x: anchor.x,
      y: anchor.y,
      count: graph.agents.filter((a) => a.group === name).length,
      colour: groupPalette(index),
    };
  });

  const groupedAgents = new Map<number, TopoAgent[]>();
  for (const agent of graph.agents) {
    const gi = groupIndex.get(agent.group) ?? 0;
    const entry = groupedAgents.get(gi) || [];
    entry.push(agent);
    groupedAgents.set(gi, entry);
  }
  for (const entry of groupedAgents.values()) {
    entry.sort((a, b) => {
      const da = graph.degree[a.id] || 0;
      const db = graph.degree[b.id] || 0;
      if (da !== db) return db - da;
      return a.label.localeCompare(b.label);
    });
  }

  const nodes: LayoutNode[] = [];
  if (smallGraph) {
    const ordered = graph.groups.flatMap((groupName) => {
      const gi = groupIndex.get(groupName) ?? 0;
      return (groupedAgents.get(gi) || []).map((agent) => ({ agent, groupIndex: gi }));
    });
    const count = Math.max(1, ordered.length);
    const ringX = Math.max(86, Math.min(width * 0.34, Math.min(width, height) * 0.34));
    const ringY = Math.max(76, Math.min(height * 0.31, Math.min(width, height) * 0.3));
    ordered.forEach(({ agent, groupIndex: gi }, index) => {
      const theta = count === 1
        ? -Math.PI / 2
        : -Math.PI / 2 + (index / count) * Math.PI * 2;
      const degree = graph.degree[agent.id] || 0;
      const emphasis = degree > 1 ? 1.03 : 1;
      nodes.push({
        id: agent.id,
        agent,
        groupIndex: gi,
        radius: nodeRadius(graph, agent.id),
        x: count === 1 ? cx : cx + Math.cos(theta) * ringX * emphasis,
        y: count === 1 ? cy : cy + Math.sin(theta) * ringY * emphasis,
      });
    });
    fitLayout(nodes, groups, width, height);

    const byId = new Map(nodes.map((node) => [node.id, node]));
    const edgeById = new Map<string, TopoGraph["edges"][number][]>();
    for (const edge of graph.edges) {
      if (!byId.has(edge.from) || !byId.has(edge.to)) continue;
      const from = edgeById.get(edge.from) || [];
      from.push(edge);
      edgeById.set(edge.from, from);
      const to = edgeById.get(edge.to) || [];
      to.push(edge);
      edgeById.set(edge.to, to);
    }
    return { nodes, byId, edgeById, groups, width, height };
  }

  for (const [gi, entry] of groupedAgents.entries()) {
    const anchor = groups[gi] || { x: cx, y: cy, count: entry.length };
    const count = Math.max(1, entry.length);
    const clusterRadius = smallGraph
      ? Math.min(Math.max(22, Math.sqrt(count) * 15), Math.min(width, height) * 0.08)
      : Math.min(
          Math.max(74, Math.sqrt(count) * 11.8),
          Math.min(width, height) * (groupCount <= 4 ? 0.175 : 0.13),
        );
    const twist = ((hash(anchor.name) % 1000) / 1000) * Math.PI * 2;
    entry.forEach((agent, index) => {
      const seed = hash(agent.id);
      const normalized = Math.sqrt((index + 0.45) / count);
      const theta = twist + index * GOLDEN_ANGLE + ((seed % 37) / 37) * 0.18;
      const radial = clusterRadius * normalized;
      const spiralBias = 0.86 + ((seed % 17) / 100);
      nodes.push({
        id: agent.id,
        agent,
        groupIndex: gi,
        radius: nodeRadius(graph, agent.id),
        x: anchor.x + Math.cos(theta) * radial * spiralBias,
        y: anchor.y + Math.sin(theta) * radial * (1.02 - ((seed % 11) / 120)),
      });
    });
  }
  fitLayout(nodes, groups, width, height);

  const byId = new Map(nodes.map((node) => [node.id, node]));
  const edgeById = new Map<string, TopoGraph["edges"][number][]>();
  for (const edge of graph.edges) {
    if (!byId.has(edge.from) || !byId.has(edge.to)) continue;
    const from = edgeById.get(edge.from) || [];
    from.push(edge);
    edgeById.set(edge.from, from);
    const to = edgeById.get(edge.to) || [];
    to.push(edge);
    edgeById.set(edge.to, to);
  }
  return { nodes, byId, edgeById, groups, width, height };
}

function screenToGraph(clientX: number, clientY: number, canvas: HTMLCanvasElement, viewport: Viewport): { x: number; y: number } {
  const rect = canvas.getBoundingClientRect();
  return {
    x: (clientX - rect.left - viewport.x) / viewport.scale,
    y: (clientY - rect.top - viewport.y) / viewport.scale,
  };
}

function applyViewport(ctx: CanvasRenderingContext2D, viewport: Viewport): void {
  ctx.translate(viewport.x, viewport.y);
  ctx.scale(viewport.scale, viewport.scale);
}

function drawCurve(ctx: CanvasRenderingContext2D, a: LayoutNode, b: LayoutNode, bend: number): void {
  const ax = a.x || 0;
  const ay = a.y || 0;
  const bx = b.x || 0;
  const by = b.y || 0;
  const mx = (ax + bx) / 2;
  const my = (ay + by) / 2;
  const dx = bx - ax;
  const dy = by - ay;
  const len = Math.hypot(dx, dy) || 1;
  const nx = -dy / len;
  const ny = dx / len;
  ctx.moveTo(ax, ay);
  ctx.quadraticCurveTo(mx + nx * bend, my + ny * bend, bx, by);
}

function curvePoint(a: LayoutNode, b: LayoutNode, bend: number, t: number): { x: number; y: number } {
  const ax = a.x || 0;
  const ay = a.y || 0;
  const bx = b.x || 0;
  const by = b.y || 0;
  const mx = (ax + bx) / 2;
  const my = (ay + by) / 2;
  const dx = bx - ax;
  const dy = by - ay;
  const len = Math.hypot(dx, dy) || 1;
  const cx = mx + (-dy / len) * bend;
  const cy = my + (dx / len) * bend;
  const mt = 1 - t;
  return {
    x: mt * mt * ax + 2 * mt * t * cx + t * t * bx,
    y: mt * mt * ay + 2 * mt * t * cy + t * t * by,
  };
}

function curveTangent(a: LayoutNode, b: LayoutNode, bend: number, t: number): { x: number; y: number } {
  const ax = a.x || 0;
  const ay = a.y || 0;
  const bx = b.x || 0;
  const by = b.y || 0;
  const mx = (ax + bx) / 2;
  const my = (ay + by) / 2;
  const dx = bx - ax;
  const dy = by - ay;
  const len = Math.hypot(dx, dy) || 1;
  const cx = mx + (-dy / len) * bend;
  const cy = my + (dx / len) * bend;
  return {
    x: 2 * (1 - t) * (cx - ax) + 2 * t * (bx - cx),
    y: 2 * (1 - t) * (cy - ay) + 2 * t * (by - cy),
  };
}

function pulseBend(a: LayoutNode, b: LayoutNode): number {
  const seed = hash(edgeKey(a.id, b.id));
  if (a.groupIndex === b.groupIndex) return (seed % 17) - 8;
  return (seed % 2 === 0 ? 1 : -1) * (22 + (seed % 34));
}

function drawArrowHead(
  ctx: CanvasRenderingContext2D,
  point: { x: number; y: number },
  tangent: { x: number; y: number },
  size: number,
): void {
  const angle = Math.atan2(tangent.y, tangent.x);
  ctx.moveTo(point.x, point.y);
  ctx.lineTo(
    point.x - Math.cos(angle - Math.PI / 6) * size,
    point.y - Math.sin(angle - Math.PI / 6) * size,
  );
  ctx.moveTo(point.x, point.y);
  ctx.lineTo(
    point.x - Math.cos(angle + Math.PI / 6) * size,
    point.y - Math.sin(angle + Math.PI / 6) * size,
  );
}

function drawBundledCurve(ctx: CanvasRenderingContext2D, a: LayoutNode, b: LayoutNode, groups: GroupAnchor[]): void {
  if (a.groupIndex === b.groupIndex) {
    const seed = hash(edgeKey(a.id, b.id));
    drawCurve(ctx, a, b, (seed % 15) - 7);
    return;
  }
  const ax = a.x || 0;
  const ay = a.y || 0;
  const bx = b.x || 0;
  const by = b.y || 0;
  const ga = groups[a.groupIndex];
  const gb = groups[b.groupIndex];
  const c1x = ga ? ga.x + (gb.x - ga.x) * 0.36 : (ax + bx) / 2;
  const c1y = ga ? ga.y + (gb.y - ga.y) * 0.36 : (ay + by) / 2;
  const c2x = gb ? gb.x + (ga.x - gb.x) * 0.36 : (ax + bx) / 2;
  const c2y = gb ? gb.y + (ga.y - gb.y) * 0.36 : (ay + by) / 2;
  ctx.moveTo(ax, ay);
  ctx.bezierCurveTo(c1x, c1y, c2x, c2y, bx, by);
}

function nodeRadius(graph: TopoGraph, id: string): number {
  if (isSmallGraph(graph)) {
    return Math.min(14, 5.6 + Math.sqrt(graph.degree[id] || 0) * 1.55);
  }
  return Math.min(8.5, 2.1 + Math.sqrt(graph.degree[id] || 0) * 0.48);
}

export function DenseGraphMap({
  graph,
  edgeMode = "all",
  activity = EMPTY_ACTIVITY,
}: DenseGraphMapProps): React.JSX.Element {
  const wrapRef = React.useRef<HTMLDivElement | null>(null);
  const canvasRef = React.useRef<HTMLCanvasElement | null>(null);
  const staticRef = React.useRef<HTMLCanvasElement | null>(null);
  const dragRef = React.useRef<{ x: number; y: number; viewport: Viewport } | null>(null);
  const [size, setSize] = React.useState({ width: 900, height: 420 });
  const [viewport, setViewport] = React.useState<Viewport>({ scale: 1, x: 0, y: 0 });
  const viewportRef = React.useRef(viewport);
  const [hoverId, setHoverId] = React.useState<string | null>(null);
  const roleIndex = React.useMemo(() => roleIndexFor(graph.roles), [graph.roles]);
  const layoutFingerprint = React.useMemo(
    () => [
      graph.agents.length,
      graph.groups.join("|"),
      graph.agents.map((a) => a.id).join("|"),
    ].join("::"),
    [graph],
  );
  const drawFingerprint = React.useMemo(
    () => `${layoutFingerprint}::edges=${graph.edges.length}::edgeMode=${edgeMode}`,
    [layoutFingerprint, graph.edges.length, edgeMode],
  );
  const layout = React.useMemo(
    () => buildLayout(graph, size.width, size.height),
    // `graph` is rebuilt every console poll. The dense layout
    // should only rerun when graph shape changes, not when an equivalent
    // REST payload is normalized into fresh object identities.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [layoutFingerprint, size.width, size.height],
  );

  React.useEffect(() => {
    viewportRef.current = viewport;
  }, [viewport]);

  React.useEffect(() => {
    dragRef.current = null;
    setHoverId(null);
    setViewport({ scale: 1, x: 0, y: 0 });
  }, [layoutFingerprint, size.width, size.height]);

  React.useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (!rect) return;
      setSize({
        width: Math.max(1, Math.floor(rect.width)),
        height: Math.max(1, Math.floor(rect.height)),
      });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const drawStatic = React.useCallback(() => {
    const host = wrapRef.current;
    if (!host) return null;
    const dpr = Math.max(1, Math.min(2, window.devicePixelRatio || 1));
    const off = document.createElement("canvas");
    off.width = Math.floor(layout.width * dpr);
    off.height = Math.floor(layout.height * dpr);
    const ctx = off.getContext("2d");
    if (!ctx) return null;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, layout.width, layout.height);

    const faint = cssVar(host, "--ink-faint", "rgba(148, 163, 184, 1)");
    const smallGraph = isSmallGraph(graph);
    const edgeAlpha = smallGraph ? 0.72 : graph.edges.length > 18000 ? 0.105 : graph.edges.length > 6000 ? 0.135 : 0.18;

    for (const group of layout.groups) {
      const grad = ctx.createRadialGradient(group.x, group.y, 10, group.x, group.y, Math.max(110, group.count * 2.1));
      grad.addColorStop(0, group.colour);
      grad.addColorStop(0.34, withAlpha(group.colour, 0.34));
      grad.addColorStop(1, "transparent");
      ctx.globalAlpha = 0.16;
      ctx.fillStyle = grad;
      ctx.beginPath();
      ctx.arc(group.x, group.y, Math.max(110, group.count * 2.1), 0, Math.PI * 2);
      ctx.fill();
    }

    if (smallGraph) {
      ctx.lineCap = "round";
      ctx.lineWidth = 1.35;
      for (const edge of graph.edges) {
        const a = layout.byId.get(edge.from);
        const b = layout.byId.get(edge.to);
        if (!a || !b) continue;
        const sameGroup = a.groupIndex === b.groupIndex;
        const seed = hash(edgeKey(a.id, b.id));
        ctx.strokeStyle = sameGroup ? (layout.groups[a.groupIndex]?.colour || faint) : faint;
        ctx.globalAlpha = sameGroup ? edgeAlpha * 0.92 : edgeAlpha * 0.68;
        ctx.beginPath();
        drawCurve(ctx, a, b, sameGroup ? (seed % 13) - 6 : (seed % 2 === 0 ? 1 : -1) * (8 + (seed % 14)));
        ctx.stroke();
      }
    } else if (edgeMode === "all") {
      ctx.lineWidth = graph.edges.length > 12000 ? 0.48 : 0.68;
      ctx.strokeStyle = faint;
      ctx.globalAlpha = edgeAlpha * 0.92;
      ctx.beginPath();
      for (const edge of graph.edges) {
        const a = layout.byId.get(edge.from);
        const b = layout.byId.get(edge.to);
        if (!a || !b) continue;
        if (a.agent.group === b.agent.group) continue;
        drawBundledCurve(ctx, a, b, layout.groups);
      }
      ctx.stroke();

      ctx.globalAlpha = Math.min(0.42, edgeAlpha * 1.8);
      for (let gi = 0; gi < layout.groups.length; gi += 1) {
        ctx.strokeStyle = layout.groups[gi]?.colour || faint;
        ctx.beginPath();
        for (const edge of graph.edges) {
          const a = layout.byId.get(edge.from);
          const b = layout.byId.get(edge.to);
          if (!a || !b || a.groupIndex !== gi || b.groupIndex !== gi) continue;
          drawBundledCurve(ctx, a, b, layout.groups);
        }
        ctx.stroke();
      }
    }
    for (const node of layout.nodes) {
      const r = nodeRadius(graph, node.id);
      const x = node.x || 0;
      const y = node.y || 0;
      ctx.globalAlpha = 0.86;
      ctx.fillStyle = layout.groups[node.groupIndex]?.colour || colourForRole(node.agent.role, roleIndex);
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.fill();
    }

    ctx.globalAlpha = 1;
    return off;
  // `graph` is intentionally keyed through `graphFingerprint` here; fresh
  // poll objects with the same shape should reuse the same static layer.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [drawFingerprint, layout, roleIndex]);

  React.useEffect(() => {
    staticRef.current = drawStatic();
  }, [drawStatic]);

  React.useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const canvas = canvasRef.current;
      if (!canvas) return;
      const current = viewportRef.current;
      const rect = canvas.getBoundingClientRect();
      const mx = event.clientX - rect.left;
      const my = event.clientY - rect.top;
      const nextScale = Math.max(0.55, Math.min(4, current.scale * (event.deltaY < 0 ? 1.12 : 0.88)));
      const gx = (mx - current.x) / current.scale;
      const gy = (my - current.y) / current.scale;
      setViewport({
        scale: nextScale,
        x: mx - gx * nextScale,
        y: my - gy * nextScale,
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const drawFrame = React.useCallback(() => {
    const canvas = canvasRef.current;
    const host = wrapRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx || !host) return;
    const dpr = Math.max(1, Math.min(2, window.devicePixelRatio || 1));
    const targetW = Math.floor(layout.width * dpr);
    const targetH = Math.floor(layout.height * dpr);
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, layout.width, layout.height);
    const staticCanvas = staticRef.current || drawStatic();
    ctx.save();
    applyViewport(ctx, viewport);
    if (staticCanvas) ctx.drawImage(staticCanvas, 0, 0, layout.width, layout.height);

    const focus = cssVar(host, "--focus", "rgb(90, 160, 255)");
    const ink = cssVar(host, "--ink", "rgb(235, 238, 245)");
    const selected = layout.byId.get(hoverId || "");
    const ok = cssVar(host, "--ok", "rgb(72, 200, 150)");
    const warn = cssVar(host, "--warn", "rgb(245, 178, 76)");
    const now = Date.now();

    if (activity.pulses.length > 0) {
      ctx.lineCap = "round";
      for (const pulse of activity.pulses) {
        const a = layout.byId.get(pulse.from);
        const b = layout.byId.get(pulse.to);
        if (!a || !b) continue;
        const age = Math.max(0, Math.min(1, (now - pulse.ts) / ACTIVITY_LIFE_MS));
        const alpha = Math.max(0, 1 - age);
        const bend = pulseBend(a, b);
        const t = 0.1 + age * 0.78;
        const bead = curvePoint(a, b, bend, t);
        const tangent = curveTangent(a, b, bend, t);
        ctx.globalAlpha = 0.24 + alpha * 0.5;
        ctx.strokeStyle = warn;
        ctx.lineWidth = Math.max(1.1, 2.2 / Math.sqrt(viewport.scale));
        ctx.beginPath();
        drawCurve(ctx, a, b, bend);
        ctx.stroke();
        ctx.globalAlpha = 0.75 + alpha * 0.25;
        ctx.fillStyle = warn;
        ctx.shadowColor = warn;
        ctx.shadowBlur = 14 / Math.sqrt(viewport.scale);
        ctx.beginPath();
        ctx.arc(bead.x, bead.y, Math.max(3.2, 4.8 / Math.sqrt(viewport.scale)), 0, Math.PI * 2);
        ctx.fill();
        ctx.shadowBlur = 0;
        ctx.globalAlpha = 0.72 + alpha * 0.22;
        ctx.beginPath();
        drawArrowHead(ctx, bead, tangent, Math.max(5, 7 / Math.sqrt(viewport.scale)));
        ctx.stroke();
      }
    }

    for (const node of layout.nodes) {
      const activeTs = activity.active[node.id] || activity.calls[node.id] || 0;
      const isBusy = activity.busy[node.id] || node.agent.responsePhase != null;
      if (!activeTs && !isBusy) continue;
      const age = activeTs ? Math.max(0, Math.min(1, (now - activeTs) / ACTIVITY_LIFE_MS)) : 0;
      const alpha = activeTs ? Math.max(0.15, 1 - age) : 0.34;
      const x = node.x || 0;
      const y = node.y || 0;
      const r = nodeRadius(graph, node.id);
      if (activeTs) {
        ctx.globalAlpha = 0.22 * alpha;
        ctx.fillStyle = activity.calls[node.id] ? warn : ok;
        ctx.beginPath();
        ctx.arc(x, y, r + 9 + (1 - alpha) * 8, 0, Math.PI * 2);
        ctx.fill();
      }
      if (isBusy) {
        ctx.globalAlpha = 0.96;
        ctx.strokeStyle = warn;
        ctx.lineWidth = Math.max(1.4, 2.2 / Math.sqrt(viewport.scale));
        ctx.setLineDash([Math.max(3, 4 / viewport.scale), Math.max(3, 5 / viewport.scale)]);
        ctx.beginPath();
        ctx.arc(x, y, r + 5.8, 0, Math.PI * 2);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = 0.7;
        ctx.strokeStyle = ok;
        ctx.lineWidth = Math.max(0.8, 1.1 / Math.sqrt(viewport.scale));
        ctx.beginPath();
        ctx.arc(x, y, r + 9.2, 0, Math.PI * 2);
        ctx.stroke();
      }
    }

    if (selected) {
      const selectedEdges = layout.edgeById.get(selected.id) || [];
      const peerSet = new Set<string>();
      for (const edge of selectedEdges) peerSet.add(edge.from === selected.id ? edge.to : edge.from);
      ctx.globalAlpha = edgeMode === "all" ? 0.52 : 0.78;
      ctx.lineWidth = Math.max(0.72, 1.08 / Math.sqrt(viewport.scale));
      ctx.strokeStyle = focus;
      ctx.beginPath();
      for (const edge of selectedEdges) {
        const peerId = edge.from === selected.id ? edge.to : edge.from;
        const peer = layout.byId.get(peerId);
        if (!peer) continue;
        const seed = hash(edgeKey(selected.id, peer.id));
        const bend = selected.groupIndex === peer.groupIndex
          ? ((seed % 15) - 7)
          : ((seed % 2 === 0 ? 1 : -1) * (18 + (seed % 26)));
        drawCurve(ctx, selected, peer, bend);
      }
      ctx.stroke();
      for (const peerId of peerSet) {
        const peer = layout.byId.get(peerId);
        if (!peer) continue;
        ctx.globalAlpha = 0.84;
        ctx.fillStyle = layout.groups[peer.groupIndex]?.colour || focus;
        ctx.strokeStyle = focus;
        ctx.lineWidth = Math.max(0.8, 1.1 / Math.sqrt(viewport.scale));
        ctx.beginPath();
        ctx.arc(peer.x || 0, peer.y || 0, Math.max(2.8, nodeRadius(graph, peer.id) + 0.9), 0, Math.PI * 2);
        ctx.fill();
        ctx.stroke();
      }
    }

    if (selected) {
      const r = nodeRadius(graph, selected.id) + 6.2 / Math.sqrt(viewport.scale);
      ctx.globalAlpha = 1;
      ctx.shadowColor = focus;
      ctx.shadowBlur = 18 / Math.sqrt(viewport.scale);
      ctx.fillStyle = layout.groups[selected.groupIndex]?.colour || ink;
      ctx.strokeStyle = focus;
      ctx.lineWidth = 3.1 / Math.sqrt(viewport.scale);
      ctx.beginPath();
      ctx.arc(selected.x || 0, selected.y || 0, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      ctx.shadowBlur = 0;
    }
    ctx.restore();
  // `graph` is keyed through drawFingerprint so equivalent polling payloads
  // do not restart expensive work.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activity, drawStatic, drawFingerprint, hoverId, layout, viewport]);

  React.useEffect(() => {
    drawFrame();
  }, [drawFrame]);

  const nearestId = React.useCallback((clientX: number, clientY: number): string | null => {
    const canvas = canvasRef.current;
    if (!canvas) return null;
    const pos = screenToGraph(clientX, clientY, canvas, viewport);
    let best: { id: string; d2: number } | null = null;
    const threshold = Math.max(12, 18 / viewport.scale);
    for (const node of layout.nodes) {
      const dx = (node.x || 0) - pos.x;
      const dy = (node.y || 0) - pos.y;
      const d2 = dx * dx + dy * dy;
      if (d2 > threshold * threshold) continue;
      if (!best || d2 < best.d2) best = { id: node.id, d2 };
    }
    return best?.id || null;
  }, [layout, viewport]);

  const hover = hoverId ? graph.byId.get(hoverId) : null;
  const hoverBusy = hoverId ? activity.busy[hoverId] || layout.byId.get(hoverId)?.agent.responsePhase != null : false;
  const hoverCalls = hoverId ? activity.pulses.filter((pulse) => pulse.from === hoverId || pulse.to === hoverId).length : 0;
  const showNodeLabels = graph.agents.length <= 12;
  const showGroupLabels = !showNodeLabels;
  const labelCenter = { x: layout.width / 2, y: layout.height / 2 };

  return (
    <div
      ref={wrapRef}
      className="topo-dense"
      data-testid="topology-dense-map"
      onPointerDown={(event) => {
        dragRef.current = { x: event.clientX, y: event.clientY, viewport };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        if (dragRef.current) {
          const dx = event.clientX - dragRef.current.x;
          const dy = event.clientY - dragRef.current.y;
          setViewport({
            ...dragRef.current.viewport,
            x: dragRef.current.viewport.x + dx,
            y: dragRef.current.viewport.y + dy,
          });
          return;
        }
        setHoverId(nearestId(event.clientX, event.clientY));
      }}
      onPointerUp={(event) => {
        dragRef.current = null;
        setHoverId(nearestId(event.clientX, event.clientY));
        event.currentTarget.releasePointerCapture(event.pointerId);
      }}
      onPointerCancel={(event) => {
        dragRef.current = null;
        event.currentTarget.releasePointerCapture(event.pointerId);
      }}
      onPointerLeave={() => setHoverId(null)}
    >
      <canvas ref={canvasRef} className="topo-dense__canvas" aria-label="Dense topology graph" />
      <div className="topo-dense__labels" aria-hidden="true">
        {showGroupLabels && layout.groups.map((g) => (
          <div
            key={g.name}
            className="topo-dense__group-label"
            style={{
              left: `${g.x * viewport.scale + viewport.x}px`,
              top: `${g.y * viewport.scale + viewport.y + 18}px`,
              borderColor: g.colour,
            }}
          >
            <strong>{g.name}</strong>
            <span>{g.count} agents</span>
          </div>
        ))}
        {showNodeLabels && layout.nodes.map((node) => {
          const x = node.x || 0;
          const y = node.y || 0;
          const angle = Math.atan2(y - labelCenter.y, x - labelCenter.x);
          const offset = node.radius + 34;
          return (
            <div
              key={node.id}
              className="topo-dense__node-label"
              style={{
                left: `${(x + Math.cos(angle) * offset) * viewport.scale + viewport.x}px`,
                top: `${(y + Math.sin(angle) * offset) * viewport.scale + viewport.y}px`,
                borderColor: layout.groups[node.groupIndex]?.colour || colourForRole(node.agent.role, roleIndex),
              }}
            >
              {node.agent.label}
            </div>
          );
        })}
      </div>
      {hover && (
        <>
        <div
          className="topo-dense__hover-label"
          style={{
            left: `${((layout.byId.get(hover.id)?.x || 0) * viewport.scale) + viewport.x}px`,
            top: `${((layout.byId.get(hover.id)?.y || 0) * viewport.scale) + viewport.y}px`,
          }}
        >
          <strong>{hover.label}</strong>
          <span>{hoverBusy ? "working" : hover.role}</span>
        </div>
        <div className="topo-dense__inspector">
          <strong>{hover.label}</strong>
          <span>{hover.group}</span>
          <span>{layout.edgeById.get(hover.id)?.length || 0} peers</span>
          {hoverBusy && <span>working</span>}
          {hoverCalls > 0 && <span>{hoverCalls} live calls</span>}
        </div>
        </>
      )}
    </div>
  );
}
