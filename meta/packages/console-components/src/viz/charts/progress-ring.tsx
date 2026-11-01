import { cx } from "../../panel-kit/cx";

/** "3 of 5 done, 1 in progress, 1 blocked". */
export function progressWords(done: number, total: number, inProgress = 0, blocked = 0): string {
  const parts = [`${done} of ${total} done`];
  if (inProgress > 0) parts.push(`${inProgress} in progress`);
  if (blocked > 0) parts.push(`${blocked} blocked`);
  return parts.join(", ");
}

/**
 * Goal progress as a small donut (spec A3.8): 16 or 24 px, a 2.5 px stroke,
 * the track in `--_cc-viz-faint`, then arcs in order done (ink), in
 * progress (the accent, "now") and blocked (warning), 1 px apart.
 */
export function ProgressRing({
  done,
  inProgress = 0,
  blocked = 0,
  total,
  size = 16,
  label,
  className,
  testId,
}: {
  done: number;
  inProgress?: number;
  blocked?: number;
  total: number;
  size?: 16 | 24;
  /** Defaults to "3 of 5 done, 1 in progress, 1 blocked". */
  label?: string;
  className?: string;
  testId?: string;
}) {
  const stroke = 2.5;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const whole = Math.max(total, done + inProgress + blocked, 0);
  const parts = [
    { kind: "done", value: Math.max(0, done) },
    { kind: "live", value: Math.max(0, inProgress) },
    { kind: "blocked", value: Math.max(0, blocked) },
  ].filter((part) => part.value > 0);
  const filled = parts.reduce((acc, part) => acc + part.value, 0);
  const rest = Math.max(0, whole - filled);
  // The gaps show the surface: the track is drawn only where nothing is.
  const pieces = [...parts, ...(rest > 0 && filled > 0 ? [{ kind: "track", value: rest }] : [])];
  const gap = pieces.length > 1 ? 1 : 0;
  let offset = 0;
  const arcs = whole > 0 && filled > 0
    ? pieces.map((part) => {
      const length = (part.value / whole) * c;
      const arc = (
        <circle
          key={part.kind}
          className={part.kind === "track" ? "cc-ring__track" : "cc-ring__arc"}
          data-kind={part.kind}
          cx={size / 2}
          cy={size / 2}
          r={r}
          strokeWidth={stroke}
          strokeDasharray={`${Math.max(0.5, length - gap).toFixed(2)} ${c.toFixed(2)}`}
          strokeDashoffset={(-offset).toFixed(2)}
        />
      );
      offset += length;
      return arc;
    })
    : [];
  return (
    <svg
      className={cx("cc-ring", className)}
      data-testid={testId}
      width={size}
      height={size}
      style={{ width: size, height: size }}
      viewBox={`0 0 ${size} ${size}`}
      role="img"
      aria-label={label ?? progressWords(done, total, inProgress, blocked)}
    >
      {arcs.length ? null : <circle className="cc-ring__track" cx={size / 2} cy={size / 2} r={r} strokeWidth={stroke} />}
      <g transform={`rotate(-90 ${size / 2} ${size / 2})`}>{arcs}</g>
    </svg>
  );
}
