import React from "react";
import { CopyButton } from "../../../packages/console-components/src/copy-button";
import { CopyGlyph } from "../../../packages/console-components/src/copy-glyph";

import type { WorkGraphWireBinding, WorkGraphWireEdge, WorkGraphWireItem } from "../types";
import {
  WORKGRAPH_GRAPH_NODE_HEIGHT,
  WORKGRAPH_GRAPH_ROW_HEIGHT,
  layoutWorkGraph,
  workGraphEdgeMidpoint,
  workGraphEdgePath,
  workGraphItemOwnerLabel,
} from "../lib/workgraph-layout";
import type { WorkGraphLayoutEdge, WorkGraphLayoutNode } from "../lib/workgraph-layout";
import { useZoomPan, viewportTransform } from "./topology/zoom-pan";
import type { Viewport } from "./topology/zoom-pan";

/// Read-only layered-DAG rendering of the workgraph snapshot: nodes carry
/// the tree panel's status language, parent edges are solid arrows into the
/// parent, blocks edges dashed amber. Pan by dragging, zoom with the wheel
/// (non-passive listener inside useZoomPan so the dock scroll container
/// never eats it), Fit resets to the fitted view. Mutations stay in the
/// tree/attention sections - the graph only selects.
///
/// Scale model: the viewBox is the measured frame size, so one SVG user
/// unit is one CSS pixel and the CSS font sizes on the labels are real
/// pixel sizes. The layout is fitted by the zoom viewport instead of by
/// `preserveAspectRatio`, and that fit never shrinks below
/// FIT_MIN_SCALE, so labels stay legible however large the graph gets (a
/// big graph opens at a readable scale and is panned, not squeezed into
/// the frame at unreadable sizes).
interface WorkGraphGraphViewProps {
  items: WorkGraphWireItem[];
  edges: WorkGraphWireEdge[];
  attention: WorkGraphWireBinding[];
  selectedId?: string;
  onSelect?: (itemId: string) => void;
}

/// Character caps used when text cannot be measured (server render, jsdom).
const TITLE_MAX_CHARS = 21;
const META_MAX_CHARS = 24;
/// Label x offset inside a node and the right-hand breathing room.
const LABEL_X = 26;
const LABEL_RIGHT_PAD = 10;
/// Must match `.workgraph-graph__node-title` / `__node-meta` in
/// console-host.css; used to measure labels on a canvas.
const TITLE_FONT = { size: 12, weight: 500, family: "--sans" } as const;
const META_FONT = { size: 10, weight: 400, family: "--mono" } as const;
/// Initial fit never shrinks the graph below this, so 12px titles render
/// at 10px or more. Smaller graphs open at 1:1 and are never enlarged.
const FIT_MIN_SCALE = 0.85;

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

type MeasureText = (text: string) => number;

/// Longest prefix of `text` (plus an ellipsis) whose measured width fits
/// `maxWidth`. Without a measurer it falls back to the character cap.
function fitLabel(
  text: string,
  maxWidth: number,
  measure: MeasureText | null,
  fallbackChars: number,
): string {
  if (!measure) return truncate(text, fallbackChars);
  if (measure(text) <= maxWidth) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (measure(`${text.slice(0, mid).trimEnd()}…`) <= maxWidth) lo = mid;
    else hi = mid - 1;
  }
  return `${text.slice(0, lo).trimEnd()}…`;
}

/// Initial/Fit viewport for a layout drawn in a frame of the given CSS
/// pixel size: scale down to fit (never up), floored at FIT_MIN_SCALE,
/// centred on any axis that fits and pinned to the top-left otherwise.
function fitViewport(
  frameWidth: number,
  frameHeight: number,
  layoutWidth: number,
  layoutHeight: number,
): Viewport {
  if (frameWidth <= 0 || frameHeight <= 0 || layoutWidth <= 0 || layoutHeight <= 0) {
    return { tx: 0, ty: 0, scale: 1 };
  }
  const contain = Math.min(1, frameWidth / layoutWidth, frameHeight / layoutHeight);
  const scale = Math.max(FIT_MIN_SCALE, contain);
  const slackX = frameWidth - layoutWidth * scale;
  const slackY = frameHeight - layoutHeight * scale;
  return {
    tx: slackX > 0 ? slackX / 2 : 0,
    ty: slackY > 0 ? slackY / 2 : 0,
    scale,
  };
}

/// Content-box size of the svg frame, tracked with a ResizeObserver.
/// Zero until measured (and always zero outside a browser).
function useFrameSize(
  ref: React.RefObject<SVGSVGElement | null>,
  mounted: boolean,
): { width: number; height: number } {
  const [size, setSize] = React.useState({ width: 0, height: 0 });
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (!mounted || !el) return;
    const apply = (width: number, height: number) => {
      const next = { width: Math.round(width), height: Math.round(height) };
      setSize((prev) => (prev.width === next.width && prev.height === next.height ? prev : next));
    };
    apply(el.clientWidth, el.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[entries.length - 1]?.contentRect;
      if (rect) apply(rect.width, rect.height);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref, mounted]);
  return size;
}

