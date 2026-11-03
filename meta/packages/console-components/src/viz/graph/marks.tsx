// The node grammar (design spec A3.9, from the brand's net.js), drawn in SVG
// around the node's centre. Colours come from viz.css classes, never from
// here; geometry is in screen pixels (marks keep their size under zoom).

import type { JSX } from "react";

import { markGeometry } from "../layout/geometry";
import type { VizItemStatus, VizMemoryState, VizNode, VizNodeKind } from "./types";

const ITEM_STATUSES: ReadonlySet<string> = new Set(["open", "ready", "in_progress", "blocked", "completed", "failed", "cancelled"]);

/** The status square a work item draws for a status word; any other word is open. */
export function itemStatusOf(status: string | undefined): VizItemStatus {
  const word = (status ?? "open").toLowerCase().replace(/[\s-]+/g, "_");
  if (ITEM_STATUSES.has(word)) return word as VizItemStatus;
  if (word === "done" || word === "complete" || word === "succeeded") return "completed";
  if (word === "claimed" || word === "running" || word === "working") return "in_progress";
  if (word === "error" || word === "errored") return "failed";
  return "open";
}

/** A point on a circle, angle in radians (0 is right, y down). */
function polar(r: number, angle: number): [number, number] {
  return [Math.round(Math.cos(angle) * r * 100) / 100, Math.round(Math.sin(angle) * r * 100) / 100];
}

/** The memory arc on the node's left, from 0.62 pi to 1.38 pi. */
export function memoryArcPath(radius: number): string {
  const [x0, y0] = polar(radius, Math.PI * 0.62);
  const [x1, y1] = polar(radius, Math.PI * 1.38);
  return `M${x0} ${y0}A${radius} ${radius} 0 0 1 ${x1} ${y1}`;
}

/** The radius the memory arc sits at for a mark. */
export function memoryArcRadius(kind: VizNodeKind, r: number): number {
  return r + (kind === "coordinator" ? 5.5 : 3);
}

const CYLINDER = { w: 14, h: 13, e: 2.4 };

export function cylinderPaths(scale = 1): { body: string; rim: string; band: string } {
  const hw = (CYLINDER.w / 2) * scale;
  const e = CYLINDER.e * scale;
  const top = -(CYLINDER.h / 2) * scale + e;
  const bottom = (CYLINDER.h / 2) * scale - e;
  return {
    body: `M${-hw} ${top}A${hw} ${e} 0 0 1 ${hw} ${top}L${hw} ${bottom}A${hw} ${e} 0 0 1 ${-hw} ${bottom}Z`,
    rim: `M${-hw} ${top}A${hw} ${e} 0 0 0 ${hw} ${top}`,
    band: `M${-hw} ${(top + bottom) / 2}A${hw} ${e} 0 0 0 ${hw} ${(top + bottom) / 2}`,
  };
}

/** The warn triangle and crit diamond badge at the mark's upper right. */
export function Badge({ kind, r }: { kind: "warn" | "crit"; r: number }): JSX.Element {
  const cx = Math.round((r * 0.78 + 2.2) * 10) / 10;
  const cy = -cx;
  if (kind === "warn") {
    return <path className="cc-viz-badge cc-viz-badge--warn" d={`M${cx} ${cy - 3.6}L${cx + 3.6} ${cy + 2.6}L${cx - 3.6} ${cy + 2.6}Z`} />;
  }
  return <path className="cc-viz-badge cc-viz-badge--crit" d={`M${cx} ${cy - 3.8}L${cx + 3.8} ${cy}L${cx} ${cy + 3.8}L${cx - 3.8} ${cy}Z`} />;
}

