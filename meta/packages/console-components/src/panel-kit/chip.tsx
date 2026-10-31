import type { ReactNode } from "react";

import { cx } from "./cx";
import type { Tone } from "./tone";

/**
 * A status or category word in a pill: tone fill, text-safe tone ink, an
 * optional leading dot. Chips always carry a word; colour is never the only
 * signal.
 */
export function Chip({
  tone = "neutral",
  dot,
  outline,
  mono,
  title,
  className,
  testId,
  children,
}: {
  tone?: Tone;
  dot?: boolean;
  /** Outline only (priority). */
  outline?: boolean;
  /** Mono for counts and codes. */
  mono?: boolean;
  title?: string;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  return (
    <span
      className={cx("cc-chip", outline && "cc-chip--outline", mono && "cc-chip--mono", className)}
      data-tone={tone}
      title={title}
      data-testid={testId}
    >
      {dot ? <span className="cc-chip__dot" aria-hidden="true" /> : null}
      {children}
    </span>
  );
}

/** A small count in mono with tabular numbers (tabs, section headers). */
export function Count({ value, className }: { value: number | string; className?: string }) {
  return <span className={cx("cc-count", className)}>{value}</span>;
}

/**
 * A 7 px status dot. Decorative by default because it always sits next to
 * words; pass a label when it stands alone. `live` adds the accent pulse
 * ring (an agent working right now); `bound` adds a steady ring.
 */
export function StatusDot({
  tone = "neutral",
  live,
  bound,
  label,
  className,
}: {
  tone?: Tone;
  live?: boolean;
  bound?: boolean;
  label?: string;
  className?: string;
}) {
  return (
    <span
      className={cx("cc-dot", live && "is-live", bound && "is-bound", className)}
      data-tone={tone}
      {...(label ? { role: "img", "aria-label": label, title: label } : { "aria-hidden": true })}
    />
  );
}
