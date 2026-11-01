import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type RefObject,
} from "react";

import { ChartTooltip } from "../../panel-kit/chart-tooltip";
import { cx } from "../../panel-kit/cx";
import { brushRange, createTimeScale, formatTimeRange, nowMarker } from "./time-axis";

// The Logs severity strip (console v2 spec B5): events over the loaded
// window in equal bins, each bin a stack of failures (the negative ink, at
// the base), warnings (the warning ink) and the rest (faint ink), so a burst
// of errors stands out in a sea of routine events. Clicking a bin asks the
// panel to show it; dragging across the strip picks a time range (the
// panel's range filter). One tab stop: Left and Right move between bins,
// Enter shows the focused bin, Space or Shift with an arrow picks a range,
// Escape clears it.

export interface SeverityBin {
  start: number;
  end: number;
  info: number;
  warn: number;
  crit: number;
}

export interface TimeRange {
  start: number;
  end: number;
}

const DRAG_PX = 4;
const AXIS_H = 13;

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

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** A bin in words: "17:02-17:03: 14 events, 2 warnings, 1 error". */
export function severityBinWords(bin: SeverityBin): string {
  const total = bin.info + bin.warn + bin.crit;
  const parts = [plural(total, "event")];
  if (bin.warn) parts.push(plural(bin.warn, "warning"));
  if (bin.crit) parts.push(plural(bin.crit, "error"));
  return `${formatTimeRange(bin)}: ${parts.join(", ")}`;
}

/** Equal bins across [start, end] counting the items by severity. */
export function severityBins(
  items: Iterable<{ t: number; severity: "info" | "warn" | "crit" }>,
  start: number,
  end: number,
  count = 60,
): SeverityBin[] {
  const n = Math.max(1, Math.floor(count));
  const span = Math.max(1, end - start);
  const bins: SeverityBin[] = Array.from({ length: n }, (_, index) => ({
    start: start + (index * span) / n,
    end: start + ((index + 1) * span) / n,
    info: 0,
    warn: 0,
    crit: 0,
  }));
  for (const item of items) {
    if (item.t < start || item.t > end) continue;
    const index = Math.min(n - 1, Math.floor(((item.t - start) / span) * n));
    bins[index][item.severity] += 1;
  }
  return bins;
}

