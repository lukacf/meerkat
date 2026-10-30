import React from "react";
import { CopyButton } from "../../../packages/console-components/src/copy-button";
import { CopyGlyph } from "../../../packages/console-components/src/copy-glyph";
import {
  AgentAvatar,
  Button,
  Chip,
  EmptyState,
  KeyValueList,
  RelativeTime,
  TechnicalDetails,
  humanizeEnum,
  joinMeta,
  plural,
  priorityTone,
  statusTone,
  useAgentDirectory,
} from "../../../packages/console-components/src/panel-kit";
import type { AgentAvatarSize } from "../../../packages/console-components/src/panel-kit";

import type { WorkGraphWireBinding, WorkGraphWireEdge, WorkGraphWireItem } from "../types";
import {
  WORKGRAPH_GRAPH_NODE_HEIGHT,
  WORKGRAPH_GRAPH_ROW_HEIGHT,
  layoutWorkGraph,
  workGraphEdgeMidpoint,
  workGraphEdgePath,
  workGraphEdgeWord,
  workGraphItemOwner,
  workGraphStatusWord,
} from "../lib/workgraph-layout";
import type { WorkGraphLayoutEdge, WorkGraphLayoutNode, WorkGraphOwnerView } from "../lib/workgraph-layout";
import { useZoomPan, viewportTransform } from "./topology/zoom-pan";
import type { Viewport } from "./topology/zoom-pan";

/// Read-only layered-DAG rendering of the workgraph snapshot: nodes carry
/// the tree panel's status language, parent edges are solid arrows into the
/// parent, blocks edges dashed amber. Pan by dragging, zoom with the wheel
/// (non-passive listener inside useZoomPan so the dock scroll container
/// never eats it), Fit resets to the fitted view. The graph is one tab
/// stop: arrow keys move focus between nodes by position, Home and End go
/// to the first and the last, and Enter or Space selects one. Mutations
/// stay in the tree/attention sections - the graph only selects.
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
const META_FONT = { size: 10.5, weight: 400, family: "--sans" } as const;
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

