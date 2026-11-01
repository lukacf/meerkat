import { useCallback, useLayoutEffect, useRef, useState, type ReactNode } from "react";

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

/** What a responsive meter gets: the width it may take, in px. */
export interface FigureMeterSlot {
  width: number;
}

/** A tile's meter: fixed (a node, kept inside the tile by clipping), or
 * responsive (a function of the width the tile measured for it). */
export type FigureMeter = ReactNode | ((slot: FigureMeterSlot) => ReactNode);

const TREND_WIDTH = 96;
const TREND_MIN_WIDTH = 32;
/** Between the figure and a slot beside it: the row's 6 px gap and the
 * slot's own 6 px margin. */
const SLOT_GAP = 12;
/** Before the first measure, and where nothing lays out (jsdom). */
const UNMEASURED_METER_WIDTH = 96;

/** The room beside a tile's figure, measured: from the figure group's right
 * edge to the end of the figure row, and the row's whole width. Null until
 * measured or where nothing lays out. */
function useFigureRoom(enabled: boolean) {
  const rowRef = useRef<HTMLSpanElement | null>(null);
  const groupRef = useRef<HTMLSpanElement | null>(null);
  const [room, setRoom] = useState<{ beside: number; row: number } | null>(null);
  const measure = useCallback(() => {
    const row = rowRef.current?.getBoundingClientRect();
    const group = groupRef.current?.getBoundingClientRect();
    if (!row || !group || !(row.width > 0)) {
      setRoom((previous) => (previous === null ? previous : null));
      return;
    }
    const next = { beside: Math.max(0, Math.floor(row.right - group.right - SLOT_GAP)), row: Math.floor(row.width) };
    setRoom((previous) => (previous && previous.beside === next.beside && previous.row === next.row ? previous : next));
  }, []);
  useLayoutEffect(() => {
    if (!enabled) return undefined;
    measure();
    const row = rowRef.current;
    const group = groupRef.current;
    if (!row || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => measure());
    observer.observe(row);
    if (group) observer.observe(group);
    return () => observer.disconnect();
  }, [enabled, measure]);
  return { rowRef, groupRef, room };
}

/**
 * One figure of a glance strip (spec A3.3): a micro-label (with the tone's
 * mark when the figure has a tone; never a fill), the figure in the figure
 * size with an optional "of 7" or unit, a trend (a 96 x 20 sparkline) or,
 * for the one hero figure of a panel, a meter (BarcodeMeter, ProgressRing)
 * beside it, and a foot line (the change against a period, and any words).
 * A figure that filters the panel is a button with aria-pressed.
 *
 * Nothing crosses the tile's bounds. The trend shrinks to the room beside
 * the figure (hidden under 32 px). A responsive meter, `meter={({ width }) =>
 * ...}`, is drawn at the width the tile measures beside the figure; under
 * `meterMinWidth` (48 px) it moves below the figure at the row's width, and
 * hides when even that is too narrow. A fixed meter (a node) shrinks and is
 * clipped at the tile's edge.
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
  meterMinWidth = 48,
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
  /** A node, or `({ width }) => node` to draw it at the width the tile has
   * for it (cap it at the meter's own width: `Math.min(150, width)`). */
  meter?: FigureMeter;
  /** The narrowest a responsive meter is drawn beside the figure; under it,
   * it goes below the figure (default 48 px). */
  meterMinWidth?: number;
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
  const responsive = typeof meter === "function";
  const showTrend = !hero && Boolean(trend);
  const { rowRef, groupRef, room } = useFigureRoom(responsive || showTrend);
  // The room beside the figure goes to the trend first, then the meter.
  let trendWidth = TREND_WIDTH;
  let meterPlace: "beside" | "below" | "hidden" = "beside";
  let meterWidth = UNMEASURED_METER_WIDTH;
  if (room) {
    let beside = room.beside;
    if (showTrend) {
      trendWidth = beside >= TREND_MIN_WIDTH ? Math.min(TREND_WIDTH, beside) : 0;
      beside = trendWidth ? beside - trendWidth - SLOT_GAP : beside;
    }
    if (responsive) {
      if (beside >= meterMinWidth) meterWidth = beside;
      else if (room.row >= meterMinWidth) {
        meterPlace = "below";
        meterWidth = room.row;
      } else meterPlace = "hidden";
    }
  }
  const meterNode = meter ? (typeof meter === "function" ? (meterPlace === "hidden" ? null : meter({ width: meterWidth })) : meter) : null;
  const meterSlot = meterNode ? (
    <span className={cx("cc-figure__meter", responsive ? meterPlace === "below" && "is-below" : "is-fixed")}>{meterNode}</span>
  ) : null;
  const meterBeside = meterPlace === "beside" ? meterSlot : null;
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
      <span className="cc-figure__fig" ref={rowRef}>
        <span className="cc-figure__group" ref={groupRef}>
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
        </span>
        {hero ? meterBeside : null}
        {showTrend && trend && trendWidth > 0 ? (
          loading ? (
            <span className="cc-figure__trend" style={{ width: trendWidth, height: 20 }} aria-hidden="true" />
          ) : (
            <Sparkline
              className="cc-figure__trend"
              values={trend}
              width={trendWidth}
              height={20}
              kind={trendKind}
              live={trendLive}
              label={trendLabel ?? `${typeof label === "string" ? label : "Figure"} trend`}
            />
          )
        ) : null}
        {hero ? null : meterBeside}
      </span>
      {meterPlace === "below" ? meterSlot : null}
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
