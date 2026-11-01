// The work item mark (design spec A3.9 and B2) as a standalone inline SVG,
// for lists, cards and drawers: open hollow mid; ready hollow ink with a
// positive centre dot; in progress accent (a live ring while its owner's run
// is open); blocked warning outline with a slash; completed ink with a
// check; failed negative with a cross; cancelled faint.

import type { JSX } from "react";

import { StatusSquareShape, itemStatusOf } from "./marks";

export interface StatusSquareProps {
  /** A work item status word ("in_progress"); unknown words draw as open. */
  status: string;
  live?: boolean;
  size?: 12 | 14 | 16;
  /** Names the mark for assistive tech; decorative without it (a word sits beside it). */
  label?: string;
  className?: string;
  testId?: string;
}

export function StatusSquare({ status, live = false, size = 12, label, className, testId }: StatusSquareProps): JSX.Element {
  const item = itemStatusOf(status);
  const box = size + 6;
  return (
    <svg
      className={className ? `cc-viz-status-square ${className}` : "cc-viz-status-square"}
      width={box}
      height={box}
      style={{ width: box, height: box }}
      viewBox={`${-box / 2} ${-box / 2} ${box} ${box}`}
      // Not data-status: the WorkGraph drawer pins exactly one [data-status].
      data-square={item}
      data-testid={testId}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
    >
      <StatusSquareShape status={item} size={size} live={live} />
    </svg>
  );
}