/// The second line of a node, in words: priority when it matters, a blocked
/// marker (status is never colour alone), the owner's name and any extra
/// parents.
function nodeMetaText(node: WorkGraphLayoutNode, ownerName = node.ownerLabel): string {
  const priority = priorityTone(node.priority) ? humanizeEnum(node.priority) : "";
  return joinMeta(
    priority,
    node.blocked ? "Blocked" : "",
    ownerName,
    node.alsoUnder.length > 0 ? `Also under ${node.alsoUnder.join(", ")}` : "",
  );
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

function nodeHoverText(node: WorkGraphLayoutNode, item: WorkGraphWireItem | undefined, ownerName: string): string {
  const lines = [node.title, `Status: ${workGraphStatusWord(node.status)}`];
  if (ownerName) lines.push(`Owner: ${ownerName}`);
  if (node.alsoUnder.length > 0) lines.push(`Also under: ${node.alsoUnder.join(", ")}`);
  if (item?.description) lines.push(item.description);
  return lines.join("\n");
}

function moreItemsNotDrawn(count: number): string {
  return `+${count} more ${count === 1 ? "item" : "items"} not drawn`;
}

type NodeBox = Pick<WorkGraphLayoutNode, "itemId" | "x" | "y" | "w" | "h">;

const ARROW_STEPS: ReadonlyMap<string, readonly [number, number]> = new Map([
  ["ArrowRight", [1, 0]],
  ["ArrowLeft", [-1, 0]],
  ["ArrowDown", [0, 1]],
  ["ArrowUp", [0, -1]],
]);

/// Where a key moves focus in the graph, by layout position. An arrow goes
/// to the nearest node ahead of it, weighing distance across the arrow
/// double so a press keeps to its column or row; Home and End go to the
/// first and the last node, column by column. One pass over the nodes;
/// null when no node lies that way.
function nodeToward(nodes: ReadonlyArray<NodeBox>, fromId: string, key: string): string | null {
  if (key === "Home" || key === "End") {
    let best: NodeBox | null = null;
    for (const node of nodes) {
      const first = !best || node.x < best.x || (node.x === best.x && node.y < best.y);
      const last = !best || node.x > best.x || (node.x === best.x && node.y > best.y);
      if (key === "Home" ? first : last) best = node;
    }
    return best ? best.itemId : null;
  }
  const step = ARROW_STEPS.get(key);
  const from = step ? nodes.find((node) => node.itemId === fromId) : undefined;
  if (!step || !from) return null;
  const [ux, uy] = step;
  const fromX = from.x + from.w / 2;
  const fromY = from.y + from.h / 2;
  let bestId: string | null = null;
  let bestScore = Infinity;
  for (const node of nodes) {
    const dx = node.x + node.w / 2 - fromX;
    const dy = node.y + node.h / 2 - fromY;
    const ahead = dx * ux + dy * uy;
    if (ahead < 1) continue;
    const score = ahead + 2 * Math.abs(dx * uy - dy * ux);
    if (score < bestScore) {
      bestScore = score;
      bestId = node.itemId;
    }
  }
  return bestId;
}

/// A person by name with the raw key in the tooltip: the kit's AgentName
/// markup, for names the work graph resolves itself (a stored display name
/// is preferred over a bare key the directory does not know).
export function WorkGraphPerson({
  person,
  avatar,
  className,
}: {
  person: WorkGraphOwnerView;
  avatar?: AgentAvatarSize;
  className?: string;
}): React.JSX.Element {
  return (
    <span className={className ? `cc-agent ${className}` : "cc-agent"} title={person.key || undefined}>
      {avatar ? <AgentAvatar name={person.name} size={avatar} /> : null}
      <span className="cc-agent__name">{person.name}</span>
    </span>
  );
}

function ItemCopyIcon({ name }: { name: string }) {
  return <CopyGlyph state={name === "i-check" ? "copied" : "idle"} />;
}

function WorkItemDetails({ itemId, item, node, hasAttention }: {
  itemId: string;
  item: WorkGraphWireItem | undefined;
  node: WorkGraphLayoutNode | undefined;
  hasAttention: boolean;
}) {
  const directory = useAgentDirectory();
  const owner = item ? workGraphItemOwner(item, directory) : null;
  const status = item?.status;
  const priority = priorityTone(item?.priority);
  return <>
    <div className="workgraph-graph__detail-heading">
      <h4 className="workgraph-graph__detail-title">{item?.title || (item ? "Untitled work item" : "Work item unavailable in this snapshot")}</h4>
      {status ? (
        <span className="workgraph-graph__detail-status" data-status={status}>
          <Chip tone={statusTone(status)} dot>{workGraphStatusWord(status)}</Chip>
        </span>
      ) : null}
    </div>
    {item?.description ? <p className="workgraph-graph__detail-description">{item.description}</p> : null}
    <KeyValueList
      className="workgraph-graph__detail-facts"
      items={[
        owner ? { key: "owner", label: "Owner", value: <WorkGraphPerson person={owner} avatar={18} /> } : null,
        priority ? { key: "priority", label: "Priority", value: <Chip tone={priority} outline={priority === "crit"}>{humanizeEnum(item?.priority)}</Chip> } : null,
        node?.alsoUnder.length ? { key: "also-under", label: "Also under", value: node.alsoUnder.join(", ") } : null,
        item?.updated_at ? { key: "updated", label: "Updated", value: <RelativeTime value={item.updated_at} /> } : null,
      ]}
    />
    <TechnicalDetails
      className="workgraph-graph__metadata"
      summary="Item details"
      items={[
        {
          label: "ID",
          value: (
            <span className="cc-tech-id workgraph-graph__detail-id">
              <code>{itemId}</code>
              <CopyButton text={itemId} label="Copy work item ID" copiedLabel="Work item ID copied" Icon={ItemCopyIcon} />
            </span>
          ),
        },
        item?.labels?.length ? {
          label: "Labels",
          value: <span className="cc-chips">{item.labels.map((label) => <Chip key={label} mono>{label}</Chip>)}</span>,
        } : null,
        typeof item?.revision === "number" ? { label: "Revision", value: item.revision } : null,
        hasAttention ? { label: "Attention", value: "Bound to this item", plain: true } : null,
      ]}
    />
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
  const directory = useAgentDirectory();
  // One tab stop for the whole graph (a roving tab index): the node focused
  // last, else the selected node, else the first. Arrow keys move between
  // nodes, so a graph of any size costs one Tab press to cross.
  const [focusedId, setFocusedId] = React.useState<string | null>(null);
  const nodeIds = React.useMemo(() => new Set(layout.nodes.map((node) => node.itemId)), [layout]);

  if (layout.nodes.length === 0) {
    return <EmptyState inline title="No work items yet" />;
  }

  const ownerNameOf = (node: WorkGraphLayoutNode): string => {
    const item = itemById.get(node.itemId);
    return (item ? workGraphItemOwner(item, directory)?.name : "") || node.ownerLabel;
  };
  const selectedNode = selectedId ? layout.nodes.find((node) => node.itemId === selectedId) : undefined;
  const tabStopId = focusedId && nodeIds.has(focusedId)
    ? focusedId
    : selectedNode ? selectedNode.itemId : nodeToward(layout.nodes, "", "Home");
  const moveFocus = (event: React.KeyboardEvent<SVGGElement>, fromId: string) => {
    const targetId = nodeToward(layout.nodes, fromId, event.key);
    if (!targetId || targetId === fromId) return;
    const target = Array.from(
      event.currentTarget.ownerSVGElement?.querySelectorAll<SVGGElement>('[data-testid="workgraph-graph-node"]') ?? [],
    ).find((element) => element.getAttribute("data-item-id") === targetId);
    setFocusedId(targetId);
    target?.focus();
  };

  return (
    <div className="workgraph-graph" data-testid="workgraph-graph-frame">
      <div className="workgraph-graph__toolbar">
        <Button size="sm" title="Fit the graph to the frame" data-testid="workgraph-graph-fit" onClick={zoom.reset}>
          Fit
        </Button>
        <span className="workgraph-graph__stats">
          {joinMeta(plural(layout.nodes.length, "item"), plural(layout.edges.length, "link"))}
        </span>
        {layout.overflowCount > 0 ? (
          <span className="workgraph-graph__overflow" data-testid="workgraph-graph-overflow">
            {moreItemsNotDrawn(layout.overflowCount)}
          </span>
        ) : null}
        <span className="workgraph-graph__hint">Drag to pan, scroll to zoom</span>
      </div>
      <svg
        data-testid="workgraph-graph"
        className={`workgraph-graph__svg${zoom.isDragging ? " is-dragging" : ""}`}
        viewBox={`0 0 ${viewBoxWidth} ${viewBoxHeight}`}
        preserveAspectRatio="xMidYMid meet"
        data-scale={zoom.viewport.scale.toFixed(3)}
        // A group, not an image: its nodes are buttons people can reach.
        role="group"
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
            const ownerName = ownerNameOf(node);
            const metaText = nodeMetaText(node, ownerName);
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
                data-tone={statusTone(node.status)}
                className={`workgraph-graph__node is-${node.status}${selected ? " is-selected" : ""}`}
                transform={`translate(${node.x} ${node.y})`}
                role="button"
                tabIndex={node.itemId === tabStopId ? 0 : -1}
                aria-label={`${node.title}, ${workGraphStatusWord(node.status)}`}
                aria-current={selected ? "true" : undefined}
                // Select on pointerdown, not click: the svg's pan handler
                // takes pointer capture, which retargets the eventual click
                // to the svg and would swallow node selection entirely.
                // stopPropagation keeps a node press from starting a pan.
                onPointerDown={(event) => {
                  event.stopPropagation();
                  onSelect?.(node.itemId);
                }}
                onFocus={() => setFocusedId(node.itemId)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    onSelect?.(node.itemId);
                    return;
                  }
                  const navigation = ARROW_STEPS.has(event.key) || event.key === "Home" || event.key === "End";
                  if (!navigation || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
                  event.preventDefault();
                  moveFocus(event, node.itemId);
                }}
              >
                <title>
                  {nodeHoverText(node, itemById.get(node.itemId), ownerName)}
                </title>
                {selected ? (
                  <rect
                    className="workgraph-graph__node-ring"
                    x={-4}
                    y={-4}
                    width={node.w + 8}
                    height={node.h + 8}
                    rx={9}
                  />
                ) : null}
                <rect className="workgraph-graph__node-box" width={node.w} height={node.h} rx={6} />
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
                {workGraphEdgeWord(edge.kind)}
              </text>
            );
          })}
        </g>
      </svg>
      <div className="workgraph-graph__detail" data-testid="workgraph-graph-detail">
        {selectedId
          ? (
            <WorkItemDetails
              key={selectedId}
              itemId={selectedId}
              item={itemById.get(selectedId)}
              node={selectedNode}
              hasAttention={boundItemIds.has(selectedId)}
            />
          )
          : <p className="workgraph-graph__detail-hint">Select an item to see its details.</p>}
      </div>
    </div>
  );
}

export const __workGraphGraphViewTest = {
  fitLabel,
  fitViewport,
  FIT_MIN_SCALE,
  nodeMetaLine,
  nodeToward,
};
