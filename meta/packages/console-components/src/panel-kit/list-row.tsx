import { useId, type MouseEvent, type ReactNode } from "react";

import { cx } from "./cx";

/**
 * One row of a list: lead (dot, avatar or icon), main (title over an
 * optional subtitle), meta (chips, time) and actions. An activatable row is
 * a button. A row that also holds actions or extra content cannot be one
 * button (a button cannot contain buttons): its title is the row's button,
 * named by the title and described by the rest of the row, and the actions
 * and extra content sit beside it, never inside. A click anywhere else on
 * the row still opens it. Actions show on hover and focus, and always on
 * touch and in narrow panes.
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
  const partId = useId();
  const classes = cx(
    "cc-row",
    !lead && "cc-row--no-lead",
    dense && "cc-row--dense",
    onActivate && "is-interactive",
    selected && "is-selected",
    className,
  );
  // A row whose title is its button describes that button by its other
  // parts, so they carry ids; the other forms need none.
  const split = Boolean(onActivate && (actions || children));
  const partIds = split
    ? { sub: `${partId}-sub`, meta: `${partId}-meta`, extra: `${partId}-extra` }
    : { sub: undefined, meta: undefined, extra: undefined };
  const titleNode = (
    <span className="cc-row__title" title={titleAttr}>
      {title}
    </span>
  );
  const leadNode = lead ? <span className="cc-row__lead">{lead}</span> : null;
  const subNode = subtitle ? <span className="cc-row__sub" id={partIds.sub}>{subtitle}</span> : null;
  const metaNode = meta ? <span className="cc-row__meta" id={partIds.meta}>{meta}</span> : null;
  const actionsNode = actions ? (
    <span className="cc-row__actions" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}>
      {actions}
    </span>
  ) : null;
  const extraNode = children ? <div className="cc-row__extra" id={partIds.extra}>{children}</div> : null;
  const common = {
    className: classes,
    "data-testid": testId,
    "data-tone": tone,
    "aria-label": ariaLabel,
    ...dataAttributes,
  };
  if (onActivate && !split) {
    return (
      <button
        type="button"
        {...common}
        aria-current={selected ? "true" : undefined}
        aria-expanded={ariaExpanded}
        onClick={onActivate}
      >
        {leadNode}
        <span className="cc-row__main">
          {titleNode}
          {subNode}
        </span>
        {metaNode}
      </button>
    );
  }
  if (onActivate) {
    const describedBy = [subNode && partIds.sub, metaNode && partIds.meta, extraNode && partIds.extra]
      .filter(Boolean)
      .join(" ");
    const onClick = (event: MouseEvent<HTMLDivElement>) => {
      // The title button and every control in the row (an action, a link,
      // a disclosure) handle their own clicks; anywhere else opens the row.
      const target = event.target as Element | null;
      if (target?.closest?.("a, button, input, select, textarea, summary, [role='button']")) return;
      onActivate();
    };
    return (
      <div
        className={cx(classes, "cc-row--split")}
        data-testid={testId}
        data-tone={tone}
        {...dataAttributes}
        onClick={onClick}
      >
        {leadNode}
        <span className="cc-row__main">
          <button
            type="button"
            className="cc-row__hit"
            aria-label={ariaLabel}
            aria-describedby={describedBy || undefined}
            aria-current={selected ? "true" : undefined}
            aria-expanded={ariaExpanded}
            onClick={onActivate}
          >
            {titleNode}
          </button>
          {subNode}
        </span>
        {metaNode}
        {actionsNode}
        {extraNode}
      </div>
    );
  }
  return (
    <div {...common}>
      {leadNode}
      <span className="cc-row__main">
        {titleNode}
        {subNode}
      </span>
      {metaNode}
      {actionsNode}
      {extraNode}
    </div>
  );
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