/// Canvas text measurers resolved against the svg's own font variables,
/// so truncation follows the active theme variant (the terminal variant
/// swaps the display face for a wider monospace). Re-resolved once web
/// fonts finish loading. Null until mounted in a browser.
function useLabelMeasurers(
  ref: React.RefObject<SVGSVGElement | null>,
  mounted: boolean,
): { title: MeasureText; meta: MeasureText } | null {
  const [fontsEpoch, setFontsEpoch] = React.useState(0);
  const [families, setFamilies] = React.useState<{ sans: string; mono: string } | null>(null);
  React.useEffect(() => {
    if (typeof document === "undefined" || !document.fonts?.ready) return;
    let live = true;
    void document.fonts.ready.then(() => {
      if (live) setFontsEpoch((epoch) => epoch + 1);
    });
    return () => {
      live = false;
    };
  }, []);
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (!mounted || !el || typeof getComputedStyle !== "function") return;
    const style = getComputedStyle(el);
    const sans = style.getPropertyValue("--sans").trim() || style.fontFamily;
    const mono = style.getPropertyValue("--mono").trim() || "monospace";
    setFamilies((prev) => (prev && prev.sans === sans && prev.mono === mono ? prev : { sans, mono }));
  });
  return React.useMemo(() => {
    if (!families || typeof document === "undefined") return null;
    const ctx = document.createElement("canvas").getContext("2d");
    if (!ctx) return null;
    const measurer = (font: { size: number; weight: number; family: string }): MeasureText => {
      const family = font.family === "--mono" ? families.mono : families.sans;
      const spec = `${font.weight} ${font.size}px ${family}`;
      const cache = new Map<string, number>();
      return (text) => {
        const hit = cache.get(text);
        if (hit !== undefined) return hit;
        ctx.font = spec;
        const width = ctx.measureText(text).width;
        cache.set(text, width);
        return width;
      };
    };
    return { title: measurer(TITLE_FONT), meta: measurer(META_FONT) };
    // fontsEpoch re-creates the measurers (and clears their caches) once
    // web fonts load, since fallback-face widths are wrong afterwards.
  }, [families, fontsEpoch]);
}

function nodeMetaText(node: WorkGraphLayoutNode): string {
  const parts: string[] = [];
  if (node.priority && node.priority !== "medium") parts.push(node.priority);
  if (node.blocked) parts.push("blocked");
  if (node.ownerLabel) parts.push(node.ownerLabel);
  if (node.alsoUnder.length > 0) parts.push(`also under ${node.alsoUnder.join(", ")}`);
  return parts.join(" · ");
}

function nodeMetaLine(node: WorkGraphLayoutNode): string {
  return truncate(nodeMetaText(node), META_MAX_CHARS);
}

/// Where an edge's kind label sits: beside a near-vertical edge (which
/// runs through the short gaps between rows of the same column), and
/// centred on the curve otherwise. The label's halo keeps it legible
/// where it crosses the line.
function edgeLabelPlacement(edge: WorkGraphLayoutEdge): {
  x: number;
  y: number;
  anchor: "start" | "middle";
} {
  const [start, , , end] = edge.points;
  if (Math.abs(end.x - start.x) < 12) {
    // Centre of the row gap next to the arrowhead: for adjacent rows that
    // is the edge midpoint, and for longer spans it keeps the label off
    // the intermediate nodes the midpoint would land on.
    const toward = Math.sign(start.y - end.y) || 1;
    const gap = WORKGRAPH_GRAPH_ROW_HEIGHT - WORKGRAPH_GRAPH_NODE_HEIGHT;
    return { x: end.x + 7, y: end.y + toward * (gap / 2), anchor: "start" };
  }
  const mid = workGraphEdgeMidpoint(edge);
  return { x: mid.x, y: mid.y, anchor: "middle" };
}

function nodeHoverText(node: WorkGraphLayoutNode, item: WorkGraphWireItem | undefined): string {
  const lines = [node.title, `status: ${node.status}`];
  if (node.ownerLabel) lines.push(`owner: ${node.ownerLabel}`);
  if (node.alsoUnder.length > 0) lines.push(`also under: ${node.alsoUnder.join(", ")}`);
  if (item?.description) lines.push(item.description);
  return lines.join("\n");
}

function ItemCopyIcon({ name }: { name: string }) {
  return <CopyGlyph state={name === "i-check" ? "copied" : "idle"} />;
}

