import { useId, type MouseEvent, type ReactNode } from "react";

import { cx } from "./cx";
import type { Tone } from "./tone";

/**
 * A summary tile: micro-label, a status (usually a Chip), a one-line summary
 * and optional links. The tone draws a 2 px top rule. `muted` is the quiet
 * dashed one-line form for checks that cannot run. A tile that navigates is
 * a button; one that also holds links is a container whose label is its
 * button, with the links beside it (never a control inside a control).
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
  const partId = useId();
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
  if (onActivate) {
    // A tile with links cannot be one <button>, and a role="button" holding
    // links nests controls. The label is the tile's real button, described by
    // the status, summary and links beside it; a click anywhere else on the
    // tile (not on a link) opens it too.
    const onClick = (event: MouseEvent<HTMLDivElement>) => {
      const target = event.target as Element | null;
      if (target?.closest?.("a, button, input, select, textarea, summary, [role='button']")) return;
      onActivate();
    };
    const describedBy = [status && `${partId}-status`, summary && `${partId}-summary`, `${partId}-links`].filter(Boolean).join(" ");
    return (
      <div
        className={cx(classes, "cc-tile--split")}
        data-tone={tone}
        title={help}
        data-testid={testId}
        onClick={onClick}
        {...dataAttributes}
      >
        <span className="cc-tile__head">
          <button type="button" className="cc-tile__hit" aria-describedby={describedBy} onClick={onActivate}>
            <span className="cc-tile__label">{label}</span>
          </button>
          {status ? <span className="cc-tile__status" id={`${partId}-status`}>{status}</span> : null}
        </span>
        {summary ? <span className="cc-tile__summary" id={`${partId}-summary`}>{summary}</span> : null}
        <div className="cc-tile__links" id={`${partId}-links`}>{children}</div>
      </div>
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
