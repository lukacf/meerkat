import { cx } from "../../panel-kit/cx";

const NICE = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000, 10000];

/** Units per segment: one each until the total exceeds the segment count,
 * then the smallest nice step that fits. */
export function barcodeUnitsPerSegment(total: number, segments: number): number {
  if (!(total > segments)) return 1;
  const need = total / Math.max(1, segments);
  return NICE.find((step) => step >= need) ?? Math.ceil(need);
}

export interface BarcodeLabels {
  filled: string;
  outlined: string;
  live: string;
}

const DEFAULT_LABELS: BarcodeLabels = { filled: "long-lived", outlined: "helpers", live: "active now" };

/** The barcode's legend: marks and words in 9.5 px meta, never coloured
 * text ("■ 7 long-lived □ 1 helper ■ 1 active now"). */
export function BarcodeLegend({
  filled,
  outlined = 0,
  live = 0,
  labels = {},
  unitNote,
  className,
}: {
  filled: number;
  outlined?: number;
  live?: number;
  labels?: Partial<BarcodeLabels>;
  /** "1 segment = 20 agents" when one segment holds more than one. */
  unitNote?: string;
  className?: string;
}) {
  const words = {
    filled: labels.filled ?? DEFAULT_LABELS.filled,
    // A label of the caller's own is used as given; the default agrees in number.
    outlined: labels.outlined ?? (outlined === 1 ? "helper" : DEFAULT_LABELS.outlined),
    live: labels.live ?? DEFAULT_LABELS.live,
  };
  return (
    <span className={cx("cc-barcode-legend", className)}>
      <span className="cc-barcode-legend__item">
        <i className="cc-barcode-legend__mark" data-kind="filled" aria-hidden="true" />
        {filled} {words.filled}
      </span>
      {outlined > 0 ? (
        <span className="cc-barcode-legend__item">
          <i className="cc-barcode-legend__mark" data-kind="outlined" aria-hidden="true" />
          {outlined} {words.outlined}
        </span>
      ) : null}
      {live > 0 ? (
        <span className="cc-barcode-legend__item">
          <i className="cc-barcode-legend__mark" data-kind="live" aria-hidden="true" />
          {live} {words.live}
        </span>
      ) : null}
      {unitNote ? <span className="cc-barcode-legend__item">{unitNote}</span> : null}
    </span>
  );
}

/**
 * The brand ladder (spec A3.8, scenes.js `meter`): `segments` slots, the
 * long-lived count in ink, the live ones in the accent as the last lit
 * segments of it, helpers as 1 px ink outlines after them, the rest unlit
 * (`--_cc-viz-faint` at 55%), and an optional peak tick. One unit per
 * segment until the total exceeds the slots, then N per segment with the
 * unit stated. The hero figure of a glance strip carries one.
 */
export function BarcodeMeter({
  filled,
  outlined = 0,
  live = 0,
  peak,
  segments = 40,
  label,
  legend,
  labels,
  scale,
  unit = "agents",
  width = 150,
  height = 12,
  className,
  testId,
}: {
  /** Long-lived units (ink), including the live ones. */
  filled: number;
  /** Helpers and workers (outlined). */
  outlined?: number;
  /** Of `filled`, the ones working now (accent). */
  live?: number;
  /** The highest count seen (a tick). */
  peak?: number;
  segments?: number;
  /** The numbers in words ("8 agents: 7 long-lived, 1 helper, 1 working"). */
  label: string;
  /** Draws the legend under the ladder. */
  legend?: boolean;
  labels?: Partial<BarcodeLabels>;
  /** Labels at 0 and the maximum under the ladder. */
  scale?: boolean;
  unit?: string;
  width?: number;
  height?: number;
  className?: string;
  testId?: string;
}) {
  const slots = Math.max(1, Math.floor(segments));
  const safe = (value: number | undefined) => (Number.isFinite(value) ? Math.max(0, Math.floor(value as number)) : 0);
  const filledN = safe(filled);
  const outlinedN = safe(outlined);
  const liveN = Math.min(filledN, safe(live));
  const per = barcodeUnitsPerSegment(filledN + outlinedN, slots);
  const filledSeg = Math.min(slots, Math.ceil(filledN / per));
  const liveSeg = Math.min(filledSeg, Math.ceil(liveN / per));
  const outlinedSeg = Math.min(slots - filledSeg, Math.ceil(outlinedN / per));
  const slot = width / slots;
  const segW = Math.max(1.5, slot * 0.6);
  const scaleH = scale ? 11 : 0;
  const svgH = height + scaleH;
  const peakSeg = peak !== undefined && peak > 0 ? Math.min(slots, Math.ceil(safe(peak) / per)) : 0;
  const unitNote = per > 1 ? `1 segment = ${per} ${unit}` : undefined;
  const rects = [];
  for (let index = 0; index < slots; index += 1) {
    const x = index * slot + (slot - segW) / 2;
    let kind: "filled" | "live" | "outlined" | "unlit" = "unlit";
    if (index < filledSeg - liveSeg) kind = "filled";
    else if (index < filledSeg) kind = "live";
    else if (index < filledSeg + outlinedSeg) kind = "outlined";
    rects.push(
      kind === "outlined" ? (
        <rect key={index} className="cc-barcode__seg" data-kind={kind} x={x + 0.5} y={0.5} width={Math.max(0.5, segW - 1)} height={height - 1} />
      ) : (
        <rect key={index} className="cc-barcode__seg" data-kind={kind} x={x} y={0} width={segW} height={height} />
      ),
    );
  }
  return (
    // With its legend, the meter is as wide as its ladder: the legend wraps
    // under it instead of widening it past the room it was given.
    <span className={cx("cc-barcode", className)} data-testid={testId} style={legend ? { width } : undefined}>
      <svg className="cc-barcode__svg" width={width} height={svgH} style={{ width, height: svgH }} viewBox={`0 0 ${width} ${svgH}`} role="img" aria-label={unitNote ? `${label} (${unitNote})` : label}>
        {rects}
        {peakSeg > 0 ? (
          <line
            className="cc-barcode__peak"
            x1={(peakSeg - 1) * slot + (slot + segW) / 2 + 0.75}
            x2={(peakSeg - 1) * slot + (slot + segW) / 2 + 0.75}
            y1={0}
            y2={height}
          />
        ) : null}
        {scale ? (
          <>
            <text className="cc-viz-tick" x={0} y={svgH - 1}>
              0
            </text>
            <text className="cc-viz-tick" x={width} y={svgH - 1} textAnchor="end">
              {slots * per}
            </text>
          </>
        ) : null}
      </svg>
      {legend ? <BarcodeLegend filled={filledN} outlined={outlinedN} live={liveN} labels={labels} unitNote={unitNote} /> : null}
    </span>
  );
}