function WorkItemDetails({ itemId, item, hasAttention }: {
  itemId: string;
  item: WorkGraphWireItem | undefined;
  hasAttention: boolean;
}) {
  const owner = item ? workGraphItemOwnerLabel(item) : "";
  const status = item?.status?.replaceAll("_", " ");
  return <>
    <div className="workgraph-graph__detail-heading">
      <h4 className="workgraph-graph__detail-title">{item?.title || (item ? "Untitled work item" : "Work item unavailable in this snapshot")}</h4>
      {status ? <span className="workgraph-graph__detail-status" data-status={item?.status}>
        <span className={`workgraph__dot is-${item?.status}`} aria-hidden="true" />
        {status[0].toUpperCase() + status.slice(1)}
      </span> : null}
    </div>
    {item?.description ? <p className="workgraph-graph__detail-description">{item.description}</p> : null}
    {owner ? <p className="workgraph-graph__detail-owner">Owner <span>{owner}</span></p> : null}
    <details className="workgraph-graph__metadata">
      <summary>Item details</summary>
      <dl>
        <dt>ID</dt>
        <dd className="workgraph-graph__detail-id"><code>{itemId}</code>
          <CopyButton text={itemId} label="Copy work item ID" copiedLabel="Work item ID copied" Icon={ItemCopyIcon} />
        </dd>
        {item?.labels?.length ? <><dt>Labels</dt><dd>{item.labels.join(", ")}</dd></> : null}
        {hasAttention ? <><dt>Attention</dt><dd>Bound to this item</dd></> : null}
      </dl>
    </details>
  </>;
}