export function SeverityStrip({
  bins,
  start,
  end,
  range,
  onRange,
  onBinActivate,
  now,
  label,
  height = 40,
  defaultWidth = 720,
  testIdPrefix,
  className,
  testId,
}: {
  bins: ReadonlyArray<SeverityBin>;
  start: number;
  end: number;
  /** The picked range, drawn as a band. */
  range?: TimeRange | null;
  /** A drag (or Space) picks a range; Escape clears it (null). */
  onRange?: (range: TimeRange | null) => void;
  /** A click (or Enter) on a bin: show it. */
  onBinActivate?: (index: number, bin: SeverityBin) => void;
  /** The "now" marker when the window reaches now. */
  now?: number;
  /** The strip in words ("1,204 events in 60 bins, 12 warnings, 3 errors"). */
  label: string;
  height?: number;
  defaultWidth?: number;
  testIdPrefix?: string;
  className?: string;
  testId?: string;
}) {
  const [rootRef, width] = useElementWidth(defaultWidth);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [active, setActive] = useState<number | null>(null);
  const [focus, setFocus] = useState(Math.max(0, bins.length - 1));
  const [anchor, setAnchor] = useState<number | null>(null);
  const [drag, setDrag] = useState<{ x0: number; x1: number } | null>(null);
  const tid = (part: string, id?: string | number) => (testIdPrefix ? `${testIdPrefix}-${part}${id === undefined ? "" : `:${id}`}` : undefined);
  const plotH = height - AXIS_H;
  const scale = createTimeScale({ start, end, range: [0, width] });
  const slot = bins.length ? width / bins.length : width;
  const max = useMemo(() => bins.reduce((most, bin) => Math.max(most, bin.info + bin.warn + bin.crit), 0), [bins]);
  const marker = now !== undefined ? nowMarker(scale, now) : null;
  const ticks = scale.ticks({ count: Math.max(2, Math.floor(width / 110)) }).filter((tick) => tick.x > 16 && tick.x < width - (marker ? 34 : 16));
  const toX = (clientX: number) => {
    const box = svgRef.current?.getBoundingClientRect();
    if (!box) return 0;
    return Math.min(width, Math.max(0, ((clientX - box.left) / Math.max(1, box.width)) * width));
  };
  const binAt = (x: number) => Math.min(bins.length - 1, Math.max(0, Math.floor(x / Math.max(1, slot))));
  const rangeOfBins = (a: number, b: number): TimeRange => {
    const lo = bins[Math.min(a, b)];
    const hi = bins[Math.max(a, b)];
    return { start: lo.start, end: hi.end };
  };

  const onPointerDown = (event: PointerEvent<SVGSVGElement>) => {
    if (event.button !== 0) return;
    const x = toX(event.clientX);
    setDrag({ x0: x, x1: x });
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };
  const onPointerMove = (event: PointerEvent<SVGSVGElement>) => {
    const x = toX(event.clientX);
    if (drag) setDrag({ x0: drag.x0, x1: x });
    setActive(bins.length ? binAt(x) : null);
  };
  const onPointerUp = (event: PointerEvent<SVGSVGElement>) => {
    if (!drag) return;
    event.currentTarget.releasePointerCapture?.(event.pointerId);
    const x = toX(event.clientX);
    const moved = Math.abs(x - drag.x0) > DRAG_PX;
    setDrag(null);
    if (!bins.length) return;
    if (moved && onRange) {
      onRange(brushRange(scale, drag.x0, x));
      return;
    }
    const index = binAt(x);
    setFocus(index);
    onBinActivate?.(index, bins[index]);
  };
  const focusBin = (index: number) => {
    const next = Math.min(bins.length - 1, Math.max(0, index));
    setFocus(next);
    setActive(next);
    return next;
  };
  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    if (!bins.length) return;
    const current = Math.min(focus, bins.length - 1);
    if (event.key === "ArrowLeft" || event.key === "ArrowRight" || event.key === "Home" || event.key === "End") {
      event.preventDefault();
      const next = focusBin(event.key === "Home" ? 0 : event.key === "End" ? bins.length - 1 : current + (event.key === "ArrowLeft" ? -1 : 1));
      if (event.shiftKey && onRange) {
        const from = anchor ?? current;
        setAnchor(from);
        onRange(rangeOfBins(from, next));
      } else {
        setAnchor(null);
      }
    } else if (event.key === "Enter") {
      event.preventDefault();
      onBinActivate?.(current, bins[current]);
    } else if (event.key === " " && onRange) {
      event.preventDefault();
      setAnchor(current);
      onRange(rangeOfBins(current, current));
    } else if (event.key === "Escape" && range && onRange) {
      event.preventDefault();
      setAnchor(null);
      onRange(null);
    }
  };

  const band = drag && Math.abs(drag.x1 - drag.x0) > DRAG_PX
    ? { x: Math.min(drag.x0, drag.x1), w: Math.abs(drag.x1 - drag.x0) }
    : range
      ? { x: scale.x(Math.max(start, range.start)), w: Math.max(2, scale.x(Math.min(end, range.end)) - scale.x(Math.max(start, range.start))) }
      : null;
  const gap = slot >= 4 ? 1 : 0;
  const barW = Math.max(1, slot - gap);
  const activeBin = active !== null ? bins[active] : undefined;
  return (
    <div ref={rootRef} className={cx("cc-sevstrip", className)} data-testid={testId ?? tid("severity-strip")} style={{ height }}>
      <svg
        ref={svgRef}
        className="cc-sevstrip__svg"
        width={width}
        height={height}
        style={{ width, height }}
        viewBox={`0 0 ${width} ${height}`}
        role="group"
        aria-label={label}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerLeave={() => {
          if (!drag) setActive(null);
        }}
        onKeyDown={onKeyDown}
        onFocus={() => setActive(Math.min(focus, bins.length - 1))}
        onBlur={() => setActive(null)}
      >
        <line className="cc-sevstrip__base" x1={0} x2={width} y1={plotH - 0.5} y2={plotH - 0.5} />
        {band ? <rect className="cc-sevstrip__band" x={band.x} y={0} width={band.w} height={plotH} /> : null}
        {bins.map((bin, index) => {
          const x = index * slot + gap / 2;
          const total = bin.info + bin.warn + bin.crit;
          const scaleH = (n: number) => (max > 0 ? (n / max) * (plotH - 3) : 0);
          // Failures and warnings keep at least 2 px, so one in a busy bin still shows.
          const critH = bin.crit ? Math.max(2, scaleH(bin.crit)) : 0;
          const warnH = bin.warn ? Math.max(2, scaleH(bin.warn)) : 0;
          const infoH = bin.info ? Math.max(1, scaleH(bin.info)) : 0;
          let y = plotH - 1;
          const segments: Array<{ kind: "crit" | "warn" | "info"; h: number }> = [
            { kind: "crit", h: critH },
            { kind: "warn", h: warnH },
            { kind: "info", h: infoH },
          ];
          return (
            <g
              key={index}
              className={cx("cc-sevstrip__bin", active === index && "is-active", focus === index && "is-focus")}
              data-testid={tid("strip-bin", index)}
              data-count={total}
              data-warn={bin.warn || undefined}
              data-crit={bin.crit || undefined}
            >
              <rect className="cc-sevstrip__hit" x={index * slot} y={0} width={slot} height={plotH} />
              {total === 0 ? <rect className="cc-sevstrip__zero" x={x} y={plotH - 2} width={barW} height={1} /> : null}
              {segments.map((segment) => {
                if (!segment.h) return null;
                y -= segment.h;
                return <rect key={segment.kind} className={`cc-sevstrip__seg cc-sevstrip__seg--${segment.kind}`} x={x} y={y} width={barW} height={segment.h} />;
              })}
            </g>
          );
        })}
        {marker ? <line className="cc-sevstrip__now" x1={marker.x - 0.5} x2={marker.x - 0.5} y1={0} y2={plotH} /> : null}
        {ticks.map((tick) => (
          <text key={tick.t} className="cc-viz-tick" x={tick.x} y={height - 2} textAnchor="middle">
            {tick.label}
          </text>
        ))}
        {marker ? (
          <text className="cc-viz-tick" x={width} y={height - 2} textAnchor="end">
            now
          </text>
        ) : null}
      </svg>
      {activeBin ? (
        <ChartTooltip x={(active ?? 0) * slot + slot / 2} y={0} width={width}>
          {severityBinWords(activeBin)}
        </ChartTooltip>
      ) : null}
    </div>
  );
}
