// The SVG renderer (up to 300 nodes and 800 edges): frames, bundles, edges,
// particles, nodes, labels and edge labels inside the zoomed world group;
// lane rules and lane labels in screen space, so the labels stay at the
// left edge while the picture pans. Marks and labels sit in counter-scaled
// groups (`cc-viz-k`): they keep their size under zoom, hairlines stay 1 px.

import React from "react";

import { estimateLaneLabelWidth } from "../layout/geometry";
import type { ClusterBundle, LayoutPoint, PlacedCluster, PlacedFrame, PlacedLane } from "../layout/types";
import { bundleWidth } from "./edge-geometry";
import type { LabelPlacement } from "./labels";
import { FlagGlyph, MarkGlyph, ProgressRingShape, StatusSquareShape, chevronPath, itemStatusOf } from "./marks";
import type { GlyphState } from "./marks";
import type { GraphClassNames, VizAttributes, VizEdge, VizMemoryState, VizNode, VizNodeBox, VizProgress, VizViewport } from "./types";

function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

function attributesOf(attributes: VizAttributes | undefined): Record<string, string | number | boolean> {
  const result: Record<string, string | number | boolean> = {};
  if (!attributes) return result;
  for (const [key, value] of Object.entries(attributes)) {
    if (value !== undefined && key.startsWith("data-")) result[key] = value;
  }
  return result;
}

export interface NodeViewProps {
  node: VizNode;
  box: VizNodeBox;
  testId?: string;
  ariaName: string;
  tabStop: boolean;
  selected: boolean;
  lit: boolean;
  /** Chain highlight: downstream of the hovered node (mid emphasis). */
  downstream: boolean;
  flash: boolean;
  accent: boolean;
  exiting: boolean;
  entering: boolean;
  /** Glyph state as primitives, so a hover re-renders only the nodes it changes. */
  live: boolean;
  memory: VizMemoryState;
  memoryLit: GlyphState["memoryLit"];
  badge: GlyphState["badge"];
  /** Clusters: draw the member as a dot of this radius (geometric). */
  dot: number;
  classNames?: GraphClassNames;
  title: string;
  meta: string;
  /** Goal nodes: the header's parts, laid out (x from the header's left edge). */
  goal?: GoalHeaderLayout;
  decor?: React.ReactNode;
}

/** A goal header, laid out: title, progress ring, meta, and the right-aligned trailer. */
export interface GoalHeaderLayout {
  title: string;
  titleX: number;
  ring?: { cx: number; progress: VizProgress };
  meta?: { x: number; text: string };
  trailing?: { right: number; text: string; avatar?: string; avatarCx?: number };
  collapsed: boolean;
}

function nodeClass(props: NodeViewProps): string {
  const { node } = props;
  const item = node.kind === "item" ? itemStatusOf(node.status) : null;
  return cx(
    "cc-viz-node",
    `cc-viz-node--${node.kind}`,
    props.classNames?.node,
    node.className,
    props.selected && "is-selected",
    props.lit && "is-lit",
    props.downstream && "is-downstream",
    (props.flash || props.live) && node.kind !== "item" && node.kind !== "goal" && "is-hot",
    props.live && "is-live",
    props.flash && "is-flash",
    props.accent && "is-accent",
    node.retired && "is-retired",
    node.dim && "is-dim",
    props.exiting && "is-exiting",
    props.entering && "is-entering",
    props.dot > 0 && "is-dot",
    item && `cc-viz-item--${item}`,
    props.goal?.collapsed && "is-collapsed",
  );
}

/**
 * Where an item card's parts sit, in node-local coordinates (origin at the
 * card's centre): the status square's centre, the title and meta baselines.
 * renderNodeDecor reads it to draw beside them.
 */
export function itemCardGeometry(w: number, h: number, twoLines: boolean): {
  left: number;
  top: number;
  square: { x: number; y: number };
  titleX: number;
  titleY: number;
  metaY: number;
} {
  const left = -w / 2;
  const top = -h / 2;
  const comfortable = h >= 44;
  const titleY = twoLines ? top + (comfortable ? 19.5 : 15) : 4.5;
  return {
    left,
    top,
    square: { x: left + 15, y: twoLines ? titleY - 4.5 : 0 },
    titleX: left + 28,
    titleY,
    metaY: top + (comfortable ? 37 : 29.5),
  };
}