export function WorkGraphGraphView({
  items,
  edges,
  attention,
  selectedId,
  onSelect,
}: WorkGraphGraphViewProps): React.JSX.Element {
  const layout = React.useMemo(() => layoutWorkGraph(items, edges), [items, edges]);
  const itemById = React.useMemo(() => {
    const map = new Map<string, WorkGraphWireItem>();
    for (const item of items) {
      if (typeof item.id === "string" && item.id) map.set(item.id, item);
    }
    return map;
  }, [items]);
  const hasNodes = layout.nodes.length > 0;
  // useZoomPan owns the svg ref; the frame hooks read it once mounted.
  const svgRef = React.useRef<SVGSVGElement | null>(null);
  const frame = useFrameSize(svgRef, hasNodes);
  const measured = frame.width > 0 && frame.height > 0;
  const fit = React.useMemo(
    () => (measured
      ? fitViewport(frame.width, frame.height, layout.width, layout.height)
      : { tx: 0, ty: 0, scale: 1 }),
    [measured, frame.width, frame.height, layout.width, layout.height],
  );
  // Until measured (and in server renders) the viewBox falls back to the
  // layout size with identity fit, i.e. the old preserveAspectRatio fit.
  const viewBoxWidth = measured ? frame.width : layout.width;
  const viewBoxHeight = measured ? frame.height : layout.height;
  const zoom = useZoomPan(viewBoxWidth, viewBoxHeight, fit);
  const measure = useLabelMeasurers(svgRef, hasNodes);
  const setSvgRef = React.useCallback((el: SVGSVGElement | null) => {
    svgRef.current = el;
    zoom.svgRef.current = el;
  }, [zoom.svgRef]);
  const boundItemIds = React.useMemo(() => {
    const bound = new Set<string>();
    for (const binding of attention) {
      const itemId = binding.work_ref?.item_id;
      if (typeof itemId === "string" && itemId) bound.add(itemId);
    }
    return bound;
  }, [attention]);

  if (layout.nodes.length === 0) {
    return <div className="workgraph__empty">No work items to draw.</div>;
  }

  return (
    <div className="workgraph-graph" data-testid="workgraph-graph-frame">
      <div className="workgraph-graph__toolbar">
        <button
          type="button"
          className="workgraph__action"
          data-testid="workgraph-graph-fit"
          onClick={zoom.reset}
        >
          Fit
        </button>
        <span className="workgraph-graph__stats">
          {layout.nodes.length} items · {layout.edges.length} edges
        </span>
        {layout.overflowCount > 0 ? (
          <span className="workgraph-graph__overflow" data-testid="workgraph-graph-overflow">
            +{layout.overflowCount} more items not drawn
          </span>
        ) : null}
        <span className="workgraph__spacer" />
        <span className="workgraph-graph__hint">drag to pan · wheel to zoom</span>
      </div>
      <svg
        data-testid="workgraph-graph"
        className={`workgraph-graph__svg${zoom.isDragging ? " is-dragging" : ""}`}
        viewBox={`0 0 ${viewBoxWidth} ${viewBoxHeight}`}
        preserveAspectRatio="xMidYMid meet"
        data-scale={zoom.viewport.scale.toFixed(3)}
        role="img"
        aria-label="Work item dependency graph"
        ref={setSvgRef}
        onPointerDown={zoom.onPointerDown}
        onPointerMove={zoom.onPointerMove}
        onPointerUp={zoom.onPointerUp}
        onPointerCancel={zoom.onPointerUp}
      >
        <defs>
          <marker
            id="workgraph-graph-arrow"
            className="workgraph-graph__arrow"
            viewBox="0 0 8 8"
            refX="7"
            refY="4"
            markerWidth="7"
            markerHeight="7"
            orient="auto-start-reverse"
          >
            <path d="M0,0 L8,4 L0,8 z" />
          </marker>
          <marker
            id="workgraph-graph-arrow-blocks"
            className="workgraph-graph__arrow is-blocks"
            viewBox="0 0 8 8"
            refX="7"
            refY="4"
            markerWidth="7"
            markerHeight="7"
            orient="auto-start-reverse"
          >
            <path d="M0,0 L8,4 L0,8 z" />
          </marker>
        </defs>
        <g data-testid="workgraph-graph-viewport" transform={viewportTransform(zoom.viewport)}>
          {layout.edges.map((edge, index) => {
            const marker = edge.kind === "blocks"
              ? "url(#workgraph-graph-arrow-blocks)"
              : "url(#workgraph-graph-arrow)";
            return (
              <g key={`${edge.kind}:${edge.fromId}:${edge.toId}:${index}`}>
                <path
                  data-testid="workgraph-graph-edge"
                  data-kind={edge.kind}
                  className={`workgraph-graph__edge is-${edge.kind}`}
                  d={workGraphEdgePath(edge)}
                  markerEnd={marker}
                />
              </g>
            );
          })}
          {layout.nodes.map((node) => {
            const labelWidth = node.w - LABEL_X - LABEL_RIGHT_PAD;
            const metaText = nodeMetaText(node);
            const meta = metaText
              ? fitLabel(metaText, labelWidth, measure?.meta ?? null, META_MAX_CHARS)
              : "";
            const title = fitLabel(node.title, labelWidth, measure?.title ?? null, TITLE_MAX_CHARS);
            const selected = node.itemId === selectedId;
            return (
              <g
                key={node.itemId}
                data-testid="workgraph-graph-node"
                data-item-id={node.itemId}
                data-status={node.status}
                className={`workgraph-graph__node is-${node.status}${selected ? " is-selected" : ""}`}
                transform={`translate(${node.x} ${node.y})`}
                // Select on pointerdown, not click: the svg's pan handler
                // takes pointer capture, which retargets the eventual click
                // to the svg and would swallow node selection entirely.
                // stopPropagation keeps a node press from starting a pan.
                onPointerDown={(event) => {
                  event.stopPropagation();
                  onSelect?.(node.itemId);
                }}
              >
                <title>
                  {nodeHoverText(node, itemById.get(node.itemId))}
                </title>
                <rect className="workgraph-graph__node-box" width={node.w} height={node.h} rx={8} />
                <circle className="workgraph-graph__node-dot" cx={14} cy={meta ? 15 : node.h / 2} r={3.5} />
                {boundItemIds.has(node.itemId) ? (
                  <circle
                    className="workgraph-graph__node-goal-ring"
                    cx={14}
                    cy={meta ? 15 : node.h / 2}
                    r={6.5}
                  />
                ) : null}
                <text
                  className="workgraph-graph__node-title"
                  x={LABEL_X}
                  y={meta ? 19 : node.h / 2 + 4}
                >
                  {title}
                </text>
                {meta ? (
                  <text className="workgraph-graph__node-meta" x={LABEL_X} y={34}>
                    {meta}
                  </text>
                ) : null}
              </g>
            );
          })}
          {/* Edge labels paint above the nodes so a label in the short
              gap between two rows is never hidden under a node border. */}
          {layout.edges.map((edge, index) => {
            if (edge.kind === "parent") return null;
            const label = edgeLabelPlacement(edge);
            return (
              <text
                key={`label:${edge.kind}:${edge.fromId}:${edge.toId}:${index}`}
                className="workgraph-graph__edge-label"
                data-testid="workgraph-graph-edge-label"
                x={label.x}
                y={label.y}
                textAnchor={label.anchor}
                dominantBaseline="central"
              >
                {edge.kind}
              </text>
            );
          })}
        </g>
      </svg>
      <div className="workgraph-graph__detail" data-testid="workgraph-graph-detail">
        {selectedId
          ? <WorkItemDetails key={selectedId} itemId={selectedId} item={itemById.get(selectedId)} hasAttention={boundItemIds.has(selectedId)} />
          : "Click a node to inspect it."}
      </div>
    </div>
  );
}

export const __workGraphGraphViewTest = {
  fitLabel,
  fitViewport,
  FIT_MIN_SCALE,
  nodeMetaLine,
};
