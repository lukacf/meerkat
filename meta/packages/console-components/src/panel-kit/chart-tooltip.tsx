import type { ReactNode } from "react";

import { cx } from "./cx";

/**
 * The hover and focus label of a chart mark (micro charts, A3.8): a small
 * e2 layer inside the chart's own box, above the point it describes. It
 * exists only while a mark is hovered or focused, so a chart's text is
 * never part of the rows around it. Decorative to assistive tech: every
 * mark carries its words in aria-label and the chart has a table or list
 * twin.
 */
export function ChartTooltip({
  x,
  y,
  width,
  children,
  className,
}: {
  /** The point, in px from the chart box's left and top. */
  x: number;
  y: number;
  /** The chart box's width, to keep the label inside it. */
  width: number;
  children?: ReactNode;
  className?: string;
}) {
  const align = x < width * 0.2 ? "start" : x > width * 0.8 ? "end" : "center";
  return (
    <span className={cx("cc-chart-tip", className)} data-align={align} style={{ left: x, top: y }} aria-hidden="true">
      {children}
    </span>
  );
}
