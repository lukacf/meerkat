import { useState, type KeyboardEvent, type PointerEvent } from "react";

import { ChartTooltip } from "../../panel-kit/chart-tooltip";
import { cx } from "../../panel-kit/cx";

export interface HeatRow {
  key: string;
  label: string;
  values: ReadonlyArray<number>;
}

/** The ink-ramp level (1-7) of each positive value by its mid-rank among
 * all positive values; zero is level 0. Uniform data reads mid-ramp. */
export function heatLevels(values: ReadonlyArray<number>): (value: number) => number {
  const positive = values.filter((value) => Number.isFinite(value) && value > 0).sort((a, b) => a - b);
  const n = positive.length;
  // The first index whose value is not below (or, `after`, not at or
  // below) the given one.
  const bound = (value: number, after: boolean) => {
    let lo = 0;
    let hi = n;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (positive[mid] < value || (after && positive[mid] === value)) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  };
  return (value: number) => {
    if (!(value > 0) || n === 0) return 0;
    const less = bound(value, false);
    const equal = bound(value, true) - less;
    const share = (less + equal / 2) / n;
    return Math.min(7, Math.max(1, Math.ceil(share * 7)));
  };
}

/**
 * Magnitude on a grid (spec A3.8): rows (agents, days) by columns (hours),
 * 10 x 10 cells, radius 2, gap 2, in the one-hue ink ramp
 * `--_cc-viz-seq-1..7` by quantile, zero in `--_cc-viz-grid`, with a
 * "less ... more" legend, a tooltip per cell (pointer, or the arrow keys
 * once the grid has focus) and a visually hidden table twin.
 */
export function HeatStrip({
  rows,
  columns,
  columnTick,
  label,
  legend = true,
  cell = 10,
  gap = 2,
  rowLabelWidth = 96,
  formatCell,
  caption,
  className,
  testId,
}: {
  rows: ReadonlyArray<HeatRow>;
  /** Each column's name for the tooltip and the table ("14:00"). */
  columns?: ReadonlyArray<string>;
  /** The tick above a column, or null for none (default: every sixth). */
  columnTick?: (index: number) => string | null;
  /** The numbers in words ("Activity per agent per hour, busiest Triage at 14:00"). */
  label: string;
  legend?: boolean;
  cell?: number;
  gap?: number;
  /** Room for the row labels at the left; 0 hides them. */
  rowLabelWidth?: number;
  formatCell?: (row: HeatRow, column: number, value: number) => string;
  /** The table twin's caption; `label` otherwise. */
  caption?: string;
  className?: string;
  testId?: string;
}) {
  const [active, setActive] = useState<{ row: number; column: number } | null>(null);
  const columnCount = rows.reduce((acc, row) => Math.max(acc, row.values.length), columns?.length ?? 0);
  const level = heatLevels(rows.flatMap((row) => row.values));
  const step = cell + gap;
  const tickH = 12;
  const width = rowLabelWidth + columnCount * step - gap;
  const height = tickH + rows.length * step - gap;
  const tickAt = columnTick ?? ((index: number) => (index % 6 === 0 ? columns?.[index]?.slice(0, 2) ?? String(index) : null));
  const describe = (rowIndex: number, column: number) => {
    const row = rows[rowIndex];
    const value = row?.values[column] ?? 0;
    return formatCell ? formatCell(row, column, value) : `${row?.label ?? ""} · ${columns?.[column] ?? column + 1} · ${value}`;
  };
  const pick = (event: PointerEvent<SVGSVGElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    const x = ((event.clientX - box.left) / Math.max(1, box.width)) * width - rowLabelWidth;
    const y = ((event.clientY - box.top) / Math.max(1, box.height)) * height - tickH;
    const column = Math.floor(x / step);
    const row = Math.floor(y / step);
    if (row < 0 || row >= rows.length || column < 0 || column >= columnCount) setActive(null);
    else setActive({ row, column });
  };
  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    const moves: Record<string, [number, number]> = { ArrowLeft: [0, -1], ArrowRight: [0, 1], ArrowUp: [-1, 0], ArrowDown: [1, 0] };
    const move = moves[event.key];
    if (!move || !rows.length) return;
    event.preventDefault();
    const current = active ?? { row: 0, column: 0 };
    setActive({
      row: Math.min(rows.length - 1, Math.max(0, current.row + move[0])),
      column: Math.min(columnCount - 1, Math.max(0, current.column + move[1])),
    });
  };
  return (
    <div className={cx("cc-heat", className)} data-testid={testId}>
      <span className="cc-heat__box" style={{ width, height }}>
        <svg
          className="cc-heat__svg"
          width={width}
          height={height}
          style={{ width, height }}
          viewBox={`0 0 ${width} ${height}`}
          role="img"
          aria-label={label}
          tabIndex={rows.length ? 0 : undefined}
          onPointerMove={pick}
          onPointerLeave={() => setActive(null)}
          onFocus={() => setActive((current) => current ?? (rows.length ? { row: 0, column: 0 } : null))}
          onBlur={() => setActive(null)}
          onKeyDown={onKeyDown}
        >
          {Array.from({ length: columnCount }, (_, column) => {
            const tick = tickAt(column);
            return tick ? (
              <text key={`tick-${column}`} className="cc-viz-tick" x={rowLabelWidth + column * step} y={tickH - 3}>
                {tick}
              </text>
            ) : null;
          })}
          {rows.map((row, rowIndex) => (
            <g key={row.key}>
              {rowLabelWidth > 0 ? (
                <text className="cc-heat__label" x={0} y={tickH + rowIndex * step + cell - 1.5}>
                  {row.label}
                </text>
              ) : null}
              {Array.from({ length: columnCount }, (_, column) => {
                const value = row.values[column] ?? 0;
                const isActive = active?.row === rowIndex && active.column === column;
                return (
                  <rect
                    key={column}
                    className={cx("cc-heat__cell", isActive && "is-active")}
                    data-level={level(value)}
                    x={rowLabelWidth + column * step}
                    y={tickH + rowIndex * step}
                    width={cell}
                    height={cell}
                    rx={2}
                  />
                );
              })}
            </g>
          ))}
        </svg>
        {active ? (
          <ChartTooltip x={rowLabelWidth + active.column * step + cell / 2} y={tickH + active.row * step} width={width}>
            {describe(active.row, active.column)}
          </ChartTooltip>
        ) : null}
      </span>
      {legend ? (
        <span className="cc-heat__legend" aria-hidden="true">
          Less
          {[0, 1, 2, 3, 4, 5, 6, 7].map((value) => (
            <i key={value} className="cc-heat__swatch" data-level={value} />
          ))}
          More
        </span>
      ) : null}
      <table className="cc-visually-hidden">
        <caption>{caption ?? label}</caption>
        <thead>
          <tr>
            <th scope="col">Row</th>
            {Array.from({ length: columnCount }, (_, column) => (
              <th key={column} scope="col">
                {columns?.[column] ?? column + 1}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.key}>
              <th scope="row">{row.label}</th>
              {Array.from({ length: columnCount }, (_, column) => (
                <td key={column}>{row.values[column] ?? 0}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
