import type { ReactNode } from "react";

import { cx } from "./cx";
import { EmptyIllustration, type IllustrationName } from "./empty-illustration";
import { KitGlyph, type KitGlyphName } from "./glyphs";
import { StatusMark } from "./status-mark";
import { TechnicalDetails, type TechnicalDetail } from "./technical-details";
import type { Tone } from "./tone";

export type EmptyStateKind = "empty" | "filtered" | "no-access" | "unavailable" | "not-configured" | "error";

const EMPTY_GLYPH: Record<EmptyStateKind, KitGlyphName> = {
  empty: "info",
  filtered: "search",
  "no-access": "shield",
  unavailable: "info",
  "not-configured": "info",
  error: "alert",
};

/**
 * A designed empty state (spec A3.7): a small mark or an illustration, a
 * title, a short explanation and an optional action. Kinds: empty, filtered
 * (offer Clear), no-access, unavailable, not configured and error (keep
 * the reason in words and the code in `details`). `illustration` draws one
 * of the kit's vignettes above the title; `inline` is the compact
 * left-aligned form for an empty section inside a panel.
 */
export function EmptyState({
  kind = "empty",
  illustration,
  title,
  body,
  action,
  details,
  inline,
  className,
  testId,
  role,
}: {
  kind?: EmptyStateKind;
  /** A vignette (A3.7) instead of the kind's small mark; `false` for none. */
  illustration?: IllustrationName | false;
  title: ReactNode;
  body?: ReactNode;
  action?: ReactNode;
  /** Technical details under the action (error codes, missing grants). */
  details?: ReadonlyArray<TechnicalDetail | null | false | undefined>;
  inline?: boolean;
  className?: string;
  testId?: string;
  role?: "status" | "alert";
}) {
  const mark = illustration ? (
    <EmptyIllustration name={illustration} size={inline ? "inline" : "panel"} className="cc-panel-empty__art" />
  ) : inline || illustration === false ? null : (
    <span className="cc-panel-empty__mark" aria-hidden="true">
      <KitGlyph name={EMPTY_GLYPH[kind]} />
    </span>
  );
  return (
    <div
      className={cx("cc-panel-empty", inline && "cc-panel-empty--inline", illustration && "cc-panel-empty--art", className)}
      data-kind={kind}
      data-testid={testId}
      role={role}
    >
      {mark}
      <p className="cc-panel-empty__title">{title}</p>
      {body ? <p className="cc-panel-empty__body">{body}</p> : null}
      {action ? <div className="cc-panel-empty__action">{action}</div> : null}
      {details ? <TechnicalDetails items={details} className="cc-panel-empty__details" /> : null}
    </div>
  );
}

export type LoadingVariant = "rows" | "cards" | "tiles" | "graph" | "timeline";

// Graph skeleton: faint nodes in three lanes with faint edges (A3.7).
const GRAPH_NODES: Array<[number, number]> = [
  [70, 30], [190, 30], [300, 30],
  [40, 92], [130, 92], [230, 92], [330, 92],
  [100, 154], [220, 154],
];
const GRAPH_EDGES: Array<[number, number]> = [[0, 3], [0, 4], [1, 4], [1, 5], [2, 5], [2, 6], [4, 7], [5, 8], [3, 7], [6, 8]];
// Timeline skeleton: lanes with faint bars.
const TIMELINE_BARS: Array<[number, number, number]> = [
  [0, 40, 70], [0, 150, 40], [0, 230, 90],
  [1, 70, 110], [1, 260, 50],
  [2, 20, 50], [2, 120, 80], [2, 300, 40],
  [3, 90, 60], [3, 200, 120],
];

/**
 * Loading (spec A3.7): a skeleton of the real layout, announced politely
 * ("Loading records"), shown after a short delay so a fast load never
 * flashes. Rows and cards shimmer; a graph (faint nodes in lanes with faint
 * edges) and a timeline (lanes with faint bars) breathe in opacity; all of
 * it holds still under reduced motion.
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
  variant?: LoadingVariant;
  className?: string;
  testId?: string;
}) {
  const count = Math.max(1, Math.min(rows, 8));
  let body: ReactNode;
  if (variant === "graph") {
    body = (
      <svg className="cc-skeleton__art" viewBox="0 0 370 184" preserveAspectRatio="xMidYMid meet" aria-hidden="true" focusable="false">
        <path className="cc-skeleton__grid" d="M0 61.5H370M0 123.5H370" />
        {GRAPH_EDGES.map(([from, to]) => (
          <path
            key={`${from}-${to}`}
            className="cc-skeleton__edge"
            d={`M${GRAPH_NODES[from][0]} ${GRAPH_NODES[from][1]}L${GRAPH_NODES[to][0]} ${GRAPH_NODES[to][1]}`}
          />
        ))}
        {GRAPH_NODES.map(([x, y]) => (
          <circle key={`${x}-${y}`} className="cc-skeleton__node" cx={x} cy={y} r={6} />
        ))}
      </svg>
    );
  } else if (variant === "timeline") {
    body = (
      <svg className="cc-skeleton__art" viewBox="0 0 370 184" preserveAspectRatio="xMidYMid meet" aria-hidden="true" focusable="false">
        <path className="cc-skeleton__grid" d="M0 46.5H370M0 92.5H370M0 138.5H370" />
        {TIMELINE_BARS.map(([lane, x, width]) => (
          <rect key={`${lane}-${x}`} className="cc-skeleton__bar" x={x} y={lane * 46 + 19} width={width} height={8} rx={2} />
        ))}
      </svg>
    );
  } else {
    body = Array.from({ length: variant === "tiles" ? Math.max(2, Math.min(count, 5)) : count }, (_, index) => (
      <span className="cc-skeleton__row" key={index} aria-hidden="true" />
    ));
  }
  return (
    <div className={cx("cc-skeleton", `cc-skeleton--${variant}`, className)} role="status" data-testid={testId}>
      <span className="cc-visually-hidden">{label}</span>
      {body}
    </div>
  );
}

const NOTICE_GLYPH: Record<Tone, KitGlyphName> = {
  neutral: "info",
  info: "info",
  live: "info",
  ok: "check",
  warn: "warning",
  crit: "alert",
  locked: "shield",
};

/**
 * A short message in the panel flow: an icon in the tone's ink (its shape:
 * a triangle for warn, a diamond for crit), the text and an optional
 * action. Replaces bare status paragraphs and grey note boxes. `role` is
 * opt-in: the status row already carries the panel's one alert.
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
      {tone === "live" ? (
        // Live speaks with its own mark (the accent dot and its pulse), not an icon.
        <span className="cc-notice__icon cc-notice__icon--mark" aria-hidden="true">
          <StatusMark tone="live" />
        </span>
      ) : (
        <KitGlyph name={NOTICE_GLYPH[tone]} className="cc-notice__icon" />
      )}
      <div className="cc-notice__text">{children}</div>
      {action ? <div className="cc-notice__action">{action}</div> : null}
    </div>
  );
}
