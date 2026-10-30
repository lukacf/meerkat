import type { ReactNode } from "react";

import { cx } from "./cx";
import type { Tone } from "./tone";

/**
 * A summary tile: micro-label, a status (usually a Chip), a one-line summary
 * and optional links. The tone draws a 2 px top rule. `muted` is the quiet
 * dashed one-line form for checks that cannot run. A tile that navigates is
 * a button.
 */
export function StatTile({
  label,
  status,
  summary,
  tone = "neutral",
  muted,
  onActivate,
  help,
  children,
  className,
  testId,
  dataAttributes,
}: {
  label: ReactNode;
  status?: ReactNode;
  summary?: ReactNode;
  tone?: Tone;
  muted?: boolean;
  onActivate?: () => void;
  /** Tooltip with the technical description. */
  help?: string;
  /** Links and actions under the summary. */
  children?: ReactNode;
  className?: string;
  testId?: string;
  dataAttributes?: Record<`data-${string}`, string | undefined>;
}) {
  const content = (
    <>
      <span className="cc-tile__head">
        <span className="cc-tile__label">{label}</span>
        {status ? <span className="cc-tile__status">{status}</span> : null}
      </span>
      {summary ? <span className="cc-tile__summary">{summary}</span> : null}
    </>
  );
  const classes = cx("cc-tile", muted && "cc-tile--muted", onActivate && "is-interactive", className);
  if (onActivate && !children) {
    return (
      <button type="button" className={classes} data-tone={tone} title={help} data-testid={testId} onClick={onActivate} {...dataAttributes}>
        {content}
      </button>
    );
  }
  return (
    <div className={classes} data-tone={tone} title={help} data-testid={testId} {...dataAttributes}>
      {content}
      {children ? <div className="cc-tile__links">{children}</div> : null}
    </div>
  );
}

/** A responsive grid of tiles inset by the panel padding. */
export function TileGrid({ className, testId, children }: { className?: string; testId?: string; children?: ReactNode }) {
  return (
    <div className={cx("cc-tiles", className)} data-testid={testId}>
      {children}
    </div>
  );
}
