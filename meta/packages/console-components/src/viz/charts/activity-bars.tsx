import { useState, type KeyboardEvent, type PointerEvent } from "react";

import { ChartTooltip } from "../../panel-kit/chart-tooltip";
import { cx } from "../../panel-kit/cx";
import { createTimeScale, formatTimeTick, placeAxisLabel, placeTickLabels } from "./time-axis";

const HOUR = 3_600_000;

/**
 * A day of activity (spec A3.8): 24 or 48 bins ending now, bars in
 * `--_cc-viz-mid` on a 1 px grid baseline, mono hour ticks (00 06 12 18 on
 * the local clock) and "now" at the right edge; the current bin takes the
 * accent when `live`, a zero bin is a 1 px faint tick. Drawer and tile
 * size; hovering or focusing a bar shows its count and hour.
 */
export function ActivityBars({
  values,
  end,
  span = 24 * HOUR,
  width = 300,
  height = 44,
  live,
  label,
  ticks = true,
  unit = "events",
  className,
  testId,
}: {
  /** Counts per bin, oldest first; the last bin ends at `end`. */
  values: ReadonlyArray<number>;
  /** The end of the last bin (epoch ms); now by default. */
  end?: number;
  /** The whole window in ms (24 h by default). */
  span?: number;
  width?: number;
  height?: number;
  live?: boolean;
  /** The numbers in words ("46 turns in the last 24 hours, most at 14:00"). */
  label: string;
  ticks?: boolean;
  /** The word after a count in the tooltip ("12 turns"). */
  unit?: string;
  className?: string;
  testId?: string;
}) {
  const [hover, setHover] = useState<number | null>(null);
  const n = values.length;
  const stop = end ?? Date.now();
  const start = stop - span;
  // "now" names the right end only when the window ends now (within a bin).
  const endsNow = end === undefined || Math.abs(Date.now() - stop) <= span / Math.max(1, n);
  const tickH = ticks ? 12 : 0;
  const plotH = Math.max(4, height - tickH - 1);
  const baseline = plotH + 0.5;
  const gap = width <= 96 ? 1 : 2;
  const barW = n > 0 ? Math.max(0.5, (width - gap * (n - 1)) / n) : 0;
  const max = values.reduce((acc, value) => Math.max(acc, Number.isFinite(value) ? value : 0), 0);
  const scale = createTimeScale({ start, end: stop, range: [0, width] });
  // Hour labels centre on their ticks, slide inside at the edges and keep
  // clear of the "now" label (the same tick-label pass as DotStrip).
  const nowLabel = ticks && endsNow ? placeAxisLabel(width, "now", width) : null;
  const hourTicks = ticks
    ? placeTickLabels(
        scale.ticks({ step: 6 * HOUR, format: "hour" }).filter((tick) => tick.x >= 0 && tick.x <= width),
        width,
        { avoid: nowLabel ? [nowLabel] : [] },
      )
    : [];
  const binStart = (index: number) => start + (index * span) / Math.max(1, n);
  const pick = (clientX: number, element: Element) => {
    const box = element.getBoundingClientRect();
    const x = ((clientX - box.left) / Math.max(1, box.width)) * width;
    return Math.min(n - 1, Math.max(0, Math.floor(x / (barW + gap))));
  };
  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key) || n === 0) return;
    event.preventDefault();
    const current = hover ?? n - 1;
    setHover(event.key === "Home" ? 0 : event.key === "End" ? n - 1 : event.key === "ArrowLeft" ? Math.max(0, current - 1) : Math.min(n - 1, current + 1));
  };
  const tip = hover !== null && hover < n
    ? `${values[hover] ?? 0} ${unit} · ${formatTimeTick(binStart(hover), "clock")}-${formatTimeTick(binStart(hover + 1), "clock")}`
    : null;
  return (
    <span className={cx("cc-activity", className)} data-testid={testId} style={{ width, height }}>
      <svg
        className="cc-activity__svg"
        width={width}
        height={height}
        style={{ width, height }}
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={label}
        tabIndex={n > 0 ? 0 : undefined}
        onPointerMove={(event: PointerEvent<SVGSVGElement>) => n > 0 && setHover(pick(event.clientX, event.currentTarget))}
        onPointerLeave={() => setHover(null)}
        onFocus={() => setHover((current) => current ?? (n > 0 ? n - 1 : null))}
        onBlur={() => setHover(null)}
        onKeyDown={onKeyDown}
      >
        <line className="cc-activity__base" x1={0} x2={width} y1={baseline} y2={baseline} />
        {values.map((raw, index) => {
          const value = Number.isFinite(raw) ? Math.max(0, raw) : 0;
          const x = index * (barW + gap);
          if (value <= 0) return <rect key={index} className="cc-activity__zero" x={x} y={plotH - 1} width={barW} height={1} />;
          const h = Math.max(1.5, (value / Math.max(max, 1e-9)) * (plotH - 2));
          return (
            <rect
              key={index}
              className={cx("cc-activity__bar", live && index === n - 1 && "is-live", hover === index && "is-hover")}
              x={x}
              y={plotH - h}
              width={barW}
              height={h}
              rx={barW >= 4 ? 1 : 0}
            />
          );
        })}
        {hourTicks.map((tick) => (
          <text key={tick.t} className="cc-viz-tick" x={tick.labelX} y={height - 1} textAnchor="middle">
            {tick.label}
          </text>
        ))}
        {nowLabel ? (
          <text className="cc-viz-tick" x={nowLabel.labelX} y={height - 1} textAnchor="middle">
            now
          </text>
        ) : null}
      </svg>
      {tip !== null && hover !== null ? (
        <ChartTooltip x={hover * (barW + gap) + barW / 2} y={0} width={width}>
          {tip}
        </ChartTooltip>
      ) : null}
    </span>
  );
}
