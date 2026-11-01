// GraphView (design spec A3.9): the one graph engine Topology, WorkGraph and
// the agent ego graph draw with. It owns layout, the zoom and pan, the node
// and edge grammar, labels, live motion (particles, flashes, memory arcs),
// the legend, minimap and zoom controls, keyboard navigation and the live
// region; panels give it nodes and edges and listen for selection.

import React from "react";

import { computeGraphLayout, effectiveLayoutKind, toLayoutEdges, toLayoutNodes } from "../layout/compute";
import { forceLayoutFrom, runForce, seedForce } from "../layout/force";
import { createTextMeasure, estimateLabelWidth, isCardKind, markGeometry } from "../layout/geometry";
import type { TextMeasure } from "../layout/geometry";
import type { GraphLayout, LayoutPoint } from "../layout/types";
import { useReducedMotion, useVizTokens } from "../tokens";
import { drawCanvasScene, nodeAtPoint } from "./canvas-renderer";
import { GraphTooltip, Legend, Minimap, ZoomControls } from "./controls";
import { edgeLength, edgeNormal, edgePath, edgeShape, angleAt, pointAt } from "./edge-geometry";
import type { EdgeShape } from "./edge-geometry";
import { isNavigationKey, nodeToward, readingOrder } from "./keyboard";
import { LABEL_GAP, LABEL_HEIGHT, fitText, labelAt, outwardSides, placeLabels } from "./labels";
import type { LabelInput, LabelPlacement, LabelSide } from "./labels";
import { buildGraphModel, highlightFor } from "./model";
import type { GraphModel, Highlight } from "./model";
import { PARTICLES, admitMessages, easeTravel, messageKey, travelMs, trimSeen } from "./particles";
import type { Flight } from "./particles";
import { panelOfKey, readMinimapHidden, readPositionCache, writeMinimapHidden, writePositionCache } from "./position-cache";
import { ClustersLayer, EdgeBadge, EdgeLabel, EdgeView, FrameView, LanesLayer, NodeLabel, NodeView, ParticlePool } from "./svg-layers";
import type { EdgeGeometry, GoalHeaderLayout } from "./svg-layers";
import { layoutGoalHeader } from "./goal-header";
import type {
  GraphTestIds,
  GraphViewHandle,
  GraphViewProps,
  VizEdge,
  VizFrame,
  VizLane,
  VizNode,
  VizNodeBox,
  VizRelation,
  VizViewport,
  VizZoomAction,
} from "./types";
import { edgeTooltipLines, nodeAccessibleName, nodeTooltipLines } from "./words";
import {
  IDENTITY_VIEWPORT,
  ZOOM,
  centreOn,
  fitViewport,
  needsMinimap,
  revealViewport,
  viewportTransform,
  wheelPixels,
  wheelZoomFactor,
  zoomAt,
  zoomTo,
} from "./zoom-pan";
import type { FitLimits } from "./zoom-pan";

/** Above these the canvas renderer draws (A3.9). */
export const SVG_LIMITS = { nodes: 300, edges: 800 } as const;
const MOVE_MS = 320;
/** Lanes never fit below this: further out the marks would crowd their labels. */
const LANE_MIN_SCALE = 0.6;
/** Below this frame width the legend starts folded and stacks its rows under its title. */
const LEGEND_NARROW = 560;
/** Layered frames sit this far in from the view's edges (px). */
const CARD_FIT_PAD = 24;
/** Item cards never fit below this: their titles stay at 10 px or more. */
const CARD_MIN_SCALE = 0.85;
const EXIT_MS = 200;
const LABEL_SETTLE_MS = 120;

function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

function now(): number {
  return typeof performance !== "undefined" ? performance.now() : Date.now();
}

const subscribeNothing = () => () => undefined;

/**
 * False while React renders on a server (renderToString, renderToStaticMarkup),
 * where no layout effect ever measures the frame; true in a browser or jsdom.
 */
function useClientRender(): boolean {
  return React.useSyncExternalStore(subscribeNothing, () => true, () => false);
}

/** The element's size; `measured` turns true once it has been read (zero in jsdom). */
function useElementSize(ref: React.RefObject<HTMLElement | null>): { width: number; height: number; measured: boolean } {
  const [size, setSize] = React.useState({ width: 0, height: 0, measured: false });
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return undefined;
    const apply = (width: number, height: number) => {
      const next = { width: Math.round(width), height: Math.round(height), measured: true };
      setSize((previous) => (previous.measured && previous.width === next.width && previous.height === next.height ? previous : next));
    };
    const rect = element.getBoundingClientRect();
    apply(rect.width, rect.height);
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver((entries) => {
      const box = entries[entries.length - 1]?.contentRect;
      if (box) apply(box.width, box.height);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return size;
}

interface Measures {
  label: TextMeasure;
  title: TextMeasure;
  meta: TextMeasure;
  lane: TextMeasure;
  edge: TextMeasure;
  /** Goal titles: 13 px / 600 (--_cc-fs-card). */
  card: TextMeasure;
}

const ESTIMATES: Measures = {
  label: estimateLabelWidth,
  title: (text) => Math.ceil(text.length * 12.5 * 0.55),
  meta: (text) => Math.ceil(text.length * 11 * 0.53),
  lane: (text) => Math.ceil(text.length * 7.4),
  edge: (text) => Math.ceil(text.length * 10 * 0.56),
  card: (text) => Math.ceil(text.length * 13 * 0.57),
};

/** Text measurers in the scope's own font, re-made once web fonts load. */
function useMeasures(ref: React.RefObject<HTMLElement | null>): Measures {
  const [family, setFamily] = React.useState<string | null>(null);
  const [epoch, setEpoch] = React.useState(0);
  // Read once, and again when a theme or variant changes the face (terminal
  // is monospace): a style read on every render forces a recalculation.
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!element || typeof getComputedStyle !== "function") return undefined;
    const read = () => {
      const next = getComputedStyle(element).fontFamily || null;
      setFamily((previous) => (previous === next ? previous : next));
    };
    read();
    if (typeof MutationObserver === "undefined") return undefined;
    const observer = new MutationObserver(read);
    observer.observe(element.ownerDocument.documentElement, {
      attributes: true,
      subtree: true,
      attributeFilter: ["data-cc-theme", "data-cc-variant", "class"],
    });
    return () => observer.disconnect();
  }, [ref]);
  React.useEffect(() => {
    // Widths change once web fonts arrive; nothing to redo when they already have.
    if (typeof document === "undefined" || !document.fonts?.ready || document.fonts.status === "loaded") return undefined;
    let live = true;
    void document.fonts.ready.then(() => {
      if (live) setEpoch((value) => value + 1);
    });
    return () => {
      live = false;
    };
  }, []);
  return React.useMemo(() => {
    if (!family) return ESTIMATES;
    const label = createTextMeasure(`500 11px ${family}`);
    if (label === estimateLabelWidth) return ESTIMATES;
    const laneBase = createTextMeasure(`600 10px ${family}`);
    return {
      label,
      title: createTextMeasure(`500 12.5px ${family}`),
      meta: createTextMeasure(`400 11px ${family}`),
      // Lane and frame labels track 0.1em: one px per character at 10 px.
      lane: (text: string) => laneBase(text) + text.length,
      edge: createTextMeasure(`500 10px ${family}`),
      card: createTextMeasure(`600 13px ${family}`),
    };
    // epoch: widths change once the web fonts arrive.
  }, [family, epoch]);
}

