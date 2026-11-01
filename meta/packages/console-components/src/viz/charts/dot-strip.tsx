import { useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

import { ChartTooltip } from "../../panel-kit/chart-tooltip";
import { cx } from "../../panel-kit/cx";
import type { Tone } from "../../panel-kit/tone";
import { createTimeScale, nowMarker, placeAxisLabel, placeTickLabels } from "./time-axis";

export type DotShape = "dot" | "hollow" | "tick" | "triangle" | "diamond" | "square";

export interface DotStripItem {
  id: string;
  /** When (epoch ms). */
  t: number;
  shape?: DotShape;
  /** The mark's status tone (ink without one). */
  tone?: Tone;
  /** Replaced, discarded, skipped: the faint ink. */
  faint?: boolean;
  /** Happening now: the accent. */
  live?: boolean;
  /** The mark in words: its accessible name and tooltip. */
  label: string;
}

const HIT_PX = 8;

/**
 * Events on a time axis (spec B3, B7, B12): one mark per item at its time
 * (record ticks, injection dots, decisions, deliveries) on a 1 px grid
 * baseline, shaped and toned per item, with optional ticks below and the
 * "now" marker. One tab stop: the arrow keys, Home and End move between
 * marks (each named by its label), Enter or Space selects; the pointer
 * picks the nearest mark within 8 px, and a tooltip shows its words.
 */
export function DotStrip({
  items,
  start,
  end,
  width = 240,
  height = 20,
  axis,
  now,
  label,
  onSelect,
  selectedId,
  markSize = 6,
  testIdPrefix,
  className,
  testId,
}: {
  items: ReadonlyArray<DotStripItem>;
  start: number;
  end: number;
  width?: number;
  height?: number;
  /** Time ticks under the strip. */
  axis?: boolean;
  /** The "now" marker (when now is inside the range). */
  now?: boolean | number;
  /** The strip in words ("74 records over 30 days, 1 quarantined"). */
  label: string;
  onSelect?: (item: DotStripItem) => void;
  selectedId?: string | null;
  markSize?: number;
  /** Each mark gets data-testid="<prefix>:<item id>". */
  testIdPrefix?: string;
  className?: string;
  testId?: string;
}) {
  const [active, setActive] = useState<number | null>(null);
  const [focusIndex, setFocusIndex] = useState(0);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const axisH = axis ? 12 : 0;
  const totalH = height + axisH;
  const scale = createTimeScale({ start, end, range: [markSize / 2 + 1, width - markSize / 2 - 1] });
  const sorted = useMemo(() => items.map((item, index) => ({ item, index })).sort((a, b) => a.item.t - b.item.t), [items]);
  const cy = height / 2;
  const nowValue = now === true ? Date.now() : typeof now === "number" ? now : null;
  const marker = nowValue !== null ? nowMarker(scale, nowValue) : null;
  // The "now" label sits at its marker (a window may start at now, or hold it
  // anywhere); the first and last tick labels slide inside the strip rather
  // than lose their ends at its edges, and none meets the "now" label.
  const nowLabel = axis && marker ? placeAxisLabel(marker.x, marker.label, width) : null;
  const ticks = axis
    ? placeTickLabels(
        scale.ticks({ count: Math.max(2, Math.floor(width / 90)) }).filter((tick) => tick.x >= 0 && tick.x <= width),
        width,
        { avoid: nowLabel ? [nowLabel] : [] },
      )
    : [];
  const nearest = (clientX: number): number | null => {
    const box = svgRef.current?.getBoundingClientRect();
    if (!box || !sorted.length) return null;
    const x = ((clientX - box.left) / Math.max(1, box.width)) * width;
    let best = -1;
    let bestDistance = Infinity;
    sorted.forEach((entry, order) => {
      const distance = Math.abs(scale.x(entry.item.t) - x);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = order;
      }
    });
    return bestDistance <= HIT_PX ? best : null;
  };
  const focusMark = (order: number) => {
    const next = Math.min(sorted.length - 1, Math.max(0, order));
    setFocusIndex(next);
    setActive(next);
    svgRef.current?.querySelector<SVGElement>(`[data-order="${next}"]`)?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    if (!sorted.length) return;
    const current = active ?? focusIndex;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight" || event.key === "Home" || event.key === "End") {
      event.preventDefault();
      focusMark(event.key === "Home" ? 0 : event.key === "End" ? sorted.length - 1 : event.key === "ArrowLeft" ? current - 1 : current + 1);
    } else if ((event.key === "Enter" || event.key === " ") && onSelect) {
      event.preventDefault();
      const entry = sorted[current];
      if (entry) onSelect(entry.item);
    }
  };
  const activeEntry = active !== null ? sorted[active] : undefined;
  const half = markSize / 2;
  return (
    <span className={cx("cc-dots", onSelect && "is-selectable", className)} data-testid={testId} style={{ width, height: totalH }}>
      <svg
        ref={svgRef}
        className="cc-dots__svg"
        width={width}
        height={totalH}
        style={{ width, height: totalH }}
        viewBox={`0 0 ${width} ${totalH}`}
        role="group"
        aria-label={label}
        onPointerMove={(event: PointerEvent<SVGSVGElement>) => setActive(nearest(event.clientX))}
        onPointerLeave={() => setActive(null)}
        onClick={(event) => {
          if (!onSelect) return;
          const order = nearest(event.clientX);
          if (order !== null) {
            setFocusIndex(order);
            onSelect(sorted[order].item);
          }
        }}
        onKeyDown={onKeyDown}
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setActive(null);
        }}
      >
        <line className="cc-dots__base" x1={0} x2={width} y1={cy + 0.5} y2={cy + 0.5} />
        {marker ? <line className="cc-dots__now" x1={marker.x} x2={marker.x} y1={1} y2={height - 1} /> : null}
        {sorted.map(({ item }, order) => {
          const x = scale.x(item.t);
          const shape = item.shape ?? "dot";
          const common = {
            "data-order": order,
            "data-shape": shape,
            "data-tone": item.tone,
            "data-testid": testIdPrefix ? `${testIdPrefix}:${item.id}` : undefined,
            className: cx(
              "cc-dots__mark",
              item.faint && "is-faint",
              item.live && "is-live",
              selectedId === item.id && "is-selected",
              active === order && "is-active",
            ),
            role: onSelect ? "button" : "img",
            "aria-label": item.label,
            tabIndex: order === focusIndex ? 0 : -1,
            onFocus: () => {
              setFocusIndex(order);
              setActive(order);
            },
          } as const;
          switch (shape) {
            case "hollow":
              return <circle key={item.id} {...common} cx={x} cy={cy} r={half - 0.75} />;
            case "tick":
              return <rect key={item.id} {...common} x={x - 0.75} y={cy - Math.min(5, height / 2 - 1)} width={1.5} height={Math.min(10, height - 2)} />;
            case "triangle":
              return <path key={item.id} {...common} d={`M${x} ${cy - half} L${x + half} ${cy + half * 0.8} L${x - half} ${cy + half * 0.8} Z`} />;
            case "diamond":
              return <path key={item.id} {...common} d={`M${x} ${cy - half} L${x + half} ${cy} L${x} ${cy + half} L${x - half} ${cy} Z`} />;
            case "square":
              return <rect key={item.id} {...common} x={x - half + 0.5} y={cy - half + 0.5} width={markSize - 1} height={markSize - 1} />;
            default:
              return <circle key={item.id} {...common} cx={x} cy={cy} r={half} />;
          }
        })}
        {ticks.map((tick) => (
          <text key={tick.t} className="cc-viz-tick" x={tick.labelX} y={totalH - 1} textAnchor="middle">
            {tick.label}
          </text>
        ))}
        {nowLabel ? (
          <text className="cc-viz-tick cc-dots__now-label" x={nowLabel.labelX} y={totalH - 1} textAnchor="middle">
            now
          </text>
        ) : null}
      </svg>
      {activeEntry ? (
        <ChartTooltip x={scale.x(activeEntry.item.t)} y={0} width={width}>
          {activeEntry.item.label}
        </ChartTooltip>
      ) : null}
    </span>
  );
}
