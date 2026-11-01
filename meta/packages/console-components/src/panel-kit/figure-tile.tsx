import type { ReactNode } from "react";

import { Sparkline, type SparkValues } from "../viz/charts/sparkline";
import { cx } from "./cx";
import { KitGlyph } from "./glyphs";
import { StatusMark } from "./status-mark";
import type { Tone } from "./tone";

export interface FigureDelta {
  /** The signed change. */
  value: number;
  /** Against what: "1h ago", "the hour before". */
  period: string;
  /** Which direction is good news (tone ok); the other is crit. Without
   * it the change stays in the meta ink. */
  goodWhen?: "up" | "down";
}

/** A figure in its compact form: 1,284; 12.9K; 3.4M. Strings pass through. */
export function formatFigure(value: number | string): string {
  if (typeof value === "string") return value;
  if (!Number.isFinite(value)) return "";
  const abs = Math.abs(value);
  const sign = value < 0 ? "-" : "";
  const trim = (n: number) => n.toFixed(1).replace(/\.0$/, "");
  if (abs >= 999_950) return `${sign}${trim(abs / 1_000_000)}M`;
  if (abs >= 10_000) return `${sign}${trim(abs / 1000)}K`;
  const whole = Math.round(abs);
  return `${sign}${String(whole).replace(/\B(?=(\d{3})+(?!\d))/g, ",")}`;
}

/** "+2 vs 1h ago", "-3 vs the hour before", "No change vs 1h ago". */
export function formatDelta(delta: FigureDelta): string {
  if (!delta.value) return `No change vs ${delta.period}`;
  return `${delta.value > 0 ? "+" : "-"}${formatFigure(Math.abs(delta.value))} vs ${delta.period}`;
}

/** The tone of a change: ok when it goes the good way, crit when the bad
 * way, none when flat or when neither way is good. */
export function deltaTone(delta: FigureDelta): Tone | undefined {
  if (!delta.value || !delta.goodWhen) return undefined;
  const up = delta.value > 0;
  return (delta.goodWhen === "up") === up ? "ok" : "crit";
}

/**
 * One figure of a glance strip (spec A3.3): a micro-label (with the tone's
 * mark when the figure has a tone; never a fill), the figure in the figure
 * size with an optional "of 7" or unit, a trend (a 96 x 20 sparkline) or,
 * for the one hero figure of a panel, a meter (BarcodeMeter, ProgressRing)
 * beside it, and a foot line (the change against a period, and any words).
 * A figure that filters the panel is a button with aria-pressed.
 */
export function FigureTile({
  label,
  value,
  unit,
  of,
  tone,
  live,
  delta,
  trend,
  trendLabel,
  trendKind = "bars",
  trendLive,
  hero,
  meter,
  foot,
  onActivate,
  pressed,
  loading,
  title,
  className,
  testId,
}: {
  label: ReactNode;
  value: number | string;
  unit?: string;
  /** The whole: "2 of 7". */
  of?: number | string;
  tone?: Tone;
  /** Shorthand for the live tone's mark. */
  live?: boolean;
  delta?: FigureDelta;
  trend?: SparkValues;
  /** The trend in words; "<label> trend" otherwise. */
  trendLabel?: string;
  trendKind?: "line" | "bars";
  trendLive?: boolean;
  /** The panel's one hero figure: 28 px, with `meter` beside it. */
  hero?: boolean;
  meter?: ReactNode;
  foot?: ReactNode;
  onActivate?: () => void;
  /** The tile's filter is on. */
  pressed?: boolean;
  /** A 40 px skeleton bar instead of the figure. */
  loading?: boolean;
  title?: string;
  className?: string;
  testId?: string;
}) {
  const markTone = live ? "live" : tone;
  const changeTone = delta ? deltaTone(delta) : undefined;
  const hasOf = of !== undefined && of !== null && of !== "";
  const hasFoot = Boolean(delta || foot);
  // A loading tile keeps every line box of the loaded one (spec A3.7: the
  // skeleton has the real layout's shape and loading moves nothing): the
  // value, "of" and unit keep their boxes around a non-breaking space, the
  // skeleton bar lies over the value, the trend keeps its slot, and the meter
  // and the foot keep their boxes, hidden. The foot line is there even
  // without a foot, so a tile is as tall with or without one.
  const blank = "\u00a0";
  // The spaces between the parts lay out as nothing (every container here is
  // a flex box) but keep the words apart in the tile's text and name: the
  // figure never runs into its foot ("1" and "1 agent on them" read "11").
  const content = (
    <>
      <span className="cc-figure__label">
        {markTone ? <StatusMark tone={markTone} /> : null}
        <span className="cc-figure__label-text">{label}</span>
      </span>{" "}
      <span className="cc-figure__fig">
        <span className="cc-figure__value">
          {loading ? (
            <>
              {blank}
              <span className="cc-figure__skeleton" aria-hidden="true" />
            </>
          ) : (
            formatFigure(value)
          )}
        </span>
        {hasOf ? (
          <>
            {" "}
            <span className="cc-figure__of">{loading ? blank : `of ${formatFigure(of)}`}</span>
          </>
        ) : null}
        {unit ? (
          <>
            {" "}
            <span className="cc-figure__of">{loading ? blank : unit}</span>
          </>
        ) : null}
        {hero && meter ? <span className="cc-figure__meter">{meter}</span> : null}
        {!hero && trend ? (
          loading ? (
            <span className="cc-figure__trend" style={{ width: 96, height: 20 }} aria-hidden="true" />
          ) : (
            <Sparkline
              className="cc-figure__trend"
              values={trend}
              width={96}
              height={20}
              kind={trendKind}
              live={trendLive}
              label={trendLabel ?? `${typeof label === "string" ? label : "Figure"} trend`}
            />
          )
        ) : null}
        {!hero && meter ? <span className="cc-figure__meter">{meter}</span> : null}
      </span>
      {hasFoot ? " " : null}
      <span className="cc-figure__foot">
        {delta ? (
          <span className="cc-figure__delta" data-tone={changeTone}>
            <KitGlyph name="arrow-right" className={cx("cc-figure__arrow", delta.value > 0 ? "is-up" : delta.value < 0 ? "is-down" : "is-flat")} />
            {formatDelta(delta)}
          </span>
        ) : null}
        {delta && foot ? " " : null}
        {foot ? <span className="cc-figure__foot-text">{foot}</span> : null}
      </span>
    </>
  );
  const classes = cx("cc-figure", hero && "cc-figure--hero", onActivate && "is-interactive", pressed && "is-pressed", loading && "is-loading", className);
  if (onActivate) {
    return (
      <button
        type="button"
        className={classes}
        data-tone={markTone}
        aria-pressed={Boolean(pressed)}
        aria-busy={loading || undefined}
        title={title}
        data-testid={testId}
        onClick={onActivate}
      >
        {content}
      </button>
    );
  }
  return (
    <div className={classes} data-tone={markTone} aria-busy={loading || undefined} title={title} data-testid={testId}>
      {content}
    </div>
  );
}