/** The mark itself, by kind. */
export function MarkShape({ kind, r }: { kind: VizNodeKind; r: number }): JSX.Element {
  switch (kind) {
    case "coordinator":
      return (
        <>
          <circle className="cc-viz-mark cc-viz-mark--fill" r={r} />
          <circle className="cc-viz-mark__ring" r={r + 3} />
        </>
      );
    case "helper":
      return <circle className="cc-viz-mark cc-viz-mark--hollow" r={r} />;
    case "person":
      return (
        <>
          <circle className="cc-viz-mark cc-viz-mark--person" r={r} />
          <circle className="cc-viz-mark__dot" r={Math.max(1.6, r * 0.36)} />
        </>
      );
    case "source": {
      const side = r * 2;
      return <rect className="cc-viz-mark cc-viz-mark--source" x={-r} y={-r} width={side} height={side} />;
    }
    case "store": {
      const paths = cylinderPaths(r / 7);
      return (
        <>
          <path className="cc-viz-mark cc-viz-mark--store" d={paths.body} />
          <path className="cc-viz-mark__rim" d={paths.rim} />
          <path className="cc-viz-mark__band" d={paths.band} />
        </>
      );
    }
    case "external":
      return (
        <>
          <circle className="cc-viz-mark cc-viz-mark--external" r={r} />
          <circle className="cc-viz-mark__dashed" r={r + 3} />
        </>
      );
    case "unknown":
      return <circle className="cc-viz-mark cc-viz-mark--unknown" r={r} />;
    case "agent":
    default:
      return <circle className="cc-viz-mark cc-viz-mark--fill" r={r} />;
  }
}

/** Agent kinds wear the live pulse; channels, stores and people only take the accent. */
const PULSE_KINDS: ReadonlySet<VizNodeKind> = new Set(["agent", "coordinator", "helper", "external", "unknown"]);

export interface GlyphState {
  live: boolean;
  memory: VizMemoryState;
  /** The arc's lit operation (a timed memory event), over `memory`. */
  memoryLit: "write" | "read" | "quarantine" | null;
  badge: "warn" | "crit" | null;
}

/** A mark with its overlays: pulse ring, memory arc, badge, selection and focus rings, hit target. */
export function MarkGlyph({ node, state, dot }: { node: VizNode; state: GlyphState; dot?: number }): JSX.Element {
  const geometry = dot ? { r: dot, outer: dot } : markGeometry(node.kind, node.size);
  const r = geometry.r;
  const memoryOp = state.memoryLit ?? (state.memory === "write" || state.memory === "read" || state.memory === "quarantine" ? state.memory : null);
  const arc = !dot && state.memory !== "none" ? memoryArcRadius(node.kind, r) : 0;
  const pulses = state.live && !dot && PULSE_KINDS.has(node.kind);
  const selectRing = geometry.outer + (dot ? 3 : pulses ? 13 : 5);
  return (
    <>
      <circle className="cc-viz-hit" r={dot ? Math.max(6, r + 3) : 12} />
      {pulses ? (
        <>
          <circle className="cc-viz-halo" r={r + 10} />
          <circle className="cc-viz-pulse" r={r + 5} />
          <circle className="cc-viz-pulse cc-viz-pulse--still" r={r + 4} />
          <circle className="cc-viz-pulse cc-viz-pulse--still" r={r + 6.5} />
        </>
      ) : null}
      {arc > 0 ? (
        <path
          className={`cc-viz-memory${memoryOp ? ` is-${memoryOp}` : ""}`}
          d={memoryArcPath(arc)}
        />
      ) : null}
      {arc > 0 && memoryOp === "write" ? <path className="cc-viz-memory__tick" d={`M${-arc + 1.5} 0L${-arc - 4.5} 0`} /> : null}
      {dot ? (
        node.kind === "helper" ? <circle className="cc-viz-mark cc-viz-mark--hollow cc-viz-mark--dot" r={r} /> : <circle className="cc-viz-mark cc-viz-mark--fill cc-viz-mark--dot" r={r} />
      ) : (
        <MarkShape kind={node.kind} r={r} />
      )}
      {state.badge && !dot ? <Badge kind={state.badge} r={geometry.outer} /> : null}
      <circle className="cc-viz-ring cc-viz-ring--select" r={selectRing} />
      <circle className="cc-viz-ring cc-viz-ring--focus" r={selectRing + 3} />
    </>
  );
}

