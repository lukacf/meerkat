// The graph's chrome (design spec A3.9): zoom controls (bottom left), the
// legend with relation toggles above them, the minimap (bottom right) and
// the hover tooltip. Plain HTML over the canvas; colours from viz.css.

import React from "react";

import type { LayoutBounds } from "../layout/types";
import { CLUSTER_DOT } from "../layout/geometry";
import { MarkShape, chevronPath } from "./marks";
import { vizKindWord, vizRelationWord } from "./words";
import type { VizNodeKind, VizRelation, VizViewport, VizZoomAction } from "./types";

function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

/** The controls' glyphs, in the sprite's 24 px stroke style. */
function ControlGlyph({ name }: { name: "minus" | "plus" | "fit" | "minimap" | "collapse" }): React.JSX.Element {
  const paths: Record<typeof name, React.ReactNode> = {
    minus: <path d="M6 12h12" />,
    plus: <path d="M6 12h12M12 6v12" />,
    fit: <path d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5" />,
    minimap: (
      <>
        <rect x="3.5" y="5.5" width="17" height="13" rx="1.5" />
        <rect x="11" y="10" width="6.5" height="5" rx="0.5" />
      </>
    ),
    collapse: <path d="m7 10 5 5 5-5" />,
  };
  return (
    <svg className="cc-viz-ctl__glyph" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      {paths[name]}
    </svg>
  );
}

export interface ZoomControlsProps {
  scale: number;
  onZoom: (action: VizZoomAction) => void;
  minimap: { available: boolean; shown: boolean } | null;
  testIdOf?: (action: VizZoomAction) => string | undefined;
}

export function ZoomControls({ scale, onZoom, minimap, testIdOf }: ZoomControlsProps): React.JSX.Element {
  const percent = `${Math.round(scale * 100)}%`;
  return (
    <div className="cc-viz-controls" role="toolbar" aria-label="Zoom" aria-orientation="horizontal">
      <button type="button" className="cc-viz-ctl" aria-label="Zoom out" title="Zoom out (-)" data-testid={testIdOf?.("out")} onClick={() => onZoom("out")}>
        <ControlGlyph name="minus" />
      </button>
      <button
        type="button"
        className="cc-viz-ctl cc-viz-ctl--text"
        aria-label={`Zoom ${percent}, reset to 100%`}
        title="Reset to 100%"
        data-testid={testIdOf?.("reset")}
        onClick={() => onZoom("reset")}
      >
        {percent}
      </button>
      <button type="button" className="cc-viz-ctl" aria-label="Zoom in" title="Zoom in (+)" data-testid={testIdOf?.("in")} onClick={() => onZoom("in")}>
        <ControlGlyph name="plus" />
      </button>
      <button type="button" className="cc-viz-ctl" aria-label="Fit to view" title="Fit (0)" data-testid={testIdOf?.("fit")} onClick={() => onZoom("fit")}>
        <ControlGlyph name="fit" />
      </button>
      {minimap?.available ? (
        <button
          type="button"
          className="cc-viz-ctl"
          aria-label="Minimap"
          aria-pressed={minimap.shown}
          title={minimap.shown ? "Hide the minimap" : "Show the minimap"}
          data-testid={testIdOf?.("minimap")}
          onClick={() => onZoom("minimap")}
        >
          <ControlGlyph name="minimap" />
        </button>
      ) : null}
    </div>
  );
}

/** A 24 px sample of a relation's stroke. */
function RelationSample({ relation }: { relation: VizRelation }): React.JSX.Element {
  const directed = relation === "route" || relation === "lineage" || relation === "blocks";
  return (
    <svg className="cc-viz-legend__sample" viewBox="0 0 24 10" width="24" height="10" style={{ width: 24, height: 10 }} aria-hidden="true" focusable="false">
      <g className={`cc-viz-edge cc-viz-edge--${relation}`}>
        <path className="cc-viz-edge__line" d={directed ? "M1 5H20" : "M1 5H23"} />
        {relation === "cross" ? <path className="cc-viz-edge__gap" d="M1 5H23" /> : null}
        {directed ? <path className="cc-viz-edge__chevron" d={chevronPath(4.5)} transform="translate(22.5 5)" /> : null}
      </g>
    </svg>
  );
}

/** A mark sample for a node kind. */
function KindSample({ kind }: { kind: VizNodeKind }): React.JSX.Element {
  if (kind === "item" || kind === "goal") {
    return (
      <svg className="cc-viz-legend__sample" viewBox="0 0 24 14" width="24" height="14" style={{ width: 24, height: 14 }} aria-hidden="true" focusable="false">
        {kind === "item" ? (
          <rect className="cc-viz-card" x="3" y="2" width="18" height="10" rx="2" />
        ) : (
          <rect className="cc-viz-frame__box" x="2" y="2" width="20" height="10" rx="2" />
        )}
      </svg>
    );
  }
  return (
    <svg className="cc-viz-legend__sample" viewBox="-12 -7 24 14" width="24" height="14" style={{ width: 24, height: 14 }} aria-hidden="true" focusable="false">
      <g className={`cc-viz-node cc-viz-node--${kind}`}>
        <MarkShape kind={kind} r={kind === "store" ? 5.5 : kind === "coordinator" ? 4 : 4.5} />
      </g>
    </svg>
  );
}

