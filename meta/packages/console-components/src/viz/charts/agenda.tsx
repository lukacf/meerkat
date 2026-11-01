import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";

import { ChartTooltip } from "../../panel-kit/chart-tooltip";
import { cx } from "../../panel-kit/cx";
import { createTimeScale, timeTicks } from "./time-axis";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
/** The pointer picks the nearest mark within this many px. */
const HIT_PX = 8;
/** Marks keep this far from the lane's ends so none is clipped. */
const EDGE_PX = 6;

/** How a planned run is drawn: once = a filled square, a repeat = a tick,
 * a calendar = a diamond (spec B10). */
export type AgendaShape = "square" | "tick" | "diamond";

/** One planned run on an agent's row. */
export interface AgendaRun {
  /** Unique in its row; the mark's test id is `<testIdPrefix>:<id>`. */
  id: string;
  /** When (epoch ms). */
  t: number;
  shape: AgendaShape;
  /** The schedule's next run: the accent. */
  next?: boolean;
  /** A paused schedule's would-be run: hollow and muted. */
  paused?: boolean;
  /** The mark in words: "Inbox sweep · Mon 07:30 Europe/Paris". */
  label: string;
  /** Payload handed back by `onSelect`. */
  data?: unknown;
}

/** A repeat too dense to draw run by run: a dashed band with its words. */
export interface AgendaBand {
  id: string;
  from: number;
  to: number;
  /** "every 5 min". */
  words: string;
  /** "Health digest · every 5 min, 288 runs in the next 24 hours". */
  label: string;
  paused?: boolean;
  data?: unknown;
}

/** One agent's row. */
export interface AgendaRow {
  key: string;
  /** The row head: the app passes the agent's name as a link. */
  label: ReactNode;
  /** The row head in words (the lane's name and the table twin). */
  labelText: string;
  runs: ReadonlyArray<AgendaRun>;
  bands?: ReadonlyArray<AgendaBand>;
}

type Item =
  | { kind: "run"; key: string; t: number; x: number; run: AgendaRun }
  | { kind: "band"; key: string; t: number; x: number; band: AgendaBand };

