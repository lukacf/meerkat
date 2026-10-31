import { CopyButton } from "../copy-button";
import { cx } from "./cx";

/** Characters kept visible at the end of a truncated ID: enough to tell two
 * IDs apart at a glance. */
export const TECHNICAL_ID_TAIL = 6;

/** "Memory ID" reads "memory ID" inside a sentence; acronyms stay. */
export function sentenceFragment(label: string): string {
  if (label.length > 1 && label[1] === label[1].toLowerCase()) {
    return label.charAt(0).toLowerCase() + label.slice(1);
  }
  return label;
}

/** Split an ID for middle truncation: the head shrinks with an ellipsis, the
 * tail always shows. Short values are not split. */
export function splitTechnicalId(value: string, tail: number = TECHNICAL_ID_TAIL): { head: string; tail: string } {
  if (tail <= 0 || value.length <= tail * 2) return { head: value, tail: "" };
  return { head: value.slice(0, value.length - tail), tail: value.slice(-tail) };
}

/**
 * An identifier, hash or key shown as secondary information: muted mono,
 * one line, middle-truncated by CSS (no measuring), the full value in the
 * tooltip and in a visually hidden span for screen readers, and a copy
 * button revealed on hover and focus (always shown on touch).
 */
export function TechnicalId({
  value,
  label,
  copy = true,
  copyLabel,
  tail = TECHNICAL_ID_TAIL,
  className,
  testId,
}: {
  value: string;
  /** What the value is ("Memory ID"); names the copy button. */
  label?: string;
  /** Off where the ID sits inside another button. */
  copy?: boolean;
  copyLabel?: string;
  tail?: number;
  className?: string;
  testId?: string;
}) {
  const parts = splitTechnicalId(value, tail);
  return (
    <span className={cx("cc-tech-id", className)} title={value} data-testid={testId}>
      <span className="cc-tech-id__text" aria-hidden="true">
        <span className="cc-tech-id__head">{parts.head}</span>
        {parts.tail ? <span className="cc-tech-id__tail">{parts.tail}</span> : null}
      </span>
      <span className="cc-visually-hidden">{value}</span>
      {copy ? (
        <CopyButton
          text={value}
          label={copyLabel ?? (label ? `Copy ${sentenceFragment(label)}` : "Copy ID")}
          className="cc-tech-id__copy"
        />
      ) : null}
    </span>
  );
}