/** The work item status square (A3.9 / B2), centred on the origin. */
export function StatusSquareShape({ status, size = 12, live = false }: { status: VizItemStatus; size?: number; live?: boolean }): JSX.Element {
  const half = size / 2;
  const inset = 0.75;
  const box = { x: -half + inset, y: -half + inset, width: size - 2 * inset, height: size - 2 * inset };
  const k = size / 12;
  const rx = 1.5 * k;
  switch (status) {
    case "ready":
      return (
        <g className="cc-viz-square is-ready">
          <rect className="cc-viz-square__box" {...box} rx={rx} />
          <circle className="cc-viz-square__dot" r={2 * k} />
        </g>
      );
    case "in_progress":
      return (
        <g className={`cc-viz-square is-in_progress${live ? " is-live" : ""}`}>
          {live ? <rect className="cc-viz-square__pulse" x={-half - 2.5} y={-half - 2.5} width={size + 5} height={size + 5} rx={rx + 2} /> : null}
          <rect className="cc-viz-square__box" x={-half} y={-half} width={size} height={size} rx={rx} />
        </g>
      );
    case "blocked":
      return (
        <g className="cc-viz-square is-blocked">
          <rect className="cc-viz-square__box" {...box} rx={rx} />
          <path className="cc-viz-square__mark" d={`M${-half + 2.2 * k} ${half - 2.2 * k}L${half - 2.2 * k} ${-half + 2.2 * k}`} />
        </g>
      );
    case "completed":
      return (
        <g className="cc-viz-square is-completed">
          <rect className="cc-viz-square__box" x={-half} y={-half} width={size} height={size} rx={rx} />
          <path className="cc-viz-square__mark" d={`M${-3 * k} ${0.2 * k}L${-0.9 * k} ${2.3 * k}L${3.2 * k} ${-2.2 * k}`} />
        </g>
      );
    case "failed":
      return (
        <g className="cc-viz-square is-failed">
          <rect className="cc-viz-square__box" x={-half} y={-half} width={size} height={size} rx={rx} />
          <path className="cc-viz-square__mark" d={`M${-2.4 * k} ${-2.4 * k}L${2.4 * k} ${2.4 * k}M${2.4 * k} ${-2.4 * k}L${-2.4 * k} ${2.4 * k}`} />
        </g>
      );
    case "cancelled":
      return (
        <g className="cc-viz-square is-cancelled">
          <rect className="cc-viz-square__box" x={-half} y={-half} width={size} height={size} rx={rx} />
        </g>
      );
    case "open":
    default:
      return (
        <g className="cc-viz-square is-open">
          <rect className="cc-viz-square__box" {...box} rx={rx} />
        </g>
      );
  }
}

/** The goal flag glyph (i-flag), 14 px tall, its pole at the origin's x. */
export function FlagGlyph(): JSX.Element {
  return <path className="cc-viz-goal__flag" d="M0 7V-7H10L8 -3.5L10 0H0" />;
}

/**
 * The A3.8 progress ring as SVG around the origin: a faint track, then done
 * (ink), in progress (accent) and blocked (warning) arcs from twelve o'clock
 * with 1 px gaps between them.
 */
export function ProgressRingShape({ done, inProgress = 0, blocked = 0, total, r = 6.5 }: {
  done: number;
  inProgress?: number;
  blocked?: number;
  total: number;
  r?: number;
}): JSX.Element {
  const circumference = 2 * Math.PI * r;
  const parts = ([
    [Math.max(0, done), "is-done"],
    [Math.max(0, inProgress), "is-live"],
    [Math.max(0, blocked), "is-blocked"],
  ] as const).filter(([count]) => count > 0);
  const whole = Math.max(total, parts.reduce((sum, [count]) => sum + count, 0), 1);
  const gap = parts.length > 1 ? 1 : 0;
  let offset = 0;
  return (
    <g className="cc-viz-ring-progress">
      <circle className="cc-viz-ring-progress__track" r={r} />
      {parts.map(([count, tone]) => {
        const length = Math.max(0.5, (count / whole) * circumference - gap);
        const arc = (
          <circle
            key={tone}
            className={`cc-viz-ring-progress__arc ${tone}`}
            r={r}
            strokeDasharray={`${Math.round(length * 100) / 100} ${Math.round(circumference * 100) / 100}`}
            strokeDashoffset={Math.round(-offset * 100) / 100}
            transform="rotate(-90)"
          />
        );
        offset += (count / whole) * circumference;
        return arc;
      })}
    </g>
  );
}

/** An open chevron pointing along +x, its tip at the origin. */
export function chevronPath(length: number): string {
  const half = length * 0.55;
  return `M${-length} ${-half}L0 0L${-length} ${half}`;
}
