import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  type RefObject,
} from "react";

import { ChartTooltip } from "../../panel-kit/chart-tooltip";
import { cx } from "../../panel-kit/cx";
import { StatusMark } from "../../panel-kit/status-mark";
import type { Tone } from "../../panel-kit/tone";
import { createTimeScale, nowMarker } from "./time-axis";

// Day swimlanes (console v2 spec B4): one lane per agent on a shared time
// axis. Turns are span bars (ink when done, the accent while open, the
// negative ink when failed) with their tool calls as 1 px ticks; a message
// is a vertical connector between two lanes with a chevron at the
// recipient; approvals are hollow diamonds, lifecycle events small squares;
// an optional PROBLEMS lane on top lines up the warnings and failures in
// time. SVG, one tab stop with arrow keys, a tooltip per mark, and every
// mark named in words. A model and callback renderer: the panel builds the
// lanes and handles selection.

export interface SwimlaneLane {
  id: string;
  label: ReactNode;
  /** The label in words (tooltip and the lane's accessible name). */
  title?: string;
  /** Working now: a live mark before the label. */
  live?: boolean;
  /** The folded lane of quieter agents: spans draw as ticks. */
  folded?: boolean;
  /** Makes the label a button (open the agent). */
  onActivate?: () => void;
  activateLabel?: string;
}

export type SwimlaneSpanState = "done" | "live" | "failed" | "cancelled";

export interface SwimlaneSpan {
  id: string;
  lane: string;
  start: number;
  /** Absent while the span runs: it reaches `now` (or the end). */
  end?: number;
  state: SwimlaneSpanState;
  /** Times of the 1 px ticks inside the bar (tool calls). */
  ticks?: readonly number[];
  /** The span in words. */
  label: string;
}

export interface SwimlaneLink {
  id: string;
  from: string;
  to: string;
  at: number;
  label: string;
}

export type SwimlaneMarkShape = "diamond" | "triangle" | "hollow-diamond" | "square" | "dot";

export interface SwimlaneMark {
  id: string;
  lane: string;
  at: number;
  shape: SwimlaneMarkShape;
  tone?: Tone;
  label: string;
}

export type SwimlaneItem =
  | { kind: "span"; id: string; span: SwimlaneSpan }
  | { kind: "link"; id: string; link: SwimlaneLink; count: number }
  | { kind: "mark"; id: string; mark: SwimlaneMark }
  | { kind: "problem"; id: string; mark: SwimlaneMark };

const PROBLEMS = "__problems";
const AXIS_H = 20;
const HIT_PX = 6;
const MIN_SPAN_W = 3;

interface Placed {
  item: SwimlaneItem;
  lane: string;
  /** Sort time. */
  t: number;
  x: number;
  y: number;
}