/** Whether the view is on screen and its page visible (the animation loop pauses otherwise). */
function useOnScreen(ref: React.RefObject<HTMLElement | null>): boolean {
  const [intersecting, setIntersecting] = React.useState(true);
  const [pageVisible, setPageVisible] = React.useState(() => typeof document === "undefined" || !document.hidden);
  React.useEffect(() => {
    const element = ref.current;
    if (!element || typeof IntersectionObserver === "undefined") return undefined;
    const observer = new IntersectionObserver((entries) => {
      const entry = entries[entries.length - 1];
      if (entry) setIntersecting(entry.isIntersecting);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  React.useEffect(() => {
    if (typeof document === "undefined") return undefined;
    const update = () => setPageVisible(!document.hidden);
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);
  return intersecting && pageVisible;
}

/** A map whose entries expire: one timer at the earliest expiry. */
function useExpiring<V>(): [Map<string, V & { until: number }>, (key: string, value: V, ms: number) => void, () => void] {
  const [entries, setEntries] = React.useState<Map<string, V & { until: number }>>(() => new Map());
  React.useEffect(() => {
    if (entries.size === 0) return undefined;
    let earliest = Infinity;
    for (const entry of entries.values()) earliest = Math.min(earliest, entry.until);
    const timer = setTimeout(() => {
      const at = now();
      setEntries((current) => {
        const next = new Map([...current].filter(([, entry]) => entry.until > at + 1));
        return next.size === current.size ? current : next;
      });
    }, Math.max(0, earliest - now()) + 4);
    return () => clearTimeout(timer);
  }, [entries]);
  const add = React.useCallback((key: string, value: V, ms: number) => {
    setEntries((current) => new Map(current).set(key, { ...value, until: now() + ms }));
  }, []);
  const clear = React.useCallback(() => setEntries((current) => (current.size === 0 ? current : new Map())), []);
  return [entries, add, clear];
}

function resolveTestId<T>(value: string | ((item: T) => string) | undefined, item: T, fallback: string | undefined): string | undefined {
  if (typeof value === "function") return value(item);
  if (typeof value === "string") return value;
  return fallback;
}

function boxesEqual(a: ReadonlyMap<string, VizNodeBox>, b: ReadonlyMap<string, VizNodeBox>): boolean {
  if (a.size !== b.size) return false;
  for (const [id, box] of a) {
    const other = b.get(id);
    if (!other || Math.abs(other.x - box.x) > 0.5 || Math.abs(other.y - box.y) > 0.5 || other.w !== box.w || other.h !== box.h) return false;
  }
  return true;
}

const easeOut = (t: number) => 1 - (1 - t) ** 3;

/** Where an edge's ends sit and how its chevron and midpoint marks are placed. */
function edgeGeometryFor(edge: VizEdge, from: VizNodeBox, to: VizNodeBox, layout: GraphLayout, offset: number, targetNode: VizNode | undefined, dots: ReadonlyMap<string, number>): { geometry: EdgeGeometry; shape: EdgeShape } {
  const shape = edgeShape(from, to, layout.routes.get(edge.id));
  const normal = shape.straight ? edgeNormal(shape.points[0], shape.points[3]) : { x: 0, y: 0 };
  // The chevron backs off a mark when the edge ends at the mark's centre
  // (straight edges, same-row arcs); routes that end on a box side need none.
  let backoff = 0;
  const end = shape.points[3];
  if (targetNode && Math.abs(end.x - to.x) < 0.5 && Math.abs(end.y - to.y) < 0.5) {
    const dot = dots.get(targetNode.id);
    backoff = dot !== undefined ? 0 : isCardKind(targetNode.kind) ? 0 : markGeometry(targetNode.kind, targetNode.size).outer + 2.5;
  }
  const geometry: EdgeGeometry = {
    d: edgePath(shape),
    end: shape.points[3],
    endAngle: angleAt(shape, 1),
    mid: pointAt(shape, 0.5),
    midAngle: angleAt(shape, 0.5),
    backoff,
    ox: normal.x * offset,
    oy: normal.y * offset,
    length: edgeLength(shape),
  };
  return { geometry, shape };
}

interface Hover {
  node?: string;
  edge?: string;
  /** Screen point of an edge hover. */
  x?: number;
  y?: number;
}

export const GraphView = React.forwardRef<GraphViewHandle, GraphViewProps>(function GraphView(props, ref) {
  const {
    nodes,
    edges,
    layout: layoutKind,
    lanes,
    frames,
    center,
    density = "comfortable",
    hoverHighlight = "neighbours",
    legend = "auto",
    minimap = "auto",
    controls = true,
    wheelZoom = "always",
    fitPadding,
    renderer = "auto",
    labels = "auto",
    edgeLabels = "auto",
    showRetired = false,
    cacheKey,
    classNames,
    testIds = {} as GraphTestIds,
    testIdPrefix,
  } = props;
  const mountedAt = React.useRef(now());
  const rootRef = React.useRef<HTMLDivElement | null>(null);
  const svgRef = React.useRef<SVGSVGElement | null>(null);
  const canvasRef = React.useRef<HTMLCanvasElement | null>(null);
  const particleRef = React.useRef<SVGGElement | null>(null);
  const size = useElementSize(rootRef);
  // A browser draws after the frame's first measure; a server render, which
  // never measures, draws at the default size.
  const clientRender = useClientRender();
  const drawable = size.measured || !clientRender;
  const reduced = useReducedMotion(rootRef);
  const measures = useMeasures(rootRef);
  const onScreen = useOnScreen(rootRef);

  // Selection, legend toggles and focus mode: controlled when the panel passes them.
  const [ownSelection, setOwnSelection] = React.useState<readonly string[]>([]);
  const selection = props.selection ?? ownSelection;
  const selectedSet = React.useMemo(() => new Set(selection), [selection]);
  const onSelectRef = React.useRef(props.onSelect);
  onSelectRef.current = props.onSelect;
  const controlledSelection = props.selection !== undefined;
  const select = React.useCallback((ids: string[]) => {
    if (!controlledSelection) setOwnSelection(ids);
    onSelectRef.current?.(ids);
  }, [controlledSelection]);
  const [ownHidden, setOwnHidden] = React.useState<readonly VizRelation[]>([]);
  const hiddenList = props.hiddenRelations ?? ownHidden;
  const hiddenSet = React.useMemo(() => new Set(hiddenList), [hiddenList]);
  const [focusMode, setFocusMode] = React.useState(false);
  const focusTarget = focusMode ? selection[0] ?? null : null;
  // The legend starts folded where asked, and in a narrow frame (until the user unfolds it).
  const [legendChoice, setLegendChoice] = React.useState<boolean | null>(null);
  const narrowFrame = size.width > 0 && size.width < LEGEND_NARROW;
  const legendCollapsed = legendChoice ?? (legend === "collapsed" || narrowFrame);
  const panel = panelOfKey(cacheKey) || testIdPrefix || "";
  const [minimapHidden, setMinimapHidden] = React.useState(() => readMinimapHidden(panel));

  const model = React.useMemo(
    () => buildGraphModel({ nodes, edges, showRetired, hiddenRelations: hiddenSet, focus: focusTarget }),
    [nodes, edges, showRetired, hiddenSet, focusTarget],
  );
  const modelRef = React.useRef(model);
  modelRef.current = model;
  const kind = effectiveLayoutKind(layoutKind, model.nodes.length);
  const cards = kind === "layered";
  const useCanvas = renderer === "canvas" || (renderer !== "svg" && (model.nodes.length > SVG_LIMITS.nodes || model.edges.length > SVG_LIMITS.edges));
  const tokens = useVizTokens(rootRef, useCanvas);

  // Layout. Lanes and clusters spread across the frame; force settles in idle chunks.
  // Layered pictures sit closer to the frame's edge (the A3.9 WorkGraph frames, 24 px in).
  const fitPad = fitPadding !== undefined && fitPadding >= 0 ? fitPadding : cards ? CARD_FIT_PAD : ZOOM.pad;
  const layoutWidth = size.width > 0 ? Math.max(320, size.width - 2 * fitPad) : 960;
  const layoutHeight = size.height > 0 ? Math.max(200, size.height - 2 * ZOOM.pad) : 560;
  // The bottom-left chrome (legend and zoom controls) stays clear of the fitted picture.
  const chromeRef = React.useRef<HTMLDivElement | null>(null);
  const [chromeHeight, setChromeHeight] = React.useState(0);
  const chromeKey = `${legend}:${controls}`;
  React.useLayoutEffect(() => {
    const element = chromeRef.current;
    if (!element) {
      setChromeHeight(0);
      return undefined;
    }
    const apply = () => setChromeHeight(Math.round(element.getBoundingClientRect().height));
    apply();
    if (typeof ResizeObserver === "undefined") return undefined;
    // The observer follows the legend opening, folding and wrapping.
    const observer = new ResizeObserver(apply);
    observer.observe(element);
    return () => observer.disconnect();
  }, [chromeKey]);
  const fitBottom = chromeHeight > 0 ? Math.max(fitPad, chromeHeight + 20) : fitPad;
  const cachedRef = React.useRef<Map<string, LayoutPoint> | null>(null);
  if (cachedRef.current === null) cachedRef.current = cacheKey ? readPositionCache(cacheKey) : new Map();
  const [settled, setSettled] = React.useState<Map<string, LayoutPoint> | null>(null);
  const nodeKey = React.useMemo(() => model.nodes.map((node) => node.id).join("\u0000"), [model.nodes]);
  const layout = React.useMemo<GraphLayout>(() => {
    if (kind === "force") {
      const known = settled ?? cachedRef.current ?? new Map<string, LayoutPoint>();
      const layoutNodes = toLayoutNodes(model.nodes, density, measures.label);
      if (model.nodes.length > 0 && model.nodes.every((node) => known.has(node.id))) return forceLayoutFrom(layoutNodes, known);
      return computeGraphLayout({ nodes: model.nodes, edges: model.edges, layout: "force", lanes, density, measure: measures.label, positions: known });
    }
    const input = {
      nodes: model.nodes,
      edges: model.edges,
      layout: kind,
      lanes,
      frames,
      center,
      density,
      width: layoutWidth,
      // Lanes share the height the fit leaves them (clear of the chrome).
      height: kind === "lanes" && size.height > 0 ? Math.max(120, size.height - ZOOM.pad - fitBottom) : layoutHeight,
      measure: measures.label,
    };
    const first = computeGraphLayout(input);
    // Lanes spread across the width at the scale the fit will use: a tall
    // picture fitted below 1 is laid out wider by as much, so it still fills
    // the frame from side to side.
    if (kind === "lanes" && first.bounds.h > 0 && size.height > 0) {
      const fitScale = Math.max(LANE_MIN_SCALE, Math.min(1, (size.height - ZOOM.pad - fitBottom) / first.bounds.h));
      if (fitScale < 0.98) return computeGraphLayout({ ...input, width: layoutWidth / fitScale });
    }
    // Frames, likewise, fill the width at the scale that fits their height.
    if (kind === "layered" && first.frames.length > 0 && first.bounds.h > 0 && size.height > 0) {
      const fitScale = Math.max(CARD_MIN_SCALE, Math.min(1, (size.height - fitPad - fitBottom) / first.bounds.h));
      if (fitScale < 0.98) return computeGraphLayout({ ...input, width: layoutWidth / fitScale });
    }
    return first;
  }, [kind, model, lanes, frames, center, density, layoutWidth, layoutHeight, measures.label, settled, size.height, fitPad, fitBottom]);
  const layoutRef = React.useRef(layout);
  layoutRef.current = layout;

  React.useEffect(() => {
    if (kind !== "force" || model.nodes.length === 0) return undefined;
    const known = settled ?? cachedRef.current ?? new Map<string, LayoutPoint>();
    const pinned = new Set(model.nodes.filter((node) => known.has(node.id)).map((node) => node.id));
    if (pinned.size === model.nodes.length) {
      if (!settled) setSettled(new Map(known));
      return undefined;
    }
    const layoutNodes = toLayoutNodes(model.nodes, density, measures.label);
    const { anchors } = seedForce(layoutNodes, (lanes ?? []).map((lane) => lane.id), known);
    const run = runForce(layoutNodes, toLayoutEdges(model.edges), layoutRef.current.nodes, anchors, { pinned });
    void run.done.then((positions) => {
      if (!positions) return;
      const merged = new Map(known);
      for (const [id, point] of positions) merged.set(id, point);
      setSettled(merged);
      if (cacheKey) writePositionCache(cacheKey, merged);
    });
    return () => run.cancel();
    // A new run only for a new node set (or edges); `settled` updates do not restart it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, nodeKey, model.edges, density]);

  // Displayed positions: tween 320 ms to a new layout; jump on resize and under reduced motion.
  const [boxes, setBoxes] = React.useState<ReadonlyMap<string, VizNodeBox>>(layout.nodes);
  const boxesRef = React.useRef(boxes);
  boxesRef.current = boxes;
  const dataKeyRef = React.useRef<string | null>(null);
  const dataKey = `${kind}\u0001${nodeKey}\u0001${density}`;
  React.useLayoutEffect(() => {
    const target = layout.nodes;
    const previous = boxesRef.current;
    const resizedOnly = dataKeyRef.current === dataKey && kind !== "force";
    const firstLayout = dataKeyRef.current === null;
    dataKeyRef.current = dataKey;
    if (boxesEqual(previous, target)) return undefined;
    if (reduced || resizedOnly || firstLayout || previous.size === 0 || typeof requestAnimationFrame !== "function") {
      setBoxes(target);
      return undefined;
    }
    const at = (k: number) => {
      const next = new Map<string, VizNodeBox>();
      for (const [id, box] of target) {
        const from = previous.get(id);
        next.set(id, from ? { ...box, x: from.x + (box.x - from.x) * k, y: from.y + (box.y - from.y) * k } : box);
      }
      return next;
    };
    // New nodes appear at once, in their place; the others move from where they were.
    setBoxes(at(0));
    const start = now();
    let frame = 0;
    const step = () => {
      const t = Math.min(1, (now() - start) / MOVE_MS);
      if (t >= 1) {
        setBoxes(target);
        return;
      }
      setBoxes(at(easeOut(t)));
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => {
      cancelAnimationFrame(frame);
      setBoxes(target);
    };
  }, [layout, dataKey, kind, reduced]);

  // Nodes that leave fade out for 200 ms; nodes that arrive later fade in.
  const previousNodesRef = React.useRef<Map<string, VizNode> | null>(null);
  const [exiting, setExiting] = React.useState<Map<string, { node: VizNode; box: VizNodeBox }>>(() => new Map());
  const [entering, setEntering] = React.useState<ReadonlySet<string>>(() => new Set());
  React.useLayoutEffect(() => {
    const previous = previousNodesRef.current;
    previousNodesRef.current = model.byId;
    if (!previous) return undefined;
    const added = new Set<string>();
    for (const id of model.byId.keys()) if (!previous.has(id)) added.add(id);
    setEntering(added);
    if (reduced) return undefined;
    const gone = new Map<string, { node: VizNode; box: VizNodeBox }>();
    for (const [id, node] of previous) {
      const box = boxesRef.current.get(id);
      if (!model.byId.has(id) && box) gone.set(id, { node, box });
    }
    if (gone.size === 0) return undefined;
    setExiting(gone);
    const timer = setTimeout(() => setExiting(new Map()), EXIT_MS);
    return () => clearTimeout(timer);
  }, [model.byId, reduced]);

  // Viewport: fit until the person moves the view; a refresh never resets their pan and zoom.
  const [viewport, setViewportState] = React.useState<VizViewport>(IDENTITY_VIEWPORT);
  const viewportRef = React.useRef(viewport);
  viewportRef.current = viewport;
  const userMovedRef = React.useRef(false);
  const onViewportChangeRef = React.useRef(props.onViewportChange);
  onViewportChangeRef.current = props.onViewportChange;
  const setViewport = React.useCallback((next: VizViewport | ((current: VizViewport) => VizViewport), byUser: boolean) => {
    setViewportState((current) => {
      const value = typeof next === "function" ? next(current) : next;
      if (value === current) return current;
      if (byUser) userMovedRef.current = true;
      onViewportChangeRef.current?.(value);
      return value;
    });
  }, []);
  const limits: FitLimits = cards
    ? { minScale: CARD_MIN_SCALE, maxScale: 1, alignTop: true }
    : kind === "lanes"
      ? { minScale: LANE_MIN_SCALE, maxScale: 1 }
      : kind === "clusters"
        ? { minScale: 0.3, maxScale: 1.6 }
        : { minScale: 0.45, maxScale: 1.5 };
  // What the fit must show: the layout, and every mark's label and helper around it.
  const fitBounds = React.useMemo(() => {
    let { x: x0, y: y0 } = layout.bounds;
    let x1 = layout.bounds.x + layout.bounds.w;
    let y1 = layout.bounds.y + layout.bounds.h;
    for (const node of model.nodes) {
      const box = layout.nodes.get(node.id);
      if (!box || isCardKind(node.kind)) continue;
      if (kind === "layered") {
        // Layered marks carry their label on the right.
        const r = markGeometry(node.kind, node.size).outer;
        const half = Math.max(r, LABEL_HEIGHT / 2) + 2;
        x0 = Math.min(x0, box.x - r - 2);
        x1 = Math.max(x1, box.x + r + LABEL_GAP + measures.label(node.label) + 2);
        y0 = Math.min(y0, box.y - half);
        y1 = Math.max(y1, box.y + half);
        continue;
      }
      const half = Math.max(box.w / 2, measures.label(node.label) / 2 + 4);
      x0 = Math.min(x0, box.x - half);
      x1 = Math.max(x1, box.x + half);
      y0 = Math.min(y0, box.y - box.h / 2 - 4);
      y1 = Math.max(y1, box.y + box.h / 2 + 22);
    }
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  }, [layout, model.nodes, measures.label, kind]);
  const fit = React.useMemo(
    () => fitViewport(fitBounds, size.width, size.height, limits, { top: fitPad, right: fitPad, bottom: fitBottom, left: fitPad }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [fitBounds.x, fitBounds.y, fitBounds.w, fitBounds.h, size.width, size.height, limits.minScale, limits.maxScale, limits.alignTop, fitPad, fitBottom],
  );
  const fitRef = React.useRef(fit);
  fitRef.current = fit;
  React.useLayoutEffect(() => {
    if (!userMovedRef.current && size.width > 0) setViewport(fit, false);
  }, [fit, size.width, setViewport]);
  const doFit = React.useCallback(() => {
    userMovedRef.current = false;
    setViewport(fitRef.current, false);
  }, [setViewport]);

  // The scale labels and edge labels are placed for: it settles 120 ms after zooming stops.
  const [labelScale, setLabelScale] = React.useState(viewport.scale);
  React.useEffect(() => {
    if (viewport.scale === labelScale) return undefined;
    const timer = setTimeout(() => setLabelScale(viewport.scale), LABEL_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [viewport.scale, labelScale]);

  // Cluster members are dots, drawn to scale.
  const dots = React.useMemo(() => {
    const map = new Map<string, number>();
    if (kind !== "clusters") return map;
    const leads = new Set(layout.clusters.map((cluster) => cluster.lead).filter((id): id is string => Boolean(id)));
    for (const [id, box] of layout.nodes) if (!leads.has(id)) map.set(id, box.w / 2);
    return map;
  }, [kind, layout]);

  // Hover, focus and highlight.
  const [hover, setHover] = React.useState<Hover | null>(null);
  const [focusedId, setFocusedId] = React.useState<string | null>(null);
  const [announcement, setAnnouncement] = React.useState("");
  const highlight = React.useMemo<Highlight | null>(() => {
    if (hover?.node && model.byId.has(hover.node)) return highlightFor(model, hover.node, hoverHighlight);
    if (hover?.edge) {
      const edge = model.edgeById.get(hover.edge);
      if (edge) return { nodes: new Set([edge.from, edge.to]), edges: new Set([edge.id]), downstream: new Set(), downstreamEdges: new Set() };
    }
    return null;
  }, [hover, model, hoverHighlight]);
  const selectedEdges = React.useMemo(() => {
    const set = new Set<string>();
    if (props.selectedEdge) set.add(props.selectedEdge);
    for (const id of selection) for (const edgeId of model.incident.get(id) ?? []) set.add(edgeId);
    return set;
  }, [props.selectedEdge, selection, model]);
  const accentNodes = React.useMemo(() => new Set(props.accentPath?.nodes ?? []), [props.accentPath]);
  const accentEdges = React.useMemo(() => new Set(props.accentPath?.edges ?? []), [props.accentPath]);

  const order = React.useMemo(() => readingOrder(layout.nodes.values(), cards), [layout.nodes, cards]);
  const tabStopId = focusedId && model.byId.has(focusedId)
    ? focusedId
    : selection.find((id) => model.byId.has(id)) ?? order[0]?.id ?? null;

  React.useEffect(() => {
    if (props.focusId && modelRef.current.byId.has(props.focusId)) setFocusedId(props.focusId);
  }, [props.focusId]);

  // Live: particles, landing flashes, memory arcs, boosts and count badges.
  const [flashes, addFlash] = useExpiring<Record<string, never>>();
  const [memoryLit, addMemory] = useExpiring<{ op: "write" | "read" | "quarantine" }>();
  const [boosts, addBoost] = useExpiring<Record<string, never>>();
  const [badges, addBadge] = useExpiring<{ count: number }>();
  const seenMessages = React.useRef(new Set<string>());
  const seenMemory = React.useRef(new Set<string>());
  const flightsRef = React.useRef<(Flight & { shape: EdgeShape; offset: number })[]>([]);
  const [loopToken, setLoopToken] = React.useState(0);
  const badgesRef = React.useRef(badges);
  badgesRef.current = badges;

  const edgeBetween = React.useCallback((from: string, to: string): VizEdge | null => {
    let best: VizEdge | null = null;
    for (const edgeId of model.incident.get(from) ?? []) {
      const edge = model.edgeById.get(edgeId);
      if (!edge) continue;
      const joins = (edge.from === from && edge.to === to) || (edge.from === to && edge.to === from);
      if (!joins) continue;
      if (edge.relation === "traffic" && edge.from === from) return edge;
      if (!best) best = edge;
    }
    return best;
  }, [model]);

  React.useEffect(() => {
    const messages = props.messages ?? [];
    if (messages.length === 0) return;
    const { fly, overflow } = admitMessages(messages, seenMessages.current, flightsRef.current.length, Date.now());
    trimSeen(seenMessages.current);
    let started = false;
    for (const message of fly) {
      const a = boxesRef.current.get(message.from);
      const b = boxesRef.current.get(message.to);
      if (!a || !b) continue;
      const edge = edgeBetween(message.from, message.to);
      if (reduced) {
        if (edge) addBoost(edge.id, {}, PARTICLES.boostMs);
        addFlash(message.to, {}, PARTICLES.flashMs);
        continue;
      }
      const forward = !edge || edge.from === message.from;
      const route = edge ? layoutRef.current.routes.get(edge.id) : undefined;
      const shape = forward ? edgeShape(a, b, route) : edgeShape(b, a, route);
      const reversed = !forward;
      const distance = edgeLength(shape) * viewportRef.current.scale;
      flightsRef.current.push({
        key: messageKey(message),
        from: message.from,
        to: message.to,
        edgeId: edge?.id ?? null,
        start: now(),
        duration: travelMs(distance),
        shape: reversed ? { straight: shape.straight, points: [shape.points[3], shape.points[2], shape.points[1], shape.points[0]] } : shape,
        offset: edge ? model.offsets.get(edge.id) ?? 0 : 0,
      });
      started = true;
    }
    for (const message of overflow) {
      const edge = edgeBetween(message.from, message.to);
      const key = edge?.id ?? `${message.from}>${message.to}`;
      const count = (badgesRef.current.get(key)?.count ?? 0) + 1;
      addBadge(key, { count }, PARTICLES.badgeMs);
    }
    if (started) setLoopToken((token) => token + 1);
  }, [props.messages, reduced, edgeBetween, model, addBoost, addFlash, addBadge]);

  React.useEffect(() => {
    const events = props.memoryEvents ?? [];
    const wall = Date.now();
    for (const event of events) {
      const key = event.id ?? `${event.node}:${event.op}@${event.at}`;
      if (seenMemory.current.has(key)) continue;
      seenMemory.current.add(key);
      if (wall - event.at > PARTICLES.staleMs) continue;
      addMemory(event.node, { op: event.op }, PARTICLES.memoryMs);
    }
    trimSeen(seenMemory.current);
  }, [props.memoryEvents, addMemory]);

  // The one animation loop: particles (and the canvas pulse); paused off screen.
  const liveIds = React.useMemo(() => new Set(model.nodes.filter((node) => node.live || node.tone === "live").map((node) => node.id)), [model.nodes]);
  const canvasPulse = useCanvas && !reduced && liveIds.size > 0;
  /** Repaints the canvas with a pulse phase, without a React render (set by the canvas effect). */
  const repaintRef = React.useRef<((phase: number | null) => void) | null>(null);
  React.useEffect(() => {
    if (!onScreen || typeof requestAnimationFrame !== "function") return undefined;
    if (flightsRef.current.length === 0 && !canvasPulse) return undefined;
    let frame = 0;
    const tick = () => {
      const at = now();
      const pool = particleRef.current;
      const scale = viewportRef.current.scale;
      const flights = flightsRef.current;
      const landed: string[] = [];
      for (let index = flights.length - 1; index >= 0; index -= 1) {
        if ((at - flights[index].start) / flights[index].duration >= 1) {
          landed.push(flights[index].to);
          flights.splice(index, 1);
        }
      }
      if (pool) {
        const groups = pool.children;
        for (let index = 0; index < groups.length; index += 1) {
          const group = groups[index] as SVGGElement;
          const flight = flights[index];
          if (!flight) {
            if (group.style.display !== "none") group.style.display = "none";
            continue;
          }
          const p = easeTravel((at - flight.start) / flight.duration);
          const head = pointAt(flight.shape, p);
          const tail = pointAt(flight.shape, Math.max(0, p - PARTICLES.trail));
          let ox = 0;
          let oy = 0;
          if (flight.offset !== 0 && flight.shape.straight) {
            const normal = edgeNormal(flight.shape.points[0], flight.shape.points[3]);
            ox = (normal.x * flight.offset) / scale;
            oy = (normal.y * flight.offset) / scale;
          }
          group.style.display = "";
          const line = group.firstElementChild as SVGLineElement | null;
          const dot = group.lastElementChild as SVGCircleElement | null;
          line?.setAttribute("x1", String(tail.x + ox));
          line?.setAttribute("y1", String(tail.y + oy));
          line?.setAttribute("x2", String(head.x + ox));
          line?.setAttribute("y2", String(head.y + oy));
          dot?.setAttribute("cx", String(head.x + ox));
          dot?.setAttribute("cy", String(head.y + oy));
          dot?.setAttribute("r", String(2.3 / scale));
        }
      }
      for (const id of landed) addFlash(id, {}, PARTICLES.flashMs);
      if (canvasPulse) repaintRef.current?.((at % 1600) / 1600);
      if (flights.length > 0 || canvasPulse) frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [loopToken, onScreen, canvasPulse, addFlash]);

  // Labels: the A3.9 level of detail, then collision placement at the settled scale.
  const sidesFor = React.useCallback((node: VizNode): LabelSide[] | undefined => {
    const angle = layout.angles.get(node.id);
    if (angle !== undefined) return outwardSides(angle);
    if (kind === "layered" || (node.kind === "coordinator" && kind === "clusters")) return ["right", "below", "left", "above"];
    return undefined;
  }, [layout, kind]);
  const labelRadius = React.useCallback((node: VizNode, scale: number) => {
    const dot = dots.get(node.id);
    return (dot !== undefined ? dot * scale : markGeometry(node.kind, node.size).outer) + (node.memory && node.memory !== "none" ? 2 : 0);
  }, [dots]);

  // The labels that stand at this zoom: the level of detail, then collision
  // placement. Hover does not reflow them (see hoverLabels).
  const baseLabels = React.useMemo(() => {
    const count = model.nodes.length;
    const always = new Set<string>(selection);
    if (focusedId) always.add(focusedId);
    const candidates: VizNode[] = [];
    for (const node of model.nodes) {
      if (isCardKind(node.kind)) continue;
      if (dots.has(node.id) && !always.has(node.id)) continue;
      const hub = model.hubs.has(node.id);
      let show: boolean;
      if (labels === "all") show = true;
      else if (labels === "hover") show = always.has(node.id);
      else if (count <= 60) show = true;
      else if (count <= 150) show = hub || always.has(node.id) || labelScale >= 1.25;
      else if (count <= 600) show = hub || always.has(node.id);
      else show = always.has(node.id);
      if (show) candidates.push(node);
    }
    if (candidates.length === 0) return new Map<string, LabelPlacement>();
    const inputs: LabelInput[] = candidates.map((node) => {
      const box = layout.nodes.get(node.id);
      return {
        id: node.id,
        width: measures.label(node.label),
        x: (box?.x ?? 0) * labelScale,
        y: (box?.y ?? 0) * labelScale,
        r: labelRadius(node, labelScale),
        priority: (selectedSet.has(node.id) ? 1e6 : 0) + (node.id === focusedId ? 5e5 : 0) + (model.hubs.has(node.id) ? 1e4 : 0) + (node.live ? 1e3 : 0) + (model.degree.get(node.id) ?? 0),
        force: always.has(node.id) || model.hubs.has(node.id),
        sides: sidesFor(node),
      };
    });
    const obstacles = model.nodes.map((node) => {
      const box = layout.nodes.get(node.id);
      const dot = dots.get(node.id);
      const outer = isCardKind(node.kind) ? (Math.max(box?.w ?? 0, box?.h ?? 0) / 2) * labelScale : dot !== undefined ? dot * labelScale : markGeometry(node.kind, node.size).outer;
      return { id: node.id, x: (box?.x ?? 0) * labelScale, y: (box?.y ?? 0) * labelScale, r: outer };
    });
    return placeLabels(inputs, obstacles);
  }, [model, layout, dots, labels, labelScale, selection, selectedSet, focusedId, measures.label, sidesFor, labelRadius]);

  // A hover shows the dropped labels of the hovered node and its
  // neighbours, on their first side (the rest dims, so overlap is fine).
  const labelPlacements = React.useMemo(() => {
    if (!hover?.node) return baseLabels;
    const ids = model.nodes.length <= 600 && highlight ? [...highlight.nodes] : [hover.node];
    const missing = ids.filter((id) => !baseLabels.has(id));
    if (missing.length === 0) return baseLabels;
    const merged = new Map(baseLabels);
    for (const id of missing) {
      const node = model.byId.get(id);
      if (!node || isCardKind(node.kind)) continue;
      const side = sidesFor(node)?.[0] ?? "below";
      merged.set(id, labelAt(side, { width: measures.label(node.label), r: labelRadius(node, viewport.scale) }).placement);
    }
    return merged;
  }, [baseLabels, hover, highlight, model, sidesFor, labelRadius, measures.label, viewport.scale]);

  // Edge geometry for the current positions.
  const edgeGeometry = React.useMemo(() => {
    const map = new Map<string, { geometry: EdgeGeometry; shape: EdgeShape }>();
    for (const edge of model.edges) {
      const from = boxes.get(edge.from);
      const to = boxes.get(edge.to);
      if (!from || !to) continue;
      map.set(edge.id, edgeGeometryFor(edge, from, to, layout, model.offsets.get(edge.id) ?? 0, model.byId.get(edge.to), dots));
    }
    return map;
  }, [model, boxes, layout, dots]);

  const visibleEdgeLabels = React.useMemo(() => {
    const ids: string[] = [];
    for (const edge of model.edges) {
      if (!edge.label) continue;
      const hot = highlight?.edges.has(edge.id) || props.selectedEdge === edge.id || selectedEdges.has(edge.id);
      let show = false;
      if (edgeLabels === "always") show = true;
      else if (edgeLabels === "hover") show = Boolean(hot);
      else {
        const length = (edgeGeometry.get(edge.id)?.geometry.length ?? 0) * labelScale;
        show = Boolean(hot) || (labelScale >= 1.6 && length > 120);
      }
      if (show) ids.push(edge.id);
    }
    return ids;
  }, [model.edges, highlight, props.selectedEdge, selectedEdges, edgeLabels, edgeGeometry, labelScale]);

  // Test ids.
  const prefixed = (suffix: string) => (testIdPrefix ? `${testIdPrefix}-${suffix}` : undefined);
  const nodeTestId = (node: VizNode) => resolveTestId(testIds.node, node, prefixed(`node:${node.id}`));
  const edgeTestId = (edge: VizEdge) => resolveTestId(testIds.edge, edge, prefixed(`edge:${edge.from}:${edge.to}:${edge.relation}`));
  const edgeLabelTestId = (edge: VizEdge) => resolveTestId(testIds.edgeLabel, edge, prefixed(`edge-label:${edge.id}`));
  const frameTestId = (frame: VizFrame) => resolveTestId(testIds.frame, frame, prefixed(`frame:${frame.id}`));
  const laneTestId = (lane: VizLane) => resolveTestId(testIds.lane, lane, prefixed(`lane:${lane.id}`));
  const zoomTestId = (action: VizZoomAction) => (testIds.zoom ? testIds.zoom(action) : prefixed(`zoom:${action}`));
  const legendItemTestId = (key: string) => (testIds.legendItem ? testIds.legendItem(key) : prefixed(`legend:${key}`));

  // Interaction.
  const rootRect = () => rootRef.current?.getBoundingClientRect() ?? { left: 0, top: 0, width: size.width, height: size.height };
  const zoomBy = React.useCallback((factor: number) => {
    setViewport((current) => zoomAt(current, factor, size.width / 2, size.height / 2), true);
  }, [setViewport, size.width, size.height]);
  const reveal = React.useCallback((id: string) => {
    const box = boxesRef.current.get(id);
    if (!box || size.width <= 0) return;
    setViewport((current) => revealViewport(current, { x: box.x, y: box.y, w: Math.max(box.w, 24 / current.scale), h: Math.max(box.h, 24 / current.scale) }, size.width, size.height), false);
  }, [setViewport, size.width, size.height]);
  const nodeElement = React.useCallback((id: string): SVGGElement | null => {
    const svg = svgRef.current;
    if (!svg) return null;
    for (const element of svg.querySelectorAll<SVGGElement>("[data-node-id]")) {
      if (element.getAttribute("data-node-id") === id && element.getAttribute("role") === "button") return element;
    }
    return null;
  }, []);
  const announce = React.useCallback((id: string) => {
    const node = model.byId.get(id);
    if (node) setAnnouncement(nodeAccessibleName(node, model.degree.get(id) ?? 0));
  }, [model]);
  const focusNode = React.useCallback((id: string) => {
    if (!model.byId.has(id)) return;
    setFocusedId(id);
    reveal(id);
    announce(id);
    const element = useCanvas ? null : nodeElement(id);
    if (!useCanvas) element?.focus({ preventScroll: true });
    else if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => svgRef.current?.querySelector<SVGGElement>(".cc-viz-proxy")?.focus({ preventScroll: true }));
  }, [model, reveal, announce, nodeElement, useCanvas]);

  const fitTo = React.useCallback((ids: readonly string[]) => {
    let x0 = Infinity;
    let y0 = Infinity;
    let x1 = -Infinity;
    let y1 = -Infinity;
    for (const id of ids) {
      const frame = layoutRef.current.frames.find((candidate) => candidate.id === id);
      const box = boxesRef.current.get(id);
      if (frame) {
        x0 = Math.min(x0, frame.x);
        y0 = Math.min(y0, frame.y);
        x1 = Math.max(x1, frame.x + frame.w);
        y1 = Math.max(y1, frame.y + frame.h);
      } else if (box) {
        x0 = Math.min(x0, box.x - Math.max(box.w / 2, 40));
        y0 = Math.min(y0, box.y - box.h / 2 - 8);
        x1 = Math.max(x1, box.x + Math.max(box.w / 2, 40));
        y1 = Math.max(y1, box.y + box.h / 2 + 24);
      }
    }
    if (!Number.isFinite(x0) || size.width <= 0) return;
    const target = fitViewport({ x: x0, y: y0, w: x1 - x0, h: y1 - y0 }, size.width, size.height, { minScale: ZOOM.min, maxScale: 1.6 }, { top: ZOOM.pad, right: ZOOM.pad, bottom: fitBottom, left: ZOOM.pad });
    setViewport(target, true);
  }, [setViewport, size.width, size.height, fitBottom]);

  React.useImperativeHandle(ref, () => ({
    fit: doFit,
    fitTo,
    zoomBy,
    reveal,
    focusNode,
    viewport: () => viewportRef.current,
  }), [doFit, fitTo, zoomBy, reveal, focusNode]);

  const onZoom = (action: VizZoomAction) => {
    if (action === "in") zoomBy(ZOOM.buttonStep);
    else if (action === "out") zoomBy(1 / ZOOM.buttonStep);
    else if (action === "reset") setViewport((current) => zoomTo(current, 1, size.width / 2, size.height / 2), true);
    else if (action === "fit") doFit();
    else if (action === "minimap") {
      setMinimapHidden((hidden) => {
        writeMinimapHidden(panel, !hidden);
        return !hidden;
      });
    }
  };

  const toggleSelection = (id: string, additive: boolean) => {
    if (additive) {
      select(selectedSet.has(id) ? selection.filter((other) => other !== id) : [...selection, id]);
    } else {
      select(selectedSet.has(id) && selection.length === 1 ? [] : [id]);
    }
  };

  const targetOf = (event: { target: EventTarget | null; clientX?: number; clientY?: number }): { node?: string; edge?: string; chrome?: boolean } => {
    const element = event.target instanceof Element ? event.target : null;
    if (element?.closest(".cc-viz-chrome")) return { chrome: true };
    if (useCanvas) {
      const rect = rootRect();
      if (event.clientX === undefined || event.clientY === undefined) return {};
      const id = nodeAtPoint(boxesRef.current, viewportRef.current, event.clientX - rect.left, event.clientY - rect.top, 10, dots);
      return id ? { node: id } : {};
    }
    const nodeEl = element?.closest("[data-node-id]");
    if (nodeEl && nodeEl.getAttribute("role") === "button") return { node: nodeEl.getAttribute("data-node-id") ?? undefined };
    const edgeEl = element?.closest("[data-edge-id]");
    if (edgeEl) return { edge: edgeEl.getAttribute("data-edge-id") ?? undefined };
    return {};
  };

  const pressRef = React.useRef<{ id: number; x: number; y: number; node?: string; edge?: string; pan: boolean; moved: boolean } | null>(null);
  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if ((event.button ?? 0) !== 0) return;
    const target = targetOf(event);
    if (target.chrome) return;
    const pan = !target.node && !target.edge;
    pressRef.current = { id: event.pointerId, x: event.clientX, y: event.clientY, node: target.node, edge: target.edge, pan, moved: false };
    if (pan) {
      event.currentTarget.setPointerCapture?.(event.pointerId);
      event.preventDefault();
    }
  };
  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const press = pressRef.current;
    if (press && press.id === event.pointerId) {
      const dx = event.clientX - press.x;
      const dy = event.clientY - press.y;
      if (!press.moved && Math.hypot(dx, dy) > 3) press.moved = true;
      if (press.pan && press.moved) {
        press.x = event.clientX;
        press.y = event.clientY;
        setViewport((current) => ({ ...current, tx: current.tx + dx, ty: current.ty + dy }), true);
      }
      return;
    }
    if (useCanvas) {
      const target = targetOf(event);
      setHover((current) => (current?.node === target.node && !current?.edge ? current : target.node ? { node: target.node } : null));
    }
  };
  const onPointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    const press = pressRef.current;
    if (!press || press.id !== event.pointerId) return;
    pressRef.current = null;
    if (press.pan) {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
      if (!press.moved) {
        if (selection.length > 0) select([]);
        props.onEdgeSelect?.(null);
      }
      return;
    }
    if (press.moved) return;
    if (press.node) {
      setFocusedId(press.node);
      toggleSelection(press.node, event.shiftKey || event.metaKey || event.ctrlKey);
    } else if (press.edge) {
      props.onEdgeSelect?.(press.edge);
    }
  };
  const onDoubleClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = targetOf(event);
    if (target.chrome) return;
    if (target.node) props.onOpen?.(target.node);
    else if (!target.edge) doFit();
  };
  const onPointerOver = (event: React.PointerEvent<HTMLDivElement>) => {
    if (useCanvas || pressRef.current?.pan) return;
    const target = targetOf(event);
    if (target.chrome) return;
    if (target.node) {
      setHover((current) => (current?.node === target.node ? current : { node: target.node }));
    } else if (target.edge) {
      const rect = rootRect();
      const x = event.clientX - rect.left;
      const y = event.clientY - rect.top;
      setHover((current) => (current?.edge === target.edge ? current : { edge: target.edge, x, y }));
    } else {
      setHover((current) => (current ? null : current));
    }
  };
  const onPointerLeave = () => {
    setHover(null);
  };

  const onHoverRef = React.useRef(props.onHover);
  onHoverRef.current = props.onHover;
  React.useEffect(() => {
    onHoverRef.current?.(hover ? (hover.node ? { node: hover.node } : { edge: hover.edge }) : null);
  }, [hover?.node, hover?.edge]);
  const onFocusChangeRef = React.useRef(props.onFocusChange);
  onFocusChangeRef.current = props.onFocusChange;
  React.useEffect(() => {
    onFocusChangeRef.current?.(focusedId);
  }, [focusedId]);

  // Wheel: zoom at the pointer; Shift pans sideways. A native, non-passive
  // listener; `wheelZoom` leaves plain scrolling (or all of it) to the page.
  const wheelZoomRef = React.useRef(wheelZoom);
  wheelZoomRef.current = wheelZoom;
  React.useEffect(() => {
    const element = rootRef.current;
    if (!element) return undefined;
    const handler = (event: WheelEvent) => {
      if (event.target instanceof Element && event.target.closest(".cc-viz-chrome")) return;
      const mode = wheelZoomRef.current;
      if (mode === "off" || (mode === "pinch" && !event.ctrlKey && !event.metaKey)) return;
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      if (event.shiftKey && mode === "always") {
        const { dx, dy } = wheelPixels(event);
        const delta = Math.abs(dx) > Math.abs(dy) ? dx : dy;
        setViewport((current) => ({ ...current, tx: current.tx - delta }), true);
        return;
      }
      const factor = wheelZoomFactor(event);
      setViewport((current) => zoomAt(current, factor, event.clientX - rect.left, event.clientY - rect.top), true);
    };
    element.addEventListener("wheel", handler, { passive: false });
    return () => element.removeEventListener("wheel", handler);
  }, [setViewport]);

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const element = event.target instanceof Element ? event.target : null;
    if (element?.closest(".cc-viz-chrome")) return;
    const fromId = element?.closest("[data-node-id]")?.getAttribute("data-node-id") ?? (element?.closest(".cc-viz-proxy") ? focusedId : null) ?? tabStopId;
    if (isNavigationKey(event.key)) {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      event.preventDefault();
      const next = nodeToward(layout.nodes, fromId, event.key, cards);
      if (next) focusNode(next);
      return;
    }
    switch (event.key) {
      case "Enter":
        if (fromId) {
          event.preventDefault();
          props.onOpen?.(fromId);
        }
        return;
      case " ":
        if (fromId) {
          event.preventDefault();
          toggleSelection(fromId, true);
        }
        return;
      case "Escape":
        if (focusMode) setFocusMode(false);
        if (selection.length > 0) {
          event.preventDefault();
          select([]);
        }
        return;
      case "+":
      case "=":
        event.preventDefault();
        zoomBy(ZOOM.buttonStep);
        return;
      case "-":
      case "_":
        event.preventDefault();
        zoomBy(1 / ZOOM.buttonStep);
        return;
      case "0":
        event.preventDefault();
        doFit();
        return;
      case "f":
      case "F":
        if (event.altKey || event.ctrlKey || event.metaKey) return;
        event.preventDefault();
        setFocusMode((on) => !on && selection.length > 0);
        return;
      default:
    }
  };
  const onFocus = (event: React.FocusEvent<HTMLDivElement>) => {
    const element = event.target instanceof Element ? event.target : null;
    const id = element?.closest("[data-node-id]")?.getAttribute("data-node-id");
    if (id && element?.getAttribute("role") === "button" && id !== focusedId) {
      setFocusedId(id);
      reveal(id);
      announce(id);
    }
  };

  // Refit after focus mode changes what is drawn.
  React.useEffect(() => {
    userMovedRef.current = false;
  }, [focusTarget]);

  // First paint, once.
  const firstRenderRef = React.useRef(props.onFirstRender);
  firstRenderRef.current = props.onFirstRender;
  React.useEffect(() => {
    let frame = 0;
    if (typeof requestAnimationFrame === "function") {
      frame = requestAnimationFrame(() => firstRenderRef.current?.(now() - mountedAt.current));
    } else {
      firstRenderRef.current?.(now() - mountedAt.current);
    }
    return () => {
      if (frame) cancelAnimationFrame(frame);
    };
  }, []);

  // Canvas: draw on every change of what it shows.
  React.useLayoutEffect(() => {
    if (!useCanvas) return;
    const canvas = canvasRef.current;
    if (!canvas || size.width <= 0 || size.height <= 0) return;
    const dpr = Math.min(2, typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1);
    const width = Math.round(size.width * dpr);
    const height = Math.round(size.height * dpr);
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return;
    const scene = {
      width: size.width,
      height: size.height,
      dpr,
      viewport,
      model,
      boxes,
      routes: layout.routes,
      dots,
      clusterMode: kind === "clusters",
      tokens,
      highlight,
      selected: selectedSet,
      flashes: new Set(flashes.keys()),
      live: liveIds,
      accentNodes,
      accentEdges,
      phase: reduced ? null : 0,
    };
    const repaint = (phase: number | null) => drawCanvasScene(context, { ...scene, phase: reduced ? null : phase });
    repaintRef.current = repaint;
    repaint(reduced ? null : (now() % 1600) / 1600);
  }, [useCanvas, size.width, size.height, viewport, model, boxes, layout, dots, kind, tokens, highlight, selectedSet, flashes, liveIds, accentNodes, accentEdges, reduced]);

  // Chrome visibility.
  const showLegend = legend === "on" || legend === "collapsed" || (legend === "auto" && (model.relations.size >= 2 || model.kinds.size >= 2 || Boolean(props.legendNote)));
  const minimapWanted = minimap === "on" || (minimap === "auto" && needsMinimap(layout.bounds, viewport, size.width, size.height));
  const minimapAvailable = minimap !== "off" && minimapWanted && model.nodes.length > 0;
  const showMinimap = minimapAvailable && !minimapHidden && size.width >= 420;

  const hovering = highlight !== null;
  const frameById = React.useMemo(() => new Map((frames ?? []).map((frame) => [frame.id, frame])), [frames]);
  const goalNodes = React.useMemo(() => new Set(model.nodes.filter((node) => node.kind === "goal").map((node) => node.id)), [model.nodes]);
  const laneById = React.useMemo(() => new Map((lanes ?? []).map((lane) => [lane.id, lane])), [lanes]);


  const renderNode = (node: VizNode, box: VizNodeBox, exitingNode: boolean) => {
    const card = isCardKind(node.kind);
    let title = node.label;
    let meta = node.sublabel ?? "";
    let goal: GoalHeaderLayout | undefined;
    if (node.kind === "item") {
      title = fitText(node.label, box.w - 28 - 10, measures.title);
      meta = node.sublabel ? fitText(node.sublabel, box.w - (node.avatar ? 28 : 10) - 10, measures.meta) : "";
    } else if (node.kind === "goal") {
      const frame = frameById.get(node.id);
      const placed = layout.frames.find((candidate) => candidate.id === node.id);
      goal = layoutGoalHeader(
        box.w,
        {
          title: node.label,
          meta: frame?.meta ?? node.sublabel,
          progress: node.progress,
          trailing: node.trailing,
          collapsed: Boolean(placed?.collapsed),
        },
        { title: measures.card, meta: measures.meta },
      );
    }
    return (
      <NodeView
        key={exitingNode ? `exit:${node.id}` : node.id}
        node={node}
        box={box}
        testId={exitingNode ? undefined : nodeTestId(node)}
        ariaName={nodeAccessibleName(node, model.degree.get(node.id) ?? 0)}
        tabStop={node.id === tabStopId}
        selected={selectedSet.has(node.id)}
        lit={hovering && highlight!.nodes.has(node.id)}
        downstream={hovering && highlight!.downstream.has(node.id)}
        flash={flashes.has(node.id)}
        accent={accentNodes.has(node.id)}
        exiting={exitingNode}
        entering={entering.has(node.id)}
        live={Boolean(node.live || node.tone === "live")}
        memory={node.memory ?? "none"}
        memoryLit={memoryLit.get(node.id)?.op ?? null}
        badge={node.badge !== undefined ? node.badge : node.tone === "crit" ? "crit" : node.tone === "warn" ? "warn" : null}
        dot={dots.get(node.id) ?? 0}
        classNames={classNames}
        title={card ? title : node.label}
        meta={meta}
        goal={goal}
        decor={props.renderNodeDecor && !exitingNode ? props.renderNodeDecor(node, box) : undefined}
      />
    );
  };

  // Canvas overlay: rings for the selected, focused and hovered nodes, and the focus proxy.
  const overlayIds = useCanvas
    ? [...new Set([...selection, ...(focusedId ? [focusedId] : []), ...(hover?.node ? [hover.node] : [])])].filter((id) => model.byId.has(id))
    : [];
  const proxyId = useCanvas ? tabStopId : null;
  const proxyBox = proxyId ? boxes.get(proxyId) : undefined;
  const proxyNode = proxyId ? model.byId.get(proxyId) : undefined;

  // Tooltip.
  let tooltip: React.ReactNode = null;
  if (hover?.node && !pressRef.current?.pan) {
    const node = model.byId.get(hover.node);
    const box = boxes.get(hover.node);
    if (node && box) {
      const card = isCardKind(node.kind);
      const dot = dots.get(node.id);
      const gap = card ? (box.h / 2) * viewport.scale + 8 : (dot !== undefined ? dot * viewport.scale : markGeometry(node.kind, node.size).outer) + 10;
      tooltip = (
        <GraphTooltip
          lines={nodeTooltipLines(node)}
          x={box.x * viewport.scale + viewport.tx}
          y={box.y * viewport.scale + viewport.ty}
          gap={gap}
          frameWidth={size.width}
          tone={node.live ? "live" : node.tone}
        />
      );
    }
  } else if (hover?.edge) {
    const edge = model.edgeById.get(hover.edge);
    if (edge) {
      tooltip = (
        <GraphTooltip
          lines={edgeTooltipLines(edge, model.byId.get(edge.from), model.byId.get(edge.to))}
          x={hover.x ?? 0}
          y={hover.y ?? 0}
          gap={14}
          frameWidth={size.width}
        />
      );
    }
  }

  const k = 1 / viewport.scale;
  const worldStyle = { "--_viz-k": Math.round(k * 10000) / 10000 } as React.CSSProperties;
  const rootTestId = testIds.root ?? prefixed("graph");
  const placedFrames = layout.frames;

  return (
    <div
      ref={rootRef}
      className={cx(
        "cc-viz-graph",
        `is-${kind}`,
        useCanvas && "is-canvas",
        density === "compact" && "is-compact",
        size.width > 0 && size.width < 480 && "is-narrow",
        reduced && "is-reduced",
        focusMode && "is-focus-mode",
        props.className,
      )}
      style={{ ...(props.height !== undefined ? { height: props.height } : {}), ...props.style }}
      role="group"
      aria-roledescription="graph"
      aria-label={props.ariaLabel}
      data-testid={rootTestId}
      data-layout={kind}
      data-renderer={useCanvas ? "canvas" : "svg"}
      data-hovering={hovering ? "true" : undefined}
      {...(props.rootAttributes ?? {})}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => {
        pressRef.current = null;
      }}
      onPointerOver={onPointerOver}
      onPointerLeave={onPointerLeave}
      onDoubleClick={onDoubleClick}
      onKeyDown={onKeyDown}
      onFocus={onFocus}
    >
      {useCanvas ? <canvas ref={canvasRef} className="cc-viz-graph__canvas" aria-hidden="true" /> : null}
      <svg ref={svgRef} className="cc-viz-graph__svg" data-testid={testIds.svg} width="100%" height="100%">
        {/* The drawing waits for the frame's first measure (one layout, not two). */}
        {drawable ? (
        <>
        {kind === "lanes" ? (
          <LanesLayer
            part="rules"
            lanes={layout.lanes}
            viewport={viewport}
            width={size.width}
            compact={size.width > 0 && size.width < 600}
            testIdOf={(lane) => laneTestId(laneById.get(lane.id) ?? { id: lane.id, label: lane.label })}
          />
        ) : null}
        <g className="cc-viz-world" data-testid={testIds.viewport} transform={viewportTransform(viewport)} style={worldStyle}>
          {placedFrames.map((frame) => {
            const source = frameById.get(frame.id);
            return (
              <FrameView
                key={frame.id}
                frame={frame}
                hasHeaderNode={goalNodes.has(frame.id)}
                label={`${frame.label.toUpperCase()}${frame.meta ? `  ${frame.meta}` : ""}`}
                testId={frameTestId(source ?? { id: frame.id, label: frame.label })}
                className={cx(classNames?.frame, source?.className)}
                attributes={source?.attributes}
              />
            );
          })}
          {kind === "clusters" ? (
            <ClustersLayer
              clusters={layout.clusters}
              bundles={layout.bundles}
              hot={new Set(hover?.node ? [layout.clusters.find((cluster) => cluster.lead === hover.node)?.id ?? ""] : [])}
            />
          ) : null}
          {!useCanvas
            ? model.edges.map((edge) => {
                const entry = edgeGeometry.get(edge.id);
                if (!entry) return null;
                if (kind === "clusters" && !(highlight?.edges.has(edge.id) || highlight?.downstreamEdges.has(edge.id) || selectedEdges.has(edge.id))) return null;
                return (
                  <EdgeView
                    key={edge.id}
                    edge={edge}
                    geometry={entry.geometry}
                    testId={edgeTestId(edge)}
                    directed={model.directed.get(edge.id) ?? false}
                    hot={Boolean(highlight?.edges.has(edge.id) || selectedEdges.has(edge.id))}
                    downstream={Boolean(highlight?.downstreamEdges.has(edge.id))}
                    accent={accentEdges.has(edge.id)}
                    boosted={boosts.has(edge.id)}
                    selected={props.selectedEdge === edge.id}
                    className={classNames?.edge}
                  />
                );
              })
            : null}
          <ParticlePool ref={particleRef} size={PARTICLES.max} />
          {!useCanvas ? (
            <>
              {model.nodes.map((node) => {
                const box = boxes.get(node.id);
                return box ? renderNode(node, box, false) : null;
              })}
              {[...exiting.values()].map(({ node, box }) => renderNode(node, box, true))}
            </>
          ) : (
            <>
              {overlayIds.map((id) => {
                const box = boxes.get(id)!;
                const node = model.byId.get(id)!;
                if (!box) return null;
                const dot = dots.get(id);
                const outer = dot !== undefined ? dot : markGeometry(node.kind, node.size).outer;
                return (
                  <g key={`ring:${id}`} className={cx("cc-viz-overlay-ring", selectedSet.has(id) && "is-selected", id === focusedId && "is-focused")} transform={`translate(${box.x} ${box.y})`}>
                    <g className={dot !== undefined ? undefined : "cc-viz-k"}>
                      <circle className="cc-viz-ring cc-viz-ring--select" r={outer + (dot !== undefined ? 3 : 5)} />
                      <circle className="cc-viz-ring cc-viz-ring--focus" r={outer + (dot !== undefined ? 5 : 8)} />
                    </g>
                  </g>
                );
              })}
              {proxyNode && proxyBox ? (
                <g
                  className="cc-viz-proxy"
                  transform={`translate(${proxyBox.x} ${proxyBox.y})`}
                  role="button"
                  tabIndex={0}
                  aria-label={nodeAccessibleName(proxyNode, model.degree.get(proxyNode.id) ?? 0)}
                  aria-pressed={selectedSet.has(proxyNode.id)}
                  data-proxy-for={proxyNode.id}
                >
                  <g className="cc-viz-k">
                    <circle className="cc-viz-hit" r={12} />
                    <circle className="cc-viz-ring cc-viz-ring--focus" r={(dots.get(proxyNode.id) ?? markGeometry(proxyNode.kind, proxyNode.size).outer) + 8} />
                  </g>
                </g>
              ) : null}
            </>
          )}
          {[...labelPlacements.entries()].map(([id, placement]) => {
            const node = model.byId.get(id);
            const box = boxes.get(id);
            if (!node || !box) return null;
            const secondary = node.kind === "person" || node.kind === "source" || node.kind === "external" || node.kind === "unknown";
            return (
              <NodeLabel
                key={id}
                id={id}
                text={node.label}
                x={box.x}
                y={box.y}
                placement={placement}
                hot={Boolean(node.live || node.tone === "live" || flashes.has(id))}
                secondary={secondary}
                strong={selectedSet.has(id) || id === focusedId}
                lit={hovering && highlight!.nodes.has(id)}
                retired={Boolean(node.retired)}
              />
            );
          })}
          {visibleEdgeLabels.map((id) => {
            const edge = model.edgeById.get(id)!;
            const entry = edgeGeometry.get(id);
            if (!entry) return null;
            return (
              <EdgeLabel
                key={id}
                id={id}
                text={edge.label!}
                x={entry.geometry.mid.x}
                y={entry.geometry.mid.y}
                lift={Math.abs(Math.cos(entry.geometry.midAngle)) > 0.85 ? 9 : 0}
                testId={edgeLabelTestId(edge)}
                className={classNames?.edgeLabel}
                hot={Boolean(highlight?.edges.has(id) || props.selectedEdge === id)}
              />
            );
          })}
          {[...badges.entries()].map(([key, badge]) => {
            const entry = edgeGeometry.get(key);
            if (!entry) return null;
            return <EdgeBadge key={`badge:${key}`} x={entry.geometry.mid.x} y={entry.geometry.mid.y} count={badge.count} />;
          })}
        </g>
        {kind === "lanes" ? (
          <LanesLayer part="labels" lanes={layout.lanes} viewport={viewport} width={size.width} compact={size.width > 0 && size.width < 600} />
        ) : null}
        </>
        ) : null}
      </svg>
      <div className="cc-viz-chrome">
        {tooltip}
        {showLegend || controls ? (
          <div className={cx("cc-viz-chrome__left", !showMinimap && "is-wide")} ref={chromeRef}>
            {showLegend ? (
              <Legend
                relations={model.relations}
                kinds={model.kinds}
                hidden={hiddenSet}
                onToggle={(relation) => {
                  const next = hiddenSet.has(relation) ? hiddenList.filter((value) => value !== relation) : [...hiddenList, relation];
                  if (props.hiddenRelations === undefined) setOwnHidden(next);
                  props.onHiddenRelationsChange?.(next);
                }}
                collapsed={legendCollapsed}
                onCollapsedChange={setLegendChoice}
                stacked={narrowFrame}
                testId={testIds.legend ?? prefixed("legend")}
                itemTestId={legendItemTestId}
                headingClassName={classNames?.legendHeading}
                dots={kind === "clusters"}
                note={props.legendNote}
              />
            ) : null}
            {controls ? (
              <ZoomControls scale={viewport.scale} onZoom={onZoom} minimap={{ available: minimapAvailable, shown: showMinimap }} testIdOf={zoomTestId} />
            ) : null}
          </div>
        ) : null}
        {showMinimap ? (
          <Minimap
            bounds={layout.bounds}
            dots={model.nodes.map((node) => {
              const box = layout.nodes.get(node.id);
              return { x: box?.x ?? 0, y: box?.y ?? 0, live: liveIds.has(node.id) };
            })}
            viewport={viewport}
            frameWidth={size.width}
            frameHeight={size.height}
            onCentre={(wx, wy) => setViewport((current) => centreOn(current, wx, wy, size.width, size.height), true)}
            onPan={(dwx, dwy) => setViewport((current) => ({ ...current, tx: current.tx - dwx * current.scale, ty: current.ty - dwy * current.scale }), true)}
            testId={testIds.minimap ?? prefixed("minimap")}
          />
        ) : null}
      </div>
      <div className="cc-viz-sr" role="status" aria-live="polite" aria-atomic="true">
        {announcement}
      </div>
    </div>
  );
});

export type { GraphModel };
