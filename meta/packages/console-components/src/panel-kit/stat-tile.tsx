import type { KeyboardEvent, MouseEvent, ReactNode } from "react";

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
  if (onActivate) {
    // A tile with links inside cannot be a <button>; it stays one for the
    // keyboard and assistive tech (role, tab stop, Enter and Space). A click
    // on a control inside belongs to that control.
    const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.target !== event.currentTarget) return;
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        onActivate();
      }
    };
    const onClick = (event: MouseEvent<HTMLDivElement>) => {
      const target = event.target as Element | null;
      const control = target?.closest?.("a, button, input, select, textarea, summary, [role='button']");
      if (control && control !== event.currentTarget) return;
      onActivate();
    };
    return (
      <div
        role="button"
        tabIndex={0}
        className={classes}
        data-tone={tone}
        title={help}
        data-testid={testId}
        onClick={onClick}
        onKeyDown={onKeyDown}
        {...dataAttributes}
      >
        {content}
        <div className="cc-tile__links">{children}</div>
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