function ItemCardBody({ props }: { props: NodeViewProps }): React.JSX.Element {
  const { node, box } = props;
  const w = box.w;
  const h = box.h;
  const status = itemStatusOf(node.status);
  const twoLines = Boolean(props.meta);
  const { left, top, square, titleY, metaY } = itemCardGeometry(w, h, twoLines);
  return (
    <>
      <rect className="cc-viz-card" x={left} y={top} width={w} height={h} rx={4} />
      {props.accent ? <rect className="cc-viz-card__rule" x={left} y={top} width={2} height={h} /> : null}
      <g transform={`translate(${square.x} ${square.y})`}>
        <StatusSquareShape status={status} live={props.live} />
      </g>
      <text className={cx("cc-viz-card__title", props.classNames?.itemTitle)} x={left + 28} y={titleY}>
        {props.title}
      </text>
      {twoLines ? (
        <>
          {node.avatar ? (
            <g className="cc-viz-card__avatar" transform={`translate(${left + 17} ${metaY - 3.6})`}>
              <circle r={7} />
              <text y={3}>{node.avatar.slice(0, 1).toUpperCase()}</text>
            </g>
          ) : null}
          <text className={cx("cc-viz-card__meta", props.classNames?.itemMeta)} x={node.avatar ? left + 28 : left + 10} y={metaY}>
            {props.meta}
          </text>
        </>
      ) : null}
      <rect className="cc-viz-ring cc-viz-ring--select" x={left - 3.5} y={top - 3.5} width={w + 7} height={h + 7} rx={6} />
      <rect className="cc-viz-ring cc-viz-ring--focus" x={left - 6} y={top - 6} width={w + 12} height={h + 12} rx={8} />
    </>
  );
}

function GoalHeaderBody({ props }: { props: NodeViewProps }): React.JSX.Element {
  const { box } = props;
  const w = box.w;
  const h = box.h;
  const left = -w / 2;
  const goal = props.goal;
  return (
    <>
      <rect className="cc-viz-goal__hit" x={left} y={-h / 2} width={w} height={h} rx={6} />
      <g transform={`translate(${left + 14} 0)`}>
        <FlagGlyph />
      </g>
      <text className={cx("cc-viz-goal__title", props.classNames?.itemTitle)} x={left + (goal?.titleX ?? 32)} y={4.5}>
        {goal?.title ?? props.title}
      </text>
      {goal?.ring ? (
        <g transform={`translate(${Math.round((left + goal.ring.cx) * 10) / 10} 0)`}>
          <ProgressRingShape {...goal.ring.progress} />
        </g>
      ) : null}
      {goal?.meta ? (
        <text className={cx("cc-viz-goal__meta", props.classNames?.itemMeta)} x={Math.round((left + goal.meta.x) * 10) / 10} y={4}>
          {goal.meta.text}
        </text>
      ) : null}
      {goal?.trailing ? (
        <g className="cc-viz-goal__trailing">
          {goal.trailing.avatar && goal.trailing.avatarCx !== undefined ? (
            <g className="cc-viz-card__avatar" transform={`translate(${Math.round((left + goal.trailing.avatarCx) * 10) / 10} 0)`}>
              <circle r={8} />
              <text y={3.3}>{goal.trailing.avatar.slice(0, 1).toUpperCase()}</text>
            </g>
          ) : null}
          <text x={Math.round((left + goal.trailing.right) * 10) / 10} y={4} textAnchor="end">
            {goal.trailing.text}
          </text>
        </g>
      ) : null}
      <rect className="cc-viz-ring cc-viz-ring--select" x={left + 2} y={-h / 2 + 2} width={w - 4} height={h - 4} rx={5} />
      <rect className="cc-viz-ring cc-viz-ring--focus" x={left - 3} y={-h / 2 - 3} width={w + 6} height={h + 6} rx={8} />
    </>
  );
}

