import { cx } from "./cx";
import type { Tone } from "./tone";

export type StatusMarkSize = "sm" | "md";

/**
 * The status shape (spec A2.4): every tone has its own shape, so colour
 * never carries meaning alone. ok and info are filled circles, warn a
 * triangle, crit a diamond, neutral a hollow circle, locked a hollow square,
 * live a filled accent circle with a pulse ring (a static double ring under
 * reduced motion). 8 px (`sm`) or 10 px (`md`).
 *
 * Decorative beside a word (aria-hidden); with `label` it stands alone as
 * an image named by the label.
 */
export function StatusMark({
  tone = "neutral",
  label,
  size = "sm",
  className,
  testId,
}: {
  tone?: Tone;
  /** The status in words when the mark stands alone. */
  label?: string;
  size?: StatusMarkSize;
  className?: string;
  testId?: string;
}) {
  return (
    <span
      className={cx("cc-mark", className)}
      data-tone={tone}
      data-size={size === "md" ? "md" : undefined}
      data-testid={testId}
      {...(label ? { role: "img", "aria-label": label, title: label } : { "aria-hidden": true })}
    />
  );
}
