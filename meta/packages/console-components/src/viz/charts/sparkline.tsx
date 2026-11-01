import { useState, type KeyboardEvent, type PointerEvent } from "react";

import { ChartTooltip } from "../../panel-kit/chart-tooltip";
import { cx } from "../../panel-kit/cx";
import { formatClockTime } from "../../panel-kit/format";

export interface SparkPoint {
  /** The bin's time (epoch ms), for the tooltip. */
  t: number;
  v: number;
}

export type SparkValues = ReadonlyArray<number> | ReadonlyArray<SparkPoint>;

function points(values: SparkValues): SparkPoint[] {
  return (values as ReadonlyArray<number | SparkPoint>).map((value, index) =>
    typeof value === "number" ? { t: Number.NaN, v: Number.isFinite(value) ? Math.max(0, value) : 0 } : { t: value.t, v: Number.isFinite(value.v) ? Math.max(0, value.v) : 0 },
  ).map((point, index) => (Number.isNaN(point.t) ? { ...point, t: index } : point));
}

/**
 * A small trend (spec A3.8): a line or bars, `--_cc-viz-mid`, the y domain
 * 0 to the maximum (never negative). Line: 1.5 px (2 px from 24 px tall),
 * round caps, an optional 10% area, and the last point as a dot with a
 * card-coloured ring, in the accent when `live`, else ink. Bars: 1 px gaps
 * up to 96 px wide, 2 px wider; a zero bin is a 1 px faint tick on the
 * baseline; the last bin is the accent when `live`. A constant series is a
 * flat line at mid height. 64 x 16 in rows, 96 x 20 in tiles, 240 x 40 in
 * drawers, where `tooltip` adds a crosshair that snaps to the nearest bin.
 */
