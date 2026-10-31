import { cx } from "./cx";
import { KitGlyph } from "./glyphs";

/**
 * Live updates (spec A4.6). A list that follows live events scrolls itself
 * only while the reader sits at the live edge; otherwise this pill appears
 * at that edge ("3 new events") and brings them into view. New rows take
 * the `cc-enter-live` class: a 200 ms fade and 8 px slide (opacity only
 * under reduced motion) with a 2 px accent left rule that fades over 1.2 s.
 */
export function NewItemsPill({
  count,
  onClick,
  noun = "event",
  plural,
  placement = "top",
  className,
  testId,
}: {
  count: number;
  onClick: () => void;
  /** "event" reads "3 new events". */
  noun?: string;
  plural?: string;
  /** Which edge is the live one. */
  placement?: "top" | "bottom";
  className?: string;
  testId?: string;
}) {
  if (count <= 0) return null;
  const words = `${count} new ${count === 1 ? noun : plural ?? `${noun}s`}`;
  return (
    <button type="button" className={cx("cc-new-pill", className)} data-placement={placement} data-testid={testId} onClick={onClick}>
      <KitGlyph name="arrow-right" className="cc-new-pill__arrow" />
      {words}
    </button>
  );
}