function useElementWidth(fallback: number): [RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(fallback);
  useEffect(() => {
    const element = ref.current;
    if (!element) return undefined;
    const read = () => {
      const next = Math.round(element.getBoundingClientRect().width);
      if (next > 0) setWidth((current) => (Math.abs(current - next) >= 1 ? next : current));
    };
    read();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(read);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

function markPath(shape: SwimlaneMarkShape, x: number, y: number, size: number): string {
  const h = size / 2;
  switch (shape) {
    case "diamond":
    case "hollow-diamond":
      return `M${x} ${y - h}L${x + h} ${y}L${x} ${y + h}L${x - h} ${y}Z`;
    case "triangle":
      return `M${x} ${y - h}L${x + h} ${y + h * 0.8}L${x - h} ${y + h * 0.8}Z`;
    case "square":
      return `M${x - h * 0.8} ${y - h * 0.8}h${size * 0.8}v${size * 0.8}h${-size * 0.8}Z`;
    default:
      return `M${x - h} ${y}a${h} ${h} 0 1 0 ${size} 0a${h} ${h} 0 1 0 ${-size} 0`;
  }
}

/**
 * Agents' lanes over a time window. `onSelect` receives the span, link or
 * mark clicked or chosen with Enter; `selectedId` rings the selected one.
 * Test ids from `testIdPrefix` "p": root `p-swimlanes`, lane `p-lane:<id>`,
 * span `p-span:<id>`, link `p-link:<id>`, mark `p-mark:<id>`, problem
 * `p-problem:<id>`.
 */
export function Swimlanes({
  lanes,
  spans = [],
  links = [],
  marks = [],
  problems,
  problemsLabel = "Problems",
  start,
  end,
  now,
  label,
  labelWidth = 136,
  laneHeight = 28,
  defaultWidth = 720,
  onSelect,
  selectedId,
  testIdPrefix,
  className,
  testId,
}: {
  lanes: ReadonlyArray<SwimlaneLane>;
  spans?: ReadonlyArray<SwimlaneSpan>;
  links?: ReadonlyArray<SwimlaneLink>;
  marks?: ReadonlyArray<SwimlaneMark>;
  /** Marks of the PROBLEMS lane on top; the lane shows when any are given. */
  problems?: ReadonlyArray<SwimlaneMark>;
  problemsLabel?: string;
  start: number;
  end: number;
  /** The "now" marker, and where open spans end. */
  now?: number;
  /** The chart in words ("6 agents, 46 turns, 2 problems"). */
  label: string;
  labelWidth?: number;
  laneHeight?: number;
  /** The width before the first measure (and without ResizeObserver). */
  defaultWidth?: number;
  onSelect?: (item: SwimlaneItem) => void;
  selectedId?: string | null;
  testIdPrefix?: string;
  className?: string;
  testId?: string;
}) {
  const [rootRef, measured] = useElementWidth(defaultWidth);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [focusId, setFocusId] = useState<string | null>(null);
  const tid = (part: string, id?: string) => (testIdPrefix ? `${testIdPrefix}-${part}${id === undefined ? "" : `:${id}`}` : undefined);

  const showProblems = Boolean(problems && problems.length > 0);
  const rows = useMemo(() => {
    const list: Array<{ id: string; lane: SwimlaneLane | null }> = [];
    if (showProblems) list.push({ id: PROBLEMS, lane: null });
    for (const lane of lanes) list.push({ id: lane.id, lane });
    return list;
  }, [lanes, showProblems]);
  const problemsH = Math.round(laneHeight * 0.8);
  const rowTop = useMemo(() => {
    const tops = new Map<string, number>();
    let y = AXIS_H;
    for (const row of rows) {
      tops.set(row.id, y);
      y += row.id === PROBLEMS ? problemsH : laneHeight;
    }
    return tops;
  }, [rows, laneHeight, problemsH]);
  const height = AXIS_H + rows.reduce((sum, row) => sum + (row.id === PROBLEMS ? problemsH : laneHeight), 0) + 2;
  const rowCenter = (id: string) => (rowTop.get(id) ?? AXIS_H) + (id === PROBLEMS ? problemsH : laneHeight) / 2;
  const plotW = Math.max(120, measured - labelWidth);
  const scale = createTimeScale({ start, end, range: [8, plotW - 8] });
  const nowMs = now ?? end;
  const marker = nowMarker(scale, nowMs);
  const ticks = scale.ticks({ count: Math.max(2, Math.floor(plotW / 96)) }).filter((tick) => tick.x > 14 && tick.x < plotW - (marker ? 34 : 14));
  const laneIds = useMemo(() => new Set(lanes.map((lane) => lane.id)), [lanes]);
  const folded = useMemo(() => new Set(lanes.filter((lane) => lane.folded).map((lane) => lane.id)), [lanes]);

  const placed = useMemo<Placed[]>(() => {
    const out: Placed[] = [];
    for (const span of spans) {
      if (!laneIds.has(span.lane)) continue;
      out.push({ item: { kind: "span", id: span.id, span }, lane: span.lane, t: span.start, x: scale.x(span.start), y: rowCenter(span.lane) });
    }
    // Messages between one pair in one pixel column (a burst) draw once,
    // with their count.
    const bursts = new Map<string, { link: SwimlaneLink; count: number }>();
    for (const link of links) {
      if (!laneIds.has(link.from) || !laneIds.has(link.to)) continue;
      const key = `${link.from}\u0000${link.to}\u0000${Math.round(scale.x(link.at) / 4)}`;
      const burst = bursts.get(key);
      if (burst) burst.count += 1;
      else bursts.set(key, { link, count: 1 });
    }
    for (const { link, count } of bursts.values()) {
      out.push({ item: { kind: "link", id: link.id, link, count }, lane: link.from, t: link.at, x: scale.x(link.at), y: rowCenter(link.from) });
    }
    for (const mark of marks) {
      if (!laneIds.has(mark.lane)) continue;
      out.push({ item: { kind: "mark", id: mark.id, mark }, lane: mark.lane, t: mark.at, x: scale.x(mark.at), y: rowCenter(mark.lane) });
    }
    if (showProblems) {
      for (const mark of problems ?? []) {
        out.push({ item: { kind: "problem", id: mark.id, mark }, lane: PROBLEMS, t: mark.at, x: scale.x(mark.at), y: rowCenter(PROBLEMS) });
      }
    }
    const order = new Map(rows.map((row, index) => [row.id, index]));
    return out.sort((a, b) => (order.get(a.lane)! - order.get(b.lane)!) || a.t - b.t || (a.item.id < b.item.id ? -1 : 1));
    // The scale and centres follow from these inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [spans, links, marks, problems, showProblems, laneIds, rows, start, end, plotW, laneHeight]);
  const byId = useMemo(() => new Map(placed.map((entry) => [entry.item.id, entry])), [placed]);
  const tabStop = focusId && byId.has(focusId) ? focusId : placed[0]?.item.id ?? null;

  const spanEnd = (span: SwimlaneSpan) => scale.x(span.end ?? Math.min(nowMs, end));
  const itemLabel = (item: SwimlaneItem) => (item.kind === "span"
    ? item.span.label
    : item.kind === "link"
      ? (item.count > 1 ? `${item.count} messages. ${item.link.label}` : item.link.label)
      : item.mark.label);

  const hitTest = (clientX: number, clientY: number): string | null => {
    const box = svgRef.current?.getBoundingClientRect();
    if (!box) return null;
    const x = clientX - box.left;
    const y = clientY - box.top;
    let best: string | null = null;
    let bestDistance = Infinity;
    for (const entry of placed) {
      const { item } = entry;
      let distance = Infinity;
      if (item.kind === "span" && !folded.has(entry.lane)) {
        const x0 = entry.x;
        const x1 = Math.max(x0 + MIN_SPAN_W, spanEnd(item.span));
        const dy = Math.abs(y - entry.y);
        if (dy <= laneHeight / 2) distance = x < x0 ? x0 - x : x > x1 ? x - x1 : 0;
        distance += dy / 4;
      } else if (item.kind === "link") {
        const y0 = Math.min(rowCenter(item.link.from), rowCenter(item.link.to));
        const y1 = Math.max(rowCenter(item.link.from), rowCenter(item.link.to));
        if (y >= y0 - 4 && y <= y1 + 4) distance = Math.abs(x - entry.x) + 1;
      } else {
        const dy = Math.abs(y - entry.y);
        if (dy <= laneHeight / 2) distance = Math.abs(x - entry.x) + dy / 4;
      }
      if (distance < bestDistance) {
        bestDistance = distance;
        best = item.id;
      }
    }
    return bestDistance <= HIT_PX ? best : null;
  };

  const focusItem = (id: string | null) => {
    if (!id) return;
    setFocusId(id);
    setActive(id);
    const element = Array.from(svgRef.current?.querySelectorAll<SVGElement>("[data-item]") ?? [])
      .find((candidate) => candidate.getAttribute("data-item") === id);
    element?.focus({ preventScroll: true });
  };

  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    if (!placed.length) return;
    const current = byId.get(tabStop ?? "") ?? placed[0];
    const sameLane = placed.filter((entry) => entry.lane === current.lane);
    const index = sameLane.indexOf(current);
    let next: Placed | undefined;
    switch (event.key) {
      case "ArrowRight": next = sameLane[Math.min(sameLane.length - 1, index + 1)]; break;
      case "ArrowLeft": next = sameLane[Math.max(0, index - 1)]; break;
      case "Home": next = sameLane[0]; break;
      case "End": next = sameLane[sameLane.length - 1]; break;
      case "ArrowDown":
      case "ArrowUp": {
        const rowIndex = rows.findIndex((row) => row.id === current.lane);
        const step = event.key === "ArrowDown" ? 1 : -1;
        for (let r = rowIndex + step; r >= 0 && r < rows.length; r += step) {
          const candidates = placed.filter((entry) => entry.lane === rows[r].id);
          if (!candidates.length) continue;
          next = candidates.reduce((best, entry) => (Math.abs(entry.t - current.t) < Math.abs(best.t - current.t) ? entry : best));
          break;
        }
        break;
      }
      case "Enter":
      case " ":
        if (onSelect) {
          event.preventDefault();
          onSelect(current.item);
        }
        return;
      case "Escape":
        setActive(null);
        return;
      default:
        return;
    }
    event.preventDefault();
    if (next) focusItem(next.item.id);
  };

  const common = (item: SwimlaneItem, testKind: string) => ({
    "data-item": item.id,
    "data-kind": item.kind,
    "data-testid": tid(testKind, item.id),
    className: cx("cc-lanes__item", active === item.id && "is-active", selectedId === item.id && "is-selected"),
    role: onSelect ? "button" : "img",
    "aria-label": itemLabel(item),
    tabIndex: tabStop === item.id ? 0 : -1,
    onFocus: () => {
      setFocusId(item.id);
      setActive(item.id);
    },
  });

  const activeEntry = active ? byId.get(active) : undefined;
  const tooltipX = activeEntry ? labelWidth + (activeEntry.item.kind === "span" ? (activeEntry.x + Math.max(activeEntry.x + MIN_SPAN_W, spanEnd(activeEntry.item.span))) / 2 : activeEntry.x) : 0;
  const tooltipY = activeEntry ? (activeEntry.item.kind === "link" ? Math.min(rowCenter(activeEntry.item.link.from), rowCenter(activeEntry.item.link.to)) : activeEntry.y) - 6 : 0;

  return (
    <div
      ref={rootRef}
      className={cx("cc-lanes", onSelect && "is-selectable", className)}
      data-testid={testId ?? tid("swimlanes")}
      style={{ height, "--_cc-lanes-label-w": `${labelWidth}px` } as CSSProperties}
    >
      <div className="cc-lanes__labels" aria-hidden={false}>
        {rows.map((row) => {
          const top = rowTop.get(row.id) ?? 0;
          const h = row.id === PROBLEMS ? problemsH : laneHeight;
          if (row.id === PROBLEMS) {
            return (
              <div key={row.id} className="cc-lanes__label cc-lanes__label--problems" style={{ top, height: h }} data-testid={tid("lane", "problems")}>
                <span className="cc-lanes__label-text">{problemsLabel}</span>
              </div>
            );
          }
          const lane = row.lane!;
          const words = lane.title ?? (typeof lane.label === "string" ? lane.label : undefined);
          return (
            <div
              key={row.id}
              className={cx("cc-lanes__label", lane.folded && "cc-lanes__label--folded", lane.live && "is-live")}
              style={{ top, height: h }}
              title={words}
              data-testid={tid("lane", lane.id)}
              data-live={lane.live ? "true" : undefined}
            >
              {lane.live ? <StatusMark tone="live" className="cc-lanes__live" /> : null}
              {lane.onActivate ? (
                <button type="button" className="cc-lanes__label-btn" onClick={lane.onActivate} aria-label={lane.activateLabel}>
                  <span className="cc-lanes__label-text">{lane.label}</span>
                </button>
              ) : (
                <span className="cc-lanes__label-text">{lane.label}</span>
              )}
            </div>
          );
        })}
      </div>
      <svg
        ref={svgRef}
        className="cc-lanes__svg"
        width={plotW}
        height={height}
        style={{ width: plotW, height }}
        viewBox={`0 0 ${plotW} ${height}`}
        role="group"
        aria-label={label}
        onPointerMove={(event: PointerEvent<SVGSVGElement>) => setActive(hitTest(event.clientX, event.clientY))}
        onPointerLeave={() => setActive(null)}
        onClick={(event) => {
          if (!onSelect) return;
          const id = hitTest(event.clientX, event.clientY);
          const entry = id ? byId.get(id) : undefined;
          if (entry) {
            setFocusId(entry.item.id);
            onSelect(entry.item);
          }
        }}
        onKeyDown={onKeyDown}
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setActive(null);
        }}
      >
        {/* Lane rules and time gridlines. */}
        {rows.map((row) => {
          const top = rowTop.get(row.id) ?? 0;
          return <line key={`rule:${row.id}`} className="cc-lanes__rule" x1={0} x2={plotW} y1={top + 0.5} y2={top + 0.5} />;
        })}
        <line className="cc-lanes__rule" x1={0} x2={plotW} y1={height - 1.5} y2={height - 1.5} />
        {ticks.map((tick) => (
          <g key={`tick:${tick.t}`}>
            <line className="cc-lanes__grid" x1={tick.x} x2={tick.x} y1={AXIS_H - 4} y2={height - 2} />
            <text className="cc-viz-tick" x={tick.x} y={AXIS_H - 8} textAnchor="middle">
              {tick.label}
            </text>
          </g>
        ))}
        {marker ? (
          <g className="cc-lanes__now">
            <line x1={marker.x} x2={marker.x} y1={AXIS_H - 4} y2={height - 2} />
            <text className="cc-viz-tick" x={Math.min(plotW - 2, marker.x + 3)} y={AXIS_H - 8} textAnchor={marker.x > plotW - 30 ? "end" : "start"}>
              now
            </text>
          </g>
        ) : null}

        {/* Messages: a connector from the sender's lane to the recipient's, a chevron at the recipient. */}
        {placed.map(({ item, x }) => {
          if (item.kind !== "link") return null;
          const y0 = rowCenter(item.link.from);
          const y1 = rowCenter(item.link.to);
          const down = y1 > y0;
          const tipY = down ? y1 - 4 : y1 + 4;
          return (
            <g key={item.id} {...common(item, "link")}>
              <line className="cc-lanes__link-hit" x1={x} x2={x} y1={y0} y2={y1} />
              <line className="cc-lanes__link" x1={x} x2={x} y1={y0} y2={tipY} />
              <circle className="cc-lanes__link-end" cx={x} cy={y0} r={1.75} />
              <path className="cc-lanes__chevron" d={down ? `M${x - 3} ${tipY - 3.5}L${x} ${tipY}L${x + 3} ${tipY - 3.5}` : `M${x - 3} ${tipY + 3.5}L${x} ${tipY}L${x + 3} ${tipY + 3.5}`} />
              {item.count > 1 ? (
                <text className="cc-lanes__count" x={x + 4} y={(y0 + y1) / 2 + 3}>
                  {item.count}
                </text>
              ) : null}
            </g>
          );
        })}

        {/* Turns. */}
        {placed.map(({ item, lane, x, y }) => {
          if (item.kind !== "span") return null;
          const span = item.span;
          if (folded.has(lane)) {
            const props = common(item, "span");
            return <rect key={item.id} {...props} data-state={span.state} className={cx("cc-lanes__fold-tick", props.className)} x={x - 0.5} y={y - 5} width={1} height={10} />;
          }
          const x1 = Math.max(x + MIN_SPAN_W, spanEnd(span));
          const barH = Math.max(6, Math.round(laneHeight * 0.36));
          return (
            <g key={item.id} {...common(item, "span")} data-state={span.state}>
              <rect className="cc-lanes__span" x={x} y={y - barH / 2} width={x1 - x} height={barH} rx={2} />
              {(span.ticks ?? []).map((t, index) => {
                const tx = scale.x(t);
                if (tx <= x + 1 || tx >= x1 - 1) return null;
                return <line key={index} className="cc-lanes__tick" x1={tx} x2={tx} y1={y - barH / 2} y2={y + barH / 2} />;
              })}
              {span.state === "live" ? <circle className="cc-lanes__live-end" cx={x1} cy={y} r={3} /> : null}
            </g>
          );
        })}

        {/* Approvals, lifecycle events and problems. */}
        {placed.map(({ item, x, y }) => {
          if (item.kind !== "mark" && item.kind !== "problem") return null;
          const mark = item.mark;
          const props = common(item, item.kind);
          return (
            <path
              key={item.id}
              {...props}
              data-shape={mark.shape}
              data-tone={mark.tone}
              className={cx("cc-lanes__mark", props.className)}
              d={markPath(mark.shape, x, y, mark.shape === "square" ? 7 : 9)}
            />
          );
        })}
      </svg>
      {activeEntry ? (
        <ChartTooltip x={tooltipX} y={tooltipY} width={labelWidth + plotW}>
          {itemLabel(activeEntry.item)}
        </ChartTooltip>
      ) : null}
    </div>
  );
}