/** One node: a mark (counter-scaled), a cluster dot, an item card or a goal header. */
export const NodeView = React.memo(function NodeView(props: NodeViewProps): React.JSX.Element {
  const { node, box } = props;
  const card = node.kind === "item" || node.kind === "goal";
  const interactive = !props.exiting;
  return (
    <g
      className={nodeClass(props)}
      transform={`translate(${Math.round(box.x * 10) / 10} ${Math.round(box.y * 10) / 10})`}
      data-node-id={node.id}
      data-kind={node.kind}
      data-tone={node.live ? "live" : node.tone}
      data-live={props.live ? "true" : undefined}
      data-selected={props.selected ? "true" : undefined}
      data-testid={props.testId}
      role={interactive ? "button" : undefined}
      tabIndex={interactive ? (props.tabStop ? 0 : -1) : undefined}
      aria-label={interactive ? props.ariaName : undefined}
      aria-pressed={interactive ? props.selected : undefined}
      aria-hidden={interactive ? undefined : true}
      {...attributesOf(node.attributes)}
    >
      {card ? (
        <g className="cc-viz-glyph cc-viz-glyph--card">
          {node.kind === "item" ? <ItemCardBody props={props} /> : <GoalHeaderBody props={props} />}
          {props.decor}
        </g>
      ) : (
        <g className={props.dot > 0 ? "cc-viz-glyph" : "cc-viz-glyph cc-viz-k"}>
          <MarkGlyph node={node} state={{ live: props.live, memory: props.memory, memoryLit: props.memoryLit, badge: props.badge }} dot={props.dot || undefined} />
          {props.decor}
        </g>
      )}
    </g>
  );
});

export interface EdgeGeometry {
  d: string;
  /** Where the chevron sits (the end) and the direction of travel there. */
  end: LayoutPoint;
  endAngle: number;
  mid: LayoutPoint;
  midAngle: number;
  /** Straight edges between marks: the chevron backs off the target mark by this many screen px. */
  backoff: number;
  /** Parallel offset as a unit normal times screen px. */
  ox: number;
  oy: number;
  /** Screen length at zoom 1. */
  length: number;
}

/** The midpoint mark an edge draws: its own, else warn when declared and crit when failed. */
export function edgeMark(edge: VizEdge): "warn" | "crit" | null {
  if (edge.mark !== undefined) return edge.mark;
  if (edge.state === "failed") return "crit";
  if (edge.state === "declared") return "warn";
  return null;
}

export interface EdgeViewProps {
  edge: VizEdge;
  geometry: EdgeGeometry;
  testId?: string;
  directed: boolean;
  hot: boolean;
  downstream: boolean;
  accent: boolean;
  boosted: boolean;
  selected: boolean;
  className?: string;
}

/** One edge in the A3.9 grammar: stroke by relation and state, chevron at the recipient, marks at the midpoint. */
export const EdgeView = React.memo(function EdgeView(props: EdgeViewProps): React.JSX.Element {
  const { edge, geometry } = props;
  const state = edge.state ?? "normal";
  const style: React.CSSProperties & Record<string, string | number> = {};
  if (geometry.ox !== 0 || geometry.oy !== 0) {
    style["--_viz-ox"] = Math.round(geometry.ox * 100) / 100;
    style["--_viz-oy"] = Math.round(geometry.oy * 100) / 100;
  }
  if (edge.relation === "traffic") {
    const count = edge.weight ?? 1;
    style["--_viz-w"] = count >= 10 ? 2 : count >= 3 ? 1.5 : 1;
  }
  const chevron = edge.relation === "blocks" ? 8 : edge.relation === "lineage" ? 4.5 : 6;
  const mark = edgeMark(edge);
  const deg = (angle: number) => Math.round((angle * 180) / Math.PI * 10) / 10;
  return (
    <g
      className={cx(
        "cc-viz-edge",
        `cc-viz-edge--${edge.relation}`,
        state !== "normal" && `is-${state}`,
        props.className,
        edge.className,
        props.hot && "is-hot",
        props.downstream && "is-downstream",
        props.accent && "is-accent",
        props.boosted && "is-boosted",
        props.selected && "is-selected",
        (geometry.ox !== 0 || geometry.oy !== 0) && "is-offset",
      )}
      style={style}
      data-edge-id={edge.id}
      data-relation={edge.relation}
      data-state={state}
      data-testid={props.testId}
      {...attributesOf(edge.attributes)}
    >
      <path className="cc-viz-edge__hit" d={geometry.d} />
      <path className="cc-viz-edge__line" d={geometry.d} />
      {edge.relation === "cross" ? <path className="cc-viz-edge__gap" d={geometry.d} /> : null}
      {props.directed ? (
        <g transform={`translate(${Math.round(geometry.end.x * 10) / 10} ${Math.round(geometry.end.y * 10) / 10}) rotate(${deg(geometry.endAngle)})`}>
          <g className="cc-viz-k">
            <path className="cc-viz-edge__chevron" d={chevronPath(chevron)} transform={geometry.backoff ? `translate(${-geometry.backoff} 0)` : undefined} />
          </g>
        </g>
      ) : null}
      {mark ? (
        <g transform={`translate(${Math.round(geometry.mid.x * 10) / 10} ${Math.round(geometry.mid.y * 10) / 10})`}>
          <g className="cc-viz-k">
            {mark === "crit" ? (
              <path className="cc-viz-edge__mark is-crit" d="M0 -3.5L3.5 0L0 3.5L-3.5 0Z" />
            ) : (
              <path className="cc-viz-edge__mark is-warn" d="M0 -3.6L3.6 2.6L-3.6 2.6Z" />
            )}
          </g>
        </g>
      ) : null}
    </g>
  );
});

