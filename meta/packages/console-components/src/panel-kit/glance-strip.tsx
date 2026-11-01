import type { ReactNode } from "react";

import { cx } from "./cx";

/**
 * The glance strip under a panel header (spec A3.3): 2 to 5 figure cells in
 * one row, separated by 1 px rules like an instrument panel (no boxes), with
 * a rule below. A 2 x 2 grid in a pane under 480 px, where a fifth cell
 * hides (put its fact in the header summary).
 */
export function GlanceStrip({
  ariaLabel,
  className,
  testId,
  children,
}: {
  /** "Topology at a glance". */
  ariaLabel?: string;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  return (
    <div className={cx("cc-glance", className)} role={ariaLabel ? "group" : undefined} aria-label={ariaLabel} data-testid={testId}>
      {children}
    </div>
  );
}
