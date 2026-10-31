import { cx } from "./cx";
import type { Tone } from "./tone";

export interface MeterSegment {
  value: number;
  tone?: Tone;
  label?: string;
}

/**
 * A 4 px track with tone segments: progress ("1 of 5 done") or composition
 * (active, quarantined, superseded). The words travel in `label`; the bar is
 * a picture of them (role="img" with the same words).
 */
export function Meter({
  segments,
  total,
  label,
  showLabel = true,
  className,
  testId,
}: {
  segments: ReadonlyArray<MeterSegment>;
  /** Defaults to the sum of the segments. */
  total?: number;
  label: string;
  showLabel?: boolean;
  className?: string;
  testId?: string;
}) {
  const sum = segments.reduce((acc, segment) => acc + Math.max(0, segment.value), 0);
  const denominator = Math.max(total ?? sum, sum, 0);
  return (
    <span className={cx("cc-meter", className)} data-testid={testId}>
      <span className="cc-meter__track" role="img" aria-label={label}>
        {denominator > 0
          ? segments.map((segment, index) =>
              segment.value > 0 ? (
                <span
                  key={`${segment.label ?? "segment"}-${index}`}
                  className="cc-meter__seg"
                  data-tone={segment.tone ?? "neutral"}
                  style={{ width: `${(Math.max(0, segment.value) / denominator) * 100}%` }}
                  title={segment.label}
                />
              ) : null,
            )
          : null}
      </span>
      {showLabel ? <span className="cc-meter__label">{label}</span> : null}
    </span>
  );
}