export interface NodeLabelProps {
  id: string;
  text: string;
  x: number;
  y: number;
  placement: LabelPlacement;
  hot: boolean;
  secondary: boolean;
  strong: boolean;
  lit: boolean;
  retired: boolean;
}

/** A node label on its halo, counter-scaled at the node's centre. */
export const NodeLabel = React.memo(function NodeLabel(props: NodeLabelProps): React.JSX.Element {
  return (
    <g
      className={cx("cc-viz-label", props.hot && "is-hot", props.secondary && "is-secondary", props.strong && "is-strong", props.lit && "is-lit", props.retired && "is-retired")}
      transform={`translate(${Math.round(props.x * 10) / 10} ${Math.round(props.y * 10) / 10})`}
      data-label-for={props.id}
      aria-hidden="true"
    >
      <g className="cc-viz-k">
        <text x={props.placement.dx} y={props.placement.dy} textAnchor={props.placement.anchor}>
          {props.text}
        </text>
      </g>
    </g>
  );
});

export interface EdgeLabelProps {
  id: string;
  text: string;
  x: number;
  y: number;
  /** Screen px above the line (near-horizontal edges keep their line whole). */
  lift: number;
  testId?: string;
  className?: string;
  hot: boolean;
}

/** An edge's word or count at its midpoint, on a card-coloured halo. */
export const EdgeLabel = React.memo(function EdgeLabel(props: EdgeLabelProps): React.JSX.Element {
  return (
    <g
      className={cx("cc-viz-edge-label", props.className, props.hot && "is-hot")}
      transform={`translate(${Math.round(props.x * 10) / 10} ${Math.round(props.y * 10) / 10})`}
      data-edge-label-for={props.id}
      data-testid={props.testId}
    >
      <g className="cc-viz-k">
        <text y={3.5 - props.lift} textAnchor="middle">
          {props.text}
        </text>
      </g>
    </g>
  );
});

export interface FrameViewProps {
  frame: PlacedFrame;
  /** The goal node draws the header itself. */
  hasHeaderNode: boolean;
  label: string;
  testId?: string;
  className?: string;
  attributes?: VizAttributes;
}

/** A frame: a dashed rectangle, its label inside the top left unless a goal node is its header. */
export const FrameView = React.memo(function FrameView({ frame, hasHeaderNode, label, testId, className, attributes }: FrameViewProps): React.JSX.Element {
  return (
    <g
      className={cx("cc-viz-frame", frame.collapsed && "is-collapsed", `is-depth-${frame.depth}`, className)}
      data-frame-id={frame.id}
      data-testid={testId}
      {...attributesOf(attributes)}
    >
      <rect className="cc-viz-frame__box" x={frame.x} y={frame.y} width={frame.w} height={frame.h} rx={6} />
      {hasHeaderNode ? null : (
        <text className="cc-viz-frame__label" x={frame.x + 14} y={frame.y + frame.header / 2 + 3.6}>
          {label}
        </text>
      )}
    </g>
  );
});

