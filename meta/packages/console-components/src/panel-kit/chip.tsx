import type { ReactNode } from "react";

import { cx } from "./cx";
import { StatusMark } from "./status-mark";
import type { Tone } from "./tone";

export type ChipVariant = "solid" | "outline" | "quiet";

/**
 * A status or category word in a pill: tone fill, text-safe tone ink, an
 * optional leading mark (the tone's shape) or dot. Chips always carry a
 * word; colour is never the only signal. Variants: `solid` (default),
 * `outline` (priority) and `quiet` (no fill, no border: the mark and the
 * word in muted ink, for dense rows).
 */
export function Chip({
  tone = "neutral",
  mark,
  dot,
  outline,
  variant,
  mono,
  title,
  className,
  testId,
  children,
}: {
  tone?: Tone;
  /** The tone's StatusMark before the word (a StatusPill). */
  mark?: boolean;
  /** The older 6 px round dot before the word. */
  dot?: boolean;
  /** Outline only (priority); the same as variant="outline". */
  outline?: boolean;
  variant?: ChipVariant;
  /** Mono for counts and codes. */
  mono?: boolean;
  title?: string;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  const look = variant ?? (outline ? "outline" : "solid");
  return (
    <span
      className={cx(
        "cc-chip",
        look === "outline" && "cc-chip--outline",
        look === "quiet" && "cc-chip--quiet",
        mark && "cc-chip--mark",
        mono && "cc-chip--mono",
        className,
      )}
      data-tone={tone}
      title={title}
      data-testid={testId}
    >
      {mark ? <StatusMark tone={tone} className="cc-chip__mark" /> : dot ? <span className="cc-chip__dot" aria-hidden="true" /> : null}
      {children}
    </span>
  );
}

/**
 * The status pill (spec A3.3): a Chip with the tone's mark before the word.
 * 20 px high, pill radius, the pinned tone fill, the tone ink.
 */
export function StatusPill({
  tone = "neutral",
  variant,
  title,
  className,
  testId,
  children,
}: {
  tone?: Tone;
  variant?: ChipVariant;
  title?: string;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  return (
    <Chip tone={tone} mark variant={variant} title={title} className={className} testId={testId}>
      {children}
    </Chip>
  );
}

/** A small count in mono with tabular numbers (tabs, section headers). */
export function Count({ value, className }: { value: number | string; className?: string }) {
  return <span className={cx("cc-count", className)}>{value}</span>;
}

/**
 * A 7 px status dot. Decorative by default because it always sits next to
 * words; pass a label when it stands alone. Warn draws a triangle and crit a
 * diamond (the tone's shape); `live` adds the accent pulse ring (an agent
 * working right now); `bound` adds a steady ring.
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