export interface LegendProps {
  relations: ReadonlyMap<VizRelation, number>;
  kinds: ReadonlyMap<VizNodeKind, number>;
  hidden: ReadonlySet<VizRelation>;
  onToggle: (relation: VizRelation) => void;
  collapsed: boolean;
  onCollapsedChange: (collapsed: boolean) => void;
  testId?: string;
  itemTestId?: (key: string) => string | undefined;
  headingClassName?: string;
  /** Clusters: helpers and members are dots, so the kind samples say so. */
  dots?: boolean;
  /** One meta line under the rows. */
  note?: React.ReactNode;
  /** A narrow frame: the title and the fold button on a line of their own, the rows under them. */
  stacked?: boolean;
}

const RELATION_DISPLAY_ORDER: readonly VizRelation[] = ["wired", "traffic", "route", "lineage", "cross", "blocks", "parent", "related"];
const KIND_DISPLAY_ORDER: readonly VizNodeKind[] = ["person", "source", "coordinator", "agent", "helper", "external", "unknown", "store", "goal", "item"];

export function Legend(props: LegendProps): React.JSX.Element {
  const relations = RELATION_DISPLAY_ORDER.filter((relation) => props.relations.has(relation));
  const kinds = KIND_DISPLAY_ORDER.filter((kind) => props.kinds.has(kind));
  if (props.collapsed) {
    return (
      <div className="cc-viz-legend is-collapsed" data-testid={props.testId}>
        <button type="button" className="cc-viz-legend__chip" aria-expanded="false" onClick={() => props.onCollapsedChange(false)}>
          <span className={cx("cc-viz-legend__title", props.headingClassName)}>Legend</span>
        </button>
      </div>
    );
  }
  return (
    <div className={cx("cc-viz-legend", props.stacked && "is-stacked")} data-testid={props.testId} role="group" aria-label="Legend">
      <span className={cx("cc-viz-legend__title", props.headingClassName)}>Legend</span>
      <ul className="cc-viz-legend__list">
        {relations.map((relation) => {
          const count = props.relations.get(relation) ?? 0;
          const shown = !props.hidden.has(relation);
          return (
            <li key={relation}>
              <button
                type="button"
                className={cx("cc-viz-legend__row", !shown && "is-off")}
                aria-pressed={shown}
                title={shown ? `Hide ${vizRelationWord(relation, 2)}` : `Show ${vizRelationWord(relation, 2)}`}
                data-testid={props.itemTestId?.(relation)}
                data-relation={relation}
                onClick={() => props.onToggle(relation)}
              >
                <RelationSample relation={relation} />
                <span className="cc-viz-legend__word">{vizRelationWord(relation, count)}</span>
                <span className="cc-viz-legend__count">{count}</span>
              </button>
            </li>
          );
        })}
        {kinds.length > 0 && relations.length > 0 ? <li className="cc-viz-legend__rule" aria-hidden="true" /> : null}
        {kinds.map((kind) => {
          const count = props.kinds.get(kind) ?? 0;
          return (
            <li key={kind} className="cc-viz-legend__row is-static" data-testid={props.itemTestId?.(`kind:${kind}`)} data-kind={kind}>
              {props.dots && (kind === "agent" || kind === "helper") ? (
                <svg className="cc-viz-legend__sample" viewBox="-12 -7 24 14" width="24" height="14" style={{ width: 24, height: 14 }} aria-hidden="true" focusable="false">
                  <g className={`cc-viz-node cc-viz-node--${kind} is-dot`}>
                    <circle className={kind === "helper" ? "cc-viz-mark cc-viz-mark--hollow cc-viz-mark--dot" : "cc-viz-mark cc-viz-mark--fill cc-viz-mark--dot"} r={kind === "helper" ? CLUSTER_DOT.hollow + 0.6 : CLUSTER_DOT.filled + 0.6} />
                  </g>
                </svg>
              ) : (
                <KindSample kind={kind} />
              )}
              <span className="cc-viz-legend__word">{vizKindWord(kind, count)}</span>
              <span className="cc-viz-legend__count">{count}</span>
            </li>
          );
        })}
      </ul>
      {props.note ? <p className="cc-viz-legend__note">{props.note}</p> : null}
      <button type="button" className="cc-viz-legend__collapse" aria-label="Collapse legend" aria-expanded="true" onClick={() => props.onCollapsedChange(true)}>
        <ControlGlyph name="collapse" />
      </button>
    </div>
  );
}

