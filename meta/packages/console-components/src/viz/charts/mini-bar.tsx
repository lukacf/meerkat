import { cx } from "../../panel-kit/cx";
import type { Tone } from "../../panel-kit/tone";

/**
 * One ratio (spec A3.8): a 4 px pill track in `--_cc-viz-faint` with the
 * fill in ink (or a tone), and the value in words at the right in mono 11 px
 * meta ("2.2 KB of 8 KB"). Storage use, goal progress in rows, quota.
 */
export function MiniBar({
  value,
  max,
  tone,
  label,
  valueLabel,
  width,
  className,
  testId,
}: {
  value: number;
  max: number;
  /** A tone for the fill (a near-limit warn); ink otherwise. */
  tone?: Tone;
  /** The ratio in words, for assistive tech ("2.2 KB of 8 KB used"). */
  label: string;
  /** Shown at the right; none when absent. */
  valueLabel?: string;
  /** The track's width in px (it fills the row otherwise). */
  width?: number;
  className?: string;
  testId?: string;
}) {
  const ratio = max > 0 && Number.isFinite(value) ? Math.min(1, Math.max(0, value / max)) : 0;
  return (
    <span className={cx("cc-minibar", className)} data-tone={tone} data-testid={testId}>
      <span className="cc-minibar__track" role="img" aria-label={label} style={width ? { width } : undefined}>
        {ratio > 0 ? <span className="cc-minibar__fill" style={{ width: `${Math.max(ratio * 100, 2)}%` }} /> : null}
      </span>
      {valueLabel ? <span className="cc-minibar__value">{valueLabel}</span> : null}
    </span>
  );
}
