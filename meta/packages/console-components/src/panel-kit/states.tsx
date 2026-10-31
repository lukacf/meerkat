import type { ReactNode } from "react";

import { cx } from "./cx";
import { KitGlyph, type KitGlyphName } from "./glyphs";
import type { Tone } from "./tone";

export type EmptyStateKind = "empty" | "filtered" | "no-access" | "unavailable" | "not-configured";

const EMPTY_GLYPH: Record<EmptyStateKind, KitGlyphName> = {
  empty: "info",
  filtered: "search",
  "no-access": "shield",
  unavailable: "info",
  "not-configured": "info",
};

/**
 * A designed empty state: a small mark, a title, a short explanation and an
 * optional action. Kinds: empty, filtered (offer Clear), no-access,
 * unavailable and not configured. `inline` is the compact left-aligned form
 * for an empty section inside a panel.
 */
export function EmptyState({
  kind = "empty",
  title,
  body,
  action,
  inline,
  className,
  testId,
  role,
}: {
  kind?: EmptyStateKind;
  title: ReactNode;
  body?: ReactNode;
  action?: ReactNode;
  inline?: boolean;
  className?: string;
  testId?: string;
  role?: "status" | "alert";
}) {
  return (
    <div
      className={cx("cc-panel-empty", inline && "cc-panel-empty--inline", className)}
      data-kind={kind}
      data-testid={testId}
      role={role}
    >
      {inline ? null : (
        <span className="cc-panel-empty__mark" aria-hidden="true">
          <KitGlyph name={EMPTY_GLYPH[kind]} />
        </span>
      )}
      <p className="cc-panel-empty__title">{title}</p>
      {body ? <p className="cc-panel-empty__body">{body}</p> : null}
      {action ? <div className="cc-panel-empty__action">{action}</div> : null}
    </div>
  );
}

/**
 * Loading: skeleton rows that hold the layout, announced politely. The
 * shimmer stops under reduced motion.
 */
export function LoadingState({
  label = "Loading",
  rows = 4,
  variant = "rows",
  className,
  testId,
}: {
  /** Announced to assistive tech: "Loading records". */
  label?: string;
  rows?: number;
  variant?: "rows" | "cards";
  className?: string;
  testId?: string;
}) {
  const count = Math.max(1, Math.min(rows, 8));
  return (
    <div className={cx("cc-skeleton", `cc-skeleton--${variant}`, className)} role="status" data-testid={testId}>
      <span className="cc-visually-hidden">{label}</span>
      {Array.from({ length: count }, (_, index) => (
        <span className="cc-skeleton__row" key={index} aria-hidden="true" />
      ))}
    </div>
  );
}

const NOTICE_GLYPH: Record<Tone, KitGlyphName> = {
  neutral: "info",
  info: "info",
  live: "info",
  ok: "check",
  warn: "info",
  crit: "info",
  locked: "shield",
};

/**
 * A short message in the panel flow: an icon in the tone's ink, the text and
 * an optional action. Replaces bare status paragraphs and grey note boxes.
 * `role` is opt-in: the status row already carries the panel's one alert.
 */
export function Notice({
  tone = "neutral",
  action,
  flush,
  className,
  testId,
  role,
  children,
}: {
  tone?: Tone;
  action?: ReactNode;
  /** Full-bleed band under the tabs instead of an inset box. */
  flush?: boolean;
  className?: string;
  testId?: string;
  role?: "status" | "alert";
  children?: ReactNode;
}) {
  return (
    <div className={cx("cc-notice", flush && "cc-notice--flush", className)} data-tone={tone} data-testid={testId} role={role}>
      <KitGlyph name={NOTICE_GLYPH[tone]} className="cc-notice__icon" />
      <div className="cc-notice__text">{children}</div>
      {action ? <div className="cc-notice__action">{action}</div> : null}
    </div>
  );
}