function useLaneWidth(fallback: number): [RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(fallback);
  useEffect(() => {
    const element = ref.current;
    if (!element) return undefined;
    const measure = () => {
      const next = Math.round(element.getBoundingClientRect().width);
      if (next > 0) setWidth((current) => (current === next ? current : next));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

/** The axis labels: "now", "+6h", "+12h" ... over a day; local days over a week. */
function axisTicks(start: number, end: number): Array<{ t: number; label: string; major: boolean }> {
  const span = end - start;
  if (span <= 2 * DAY) {
    const step = span <= 6 * HOUR ? HOUR : span <= 12 * HOUR ? 3 * HOUR : 6 * HOUR;
    const ticks: Array<{ t: number; label: string; major: boolean }> = [];
    for (let t = start + step; t <= end; t += step) {
      ticks.push({ t, label: `+${Math.round((t - start) / HOUR)}h`, major: false });
    }
    return ticks;
  }
  return timeTicks(start, end, { step: DAY, format: "day" }).map((tick) => ({ t: tick.t, label: tick.label, major: true }));
}

const SHAPE_WORDS: Record<AgendaShape, string> = { square: "once", tick: "repeating", diamond: "calendar" };

/**
 * The Schedules agenda (spec B10): one row per agent on a shared time axis
 * that starts now, each planned run a mark (once = a filled square, a
 * repeat = a tick, a calendar = a diamond), each schedule's next run in
 * the accent, a paused schedule's runs hollow and muted, and a repeat too
 * dense to draw as a dashed band with its words. Hovering or focusing a
 * mark shows its words. The chart is one tab stop: the arrow keys move
 * along a row (Left, Right, Home, End) and between rows (Up, Down, to the
 * nearest run in time); Enter or Space selects. A visually hidden table
 * lists every run.
 */
export function Agenda({
  rows,
  start,
  end,
  label,
  onSelect,
  selectedId,
  rowHeight = 32,
  testIdPrefix,
  rowTestIdPrefix,
  caption,
  className,
  testId,
}: {
  rows: ReadonlyArray<AgendaRow>;
  /** The window, epoch ms: `start` is now. */
  start: number;
  end: number;
  /** The chart in words ("23 runs in the next 24 hours across 4 agents"). */
  label: string;
  onSelect?: (run: AgendaRun | AgendaBand, row: AgendaRow) => void;
  /** The selected mark: `<row key>:<run id>`. */
  selectedId?: string | null;
  rowHeight?: number;
  /** Each mark gets data-testid="<prefix>:<run id>". */
  testIdPrefix?: string;
  /** Each row gets data-testid="<prefix>:<row key>". */
  rowTestIdPrefix?: string;
  /** The table twin's caption; `label` otherwise. */
  caption?: string;
  className?: string;
  testId?: string;
}) {
  const [laneRef, laneWidth] = useLaneWidth(640);
  const width = Math.max(120, laneWidth);
  const scale = useMemo(() => createTimeScale({ start, end, range: [EDGE_PX, width - EDGE_PX] }), [start, end, width]);
  const ticks = useMemo(() => axisTicks(start, end), [start, end]);
  const lanes = useMemo(
    () =>
      rows.map((row) => {
        const items: Item[] = [
          ...row.runs.filter((run) => run.t >= start && run.t <= end).map((run) => ({ kind: "run" as const, key: `${row.key}:${run.id}`, t: run.t, x: scale.x(run.t), run })),
          ...(row.bands ?? []).map((band) => {
            const t = Math.max(start, band.from);
            return { kind: "band" as const, key: `${row.key}:${band.id}`, t, x: scale.x(t), band };
          }),
        ].sort((a, b) => a.t - b.t);
        return { row, items };
      }),
    [rows, scale, start, end],
  );
  const firstKey = lanes.find((lane) => lane.items.length)?.items[0]?.key ?? null;
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const tabKey = focusKey && lanes.some((lane) => lane.items.some((item) => item.key === focusKey)) ? focusKey : firstKey;
  const rootRef = useRef<HTMLDivElement | null>(null);
  const cy = rowHeight / 2;

  const locate = (key: string | null): { lane: number; index: number } | null => {
    if (!key) return null;
    for (let lane = 0; lane < lanes.length; lane += 1) {
      const index = lanes[lane].items.findIndex((item) => item.key === key);
      if (index >= 0) return { lane, index };
    }
    return null;
  };

  const focusItem = (key: string) => {
    setFocusKey(key);
    setActive(key);
    const marks = rootRef.current?.querySelectorAll<SVGElement>("[data-agenda-key]") ?? [];
    for (const mark of Array.from(marks)) {
      if (mark.getAttribute("data-agenda-key") === key) {
        mark.focus();
        break;
      }
    }
  };

  const select = (item: Item, row: AgendaRow) => {
    if (!onSelect) return;
    onSelect(item.kind === "run" ? item.run : item.band, row);
  };

  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    // The focused mark names itself; the remembered one otherwise.
    const own = (event.target as Element | null)?.getAttribute?.("data-agenda-key") ?? null;
    const here = locate(own ?? focusKey ?? tabKey);
    if (!here) return;
    const lane = lanes[here.lane];
    const item = lane.items[here.index];
    switch (event.key) {
      case "ArrowLeft":
      case "ArrowRight": {
        event.preventDefault();
        const next = lane.items[Math.min(lane.items.length - 1, Math.max(0, here.index + (event.key === "ArrowLeft" ? -1 : 1)))];
        if (next) focusItem(next.key);
        return;
      }
      case "Home":
      case "End": {
        event.preventDefault();
        const next = event.key === "Home" ? lane.items[0] : lane.items[lane.items.length - 1];
        if (next) focusItem(next.key);
        return;
      }
      case "ArrowUp":
      case "ArrowDown": {
        event.preventDefault();
        const step = event.key === "ArrowUp" ? -1 : 1;
        for (let other = here.lane + step; other >= 0 && other < lanes.length; other += step) {
          const candidates = lanes[other].items;
          if (!candidates.length) continue;
          let best = candidates[0];
          for (const candidate of candidates) if (Math.abs(candidate.t - item.t) < Math.abs(best.t - item.t)) best = candidate;
          focusItem(best.key);
          return;
        }
        return;
      }
      case "Enter":
      case " ":
        if (onSelect) {
          event.preventDefault();
          select(item, lane.row);
        }
        return;
      default:
        return;
    }
  };

  const nearest = (laneIndex: number, clientX: number, element: Element): Item | null => {
    const box = element.getBoundingClientRect();
    const x = ((clientX - box.left) / Math.max(1, box.width)) * width;
    let best: Item | null = null;
    let distance = Infinity;
    for (const item of lanes[laneIndex].items) {
      if (item.kind === "band") {
        const x0 = scale.x(Math.max(start, item.band.from));
        const x1 = scale.x(Math.min(end, item.band.to));
        if (x >= x0 - HIT_PX && x <= x1 + HIT_PX) return item;
        continue;
      }
      const gap = Math.abs(item.x - x);
      if (gap < distance) {
        distance = gap;
        best = item;
      }
    }
    return distance <= HIT_PX ? best : null;
  };

  const activeItem = active ? lanes.flatMap((lane) => lane.items).find((item) => item.key === active) ?? null : null;
  const activeLane = activeItem ? lanes.findIndex((lane) => lane.items.includes(activeItem)) : -1;
  const totalRuns = lanes.reduce((sum, lane) => sum + lane.items.filter((item) => item.kind === "run").length, 0);

  return (
    <div ref={rootRef} className={cx("cc-agenda", onSelect && "is-selectable", className)} role="group" aria-label={label} data-testid={testId} data-runs={totalRuns}>
      <div className="cc-agenda__row cc-agenda__row--axis" aria-hidden="true">
        <span className="cc-agenda__head" />
        <div ref={laneRef} className="cc-agenda__lane">
          <svg className="cc-agenda__svg" width={width} height={18} viewBox={`0 0 ${width} 18`} focusable="false">
            <text className="cc-viz-tick cc-agenda__now-label" x={EDGE_PX} y={12}>
              now
            </text>
            {ticks
              .filter((tick) => scale.x(tick.t) > EDGE_PX + 28 && scale.x(tick.t) <= width - 4)
              .map((tick) => (
                <text key={tick.t} className="cc-viz-tick" x={scale.x(tick.t)} y={12} textAnchor={scale.x(tick.t) > width - 24 ? "end" : "middle"}>
                  {tick.label}
                </text>
              ))}
          </svg>
        </div>
      </div>
      {lanes.map((lane, laneIndex) => (
        <div
          key={lane.row.key}
          className="cc-agenda__row"
          data-testid={rowTestIdPrefix ? `${rowTestIdPrefix}:${lane.row.key}` : undefined}
          style={{ minHeight: rowHeight }}
        >
          <span className="cc-agenda__head">{lane.row.label}</span>
          <div className="cc-agenda__lane">
            <svg
              className="cc-agenda__svg"
              width={width}
              height={rowHeight}
              viewBox={`0 0 ${width} ${rowHeight}`}
              role="group"
              aria-label={`${lane.row.labelText}: ${lane.items.filter((item) => item.kind === "run").length} planned runs`}
              onKeyDown={onKeyDown}
              onPointerMove={(event) => {
                const item = nearest(laneIndex, event.clientX, event.currentTarget);
                setActive(item ? item.key : null);
              }}
              onPointerLeave={() => setActive(null)}
              onClick={(event) => {
                const item = nearest(laneIndex, event.clientX, event.currentTarget);
                if (item) {
                  setFocusKey(item.key);
                  select(item, lane.row);
                }
              }}
              onBlur={(event) => {
                if (!rootRef.current?.contains(event.relatedTarget as Node | null)) setActive(null);
              }}
            >
              <line className="cc-dots__base" x1={0} x2={width} y1={cy + 0.5} y2={cy + 0.5} />
              {ticks.map((tick) => (
                <line key={tick.t} className={cx("cc-agenda__grid", tick.major && "is-major")} x1={scale.x(tick.t)} x2={scale.x(tick.t)} y1={2} y2={rowHeight - 2} />
              ))}
              <line className="cc-dots__now" x1={EDGE_PX} x2={EDGE_PX} y1={2} y2={rowHeight - 2} />
              {lane.items.map((item) => {
                const isActive = active === item.key;
                const common = {
                  "data-agenda-key": item.key,
                  tabIndex: item.key === tabKey ? 0 : -1,
                  onFocus: () => {
                    setFocusKey(item.key);
                    setActive(item.key);
                  },
                } as const;
                if (item.kind === "band") {
                  const x0 = scale.x(Math.max(start, item.band.from));
                  const x1 = scale.x(Math.min(end, item.band.to));
                  return (
                    <g
                      key={item.key}
                      {...common}
                      className={cx("cc-agenda__band", item.band.paused && "is-paused", isActive && "is-active", selectedId === item.key && "is-selected")}
                      role={onSelect ? "button" : "img"}
                      aria-label={item.band.label}
                      data-testid={testIdPrefix ? `${testIdPrefix}:${item.band.id}` : undefined}
                    >
                      <line className="cc-agenda__band-line" x1={x0} x2={x1} y1={cy} y2={cy} />
                      <rect className="cc-agenda__band-hit" x={x0} y={cy - 6} width={Math.max(1, x1 - x0)} height={12} />
                    </g>
                  );
                }
                const { run } = item;
                const x = item.x;
                const props = {
                  ...common,
                  className: cx(
                    "cc-dots__mark",
                    "cc-agenda__mark",
                    run.next && !run.paused && "is-live",
                    isActive && "is-active",
                    selectedId === item.key && "is-selected",
                  ),
                  "data-shape": run.shape,
                  "data-paused": run.paused ? "true" : undefined,
                  role: onSelect ? "button" : "img",
                  "aria-label": run.label,
                  "data-testid": testIdPrefix ? `${testIdPrefix}:${run.id}` : undefined,
                } as const;
                if (run.shape === "tick") {
                  return <rect key={item.key} {...props} x={x - 0.75} y={cy - 6} width={1.5} height={12} />;
                }
                if (run.shape === "diamond") {
                  return <path key={item.key} {...props} d={`M${x} ${cy - 5} L${x + 5} ${cy} L${x} ${cy + 5} L${x - 5} ${cy} Z`} />;
                }
                return <rect key={item.key} {...props} x={x - 4} y={cy - 4} width={8} height={8} />;
              })}
              {lane.items
                .filter((item): item is Extract<Item, { kind: "band" }> => item.kind === "band")
                .map((item) => (
                  <text key={`${item.key}:words`} className="cc-agenda__band-words" x={scale.x(Math.max(start, item.band.from)) + 4} y={cy - 5}>
                    {item.band.words}
                  </text>
                ))}
            </svg>
            {activeItem && activeLane === laneIndex ? (
              <ChartTooltip x={activeItem.x} y={0} width={width}>
                {activeItem.kind === "run" ? activeItem.run.label : activeItem.band.label}
              </ChartTooltip>
            ) : null}
          </div>
        </div>
      ))}
      <table className="cc-visually-hidden">
        <caption>{caption ?? label}</caption>
        <thead>
          <tr>
            <th scope="col">Agent</th>
            <th scope="col">Run</th>
            <th scope="col">Kind</th>
          </tr>
        </thead>
        <tbody>
          {lanes.flatMap((lane) =>
            lane.items.map((item) => (
              <tr key={item.key}>
                <th scope="row">{lane.row.labelText}</th>
                <td>{item.kind === "run" ? item.run.label : item.band.label}</td>
                <td>
                  {item.kind === "run"
                    ? `${SHAPE_WORDS[item.run.shape]}${item.run.next ? ", next" : ""}${item.run.paused ? ", paused" : ""}`
                    : `repeating${item.band.paused ? ", paused" : ""}`}
                </td>
              </tr>
            )),
          )}
        </tbody>
      </table>
    </div>
  );
}