export const MINIMAP = { width: 168, height: 112, pad: 6 } as const;

export interface MinimapProps {
  bounds: LayoutBounds;
  dots: readonly { x: number; y: number; live: boolean }[];
  viewport: VizViewport;
  frameWidth: number;
  frameHeight: number;
  /** Pans so the world point is at the frame's centre. */
  onCentre: (wx: number, wy: number) => void;
  /** Pans by a world delta (dragging the view rectangle). */
  onPan: (dwx: number, dwy: number) => void;
  testId?: string;
}

/** All nodes as 2 px dots (live in the accent), the view as an accent rectangle; drag it to pan, click to centre. */
export function Minimap(props: MinimapProps): React.JSX.Element {
  const { bounds, viewport } = props;
  const inner = { w: MINIMAP.width - 2 * MINIMAP.pad, h: MINIMAP.height - 2 * MINIMAP.pad };
  const k = bounds.w > 0 && bounds.h > 0 ? Math.min(inner.w / bounds.w, inner.h / bounds.h) : 1;
  const ox = MINIMAP.pad + (inner.w - bounds.w * k) / 2 - bounds.x * k;
  const oy = MINIMAP.pad + (inner.h - bounds.h * k) / 2 - bounds.y * k;
  const dotPath = (live: boolean) => {
    let d = "";
    for (const dot of props.dots) {
      if (dot.live !== live) continue;
      const x = Math.round((dot.x * k + ox) * 10) / 10;
      const y = Math.round((dot.y * k + oy) * 10) / 10;
      d += `M${x - 1} ${y}a1 1 0 1 0 2 0a1 1 0 1 0 -2 0`;
    }
    return d;
  };
  const view = {
    x: (-viewport.tx / viewport.scale) * k + ox,
    y: (-viewport.ty / viewport.scale) * k + oy,
    w: (props.frameWidth / viewport.scale) * k,
    h: (props.frameHeight / viewport.scale) * k,
  };
  const drag = React.useRef<{ id: number; x: number; y: number } | null>(null);
  const local = (event: React.PointerEvent<SVGSVGElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  };
  return (
    <div className="cc-viz-minimap" data-testid={props.testId}>
      <svg
        width={MINIMAP.width}
        height={MINIMAP.height}
        style={{ width: MINIMAP.width, height: MINIMAP.height }}
        viewBox={`0 0 ${MINIMAP.width} ${MINIMAP.height}`}
        role="img"
        aria-label="Overview of the whole graph"
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.stopPropagation();
          const point = local(event);
          const insideView = point.x >= view.x && point.x <= view.x + view.w && point.y >= view.y && point.y <= view.y + view.h;
          if (!insideView) props.onCentre((point.x - ox) / k, (point.y - oy) / k);
          event.currentTarget.setPointerCapture?.(event.pointerId);
          drag.current = { id: event.pointerId, x: point.x, y: point.y };
        }}
        onPointerMove={(event) => {
          const current = drag.current;
          if (!current || current.id !== event.pointerId) return;
          const point = local(event);
          const dx = point.x - current.x;
          const dy = point.y - current.y;
          current.x = point.x;
          current.y = point.y;
          if (dx !== 0 || dy !== 0) props.onPan(dx / k, dy / k);
        }}
        onPointerUp={(event) => {
          if (drag.current?.id === event.pointerId) drag.current = null;
        }}
        onPointerCancel={() => {
          drag.current = null;
        }}
      >
        <path className="cc-viz-minimap__dots" d={dotPath(false)} />
        <path className="cc-viz-minimap__live" d={dotPath(true)} />
        <rect
          className="cc-viz-minimap__view"
          x={Math.round(view.x * 10) / 10}
          y={Math.round(view.y * 10) / 10}
          width={Math.max(2, Math.round(view.w * 10) / 10)}
          height={Math.max(2, Math.round(view.h * 10) / 10)}
        />
      </svg>
    </div>
  );
}

export interface TooltipProps {
  lines: readonly string[];
  /** Anchor in the graph's own pixels; the tooltip sits above it (below near the top). */
  x: number;
  y: number;
  /** Space between the anchor and the tooltip. */
  gap: number;
  frameWidth: number;
  tone?: string;
}

export function GraphTooltip({ lines, x, y, gap, frameWidth, tone }: TooltipProps): React.JSX.Element | null {
  if (lines.length === 0) return null;
  const below = y < 72;
  const left = Math.max(8, Math.min(frameWidth - 8, x));
  return (
    <div
      className={cx("cc-viz-tooltip", below && "is-below")}
      role="tooltip"
      style={{ left, top: below ? y + gap : y - gap }}
      data-tone={tone}
    >
      <div className="cc-viz-tooltip__title">{lines[0]}</div>
      {lines.slice(1).map((line, index) => (
        <div key={index} className="cc-viz-tooltip__line">
          {line}
        </div>
      ))}
    </div>
  );
}
