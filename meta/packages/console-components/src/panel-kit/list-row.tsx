import type { KeyboardEvent, MouseEvent, ReactNode } from "react";

import { cx } from "./cx";

/**
 * One row of a list: lead (dot, avatar or icon), main (title over an
 * optional subtitle), meta (chips, time) and actions. Activatable rows are a
 * button, or role="button" with Enter and Space when they hold actions (a
 * button cannot contain buttons). Actions show on hover and focus, and
 * always on touch and in narrow panes.
 */
export function ListRow({
  lead,
  title,
  subtitle,
  meta,
  actions,
  onActivate,
  selected,
  dense,
  tone,
  className,
  testId,
  ariaLabel,
  ariaExpanded,
  titleAttr,
  children,
  dataAttributes,
}: {
  lead?: ReactNode;
  title: ReactNode;
  subtitle?: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
  onActivate?: () => void;
  selected?: boolean;
  dense?: boolean;
  /** A tone for the row's leading rule (signals); rarely needed. */
  tone?: string;
  className?: string;
  testId?: string;
  ariaLabel?: string;
  ariaExpanded?: boolean;
  /** Tooltip for the title (full text or the raw key behind a name). */
  titleAttr?: string;
  /** Extra content under the main line (expanded detail). */
  children?: ReactNode;
  dataAttributes?: Record<`data-${string}`, string | undefined>;
}) {
  const classes = cx(
    "cc-row",
    !lead && "cc-row--no-lead",
    dense && "cc-row--dense",
    onActivate && "is-interactive",
    selected && "is-selected",
    className,
  );
  const body = (
    <>
      {lead ? <span className="cc-row__lead">{lead}</span> : null}
      <span className="cc-row__main">
        <span className="cc-row__title" title={titleAttr}>
          {title}
        </span>
        {subtitle ? <span className="cc-row__sub">{subtitle}</span> : null}
      </span>
      {meta ? <span className="cc-row__meta">{meta}</span> : null}
      {actions ? (
        <span className="cc-row__actions" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}>
          {actions}
        </span>
      ) : null}
      {children ? <div className="cc-row__extra">{children}</div> : null}
    </>
  );
  const common = {
    className: classes,
    "data-testid": testId,
    "data-tone": tone,
    "aria-label": ariaLabel,
    ...dataAttributes,
  };
  if (onActivate && !actions && !children) {
    return (
      <button
        type="button"
        {...common}
        aria-current={selected ? "true" : undefined}
        aria-expanded={ariaExpanded}
        onClick={onActivate}
      >
        {body}
      </button>
    );
  }
  if (onActivate) {
    const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.target !== event.currentTarget) return;
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        onActivate();
      }
    };
    const onClick = (event: MouseEvent<HTMLDivElement>) => {
      // A click on a control inside the row (an action, a link, a disclosure)
      // belongs to that control, not to the row.
      const target = event.target as Element | null;
      const control = target?.closest?.("a, button, input, select, textarea, summary, [role='button']");
      if (control && control !== event.currentTarget) return;
      onActivate();
    };
    return (
      <div
        role="button"
        tabIndex={0}
        {...common}
        aria-current={selected ? "true" : undefined}
        aria-expanded={ariaExpanded}
        onClick={onClick}
        onKeyDown={onKeyDown}
      >
        {body}
      </div>
    );
  }
  return <div {...common}>{body}</div>;
}

/** A plain list container: rows separated by rules. */
export function RowList({
  className,
  testId,
  children,
}: {
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  return (
    <div className={cx("cc-rows", className)} data-testid={testId}>
      {children}
    </div>
  );
}