export function Sparkline({
  values,
  width = 64,
  height = 16,
  kind = "line",
  live,
  area,
  label,
  tooltip,
  formatValue = (value) => String(value),
  binLabel,
  className,
  testId,
}: {
  values: SparkValues;
  width?: number;
  height?: number;
  kind?: "line" | "bars";
  /** Data in the last 5 minutes: the last point or bin takes the accent. */
  live?: boolean;
  area?: boolean;
  /** The numbers in words ("48 messages in the last hour, rising"). */
  label: string;
  tooltip?: boolean;
  formatValue?: (value: number) => string;
  /** The tooltip's words for a bin ("16:00-16:05"). */
  binLabel?: (index: number, point: SparkPoint) => string;
  className?: string;
  testId?: string;
}) {
  const data = points(values);
  const [hover, setHover] = useState<number | null>(null);
  const n = data.length;
  const max = data.reduce((acc, point) => Math.max(acc, point.v), 0);
  const constant = n > 0 && data.every((point) => point.v === data[0].v);
  const isBars = kind === "bars";
  const padTop = isBars ? 0 : 3;
  const padBottom = isBars ? 0 : 3;
  const padLeft = isBars ? 0 : 1.5;
  const padRight = isBars ? 0 : 3.5;
  const plotW = Math.max(1, width - padLeft - padRight);
  const plotH = Math.max(1, height - padTop - padBottom);
  const gap = isBars ? (width <= 96 ? 1 : 2) : 0;
  const barW = isBars && n > 0 ? Math.max(0.5, (width - gap * (n - 1)) / n) : 0;
  const xAt = (index: number) =>
    isBars ? index * (barW + gap) + barW / 2 : n > 1 ? padLeft + (index * plotW) / (n - 1) : padLeft + plotW / 2;
  const yAt = (value: number) => (constant || max === 0 ? padTop + plotH / 2 : padTop + (1 - value / max) * plotH);
  const stroke = height >= 24 ? 2 : 1.5;
  const pickIndex = (clientX: number, element: Element) => {
    const box = element.getBoundingClientRect();
    const x = ((clientX - box.left) / Math.max(1, box.width)) * width;
    let best = 0;
    for (let index = 1; index < n; index += 1) if (Math.abs(xAt(index) - x) < Math.abs(xAt(best) - x)) best = index;
    return best;
  };
  const interactive = Boolean(tooltip && n > 0);
  const onPointerMove = (event: PointerEvent<SVGSVGElement>) => setHover(pickIndex(event.clientX, event.currentTarget));
  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight" && event.key !== "Home" && event.key !== "End") return;
    event.preventDefault();
    const current = hover ?? n - 1;
    const next = event.key === "Home" ? 0 : event.key === "End" ? n - 1 : event.key === "ArrowLeft" ? Math.max(0, current - 1) : Math.min(n - 1, current + 1);
    setHover(next);
  };
  const hovered = hover !== null && hover < n ? data[hover] : null;
  const tipLabel = (index: number, point: SparkPoint) => {
    const value = formatValue(point.v);
    const when = binLabel ? binLabel(index, point) : point.t > 1e11 ? formatClockTime(point.t) : "";
    return when ? `${value} · ${when}` : value;
  };

  let body = null;
  if (n === 0) {
    body = <line className="cc-spark__base" x1={0} x2={width} y1={height - 0.5} y2={height - 0.5} />;
  } else if (isBars) {
    body = data.map((point, index) => {
      const x = index * (barW + gap);
      const last = index === n - 1;
      if (point.v <= 0) {
        return <rect key={index} className="cc-spark__zero" x={x} y={height - 1} width={barW} height={1} />;
      }
      const h = Math.max(1.5, (point.v / Math.max(max, 1e-9)) * height);
      return (
        <rect
          key={index}
          className={cx("cc-spark__bar", last && live && "is-live", hover === index && "is-hover")}
          x={x}
          y={height - h}
          width={barW}
          height={h}
          rx={barW >= 3 ? 1 : 0}
        />
      );
    });
  } else {
    const path = data.map((point, index) => `${index ? "L" : "M"}${xAt(index).toFixed(2)} ${yAt(point.v).toFixed(2)}`).join(" ");
    const lastX = xAt(n - 1);
    const lastY = yAt(data[n - 1].v);
    body = (
      <>
        {area ? (
          <path
            className="cc-spark__area"
            d={`${path} L${lastX.toFixed(2)} ${(height - padBottom).toFixed(2)} L${xAt(0).toFixed(2)} ${(height - padBottom).toFixed(2)} Z`}
          />
        ) : null}
        <path className="cc-spark__line" d={path} strokeWidth={stroke} />
        <circle className="cc-spark__ring" cx={lastX} cy={lastY} r={3.5} />
        <circle className={cx("cc-spark__dot", live && "is-live")} cx={lastX} cy={lastY} r={2} />
      </>
    );
  }
  return (
    <span className={cx("cc-spark", interactive && "is-interactive", className)} data-testid={testId} style={{ width, height }}>
      <svg
        className="cc-spark__svg"
        width={width}
        height={height}
        style={{ width, height }}
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={label}
        tabIndex={interactive ? 0 : undefined}
        onPointerMove={interactive ? onPointerMove : undefined}
        onPointerLeave={interactive ? () => setHover(null) : undefined}
        onFocus={interactive ? () => setHover((current) => current ?? n - 1) : undefined}
        onBlur={interactive ? () => setHover(null) : undefined}
        onKeyDown={interactive ? onKeyDown : undefined}
      >
        {body}
        {interactive && hovered && hover !== null ? (
          <line className="cc-spark__cross" x1={xAt(hover)} x2={xAt(hover)} y1={0} y2={height} />
        ) : null}
      </svg>
      {interactive && hovered && hover !== null ? (
        <ChartTooltip x={xAt(hover)} y={0} width={width}>
          {tipLabel(hover, hovered)}
        </ChartTooltip>
      ) : null}
    </span>
  );
}