/** Clusters: one line per group pair, then each group's label and count under its rings. */
export function ClustersLayer({ clusters, bundles, hot }: {
  clusters: readonly PlacedCluster[];
  bundles: readonly ClusterBundle[];
  hot: ReadonlySet<string>;
}): React.JSX.Element {
  const byId = new Map(clusters.map((cluster) => [cluster.id, cluster]));
  const at = (value: number) => Math.round(value * 10) / 10;
  return (
    <g className="cc-viz-clusters">
      {bundles.map((bundle) => {
        const a = byId.get(bundle.from);
        const b = byId.get(bundle.to);
        if (!a || !b) return null;
        // Touching groups need no line.
        if (Math.hypot(b.cx - a.cx, b.cy - a.cy) <= a.r + b.r + 8) return null;
        const { x0, y0, cx: qx, cy: qy, x1, y1, bowed } = bundle.route;
        return (
          <path
            key={bundle.id}
            className={cx("cc-viz-bundle", (hot.has(bundle.from) || hot.has(bundle.to)) && "is-hot")}
            d={bowed ? `M${at(x0)} ${at(y0)}Q${at(qx)} ${at(qy)} ${at(x1)} ${at(y1)}` : `M${at(x0)} ${at(y0)}L${at(x1)} ${at(y1)}`}
            style={{ strokeWidth: bundleWidth(bundle.count) }}
            data-bundle={`${bundle.from}:${bundle.to}`}
            data-count={bundle.count}
          />
        );
      })}
      {clusters.map((cluster) => {
        const label = cluster.label ? cluster.label.toUpperCase() : "";
        // A plate in the background colour: bundles pass under the label, not through it.
        const plate = Math.max(estimateLaneLabelWidth(label), 8 * String(cluster.count).length) + 12;
        return (
          <g key={cluster.id} className="cc-viz-cluster" transform={`translate(${Math.round(cluster.cx)} ${Math.round(cluster.cy + cluster.r + 14)})`}>
            <g className="cc-viz-k">
              <rect className="cc-viz-cluster__plate" x={-plate / 2} y={-11} width={plate} height={30} rx={3} />
              <text className="cc-viz-cluster__label" textAnchor="middle" y={0}>
                {label}
              </text>
              <text className="cc-viz-cluster__count" textAnchor="middle" y={13}>
                {cluster.count}
              </text>
            </g>
          </g>
        );
      })}
    </g>
  );
}

/**
 * Lane rules across the frame (drawn under the picture) or the lane labels
 * pinned to its left edge (drawn over it), in screen space.
 */
export function LanesLayer({ part, lanes, viewport, width, compact, testIdOf }: {
  part: "rules" | "labels";
  lanes: readonly PlacedLane[];
  viewport: VizViewport;
  width: number;
  compact: boolean;
  testIdOf?: (lane: PlacedLane) => string | undefined;
}): React.JSX.Element {
  if (part === "rules") {
    return (
      <g className="cc-viz-lanes" aria-hidden="true">
        {lanes.map((lane, index) => {
          const top = lane.y * viewport.scale + viewport.ty;
          return (
            <g key={lane.id} className="cc-viz-lane" data-lane-id={lane.id} data-testid={testIdOf?.(lane)}>
              {index > 0 ? <path className="cc-viz-lane__rule" d={`M0 ${Math.round(top) + 0.5}H${Math.max(0, Math.round(width))}`} /> : null}
            </g>
          );
        })}
      </g>
    );
  }
  return (
    <g className={cx("cc-viz-lane-labels", compact && "is-compact")} aria-hidden="true">
      {lanes.map((lane) => {
        if (!lane.label) return null;
        const top = lane.y * viewport.scale + viewport.ty;
        const y = compact ? top + 14 : lane.labelY * viewport.scale + viewport.ty + 3.5;
        return (
          <text key={lane.id} className="cc-viz-lane__label" x={16} y={Math.round(y)}>
            {lane.label.toUpperCase()}
          </text>
        );
      })}
    </g>
  );
}

/** The particle pool: the animation loop moves these without a React render. */
export const ParticlePool = React.forwardRef<SVGGElement, { size: number }>(function ParticlePool({ size }, ref) {
  return (
    <g className="cc-viz-particles" ref={ref} aria-hidden="true">
      {Array.from({ length: size }, (_, index) => (
        <g key={index} className="cc-viz-particle" style={{ display: "none" }}>
          <line className="cc-viz-particle__trail" />
          <circle className="cc-viz-particle__dot" r={2.3} />
        </g>
      ))}
    </g>
  );
});

/** Overflow messages on an edge: a small count at its midpoint. */
export function EdgeBadge({ x, y, count }: { x: number; y: number; count: number }): React.JSX.Element {
  const text = `+${count}`;
  const width = 8 + text.length * 6;
  return (
    <g className="cc-viz-edge-badge" transform={`translate(${Math.round(x * 10) / 10} ${Math.round(y * 10) / 10})`} aria-hidden="true">
      <g className="cc-viz-k">
        <rect x={-width / 2} y={-7.5} width={width} height={15} rx={7.5} />
        <text y={3.5} textAnchor="middle">
          {text}
        </text>
      </g>
    </g>
  );
}
